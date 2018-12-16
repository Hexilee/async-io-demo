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
