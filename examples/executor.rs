//use crossbeam_channel::{unbounded, Sender};
//use std::fs::File;
//use std::io::Read;
//use std::boxed::FnBox;
//use mio::*;
//use std::thread;
//use std::borrow::Borrow;
//use slab::Slab;
//
//const MAX_USIZE: usize = 1 << 64;
//const AWAKE_TOKEN: Token = Token(MAX_USIZE);
//
//struct Executor {
//    poll: Poll,
//    awake_readiness: SetReadiness,
//    read_tasks: Slab<Box<dyn ReadTask>>,
//    write_tasks: Slab<Box<dyn WriteTask>>,
//}
//
//trait ReadTask {
//    fn read(&mut self, executor: &Executor);
//}
//
//trait WriteTask {
//}
//
//impl Executor {
//    pub fn new() -> Result<Self, failure::Error> {
//        let poll = Poll::new()?;
//        let (registration, exit_readiness) = Registration::new2();
//        poll.register(&registration, EXIT_TOKEN, Ready::readable(), PollOpt::oneshot())?;
//        Ok(Executor { poll, handlers: RefCell::new(Vec::new()), exit_readiness })
//    }
//
//    pub fn run(&self) -> Result<(), failure::Error> {
//        let mut events = Events::with_capacity(1024);
//        'outer: loop {
//            self.poll.poll(&mut events, None)?;
//            for event in events.iter() {
//                if event.token() == EXIT_TOKEN {
//                    break 'outer;
//                }
//                for handler in self.handlers.borrow().iter() {
//                    if event.token() == handler.token {
//                        (handler.action)(&self.poll, event, handler.evented.borrow())?;
//                    }
//                }
//            }
//        }
//        Ok(())
//    }
//
//    pub fn register<A>(&self, evented: Box<dyn SizedEvented>,
//                       token: Token,
//                       interest: Ready,
//                       opts: PollOpt,
//                       action: A) -> Result<(), failure::Error>
//        where A: Fn(&Poll, Event, &(dyn SizedEvented)) -> Result<(), failure::Error> + 'static {
//        self.poll.register(evented.borrow(), token, interest, opts)?;
//        self.handlers.borrow_mut().push(Handler { token, evented, action: Box::new(action) });
//        Ok(())
//    }
//
//    pub fn shutdown(&self) -> Result<(), failure::Error> {
//        Ok(self.exit_readiness.set_readiness(Ready::readable())?)
//    }
//}
//
//type FileCallback = Box<FnBox(File) + Send>;
//type StringCallback = Box<FnBox(String) + Send>;
//
//pub enum Task {
//    Open(String, FileCallback),
//    ReadToString(File, StringCallback),
//}
//
//pub enum TaskResult {
//    Open(File, FileCallback),
//    ReadToString(String, StringCallback),
//}
//
//const TEST_FILE_VALUE: &str = "Hello, World!";
//
//#[test]
//fn test_executor() {
//    let executor = Executor::new().unwrap();
//    let (task_sender, task_receiver) = unbounded();
//    let (result_sender, result_receiver) = unbounded();
//    let (fs_registration, fs_set_readiness) = Registration::new2();
//    thread::spawn(move || {
//        loop {
//            match task_receiver.recv() {
//                Ok(task) => {
//                    match task {
//                        Task::Open(path, callback) => {
//                            result_sender
//                                .send(TaskResult::Open(File::open(path).unwrap(), callback))
//                                .unwrap();
//                            fs_set_readiness.set_readiness(Ready::readable()).unwrap();
//                        }
//                        Task::ReadToString(mut file, callback) => {
//                            let mut value = String::new();
//                            file.read_to_string(&mut value).unwrap();
//                            result_sender
//                                .send(TaskResult::ReadToString(value, callback))
//                                .unwrap();
//                            fs_set_readiness.set_readiness(Ready::readable()).unwrap();
//                        }
//                    }
//                }
//                Err(_) => {
//                    break;
//                }
//            }
//        };
//    });
//
//    executor.register(Box::new(fs_registration), FS_TOKEN, Ready::readable(), PollOpt::oneshot(), move |poll, event, evented| {
//        loop {
//            match result_receiver.try_recv() {
//                Ok(result) => {
//                    match result {
//                        TaskResult::ReadToString(value, callback) => callback(value),
//                        TaskResult::Open(file, callback) => callback(file),
//                    }
//                }
//                Err(_) => {
//                    break;
//                }
//            }
//        }
//        Ok(poll.reregister(evented, FS_TOKEN, Ready::readable(), PollOpt::oneshot())?)
//    }).unwrap();
//}