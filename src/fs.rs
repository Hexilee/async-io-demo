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

    pub fn println(&self, string: String) -> Result<(), SendError<Task>> {
        self.task_sender.send(Task::Println(string))
    }
}

pub enum Task {
    Exit,
    Println(String),
}

