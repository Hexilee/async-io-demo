Table of Contents
=================

* [引言](#引言)
* [异步 IO 的基石 - mio](#异步-io-的基石---mio)
    * [异步网络 IO](#异步网络-io)
    * [容错性原则](#容错性原则)
    * [Poll Option](#poll-option)
    * [Still Block](#still-block)
    * [自定义事件](#自定义事件)
    * [Callback is evil](#callback-is-evil)
* [coroutine](#coroutine)
    * [generator](#generator)
    * [自引用](#自引用)
    * [Pin](#pin)
    * [合理的抽象](#合理的抽象)
        * [Poll&lt;T&gt;](#pollt)
        * [await!](#await)
        * [async](#async)
    * [asynchronous coroutine](#asynchronous-coroutine)
        * [Executor](#executor)
        * [block_on](#block_on)
        * [spawn](#spawn)
        * [TcpListener](#tcplistener)
        * [TcpStream](#tcpstream)
* [后记](#后记)



### 引言

2018 年接近尾声，`rust` 团队勉强立住了异步 `IO` 的 flag，`async` 成为了关键字，`Pin`, `Future`, `Poll` 和 `await!` 也进入了标准库。不过一直以来实际项目中用不到这套东西，所以也没有主动去了解过。

最近心血来潮想用 `rust` 写点东西，但并找不到比较能看的文档（可能是因为 `rust` 发展太快了，很多都过时了），最后参考[这篇文章](https://cafbit.com/post/tokio_internals/)和 `"new tokio"`( [romio](https://github.com/withoutboats/romio) ) 写了几个 `demo`，并基于 `mio` 在 `coroutine` 中实现了简陋的异步 `IO`。

最终效果如下：

```rust
// examples/async-echo.rs

#![feature(async_await)]
#![feature(await_macro)]

#[macro_use]
extern crate log;

use asyncio::executor::{block_on, spawn, TcpListener};
use failure::Error;

fn main() -> Result<(), Error> {
    env_logger::init();
    block_on(
        async {
            let mut listener = TcpListener::bind(&"127.0.0.1:7878".parse().unwrap())
                .expect("TcpListener bind fail");
            info!("Listening on 127.0.0.1:7878");
            while let Ok((mut stream, addr)) = await!(listener.accept()) {
                info!("connection from {}", addr);
                spawn(
                    async move {
                        let client_hello = await!(stream.read()).expect("read from stream fail");
                        let read_length = client_hello.len();
                        let write_length =
                            await!(stream.write(client_hello)).expect("write to stream fail");
                        assert_eq!(read_length, write_length);
                        stream.close();
                    },
                )
                .expect("spawn stream fail");
            }
        },
    )
}

```

写这篇文章的主要目的是梳理和总结，同时也希望能给对这方面有兴趣的 `Rustacean` 作为参考。本文代码以易于理解为主要编码原则，某些地方并没有太考虑性能，还请见谅；但如果文章和代码中有明显错误，欢迎指正。

本文代码仓库在 [Github](https://github.com/Hexilee/async-io-demo) （部分代码较长，建议 `clone` 下来用编辑器看），所有 `examples` 在 `nightly-x86_64-apple-darwin 2018 Edition` 上均能正常运行。运行 `example/async-echo`  时设置 `RUST_LOG` 为 `info` 可以在 terminal 看到基本的运行信息，`debug` 则可见事件循环中的事件触发顺序。

### 异步 `IO` 的基石 - `mio`

`mio` 是一个极简的底层异步 `IO` 库，如今 `rust` 生态中几乎所有的异步 `IO` 程序都基于它。

随着 `channel`, `timer` 等 `sub module` 在 `0.6.5` 版本被标为 `deprecated`，如今的 mio 提供的唯二两个核心功能分别是：

- 对操作系统异步网络 `IO` 的封装
- 用户自定义事件队列

第一个核心功能对应到不同操作系统分别是

- `Linux(Android) => epoll`
- `Windows => iocp`
- `MacOS(iOS), FreeBSD => kqueue` 
- `Fuchsia => <unknown>`

mio 把这些不同平台上的 API 封装出了一套 `epoll like` 的异步网络 API，支持 `udp 和 tcp`。

> 除此之外还封装了一些不同平台的拓展 API，比如 `uds`，本文不对这些 API 做介绍。

#### 异步网络 IO

下面是一个 `tcp` 的 `demo`

```rust
// examples/tcp.rs

use mio::*;
use mio::net::{TcpListener, TcpStream};
use std::io::{Read, Write, self};
use failure::Error;
use std::time::{Duration, Instant};

const SERVER_ACCEPT: Token = Token(0);
const SERVER: Token = Token(1);
const CLIENT: Token = Token(2);
const SERVER_HELLO: &[u8] = b"PING";
const CLIENT_HELLO: &[u8] = b"PONG";

fn main() -> Result<(), Error> {
    let addr = "127.0.0.1:13265".parse()?;

// Setup the server socket
    let server = TcpListener::bind(&addr)?;

// Create a poll instance
    let poll = Poll::new()?;

// Start listening for incoming connections
    poll.register(&server, SERVER_ACCEPT, Ready::readable(),
                  PollOpt::edge())?;

// Setup the client socket
    let mut client = TcpStream::connect(&addr)?;

    let mut server_handler = None;

// Register the client
    poll.register(&client, CLIENT, Ready::readable() | Ready::writable(),
                  PollOpt::edge())?;

// Create storage for events
    let mut events = Events::with_capacity(1024);

    let start = Instant::now();
    let timeout = Duration::from_millis(10);
    'top: loop {
        poll.poll(&mut events, None)?;
        for event in events.iter() {
            if start.elapsed() >= timeout {
                break 'top
            }
            match event.token() {
                SERVER_ACCEPT => {
                    let (handler, addr) = server.accept()?;
                    println!("accept from addr: {}", &addr);
                    poll.register(&handler, SERVER, Ready::readable() | Ready::writable(), PollOpt::edge())?;
                    server_handler = Some(handler);
                }

                SERVER => {
                    if event.readiness().is_writable() {
                        if let Some(ref mut handler) = &mut server_handler {
                            match handler.write(SERVER_HELLO) {
                                Ok(_) => {
                                    println!("server wrote");
                                }
                                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                                err => {
                                    err?;
                                }
                            }
                        }
                    }
                    if event.readiness().is_readable() {
                        let mut hello = [0; 4];
                        if let Some(ref mut handler) = &mut server_handler {
                            match handler.read_exact(&mut hello) {
                                Ok(_) => {
                                    assert_eq!(CLIENT_HELLO, &hello);
                                    println!("server received");
                                }
                                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                                err => {
                                    err?;
                                }
                            }
                        }
                    }
                }
                CLIENT => {
                    if event.readiness().is_writable() {
                        match client.write(CLIENT_HELLO) {
                            Ok(_) => {
                                println!("client wrote");
                            }
                            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                            err => {
                                err?;
                            }
                        }
                    }
                    if event.readiness().is_readable() {
                        let mut hello = [0; 4];
                        match client.read_exact(&mut hello) {
                            Ok(_) => {
                                assert_eq!(SERVER_HELLO, &hello);
                                println!("client received");
                            }
                            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                            err => {
                                err?;
                            }
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
    };
    Ok(())
}
```

这个 `demo` 稍微有点长，接下来我们把它一步步分解。

直接看主循环

```rust
fn main() {
    // ...
    loop {
        poll.poll(&mut events, None).unwrap();
        // ...
    }
}
```

每次循环都得执行 `poll.poll`，第一个参数是用来存 `events` 的 `Events`， 容量是 `1024`；

```rust
let mut events = Events::with_capacity(1024);
```

第二个参数是 `timeout`，即一个 `Option<Duration>`，超时会直接返回。返回类型是 `io::Result<usize>`。

> 其中的 `usize` 代表 `events` 的数量，这个返回值是 `deprecated` 并且会在之后的版本移除，仅供参考

这里我们设置了 `timeout = None`，所以当这个函数返回时，必然是某些事件被触发了。让我们遍历 `events`：

```rust
  match event.token() {
      SERVER_ACCEPT => {
          let (handler, addr) = server.accept()?;
          println!("accept from addr: {}", &addr);
          poll.register(&handler, SERVER, Ready::readable() | Ready::writable(), PollOpt::edge())?;
          server_handler = Some(handler);
      }

      SERVER => {
          if event.readiness().is_writable() {
              if let Some(ref mut handler) = &mut server_handler {
                  match handler.write(SERVER_HELLO) {
                      Ok(_) => {
                          println!("server wrote");
                      }
                      Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                      err => {
                          err?;
                      }
                  }
              }
          }
          if event.readiness().is_readable() {
              let mut hello = [0; 4];
              if let Some(ref mut handler) = &mut server_handler {
                  match handler.read_exact(&mut hello) {
                      Ok(_) => {
                          assert_eq!(CLIENT_HELLO, &hello);
                          println!("server received");
                      }
                      Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                      err => {
                          err?;
                      }
                  }
              }
          }
      }
      CLIENT => {
          if event.readiness().is_writable() {
              match client.write(CLIENT_HELLO) {
                  Ok(_) => {
                      println!("client wrote");
                  }
                  Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                  err => {
                      err?;
                  }
              }
          }
          if event.readiness().is_readable() {
              let mut hello = [0; 4];
              match client.read_exact(&mut hello) {
                  Ok(_) => {
                      assert_eq!(SERVER_HELLO, &hello);
                      println!("client received");
                  }
                  Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                  err => {
                      err?;
                  }
              }
          }
      }
      _ => unreachable!(),
  }
```

我们匹配每一个 `event` 的 `token`，这里的 `token` 就是我用来注册的那些 `token`。比如我在上面注册了 `server`

```rust
// Start listening for incoming connections
poll.register(&server, SERVER_ACCEPT, Ready::readable(),
                  PollOpt::edge()).unwrap();

```

第二个参数就是 `token`

```rust
const SERVER_ACCEPT: Token = Token(0);
```

这样当 `event.token() == SERVER_ACCEPT` 时，就说明这个事件跟我们注册的 `server` 有关，于是我们试图 `accept` 一个新的连接并把它注册进 `poll`，使用的 `token` 是 `SERVER`。

```rust
let (handler, addr) = server.accept()?;
println!("accept from addr: {}", &addr);
poll.register(&handler, SERVER, Ready::readable() | Ready::writable(), PollOpt::edge())?;
server_handler = Some(handler);
```

这样我们之后如果发现 `event.token() == SERVER`，我们就认为它和注册的 `handler` 有关：

```rust
if event.readiness().is_writable() {
    if let Some(ref mut handler) = &mut server_handler {
        match handler.write(SERVER_HELLO) {
            Ok(_) => {
                println!("server wrote");
            }
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
            err => {
                err?;
            }
        }
    }
}
if event.readiness().is_readable() {
    let mut hello = [0; 4];
    if let Some(ref mut handler) = &mut server_handler {
        match handler.read_exact(&mut hello) {
            Ok(_) => {
                assert_eq!(CLIENT_HELLO, &hello);
                println!("server received");
            }
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
            err => {
                err?;
            }
        }
    }
}
```

这时候我们还需要判断 `event.readiness()`，这就是 `register` 函数的第三个参数，叫做 `interest`，顾名思义，就是“感兴趣的事”。它的类型是 `Ready`，一共四种，`readable, writable, error 和 hup`，可进行并运算。

在上面我们给 `handler` 注册了 `Ready::readable() | Ready::writable()`，所以 `event` 可能是 `readable` 也可能是 `writable`，所以我们要经过判断来执行相应的逻辑。注意这里的判断是

```rust
if ... {
    ...
}

if ... {
    ...
}
```

而非

```rust
if ... {
    ...
} else if ... {
    ...
}
```

因为一个事件可能同时是 `readable` 和 `writable`。

#### 容错性原则

大概逻辑先讲到这儿，这里先讲一下 `mio` 的“容错性原则”，即不能完全相信 `event`。

可以看到我上面有一段代码是这么写的 

```rust
match event.token() {
     SERVER_ACCEPT => {
         let (handler, addr) = server.accept()?;
         println!("accept from addr: {}", &addr);
         poll.register(&handler, SERVER, Ready::readable() | Ready::writable(), PollOpt::edge())?;
         server_handler = Some(handler);
     }
```

`server.accept()` 返回的是 `io::Result<(TcpStream, SocketAddr)>`。如果我们选择完全相信 `event` 的话，在这里 `unwrap()` 并没有太大问题 —— 如果真的有一个新的连接就绪，`accept()` 产生的 `io::Result` 是我们无法预料且无法处理的，我们应该抛给调用者或者直接 `panic`。

但问题就是，我们可以认为 `event` 的伪消息是可预料的，可能并没有一个新的连接准备就绪，这时候我们 `accept()` 会引发 `WouldBlock Error`。但我们不应该认为 `WouldBlock` 是一种错误 —— 这是一种友善的提醒。`server` 告诉我们：“并没有新的连接，请下次再来吧。”，所以在这里我们应该忽略（可以打个 `log`）它并重新进入循环。

像我后面写的那样：

```rust
match client.write(CLIENT_HELLO) {
   Ok(_) => {
       println!("client wrote");
   }
   Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
   err => {
       err?;
   }
}
```

#### Poll Option

好了，现在我们可以运行：

```bash
[async-io-demo] cargo run --example tcp
```

terminal 里打印出了

```bash
client wrote
accept from addr: 127.0.0.1:53205
client wrote
server wrote
server received
...
```

我们可以发现，在短短的 `10 millis` 内（`let timeout = Duration::from_millis(10);`），`server` 和 `client` 分别进行了数十次的读写！

如果我们不想进行这么多次读写呢？比如，我们只想让 `server` 写一次。在网络比较通畅的情况下，`client` 和 `server` 几乎一直是可写的，所以 `Poll::poll` 在数微秒内就返回了。

这时候就要看 `register` 的第四个参数了。


```rust
poll.register(&server, SERVER_ACCEPT, Ready::readable(),
                  PollOpt::edge()).unwrap();

```

`PollOpt::edge()` 的类型是 `PollOpt`，一共有 `level, edge, oneshot` 三种，他们有什么区别呢？

比如在我上面的代码里，

```rust
if event.readiness().is_readable() {
    let mut hello = [0; 4];
    match client.read_exact(&mut hello) {
        Ok(_) => {
            assert_eq!(SERVER_HELLO, &hello);
            println!("client received");
        }
        Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
        err => {
            err?;
        }
    }
}
```

我在收到一个 `readable readiness` 时，只读了四个字节。如果这时候缓冲区里有八字节的数据，那么：

- 如果我注册时使用 `PollOpt::level()`，我在下次 `poll` 时 **一定** 还能收到一次 `readable readiness event` （只要我没有主动执行 `set_readiness(Read::empty())`）；
- 如果我注册时使用 `PollOpt::edge()`，我在下次 `poll` 时 **不一定** 还能收到一次 `readable readiness event`；

所以，使用 `PollOpt::edge()` 时有一个“排尽原则（`Draining readiness`）”，即每次触发 `event` 时一定要操作到资源耗尽返回 `WouldBlock`，即上面的代码要改成：

```rust
if event.readiness().is_readable() {
    let mut hello = [0; 4];
    loop {
        match client.read_exact(&mut hello) {
            Ok(_) => {
                assert_eq!(SERVER_HELLO, &hello);
                println!("client received");
            }
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => break,
            err => {
                err?;
            }
        }
    }
}
```

那么，`oneshot` 又是怎样的行为呢？让我们回到上面的问题，如果我们只想让 `handler` 写一次，怎么办 —— 注册时使用 `PollOpt::oneshot()`，即

```rust
let (handler, addr) = server.accept()?;
println!("accept from addr: {}", &addr);
poll.register(&handler, SERVER_WRITE, Ready::writable(), PollOpt::oneshot())?;
server_handler = Some(handler);
```

这样的话，你只能收到一次 `SERVER_WRITE` 事件，除非你使用 `Poll::reregister` 重新注册 `handler`。

> `Poll::reregister` 可以更改 `PollOpt` 和 `interest`


#### Still Block

其实上面这个 `demo` 还存在一个问题，即我们在回调代码块中使用了同步的 `IO` 操作 `println!`。我们要尽可能避免在回调的代码块里使用耗时的 `IO` 操作。

考虑到文件 `IO` (包括 `Stdin, Stdout, Stderr`) 速度很慢，我们只需要把所有的文件 `IO` 交给一个线程进行即可。

```rust
use std::sync::mpsc::{Sender, Receiver, channel, SendError};

#[derive(Clone)]
pub struct Fs {
    task_sender: Sender<Task>,
}

impl Fs {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            loop {
                match receiver.recv() {
                    Ok(task) => {
                        match task {
                            Task::Println(ref string) => println!("{}", string),
                            Task::Exit => return
                        }
                    },
                    Err(_) => {
                        return;
                    }
                }
            }
        });
        Fs { task_sender: sender }
    }

    pub fn println(&self, string: String) {
        self.task_sender.send(Task::Println(string)).unwrap()
    }
}

pub enum Task {
    Exit,
    Println(String),
}
```

之后，可以使用 `Fs::println` 替换所有的 `println!`。

#### 自定义事件


上面我们实现异步 `println` 比较简单，这是因为 `println` 并没有返回值，不需要进行后续操作。设想一下，如果要我们实现 `open` 和 `ready_to_string`，先异步地 `open` 一个文件，然后异步地 `read_to_string`，最后再异步地 `println`, 我们要怎么做？

最简单的写法是回调，像这样：

```rust
// src/fs.rs

use crossbeam_channel::{unbounded, Sender};
use std::fs::File;
use std::io::Read;
use std::boxed::FnBox;
use std::thread;
use failure::Error;

#[derive(Clone)]
pub struct Fs {
    task_sender: Sender<Task>,
}

pub struct FsHandler {
    io_worker: thread::JoinHandle<Result<(), Error>>,
    executor: thread::JoinHandle<Result<(), Error>>,
}

pub fn fs_async() -> (Fs, FsHandler) {
    let (task_sender, task_receiver) = unbounded();
    let (result_sender, result_receiver) = unbounded();
    let io_worker = std::thread::spawn(move || {
        loop {
            match task_receiver.recv() {
                Ok(task) => {
                    match task {
                        Task::Println(ref string) => println!("{}", string),
                        Task::Open(path, callback, fs) => {
                            result_sender
                                .send(TaskResult::Open(File::open(path)?, callback, fs))?
                        }
                        Task::ReadToString(mut file, callback, fs) => {
                            let mut value = String::new();
                            file.read_to_string(&mut value)?;
                            result_sender
                                .send(TaskResult::ReadToString(value, callback, fs))?
                        }
                        Task::Exit => {
                            result_sender
                                .send(TaskResult::Exit)?;
                            break;
                        }
                    }
                }
                Err(_) => {
                    break;
                }
            }
        }
        Ok(())
    });
    let executor = std::thread::spawn(move || {
        loop {
            let result = result_receiver.recv()?;
            match result {
                TaskResult::ReadToString(value, callback, fs) => callback.call_box((value, fs))?,
                TaskResult::Open(file, callback, fs) => callback.call_box((file, fs))?,
                TaskResult::Exit => break
            };
        };
        Ok(())
    });

    (Fs { task_sender }, FsHandler { io_worker, executor })
}

impl Fs {
    pub fn println(&self, string: String) -> Result<(), Error> {
        Ok(self.task_sender.send(Task::Println(string))?)
    }

    pub fn open<F>(&self, path: &str, callback: F) -> Result<(), Error>
        where F: FnOnce(File, Fs) -> Result<(), Error> + Sync + Send + 'static {
        Ok(self.task_sender.send(Task::Open(path.to_string(), Box::new(callback), self.clone()))?)
    }

    pub fn read_to_string<F>(&self, file: File, callback: F) -> Result<(), Error>
        where F: FnOnce(String, Fs) -> Result<(), Error> + Sync + Send + 'static {
        Ok(self.task_sender.send(Task::ReadToString(file, Box::new(callback), self.clone()))?)
    }

    pub fn close(&self) -> Result<(), Error> {
        Ok(self.task_sender.send(Task::Exit)?)
    }
}

impl FsHandler {
    pub fn join(self) -> Result<(), Error> {
        self.io_worker.join().unwrap()?;
        self.executor.join().unwrap()
    }
}

type FileCallback = Box<FnBox(File, Fs) -> Result<(), Error> + Sync + Send>;
type StringCallback = Box<FnBox(String, Fs) -> Result<(), Error> + Sync + Send>;

pub enum Task {
    Exit,
    Println(String),
    Open(String, FileCallback, Fs),
    ReadToString(File, StringCallback, Fs),
}

pub enum TaskResult {
    Exit,
    Open(File, FileCallback, Fs),
    ReadToString(String, StringCallback, Fs),
}

```

```rust
// examples/fs.rs

use asyncio::fs::fs_async;
use failure::Error;

const TEST_FILE_VALUE: &str = "Hello, World!";

fn main() -> Result<(), Error> {
    let (fs, fs_handler) = fs_async();
    fs.open("./examples/test.txt", |file, fs| {
        fs.read_to_string(file, |value, fs| {
            assert_eq!(TEST_FILE_VALUE, &value);
            fs.println(value)?;
            fs.close()
        })
    })?;
    fs_handler.join()?;
    Ok(())
}
```

测试

```bash
[async-io-demo] cargo run --example fs
```

这样写在逻辑上的确是对的，但是负责跑 `callback` 的 `executor` 线程其实被负责 `io` 的线程阻塞住了（`result_receiver.recv()`）。那我们能不能在 `executor` 线程里跑一个事件循环，以达到不被 `io` 线程阻塞的目的呢？（即确定 `result_receiver` 中有 `result` 时，`executor` 才会进行 `result_receiver.recv()`）.

这就到了体现 `mio` 强大可拓展性的时候：注册用户态的事件队列。

把上面的代码稍加修改，就成了这样：

```rust
// src/fs_mio.rs

use crossbeam_channel::{unbounded, Sender, TryRecvError};
use std::fs::File;
use std::io::{Read};
use std::boxed::FnBox;
use std::thread;
use failure::Error;
use std::time::Duration;
use mio::*;

#[derive(Clone)]
pub struct Fs {
    task_sender: Sender<Task>,
}

pub struct FsHandler {
    io_worker: thread::JoinHandle<Result<(), Error>>,
    executor: thread::JoinHandle<Result<(), Error>>,
}

const FS_TOKEN: Token = Token(0);

pub fn fs_async() -> (Fs, FsHandler) {
    let (task_sender, task_receiver) = unbounded();
    let (result_sender, result_receiver) = unbounded();
    let poll = Poll::new().unwrap();
    let (registration, set_readiness) = Registration::new2();
    poll.register(&registration, FS_TOKEN, Ready::readable(), PollOpt::oneshot()).unwrap();
    let io_worker = std::thread::spawn(move || {
        loop {
            match task_receiver.recv() {
                Ok(task) => {
                    match task {
                        Task::Println(ref string) => println!("{}", string),
                        Task::Open(path, callback, fs) => {
                            result_sender
                                .send(TaskResult::Open(File::open(path)?, callback, fs))?;
                            set_readiness.set_readiness(Ready::readable())?;
                        }
                        Task::ReadToString(mut file, callback, fs) => {
                            let mut value = String::new();
                            file.read_to_string(&mut value)?;
                            result_sender
                                .send(TaskResult::ReadToString(value, callback, fs))?;
                            set_readiness.set_readiness(Ready::readable())?;
                        }
                        Task::Exit => {
                            result_sender
                                .send(TaskResult::Exit)?;
                            set_readiness.set_readiness(Ready::readable())?;
                            break;
                        }
                    }
                }
                Err(_) => {
                    break;
                }
            }
        }
        Ok(())
    });

    let executor = thread::spawn(move || {
        let mut events = Events::with_capacity(1024);
        'outer: loop {
            poll.poll(&mut events, Some(Duration::from_secs(1)))?;
            for event in events.iter() {
                match event.token() {
                    FS_TOKEN => {
                        loop {
                            match result_receiver.try_recv() {
                                Ok(result) => {
                                    match result {
                                        TaskResult::ReadToString(value, callback, fs) => callback.call_box((value, fs))?,
                                        TaskResult::Open(file, callback, fs) => callback.call_box((file, fs))?,
                                        TaskResult::Exit => break 'outer
                                    }
                                }
                                Err(e) => {
                                    match e {
                                        TryRecvError::Empty => break,
                                        TryRecvError::Disconnected => Err(e)?
                                    }
                                }
                            }
                        }
                        poll.reregister(&registration, FS_TOKEN, Ready::readable(), PollOpt::oneshot())?;
                    }
                    _ => unreachable!()
                }
            }
        };
        Ok(())
    });
    (Fs { task_sender }, FsHandler { io_worker, executor })
}

impl Fs {
    pub fn println(&self, string: String) -> Result<(), Error> {
        Ok(self.task_sender.send(Task::Println(string))?)
    }

    pub fn open<F>(&self, path: &str, callback: F) -> Result<(), Error>
        where F: FnOnce(File, Fs) -> Result<(), Error> + Sync + Send + 'static {
        Ok(self.task_sender.send(Task::Open(path.to_string(), Box::new(callback), self.clone()))?)
    }

    pub fn read_to_string<F>(&self, file: File, callback: F) -> Result<(), Error>
        where F: FnOnce(String, Fs) -> Result<(), Error> + Sync + Send + 'static {
        Ok(self.task_sender.send(Task::ReadToString(file, Box::new(callback), self.clone()))?)
    }

    pub fn close(&self) -> Result<(), Error> {
        Ok(self.task_sender.send(Task::Exit)?)
    }
}

impl FsHandler {
    pub fn join(self) -> Result<(), Error> {
        self.io_worker.join().unwrap()?;
        self.executor.join().unwrap()
    }
}

type FileCallback = Box<FnBox(File, Fs) -> Result<(), Error> + Sync + Send>;
type StringCallback = Box<FnBox(String, Fs) -> Result<(), Error> + Sync + Send>;

pub enum Task {
    Exit,
    Println(String),
    Open(String, FileCallback, Fs),
    ReadToString(File, StringCallback, Fs),
}

pub enum TaskResult {
    Exit,
    Open(File, FileCallback, Fs),
    ReadToString(String, StringCallback, Fs),
}

```

```rust
// examples/fs-mio.rs

use asyncio::fs_mio::fs_async;
use failure::Error;

const TEST_FILE_VALUE: &str = "Hello, World!";

fn main() -> Result<(), Error> {
    let (fs, fs_handler) = fs_async();
    fs.open("./examples/test.txt", |file, fs| {
        fs.read_to_string(file, |value, fs| {
            assert_eq!(TEST_FILE_VALUE, &value);
            fs.println(value)?;
            fs.close()
        })
    })?;
    fs_handler.join()?;
    Ok(())
}
```



可以注意到，上面的代码发生的改变就是，`executor` 不再被 `result_receiver.recv` 阻塞，而变成了注册事件（`registration`）后等待 `Poll::poll` 返回事件；只有等到了新的事件，才会进行 `result_receiver.try_recv`。同时，`io_worker` 线程在 `send result` 之后会执行 `set_readiness.set_readiness(Ready::readable())?;`，以通知 `executor` 线程对相应结果做处理。

这样的话，`executor` 就不会被 `io worker` 阻塞了，因为我们可以把所有的事件都注册到 `executor` 上，`mio::Poll` 会同时监听多个事件（比如把 `fs` 和 `tcp` 结合起来）。

测试

```bash
[async-io-demo] cargo run --example fs-mio
```

#### Callback is evil

既然文件 `IO` 的 `executor` 不再会被 `io worker` 线程阻塞了，那我们来试试让 `fs` 和 `tcp`  共用一个 `poll` 然后建立一个简单的文件服务器吧。

但可以先等等，因为我已经开始觉得写 `callback` 有点难受了 —— 如果我们还想处理错误的话，会觉得更难受，像这样

```rust
use asyncio::fs_mio::fs_async;
use failure::Error;

const TEST_FILE_VALUE: &str = "Hello, World!";

fn main() -> Result<(), Error> {
    let (fs, fs_handler) = fs_async();
    fs.open("./examples/test.txt", 
        |file, fs| {
            fs.read_to_string(file, 
                |value, fs| {
                    assert_eq!(TEST_FILE_VALUE, &value);
                    fs.println(value, 
                        |err| {
                            ...
                        }
                    );
                    fs.close()
                },
                |err| {
                    ...
                }
            )
        },
        |err| {
            ...
        }
    )?;
    fs_handler.join()?;
    Ok(())
}
```

而且对 `rust` 来说，更加艰难的是闭包中的生命周期问题（闭包几乎不能通过捕获来借用环境变量）。这就意味着，如果我要借用环境中的某个变量，我要么 `clone` 它（如果它实现了 `Clone` 的话），要么把它作为闭包参数传入（意味着你要根据需要改每一层回调函数的签名，这太屎了）。

考虑到各种原因，`rust` 最终选择用 `coroutine` 作为异步 `IO` 的 `API` 抽象。

### coroutine

这里所说的 `coroutine` 是指基于 `rust generator` 的 `stackless coroutine` 而非早期被 `rust` 抛弃的 `green thread(stackful coroutine)`。

#### generator

`rust` 大概在今年五月份引入了 `generator`，但到现在还是 unstable 的 —— 虽说也没多少人用 stable（误

一个典型的斐波那契 `generator` 如下

```rust
// examples/fab.rs

#![feature(generators, generator_trait)]

use std::ops::{Generator, GeneratorState};

fn main() {
    let mut gen = fab(5);
    loop {
        match unsafe { gen.resume() } {
            GeneratorState::Yielded(value) => println!("yield {}", value),
            GeneratorState::Complete(ret) => {
                println!("return {}", ret);
                break;
            }
        }
    }
}

fn fab(mut n: u64) -> impl Generator<Yield=u64, Return=u64> {
    move || {
        let mut last = 0u64;
        let mut current = 1;
        yield last;
        while n > 0 {
            yield current;
            let tmp = last;
            last = current;
            current = tmp + last;
            n -= 1;
        }
        return last;
    }
}
```

由于 `generator` 的“中断特性”，我们很自然的可以想到，如果用 `generator` 搭配 `mio`，给每个 `generator` 分配一个 `token`，然后 `poll mio` 的事件循环，收到一个唤醒事件就 `resume` 相应的 `generator`；每个 `generator` 在要阻塞的时候拿自己的 `token` 注册一个唤醒事件然后 `yield`，不就实现了“同步代码”的异步 `IO` 吗？

这样看来原理上来说已经稳了，但 `rust` 异步 `IO` 的天空依旧漂浮着两朵乌云。

#### 自引用

第一朵乌云和 `rust` 自身的内存管理机制有关。

如果你写出这样的 `generator`

```rust
fn self_ref_generator() -> impl Generator<Yield=u64, Return=()> {
    || {
        let x: u64 = 1;
        let ref_x: &u64 = &x;
        yield 0;
        yield *ref_x;
    }
}
```

`rust` 一定会给你抛个错然后告诉你 "borrow may still be in use when generator yields"。编译器没有教你怎么修正可能会让你有些恐慌，去不存在的搜索引擎上查了查，你发现这和 `generator` 的实现有关。

前文中提到，`rust generator` 是 `stackless` 的，即它并不会保留一个完整的栈，而是根据不同的状态保留需要的变量。如果你把上面的代码改成

```rust
fn no_ref_generator() -> impl Generator<Yield=u64, Return=()> {
    || {
        let x: u64 = 1;
        let ref_x: &u64 = &x;
        yield *ref_x;
        yield 0;
    }
}
```

在第一次 `yield` 结束之后，编译器会发现 `generator` 唯一需要保留的是字面量 `0`，所以这段代码可以顺利编译通过。但是，对于前面的 `generator`，第一次 `yield` 过后，编译器发现你需要同时保留 `x` 和它的引用 `ref_x`，这样的话 `generator` 就会变成类似这样的结构（仅供参考）：

```rust
enum SomeGenerator<'a> {
    ...
    SomeState {
        _yield: u64,
        x: u64
        ref_x: &'a u64
    }
    ...
}
```



这就是 `rust` 中“臭名昭著” 的自引用，下面这段代码会发生什么呢



```rust
struct A<'a> {
    b: u64,
    ref_b: Option<&'a u64>
}

impl<'a> A<'a> {
    fn new() -> Self {
        let mut a = A{b: 1, ref_b: None};
		a.ref_b = Some(&a.b);
        a
    }
}
```



你会发现它编译不过，当然这是很合理的，栈上的 a 变量拷贝出去之后其成员 b 的引用会失效，`rust`的生命周期机制帮你规避了这个问题。但即使你改成这样



```rust
use std::borrow::{BorrowMut};

struct A<'a> {
    b: u64,
    ref_b: Option<&'a u64>
}

impl<'a> A<'a> {
    fn boxed() -> Box<Self> {
        let mut a = Box::new(A{b: 1, ref_b: None});
        let mut_ref: &mut A = a.borrow_mut();
		mut_ref.ref_b = Some(&mut_ref.b);
        a
    }
}
```



这样按道理来说是没问题的，因为 a 的实体已经在堆上了，即使你拷贝它在栈上的引用，也不会改变其成员 b 的地址，引用一直是有效的 —— 但问题是，你没法跟编译器解释这事，编译器认为函数里面的 `&mut_ref.b`只能活到函数结束，这样含有这个引用的 a 自然也不能 move 出来。

那你可能会想，那我就在外面再取引用就好了

```rust
struct A<'a> {
    b: u64,
    ref_b: Option<&'a u64>
}

impl<'a> A<'a> {
    fn new() -> Self {
        A{b: 1, ref_b: None}
    }
}

fn main() {
    let mut a = A::new();
    a.ref_b = Some(&a.b);
}
```



这样的确没啥毛病，但是，你会发现自引用不仅阻止了 move，还阻止了你对 A 可变引用。。比如这样就编译不过

```rust
struct A<'a> {
    b: u64,
    ref_b: Option<&'a u64>
}

impl<'a> A<'a> {
    fn new() -> Self {
        A{b: 1, ref_b: None}
    }

    fn mute(&mut self) {

    }
}

fn main() {
    let mut a = A::new();
    a.ref_b = Some(&a.b);
    a.mute();
}
```



但远古的 `Future::poll` 签名就长这样

```rust
fn poll(&mut self) -> Poll<Self::Item, Self::Error>;
```

而直到现在 `Generator::resume` 的签名还是这样

```rust
unsafe fn resume(&mut self) -> GeneratorState<Self::Yield, Self::Return>;
```

这样的话自引用会导致 `generator` 无法实现 `Generator` 和 `Future` 

在这种情况下，我们可以使用 `NonNull`来避过编译器的检查

```rust
use std::ptr::NonNull;

struct A {
    b: u64,
    ref_b: NonNull<u64>
}

impl A {
    fn new() -> Self {
        A{b: 1, ref_b: NonNull::dangling()}
    }
}

fn main() {
    let mut a = A::new();
    a.ref_b = NonNull::from(&a.b);
}
```



这样的确没有了烦人的生命周期约束，但也意味着你要自己保证内存安全 —— 绝对不能 move，也不能对其可变引用使用 `mem::replace` 或 `mem::swap` ，这样非常不妙。

#### Pin

那有没有办法通过其它方式来保证能保证它不能被 move 或者取可变引用呢？这就是 `pin`的应用场景了。`pin`具体的内容可以看这篇 [RFC](https://github.com/rust-lang/rfcs/blob/master/text/2349-pin.md)，本文只是简要说明一下。

`rust` 默认给大部分类型实现了 `trait std::marker::Unpin`，这只是一个标记，表示这个类型 move 是安全的，这时候，`Pin<'a, T>` 跟 `&'a mut T` 没有区别，你也可以安全地通过 `Pin::new(&mut T)` 和 `Pin::as_mut(self: &mut Pin<T>)`相互转换。

但对于不能安全 move 的类型，比如上面的 `A`，我们得先把它标记为 `!Unpin`，安全的标记方法是给它一个 `!Unpin`的成员，比如 `Pinned`。

```rust
#![feature(pin)]
use std::marker::{Pinned};

use std::ptr::NonNull;

struct A {
    b: u64,
    ref_b: NonNull<u64>,
    _pin: Pinned,
}

impl A {
    fn new() -> Self {
        A {
            b: 1,
            ref_b: NonNull::dangling(),
            _pin: Pinned,
        }
    }
}

fn main() {
    let mut a = A::new();
    let mut pinned = unsafe { Pin::new_unchecked(&mut a) };
    let ref_b = NonNull::from(&pinned.b);
    let mut_ref: Pin<&mut A> = pinned.as_mut();
    unsafe {Pin::get_mut_unchecked(mut_ref).ref_b = ref_b};
    let unmoved = pinned;
    assert_eq!(unmoved.ref_b, NonNull::from(&unmoved.b));
}
```



从 `!Unpin` 的类型构建 `Pin` 总是 `unsafe` 的，它们通过 `Pin::new_unchecked` 和 `Pin::get_mut_unchecked` 相互转换。当然，我们在构建时是可以保证它是 `safe` ，我们只要完成这两个 `unsafe`的操作，就可以保证：

- 永远不能 `safe` 地获得可变引用： `Pin::get_mut_unchecked` 是 `unsafe` 的
- 永远不能 `safe` 地 move：因为 `Pin` 只拥有可变引用，且由于`Pin::get_mut_unchecked` 是 `unsafe` 的，你不能 `safe` 地对其可变引用使用 `mem::replace` 或 `mem::swap`

当然，如果你不想在构建时使用 `unsafe`或者想获得 `a` 的所有权以便在函数间传递，你可以使用 `Box::pinned`从而把它分配在堆上

```rust
struct A {
    b: u64,
    ref_b: NonNull<u64>,
    _pin: Pinned,
}

impl A {
    fn boxed() -> Pin<Box<Self>> {
        let mut boxed = Box::pinned(A {
            b: 1,
            ref_b: NonNull::dangling(),
            _pin: Pinned,
        });
        let ref_b = NonNull::from(&boxed.b);
        let mut_ref: Pin<&mut A> = boxed.as_mut();
        unsafe { Pin::get_mut_unchecked(mut_ref).ref_b = ref_b };
        boxed
    }
}

fn main() {
    let boxed = A::boxed();
    let unmoved = boxed;
    assert_eq!(unmoved.ref_b, NonNull::from(&unmoved.b));
}
```

有了 `Pin` 之后，新版 `Future` 的定义就是这样的了

```rust
pub trait Future {
    type Output;
    fn poll(self: Pin<&mut Self>, lw: &LocalWaker) -> Poll<Self::Output>;
}
```

#### 合理的抽象

既然已经打算钦定了 `coroutine` 作为异步 `IO` 的 `API` 抽象，那应该把哪些东西加入标准库、哪些东西加入语法支持、哪些东西交给第三方实现呢？让开发者手动调用 `unsafe` 的 `Generator::resume` 终归不是很妙，也不好把 `mio` 作为唯一的底层异步 `IO` 实现（如果这样的话不如把 `mio` 也并入标准库）。

现在的 `rust` 提供了 `async` 的语法支持（以前是用过程宏的实现的）、`await!`的标准库宏支持，标准库 `std::future` 的 `trait Future` 和 `struct GenFuture` ， 标准库 `std::task` 的  `enum Poll<T>, struct LocalWaker, struct Waker ` 和 `trait UnsafeWaker`。

你需要给你的 `MyWaker` 实现 `trait UnsafeWaker`，用 `mio` 的话就用 `SetReadiness`，`unsafe fn wake(&self)` 用 `SetReadiness::set_readiness` 实现。然后把 `MyWaker` 包在 `Waker, LocalWaker` 里面。



##### Poll\<T\>

`Poll<T>` 的定义为

```rust
pub enum Poll<T> {
    Ready(T),
    Pending,
}
```



##### await!

`await!` 宏只能在 `async` 函数或者块里面用，传入一个 `Future`

`await!(future)`会被展开成

```rust
loop {
    if let Poll::Ready(x) = ::future::poll_with_tls(unsafe{
        Pin::new_unchecked(&mut future)
    }) {
        break x;
    }
    yield
}
```

`::future::poll_with_tls` 即 `thread-local waker`，就是你传给这个 `GenFuture::poll` 的 `LocalWaker`，



##### async

`async`则会把 `Generator` 包装成 `Future(GenFuture)` 。

`GenFuture` 的相关定义如下

```rust
struct GenFuture<T: Generator<Yield = ()>>(T);

impl<T: Generator<Yield = ()>> !Unpin for GenFuture<T> {}

impl<T: Generator<Yield = ()>> Future for GenFuture<T> {
    type Output = T::Return;
    fn poll(self: Pin<&mut Self>, lw: &LocalWaker) -> Poll<Self::Output> {
        set_task_waker(lw, || match unsafe { Pin::get_mut_unchecked(self).0.resume() } {
            GeneratorState::Yielded(()) => Poll::Pending,
            GeneratorState::Complete(x) => Poll::Ready(x),
        })
    }
}

pub fn from_generator<T: Generator<Yield = ()>>(x: T) -> impl Future<Output = T::Return> {
    GenFuture(x)
}
```



这里可以看到，`GenFuture` 在每次调用 `self.0.resume` 之前会 `set_task_waker`，通过一个 `thread_local` 的变量中转，从而 `generator` 里面的 `future::poll` 能通过 `poll_with_tls` 拿到这个 `LocalWaker`。

所以，下面的代码

```rust
async fn async_recv(string_channel: Receiver<String>) -> String {
    await!(string_channel.recv_future())
}
```

会被类似地展开为这样

```rust
fn async_recv(string_channel: Receiver<String>) -> impl Future<Output = T::Return> {
	from_generator(move || {
        let recv_future = string_channel.recv_future();
        loop {
            if let Poll::Ready(x) = ::future::poll_with_tls(unsafe{
                Pin::new_unchecked(&mut recv_future)
            }) {
                break x;
            }
            yield
        }
    })
}
```

#### asynchronous coroutine

掌握了上文的基础知识后，我们就可以开始实践了。

coroutine 本身并不意味着“异步”，你完全可以在两次 `yield` 之间调用同步 `IO` 的 `API` 从而导致 `IO` 阻塞。 异步的关键在于，在将要阻塞的时候（比如某个 `API` 返回了 `io::ErrorKind::WouldBlock`），`GenFuture::poll`中 用底层异步接口注册一个事件和唤醒回调（`waker`）然后自身休眠（`yield`），底层异步调度在特定事件发生的时候回调唤醒这个 `Future`。

下面我参照 `romio` 的异步调度实现了 `Executor` `block_on, spawn, TcpListener` 和 `TcpStream`，代码较长，建议 `clone` 后用编辑器看。（请注意区分 `Poll(mio::Poll)` 与 `task::Poll` 以及 `net::{TcpListener, TcpStream}(mio::net::{TcpListener, TcpStream})` 与 `TcpListener, TcpStream`）

[src/executor.rs](https://github.com/Hexilee/async-io-demo/blob/master/src/executor.rs)

##### Executor

`Executor` 中包含 `mio::Poll`，`main task waker` 及用来管理 `task` 和 `source` 的 `Slab` 各一个。其本身并没有实现什么特别的方法，主要是初始化为 `thread_local` 的 `EXECUTOR` 供其它函数借用。

##### block_on

`block_on` 函数会阻塞当前线程，传入参数是一个 `future: Future<Output=T>`，被称为 `main task`；返回值类型是 `T`。该函数一般在最外层被调用。

`block_on` 会引用 `thread_local EXECUTOR`，主要逻辑是调用 `mio::Poll::poll` 来响应事件。`block_on` 把 `0 - MAX_RESOURCE_NUM(1 << 31)` 个 `Token` 分为三类。

- `main task token`

  收到 `Token` 为 `MAIN_TASK_TOKEN` 的事件即表示需要唤醒 `main task`，执行 `main_task.poll`，返回 `task::Poll::Ready(T)` 则 `block_on` 函数返回。

- `task token`

  奇数 `token` 表示由 `spawn` 函数分发的其它任务需要被唤醒，执行相应的 `task.poll`，`token` 和该事件在 `EXECUTOR.tasks` 中的 `index` 一一映射。

- `source token`

  偶数 `token` 表示由 `register_source` 函数注册的 `source`需要被分发，执行相应 `source` 的 `waker()` 以唤醒分发它们的 `task`。

##### spawn

分发任务

##### TcpListener

包装了 `mio::net::TcpListener`，`accept` 方法返回一个 `Future`。

##### TcpStream

包装了 `mio::net::TcpStream`, `read`和 `write` 方法均返回 `Future`。



### 后记

实现了 `executor` 之后，我们就可以运行文章开头给的 `example`	 了，

```bash
RUST_LOG=info cargo run --example async-echo
```

可以用 `telnet` 连连试试看。

当然最后还留了一个问题，就是把文件 `IO` 也封装为 `coroutine` 的异步 `IO`，当然我还没有写，读者有兴趣可以试着实现一下，我们接下来再谈谈现在 `coroutine API` 的不足。

我目前发现的主要问题就是不能在 `Future::poll` 或者 `async` 中使用 `try`，导致出现 `Result` 的地方只能 `match`，希望之后会有比较好的解决方案。

第二个问题是 `Waker` 最里面装的是 `UnsafeWaker`的 `NonNull` 指针，当然我能理解 `rust` 团队有性能等其它方面的考虑，但如果用 `mio` 的 `set_readiness` 封装出 `MyWaker` 的话，`clone` 完全不需要 `NonNull`，而且我在实际编码时因为这个出过空指针错误。。希望以后能提供一个更安全的选择。