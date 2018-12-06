use std::sync::mpsc::{Sender, Receiver, channel, SendError};

#[derive(Clone)]
struct Fs {
    task_sender: Sender<Task>,
}

impl Fs {
    fn new() -> Self {
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            loop {
                match receiver.recv() {
                    Ok(task) => {
                        match task {
                            Task::Print(ref string) => println!("{}", string),
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

    fn println(&self, string: &str) -> Result<(), SendError<Task>> {
        self.task_sender.send(Task::Print(string.to_string()))
    }
}

enum Task {
    Exit,
    Print(String),
}

