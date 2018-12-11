use mio::*;
use std::time::Duration;
use crossbeam_channel::unbounded;
use std::io;
use std::error::Error;

const TOKEN: Token = Token(0);

fn main() -> Result<(), Box<Error>> {
    let poll = Poll::new()?;
    let (registration, set_readiness) = Registration::new2();
    poll.register(&registration, TOKEN, Ready::readable(), PollOpt::edge())?;
    let (task_sender, task_receiver) = unbounded();
    let worker_handler = std::thread::spawn(move || -> io::Result<()> {
        let mut task_counter = 0;
        loop {
            match task_receiver.recv() {
                Ok(_) => {
                    task_counter += 1;
                    if task_counter > 50 {
                        break;
                    }
                    set_readiness.set_readiness(Ready::readable())?;
                }

                Err(err) => {
                    println!("err: {}", err);
                    break;
                }
            }
        }
        println!("task counter: {}", task_counter);
        Ok(())
    });

    let mut events = Events::with_capacity(1024);

    let mut total_event_counter = 0;
    let mut token_event_counter = 0;
    task_sender.send(())?;
    loop {
        match poll.poll(&mut events, Some(Duration::from_secs(1))) {
            Ok(n) => {
                if n == 0 {
                    // timeout
                    break;
                }
                for event in events.iter() {
                    total_event_counter += 1;
                    match event.token() {
                        TOKEN => {
                            token_event_counter += 1;
                            task_sender.send(())?;
                        }

                        _ => unreachable!()
                    }
                }
            }

            _ => unreachable!()
        }
    }
    worker_handler.join().unwrap()?;
    println!("total_event_counter: {}, token_event_counter: {}", total_event_counter, token_event_counter);
    Ok(())
}