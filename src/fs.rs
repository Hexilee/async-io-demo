//use crossbeam_channel::{unbounded, Sender};
//use std::fs::File;
//use std::io::Read;
//use std::boxed::FnBox;
//use std::thread;
//
//#[derive(Clone)]
//pub struct Fs {
//    task_sender: Sender<Task>,
//}
//
//pub struct FsHandler {
//    io_worker: thread::JoinHandle<()>,
//    executor: thread::JoinHandle<()>,
//}
//
//pub fn fs_async() -> (Fs, FsHandler) {
//    let (task_sender, task_receiver) = unbounded();
//    let (result_sender, result_receiver) = unbounded();
//    let io_worker = std::thread::spawn(move || {
//        loop {
//            match task_receiver.recv() {
//                Ok(task) => {
//                    match task {
//                        Task::Println(ref string) => println!("{}", string),
//                        Task::Open(path, callback, fs) => {
//                            result_sender
//                                .send(TaskResult::Open(File::open(path).unwrap(), callback, fs))
//                                .unwrap();
//                        }
//                        Task::ReadToString(mut file, callback, fs) => {
//                            let mut value = String::new();
//                            file.read_to_string(&mut value).unwrap();
//                            result_sender
//                                .send(TaskResult::ReadToString(value, callback, fs))
//                                .unwrap()
//                        }
//                        Task::Exit => {
//                            result_sender
//                                .send(TaskResult::Exit)
//                                .unwrap();
//                            return;
//                        }
//                    }
//                }
//                Err(_) => {
//                    return;
//                }
//            }
//        }
//    });
//    let executor = std::thread::spawn(move || {
//        loop {
//            match result_receiver.recv() {
//                Ok(result) => {
//                    match result {
//                        TaskResult::ReadToString(value, callback, fs) => callback(value, fs),
//                        TaskResult::Open(file, callback, fs) => callback(file, fs),
//                        TaskResult::Exit => return
//                    }
//                }
//                Err(_) => {
//                    return;
//                }
//            }
//        }
//    });
//
//    (Fs { task_sender }, FsHandler { io_worker, executor })
//}
//
//impl Fs {
//    pub fn println(&self, string: String) {
//        self.task_sender.send(Task::Println(string)).unwrap()
//    }
//
//    pub fn open<F: FnOnce(File, Fs) + Send + 'static>(&self, path: &str, callback: F) {
//        self.task_sender.send(Task::Open(path.to_string(), Box::new(callback), self.clone())).unwrap()
//    }
//
//    pub fn read_to_string<F: FnOnce(String, Fs) + Send + 'static>(&self, file: File, callback: F) {
//        self.task_sender.send(Task::ReadToString(file, Box::new(callback), self.clone())).unwrap()
//    }
//
//    pub fn close(&self) {
//        self.task_sender.send(Task::Exit).unwrap();
//    }
//}
//
//impl FsHandler {
//    pub fn join(self) {
//        self.io_worker.join().unwrap();
//        self.executor.join().unwrap();
//    }
//}
//
//type FileCallback = Box<FnBox(File, Fs) + Send>;
//type StringCallback = Box<FnBox(String, Fs) + Send>;
//
//pub enum Task {
//    Exit,
//    Println(String),
//    Open(String, FileCallback, Fs),
//    ReadToString(File, StringCallback, Fs),
//}
//
//pub enum TaskResult {
//    Exit,
//    Open(File, FileCallback, Fs),
//    ReadToString(String, StringCallback, Fs),
//}
//
//
//const TEST_FILE_VALUE: &str = "Hello, World!";
//
//#[test]
//fn test_fs() {
//    let (fs, fs_handler) = fs_async();
//    fs.open("./src/test.txt", |file, fs| {
//        fs.read_to_string(file, |value, fs| {
//            assert_eq!(TEST_FILE_VALUE, &value);
//            fs.println(value);
//            fs.close();
//        })
//    });
//    fs_handler.join();
//}