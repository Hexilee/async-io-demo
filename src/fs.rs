use std::sync::mpsc::{Sender, channel, SendError};
use std::fs::File;
use std::io::{self, Read, Write};

#[derive(Clone)]
pub struct Fs {
    task_sender: Sender<Task>,
}

impl Fs {
    pub fn new() -> Self {
        let (task_sender, task_receiver) = channel();
        let (result_sender, result_receiver) = channel();
        std::thread::spawn(move || {
            loop {
                match result_receiver.recv() {
                    Ok(result) => {
                        match result {
                            TaskResult::ReadToString(value, callback) => callback(value),
                            TaskResult::Open(file, callback) => callback(file),
                            TaskResult::Exit => return
                        }
                    }
                    Err(_) => {
                        return;
                    }
                }
            }
        });

        std::thread::spawn(move || {
            loop {
                match task_receiver.recv() {
                    Ok(task) => {
                        match task {
                            Task::Println(ref string) => println!("{}", string),
                            Task::Open(path, callback) => {
                                result_sender
                                    .clone()
                                    .send(TaskResult::Open(File::open(path), callback))
                                    .unwrap();
                            }
                            Task::ReadToString(mut file, callback) => {
                                let mut value = String::new();
                                match file.read_to_string(&mut value) {
                                    Ok(_) => result_sender
                                        .clone()
                                        .send(TaskResult::ReadToString(Ok(value), callback))
                                        .unwrap(),
                                    Err(err) => result_sender
                                        .clone()
                                        .send(TaskResult::ReadToString(Err(err), callback))
                                        .unwrap(),
                                }
                            }
                            Task::Exit => {
                                result_sender
                                    .clone()
                                    .send(TaskResult::Exit);
                                return;
                            }
                        }
                    }
                    Err(_) => {
                        return;
                    }
                }
            }
        });
        Fs { task_sender }
    }

    pub fn println(&self, string: String) -> Result<(), SendError<Task>> {
        self.task_sender.send(Task::Println(string))
    }

    pub fn open(&self, path: String, callback: FileCallback) -> Result<(), SendError<Task>> {
        self.task_sender.send(Task::Open(path, callback))
    }

    pub fn read_to_string(&self, file: File, callback: StringCallback) -> Result<(), SendError<Task>> {
        self.task_sender.send(Task::ReadToString(file, callback))
    }

    pub fn close(&self) -> Result<(), SendError<Task>> {
        self.task_sender.send(Task::Exit)
    }
}

type FileCallback = Box<Fn(io::Result<File>) + Send>;
type StringCallback = Box<Fn(io::Result<String>) + Send>;

pub enum Task {
    Exit,
    Println(String),
    Open(String, FileCallback),
    ReadToString(File, StringCallback),
}

pub enum TaskResult {
    Exit,
    Open(io::Result<File>, FileCallback),
    ReadToString(io::Result<String>, StringCallback),
}

