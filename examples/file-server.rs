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
                        let file_name = await!(stream.read()).expect("read from stream fail");
                        let file_contents = await!(read_to_string(String::from_utf8(file_name).unwrap())).expect("read to string from file fail");
                        await!(stream.write_str(&file_contents)).expect("write file contents to stream fail");
                        stream.close();
                    },
                )
                .expect("spawn stream fail");
            }
        },
    )
}
