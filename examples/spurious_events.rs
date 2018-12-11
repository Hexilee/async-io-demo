use mio::*;
use std::time::Duration;
use crossbeam_channel::unbounded;

const TOKEN: Token = Token(0);

fn main() {
    let poll = Poll::new().unwrap();
    let (registration, set_readiness) = Registration::new2();
    poll.register(&registration, TOKEN, Ready::readable(), PollOpt::edge()).unwrap();
    let (task_sender, task_receiver) = unbounded();
    std::thread::spawn(move || {
        let mut task_counter = 0;
        loop {
            match task_receiver.recv() {
                Ok(_) => {
                    task_counter += 1;
                    if task_counter > 50 {
                        break;
                    }
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }

                Err(err) => {
                    println!("err: {}", err);
                    break;
                }
            }
        }
        println!("task counter: {}", task_counter);
    });

    let mut events = Events::with_capacity(1024);

    let mut total_event_counter = 0;
    let mut token_event_counter = 0;
    task_sender.send(()).unwrap();
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
                            task_sender.send(()).unwrap();
                        }

                        _ => unreachable!()
                    }
                }
            }

            _ => unreachable!()
        }
    }
    println!("total_event_counter: {}, token_event_counter: {}", total_event_counter, token_event_counter);
}