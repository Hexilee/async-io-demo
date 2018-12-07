use crossbeam_channel::{unbounded, Sender};
use std::fs::File;
use std::io::Read;
use std::boxed::FnBox;

#[derive(Clone)]
pub struct Fs {
    task_sender: Sender<Task>,
}

impl Fs {
    pub fn new() -> Self {
        let (task_sender, task_receiver) = unbounded();
        let (result_sender, result_receiver) = unbounded();
        std::thread::spawn(move || {
            loop {
                match task_receiver.recv() {
                    Ok(task) => {
                        match task {
                            Task::Println(ref string) => println!("{}", string),
                            Task::Open(path, callback) => {
                                result_sender
                                    .send(TaskResult::Open(File::open(path).unwrap(), callback))
                                    .unwrap();
                            }
                            Task::ReadToString(mut file, callback) => {
                                let mut value = String::new();
                                file.read_to_string(&mut value).unwrap();
                                result_sender
                                    .send(TaskResult::ReadToString(value, callback))
                                    .unwrap()
                            }
                            Task::Exit => {
                                result_sender
                                    .send(TaskResult::Exit)
                                    .unwrap();
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

        Fs { task_sender }
    }

    pub fn println(&self, string: String) {
        self.task_sender.send(Task::Println(string)).unwrap()
    }

    pub fn open(&self, path: &str, callback: FileCallback) {
        self.task_sender.send(Task::Open(path.to_string(), callback)).unwrap()
    }

    pub fn read_to_string(&self, file: File, callback: StringCallback) {
        self.task_sender.send(Task::ReadToString(file, callback)).unwrap()
    }

    pub fn close(&self) {
        self.task_sender.send(Task::Exit).unwrap()
    }
}

type FileCallback = Box<FnBox(File) + Send>;
type StringCallback = Box<FnBox(String) + Send>;

pub enum Task {
    Exit,
    Println(String),
    Open(String, FileCallback),
    ReadToString(File, StringCallback),
}

pub enum TaskResult {
    Exit,
    Open(File, FileCallback),
    ReadToString(String, StringCallback),
}

const TEST_FILE_VALUE: &str = "Hello, World!";

#[test]
fn test_fs() {
    let fs = Fs::new();
    fs.clone().open("./src/test.txt", Box::new(move |file| {
        fs.clone().read_to_string(file, Box::new(move |value| {
            assert_eq!(TEST_FILE_VALUE, &value);
            fs.println(value);
            fs.close();
        }))
    }));
}