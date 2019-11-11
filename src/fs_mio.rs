use crossbeam_channel::{unbounded, Sender, TryRecvError};
use failure::Error;
use mio::*;
use std::fs::File;
use std::io::Read;
use std::thread;
use std::time::Duration;

#[derive(Clone)]
pub struct Fs {
    task_sender: Sender<Task>,
}

pub struct FsHandler {
    io_worker: thread::JoinHandle<Result<(), Error>>,
    executor: thread::JoinHandle<Result<(), Error>>,
}

const FS_TOKEN: Token = Token(0);

pub fn fs_async() -> (Fs, FsHandler) {
    let (task_sender, task_receiver) = unbounded();
    let (result_sender, result_receiver) = unbounded();
    let poll = Poll::new().unwrap();
    let (registration, set_readiness) = Registration::new2();
    poll.register(
        &registration,
        FS_TOKEN,
        Ready::readable(),
        PollOpt::oneshot(),
    )
    .unwrap();
    let io_worker = std::thread::spawn(move || {
        while let Ok(task) = task_receiver.recv() {
            match task {
                Task::Println(ref string) => println!("{}", string),
                Task::Open(path, callback, fs) => {
                    result_sender.send(TaskResult::Open(File::open(path)?, callback, fs))?;
                    set_readiness.set_readiness(Ready::readable())?;
                }
                Task::ReadToString(mut file, callback, fs) => {
                    let mut value = String::new();
                    file.read_to_string(&mut value)?;
                    result_sender.send(TaskResult::ReadToString(value, callback, fs))?;
                    set_readiness.set_readiness(Ready::readable())?;
                }
                Task::Exit => {
                    result_sender.send(TaskResult::Exit)?;
                    set_readiness.set_readiness(Ready::readable())?;
                    break;
                }
            }
        }
        Ok(())
    });

    let executor = thread::spawn(move || {
        let mut events = Events::with_capacity(1024);
        'outer: loop {
            poll.poll(&mut events, Some(Duration::from_secs(1)))?;
            for event in events.iter() {
                match event.token() {
                    FS_TOKEN => {
                        loop {
                            match result_receiver.try_recv() {
                                Ok(result) => match result {
                                    TaskResult::ReadToString(value, callback, fs) => {
                                        callback(value, fs)?
                                    }
                                    TaskResult::Open(file, callback, fs) => callback(file, fs)?,
                                    TaskResult::Exit => break 'outer,
                                },
                                Err(e) => match e {
                                    TryRecvError::Empty => break,
                                    TryRecvError::Disconnected => return Err(e.into()),
                                },
                            }
                        }
                        poll.reregister(
                            &registration,
                            FS_TOKEN,
                            Ready::readable(),
                            PollOpt::oneshot(),
                        )?;
                    }
                    _ => unreachable!(),
                }
            }
        }
        Ok(())
    });
    (
        Fs { task_sender },
        FsHandler {
            io_worker,
            executor,
        },
    )
}

impl Fs {
    pub fn println(&self, string: String) -> Result<(), Error> {
        Ok(self.task_sender.send(Task::Println(string))?)
    }

    pub fn open<F>(&self, path: &str, callback: F) -> Result<(), Error>
    where
        F: FnOnce(File, Fs) -> Result<(), Error> + Sync + Send + 'static,
    {
        Ok(self.task_sender.send(Task::Open(
            path.to_string(),
            Box::new(callback),
            self.clone(),
        ))?)
    }

    pub fn read_to_string<F>(&self, file: File, callback: F) -> Result<(), Error>
    where
        F: FnOnce(String, Fs) -> Result<(), Error> + Sync + Send + 'static,
    {
        Ok(self
            .task_sender
            .send(Task::ReadToString(file, Box::new(callback), self.clone()))?)
    }

    pub fn close(&self) -> Result<(), Error> {
        Ok(self.task_sender.send(Task::Exit)?)
    }
}

impl FsHandler {
    pub fn join(self) -> Result<(), Error> {
        self.io_worker.join().unwrap()?;
        self.executor.join().unwrap()
    }
}

type FileCallback = Box<dyn FnOnce(File, Fs) -> Result<(), Error> + Sync + Send>;
type StringCallback = Box<dyn FnOnce(String, Fs) -> Result<(), Error> + Sync + Send>;

pub enum Task {
    Exit,
    Println(String),
    Open(String, FileCallback, Fs),
    ReadToString(File, StringCallback, Fs),
}

pub enum TaskResult {
    Exit,
    Open(File, FileCallback, Fs),
    ReadToString(String, StringCallback, Fs),
}
