use crate::executor::register_source;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender, TryRecvError};
use failure::Error;
use log::debug;
use mio::{Ready, Registration, SetReadiness, Token};
use std::fs;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{self, Context};
use std::thread;

struct BlockTaskWorker {
    task_sender: Sender<Box<dyn BlockTask>>,
}

impl BlockTaskWorker {
    fn new() -> Self {
        let (task_sender, task_receiver) = unbounded();
        let worker = BlockTaskWorker { task_sender };
        thread::spawn(move || loop {
            match task_receiver.recv() {
                Ok(mut task) => task.exec(),
                Err(err) => panic!("{}", err),
            }
        });
        worker
    }
}

thread_local! {
    static TASK_WORKER: BlockTaskWorker = BlockTaskWorker::new();
}

fn send_block_task<T: BlockTask + 'static>(task: T) {
    let boxed_task = Box::new(task);
    TASK_WORKER.with(move |task_worker| {
        task_worker.task_sender.send(boxed_task).unwrap();
    })
}

trait BlockTask: Send {
    fn exec(&mut self);
}

struct ReadFileTask {
    file_name: String,
    string_sender: Sender<io::Result<String>>,
    set_readiness: SetReadiness,
}

struct ReadFileState {
    source_token: Option<Token>,
    registration: Option<Registration>,
    string_receiver: Receiver<io::Result<String>>,
}

impl BlockTask for ReadFileTask {
    fn exec(&mut self) {
        debug!("ready to open file: {}", &self.file_name);
        self.string_sender
            .send(fs::read_to_string(&self.file_name))
            .unwrap();
        debug!("sent file named {}", &self.file_name);
        self.set_readiness.set_readiness(Ready::readable()).unwrap();
    }
}

pub fn read_to_string(file_name: String) -> impl Future<Output = Result<String, Error>> {
    let (registration, set_readiness) = Registration::new2();
    let (string_sender, string_receiver) = bounded(1);
    send_block_task(ReadFileTask {
        file_name,
        string_sender,
        set_readiness,
    });
    ReadFileState {
        source_token: None,
        registration: Some(registration),
        string_receiver,
    }
}

impl Future for ReadFileState {
    type Output = Result<String, Error>;
    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> task::Poll<<Self as Future>::Output> {
        if self.source_token.is_none() {
            self.source_token = Some(
                match register_source(
                    self.registration.take().unwrap(),
                    cx.waker().clone(),
                    Ready::readable(),
                ) {
                    Ok(token) => token,
                    Err(err) => return task::Poll::Ready(Err(err)),
                },
            )
        };

        match self.string_receiver.try_recv() {
            Ok(read_result) => match read_result {
                Ok(value) => {
                    debug!("read value {}", &value);
                    task::Poll::Ready(Ok(value))
                }
                Err(err) => {
                    debug!("read err {}", &err);
                    task::Poll::Ready(Err(err.into()))
                }
            },
            Err(TryRecvError::Empty) => {
                debug!("read file pending");
                task::Poll::Pending
            }
            Err(err) => {
                debug!("read file disconnecting");
                task::Poll::Ready(Err(err.into()))
            }
        }
    }
}
