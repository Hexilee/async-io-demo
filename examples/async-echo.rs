#[macro_use]
extern crate log;

use asyncio::executor::{block_on, spawn, TcpListener};
use failure::Error;

fn main() -> Result<(), Error> {
    env_logger::init();
    block_on(
        async {
            let mut listener = TcpListener::bind(&"127.0.0.1:7878".parse()?)?;
            info!("Listening on 127.0.0.1:7878");
            while let Ok((mut stream, addr)) = listener.accept().await {
                info!("connection from {}", addr);
                spawn(
                    async move {
                        let client_hello = stream.read().await?;
                        let read_length = client_hello.len();
                        let write_length =
                            stream.write(client_hello).await?;
                        assert_eq!(read_length, write_length);
                        stream.close();
                        Ok(())
                    },
                )?;
            };
            Ok(())
        },
    )?
}
