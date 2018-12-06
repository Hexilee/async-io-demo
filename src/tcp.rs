use super::fs::Fs;
use mio::*;
use mio::net::{TcpListener, TcpStream};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

const SERVER_ACCEPT: Token = Token(0);
const SERVER: Token = Token(1);
const CLIENT: Token = Token(2);
const SERVER_HELLO: &[u8] = b"PING";
const CLIENT_HELLO: &[u8] = b"PONG";

#[test]
fn tcp_test() {
    let addr = "127.0.0.1:13265".parse().unwrap();

// Setup the server socket
    let server = TcpListener::bind(&addr).unwrap();

// Create a poll instance
    let poll = Poll::new().unwrap();

// Start listening for incoming connections
    poll.register(&server, SERVER_ACCEPT, Ready::readable(),
                  PollOpt::edge()).unwrap();

// Setup the client socket
    let mut client = TcpStream::connect(&addr).unwrap();

    let mut server_handler = None;

// Register the client
    poll.register(&client, CLIENT, Ready::readable() | Ready::writable(),
                  PollOpt::edge()).unwrap();

// Create storage for events
    let mut events = Events::with_capacity(1024);

    let start = Instant::now();
    let timeout = Duration::from_millis(10);
    let fs = Fs::new();
    loop {
        poll.poll(&mut events, None).unwrap();
        for event in events.iter() {
            if start.elapsed() >= timeout {
                return;
            }
            match event.token() {
                SERVER_ACCEPT => {
                    let (handler, addr) = server.accept().unwrap();
                    fs.clone().println(format!("accept from addr: {}", &addr)).unwrap();
                    poll.register(&handler, SERVER, Ready::readable() | Ready::writable(), PollOpt::edge()).unwrap();
                    server_handler = Some(handler);
                }

                SERVER => {
                    if event.readiness().is_writable() {
                        if let Some(ref mut handler) = &mut server_handler {
                            handler.write(SERVER_HELLO).unwrap();
                            fs.clone().println("server wrote".to_string()).unwrap();
                        }
                    }
                    if event.readiness().is_readable() {
                        let mut hello = [0; 4];
                        if let Some(ref mut handler) = &mut server_handler {
                            match handler.read_exact(&mut hello) {
                                Ok(_) => {
                                    assert_eq!(CLIENT_HELLO, &hello);
                                    fs.clone().println("server received".to_string()).unwrap();
                                }
                                _ => continue
                            }
                        }
                    }
                }
                CLIENT => {
                    if event.readiness().is_writable() {
                        client.write(CLIENT_HELLO).unwrap();
                        fs.clone().println("client wrote".to_string()).unwrap();
                    }
                    if event.readiness().is_readable() {
                        let mut hello = [0; 4];
                        match client.read_exact(&mut hello) {
                            Ok(_) => {
                                assert_eq!(SERVER_HELLO, &hello);
                                fs.clone().println("client received".to_string()).unwrap();
                            }
                            _ => continue
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
    };
}