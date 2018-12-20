### Introduction

2019 is approaching. The rust team keeps their promise about asynchronous IO: `async` is introduced as keywords, `Pin, Future, Poll` and `await!` is introduced into standard library. 

I have never used rust for asynchronous IO programming before so almost know nothing about it. However, I would use it for a project recently but couldn't find many documents that are remarkably helpful for newbie of rust asynchronous programming.

Eventually, I wrote several demo and implemented simple asynchronous IO based on `mio` and `coroutine` with the help of both of this blog ([Tokio internals: Understanding Rust's asynchronous I/O framework from the bottom up](https://cafbit.com/post/tokio_internals/)) and source code of "new tokio" [romio](https://github.com/withoutboats/romio) .

This is the final asynchronous coroutine:

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

My purpose of writing this blog is to review and summarize, I will be happy if it can help someone who are interested in rust asynchronous programming. Given that the readability is the primary consideration when I wrote the code appearing in this blog, there may be some performance problem in code, please forgive me. If there are obvious problems in blog or code, you are welcome to point them up.

Most of the code appearing in this blog is collected in this [repo](https://github.com/Hexilee/async-io-demo) (some code is too long, you had better clone and view it in editor), all examples work well at `nightly-x86_64-apple-darwin 2018 Edition, rustc-1.32.0`. 

> When executing examples/async-echo, set environment variable `RUST_LOG=info` for basic runtime information; set `RUST_LOG=debug`  for events polling information.



### mio: the footstone of asynchronous IO

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



#### asynchronous network IO

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