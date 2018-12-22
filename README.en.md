### Introduction

2019 is approaching. The rust team keeps their promise about asynchronous IO: `async` is introduced as keywords, `Pin, Future, Poll` and `await!` is introduced into standard library. 

I have never used rust for asynchronous IO programming before, so I almost know nothing about it. However, I would use it for a project recently but couldn't find many documents that are remarkably helpful for newbie of rust asynchronous programming.

Eventually, I wrote several demo and implemented simple asynchronous IO based on `mio` and `coroutine` with the help of both of this blog ([Tokio internals: Understanding Rust's asynchronous I/O framework from the bottom up](https://cafbit.com/post/tokio_internals/)) and source code of "new tokio" [romio](https://github.com/withoutboats/romio) .

This is the final file server:

```rust
// examples/file-server.rs

#![feature(async_await)]
#![feature(await_macro)]

#[macro_use]
extern crate log;

use asyncio::executor::{block_on, spawn, TcpListener};
use asyncio::fs_future::{read_to_string};
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
                        await!(stream.write_str("Please enter filename: ")).expect("write to stream fail");
                        let file_name_vec = await!(stream.read()).expect("read from stream fail");
                        let CRLF: &[char] = &['\r', '\n'];
                        let file_name = String::from_utf8(file_name_vec).unwrap().trim_matches(CRLF).to_owned();
                        let file_contents = await!(read_to_string(file_name)).expect("read to string from file fail");
                        await!(stream.write_str(&file_contents)).expect("write file contents to stream fail");
                        stream.close();
                    },
                )
                .expect("spawn stream fail");
            }
        },
    )
}
```

My purpose of writing this blog is to review and summarize, I will be happy if it can help someone who are interested in rust asynchronous programming. Given that the readability is the primary consideration when I wrote the code appearing in this blog, there may be some performance problem in code, please forgive me. If there are obvious problems in blog or code, you are welcome to point them up.

Most of the code appearing in this blog is collected in this [repo](https://github.com/Hexilee/async-io-demo) (some code is too long, you had better clone and view it in editor), all examples work well at `nightly-x86_64-apple-darwin 2018 Edition, rustc-1.32.0`. 

> When executing examples/async-echo, set environment variable `RUST_LOG=info` for basic runtime information; set `RUST_LOG=debug`  for events polling information.



### mio: The Footstone of Asynchronous IO

`mio` is a tidy, low-level asynchronous IO library. Nowadays, almost all asynchronous IO libraries in rust ecosystem are based on `mio`.

As sub modules like `channel, timer` have been marked as deprecated since version-0.6.5, `mio` provides only two core functions:

- basic encapsulation for OS asynchronous network IO
- custom events

The first core function corresponded to API in different OS respectively are:

- Linux(Android) => epoll
- Windows => iocp
- MacOS(iOS), FreeBSD => kqueue
- Fuchsia => \<unknow>

`mio` wraps different asynchronous network API in different OS into a common epoll-like asynchronous API, which supports both of `udp` and `tcp`.

> besides `udp` and `tcp`, `mio` also provides some OS-specific API, like `uds`, we won't talk about them, you can find the usage in source code of mio.
>
>



#### Asynchronous Network IO

This is a demo of asynchronous tcp IO:

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



This demo is a little long, let's talk about its main loop first:

```rust
fn main() {
    // ...
    loop {
        poll.poll(&mut events, None).unwrap();
        // ...
    }
}
```

We need call `Poll::poll` in each loop, the first parameter `events` is used for store events, we set it   with capacity 1024 here.

```rust
let mut events = Events::with_capacity(1024);
```

The type of second parameter `timeout` is `Option<Duration>`, method will return `Ok(usize)` when some events occur or timeout if `Some(duration) = timeout`. 

> The usize in Ok refers to the number of events, this value is deprecated and will be removed in 0.7.0.

Here we deliver `timeout = None` ，so when the method return without error, there must be some events. Let's traverse events:

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

We need match token of each event, these tokens are just those we used to register. For example, we register `server` using token `SERVER_ACCEPT`.

```rust
const SERVER_ACCEPT: Token = Token(0);

...

// Start listening for incoming connections
poll.register(&server, SERVER_ACCEPT, Ready::readable(),
                  PollOpt::edge()).unwrap();
```

In this case, when we find `event.token() == SERVER_ACCEPT`, we should think it's relevant to `server`, so we try to accept a new `TcpStream` and register it, using token `SERVER`:

```rust
let (handler, addr) = server.accept()?;
println!("accept from addr: {}", &addr);
poll.register(&handler, SERVER, Ready::readable() | Ready::writable(),       PollOpt::edge()).unwrap();
server_handler = Some(handler);
```

As the same, if we find `event.token() == SERVER`，we should think it's relevant to `handler`:

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

In this case, we shoud response differently to different `event.readiness()`, this is the third parameter of register, which named `interest`. As its name, `interest` means 'something you are interested', its type is `Ready`. `mio` support four kinds of `Ready`, `readable`, `writable`, `error` and `hup`, you can union them.

We register `handler`with `Ready::readable() | Ready::writable()`, so event can be `readable` or `writable` or both, you can see it in control flow: 

using

```rust
if event.readiness().is_writable() {
    ...
}

if event.readiness().is_readable() {
    ...
}
```

instead of

```rust
if event.readiness().is_writable() {
    ...
} else if event.readiness().is_readable() {
    ...
}
```

#### Spurious Events

The code for dealing with event `SERVER_ACCEPT` above is:

```rust
match event.token() {
     SERVER_ACCEPT => {
         let (handler, addr) = server.accept()?;
         println!("accept from addr: {}", &addr);
         poll.register(&handler, SERVER, Ready::readable() | Ready::writable(), PollOpt::edge()).unwrap();
         server_handler = Some(handler);
     }
```



The result `server.accept()` returned is `io::Result<(TcpStream, SocketAddr)>`.  If we trust `event` entirely, using `try` is the right choise (if there should be a new `TcpStream` is ready, `server.accept()` returning `Err` is unforeseen and unmanageable).

However, we should think event may be spurious, the possibility depends on OS and custom implement. There may not be a new `TcpStream` is ready, in this case, `server.accept()` will return `WouldBlock Error`. We should regard `WouldBlock Error ` as a friendly warning: "there isn't a new `TcpStream` is ready, please do it later again." So we should ignore it and continue the loop.

Like the code for dealing with event `SERVER`

```rust
match handler.write(SERVER_HELLO) {
    Ok(_) => {
        println!("server wrote");
    }
    Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => continue,
    err => {
        err?;
    }
}
```

#### Poll Option

Now we can execute:

```bash
cargo run --example tcp
```

The terminal print some log:

```bash
client wrote
accept from addr: 127.0.0.1:53205
client wrote
server wrote
server received
...
```

We can see, in 10 milliseconds (`let timeout = Duration::from_millis(10);`), `server` and `client` did dozens of  writing and reading!

How should we do if we don't need dozens of writing and reading? In a pretty network environment, `client ` and `server` is almost always writable, so `Poll::poll` may return in dozens of microseconds.

In this case, we should change the forth parameter of `register`:

```rust
poll.register(&server, SERVER_ACCEPT, Ready::readable(),
                  PollOpt::edge()).unwrap();
```

The type of `PollOpt::edge()` is `PollOpt`, means poll option. There are three kinds of poll options: `level`, `edge` and `oneshot`, what's the difference of them?

For example, in this code:

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



When I receive a readable readiness, I read 4 bytes only. If there are 8 bytes in buffer:

- if I register this `TcpStream` with `PollOpt::level()`, I **MUST** receive a `readable readiness event` in next polling;
- if I register this `TcpStream` with `PollOpt::edge()`, I **MAY** cannot receive a `readable readiness event` in next polling;

So, we can say that readiness using edge-triggered mode is a `Draining readiness`, once a readiness event is received, the corresponding operation must be performed repeatedly until it returns `WouldBlock`. We should alter code above into:

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



Then, what's the behavior of `PollOpt::onshot()`? Let's talk about the first question of this section: if we want `handler` to write only once, how should we do? The answer is: register it using `PollOpt::oneshot()`

```rust
let (handler, addr) = server.accept()?;
println!("accept from addr: {}", &addr);
poll.register(&handler, SERVER, Ready::readable() | Ready::writable(), PollOpt::oneshot())?;
server_handler = Some(handler);
```

In this case, you can only receive event `SERVER` once, unless you re-register `handler` using `Poll::reregister`.

> `Poll::reregister` can using different `PollOpt` and `interest` from the last registering



#### Still Block

There is still a problem in the code above: we using blocking IO macro `println!`. We should avoid using blocking IO in the code for dealing events.

Given that FS(file-system) IO(including `stdin, stdout, stderr`) is slow, we can send all FS-IO task to a worker thread.

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

Now, we can replace all `println!` with `Fs::println`.

#### Custom Event 

Implementing non-blocking `println` is easy, because this function has no return. How should we do if we want other non-blocking FS-IO functions? For example, opening a file, then reading it to string, then printing the string, how should we do?

The easiest way is using callback, like this:

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

running this example:

```bash
cargo run --example fs
```

This implementation work well, but the executor thread is still blocked by the io-worker thread `(result_receiver.recv()`). Can we run a polling loop in executor thread to not be blocked by IO? (executor should execute `(result_receiver.recv()` only when there are some results in result channel).

To implement a non-blocking executor, we can use custom events supported by `mio`.

Altering the code above:

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



running this example:

```bash
cargo run --example fs-mio
```



We can see the difference between two implementations. On the one hand, executor will never be blocking by `result_receiver.recv()`, instead, it will wait for `Poll::poll` returning; on the other hand, io worker thread will execute `set_readiness.set_readiness(Ready::readable())?` after executing `result_sender.send`, to inform executor there are some events happens.

In this case, executor will never be blocked by io worker, because we can register all events in executor, and `mio::Poll` will listen to all events (eg, combine `fs-mio` with `tcp` into a file server).



#### Callback is Evil

Before writing a file server, we should talk about the problems of callback.

Callback can make code confusing, you can realize it more clearly when handling error, like this:

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



Moreover, there is another problem in rust when we 