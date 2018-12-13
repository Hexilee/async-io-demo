#![feature(futures_api)]
#![feature(fnbox)]
#![feature(pin)]

use crossbeam_channel::{unbounded, Sender, Receiver};
use std::future::Future;
use std::fs::File;
use std::io::{Read, self};
use std::boxed::FnBox;
use std::pin::Pin;
use std::task::{LocalWaker, Waker, UnsafeWake, self};
use std::thread;
use std::borrow::Borrow;
use std::ptr::NonNull;
use std::cell::{RefCell, Cell};
use std::time::Duration;
use std::rc::Rc;
use slab::Slab;
use mio::*;
use failure::Error;

const MAX_RESOURCE_NUM: usize = std::usize::MAX;
const MAIN_TASK_TOKEN: Token = Token(MAX_RESOURCE_NUM);
const EVENT_CAP: usize = 1024;
const POLL_TIME_OUT_MILL: u64 = 100;

const fn get_source_token(index: usize) -> Token {
    Token(index * 2)
}

const fn get_task_token(index: usize) -> Token {
    Token(index * 2 + 1)
}

// panic when token is ord
unsafe fn index_from_source_token(token: Token) -> usize {
    if !is_source(token) {
        panic!(format!("not a source token: {}", token.0));
    }
    token.0 / 2
}

// panic when token is not ord
fn index_from_task_token(token: Token) -> usize {
    if !is_task(token) {
        panic!(format!("not a task token: {}", token.0));
    }
    (token.0 - 1) / 2
}

const fn is_source(token: Token) -> bool {
    token.0 % 2 == 0
}

const fn is_task(token: Token) -> bool {
    token.0 % 2 == 1
}

type PinFuture<T> = Pin<Box<dyn Future<Output=T>>>;

struct Executor {
    pub(crate) poll: Poll,
    pub(crate) main_waker: InnerWaker,
    pub(crate) tasks: RefCell<Slab<Task>>,
    pub(crate) sources: RefCell<Slab<Source>>,
}

struct InnerWaker {
    awake_readiness: SetReadiness,
    pub(crate) awake_registration: Registration,
}

struct Source {
    pub(crate) task_waker: LocalWaker,
    pub(crate) evented: Box<dyn Evented>,
}

struct Task {
    pub(crate) waker: InnerWaker,
    pub(crate) inner_task: PinFuture<()>,
}

struct TcpListener(net::TcpListener);

struct TcpStream(net::TcpStream);

unsafe impl UnsafeWake for InnerWaker {
    unsafe fn clone_raw(&self) -> Waker {
        Waker::new(NonNull::from(self))
    }

    unsafe fn drop_raw(&self) {}

    unsafe fn wake(&self) {
        self.awake_readiness.set_readiness(Ready::readable()).unwrap();
    }
}

impl InnerWaker {
    pub(crate) fn gen_local_waker(&self) -> LocalWaker {
        unsafe {
            LocalWaker::new(NonNull::from(self))
        }
    }
}

impl Executor {
    pub fn new() -> Result<Self, Error> {
        let poll = Poll::new()?;
        let (awake_registration, awake_readiness) = Registration::new2();
        poll.register(&awake_registration, MAIN_TASK_TOKEN, Ready::all(), PollOpt::level())?;
        Ok(Executor {
            poll,
            main_waker: InnerWaker { awake_registration, awake_readiness },
            tasks: RefCell::new(Slab::new()),
            sources: RefCell::new(Slab::new()),
        })
    }

    pub(crate) fn main_waker(&self) -> LocalWaker {
        unsafe {
            LocalWaker::new(NonNull::from(&self.main_waker))
        }
    }
}

thread_local! {
    static EXECUTOR: Executor = Executor::new().expect("initializing executor failed!")
}

pub fn block_on<R, F>(main_task: F)
    where R: Sized,
          F: Future<Output=R> {
    EXECUTOR.with(move |executor: &Executor| {
        let mut pinned_task = Box::pinned(main_task);
        let mut events = Events::with_capacity(EVENT_CAP);
        match pinned_task.as_mut().poll(&executor.main_waker()) {
            task::Poll::Ready(result) => return,
            task::Poll::Pending => {
                loop {
                    executor.poll.poll(&mut events, Some(Duration::from_millis(POLL_TIME_OUT_MILL))).expect("polling failed");
                    for event in events.iter() {
                        match event.token() {
                            MAIN_TASK_TOKEN => {
                                match pinned_task.as_mut().poll(&executor.main_waker()) {
                                    task::Poll::Ready(result) => return,
                                    task::Poll::Pending => continue
                                }
                            }
                            token if is_source(token) => {
                                let index = unsafe { index_from_source_token(token) };
                                let source = &executor.sources.borrow_mut()[index];
                                source.task_waker.wake();
                            }

                            token if is_task(token) => {
                                let index = unsafe { index_from_task_token(token) };
                                let task = &mut executor.tasks.borrow_mut()[index];
                                match task.inner_task.as_mut().poll(&task.waker.gen_local_waker()) {
                                    task::Poll::Ready(result) => {
                                        executor.poll.deregister(&task.waker.awake_registration);
                                        executor.tasks.borrow_mut().remove(index);
                                    }
                                    task::Poll::Pending => {
                                        continue
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    executor.main_waker.awake_readiness.set_readiness(Ready::empty());
                }
            }
        }
    });
}

pub fn spawn<F: Future<Output=()> + 'static>(task: F) {
    EXECUTOR.with(move |executor: &Executor| {
        let (awake_registration, awake_readiness) = Registration::new2();
        let index = executor.tasks.borrow_mut().insert(Task {
            inner_task: Box::pinned(task),
            waker: InnerWaker { awake_readiness, awake_registration },
        });
        let token = get_task_token(index);
        let task = &mut executor.tasks.borrow_mut()[index];
        match task.inner_task.as_mut().poll(&task.waker.gen_local_waker()) {
            task::Poll::Ready(result) => {
                executor.tasks.borrow_mut().remove(index);
            }
            task::Poll::Pending => {
                executor.poll.register(&task.waker.awake_registration, token, Ready::all(), PollOpt::level()).expect("task registration failed");
            }
        }
    });
}

fn register_source<T: Evented + 'static>(evented: T, task_waker: LocalWaker, interest: Ready) -> Token {
    let ret_token = Rc::new(Cell::new(None));
    let ret_token_clone = ret_token.clone();
    EXECUTOR.with(move |executor: &Executor| {
        let (awake_registration, awake_readiness) = Registration::new2();
        let index = executor.sources.borrow_mut().insert(Source {
            task_waker,
            evented: Box::new(evented),
        });
        let token = get_source_token(index);
        let source = &executor.sources.borrow()[index];
        executor.poll.register(&source.evented, token, interest, PollOpt::oneshot()).expect("task registration failed");
        ret_token.set(Some(token))
    });
    ret_token_clone.get().expect("ret token is None")
}

// panic when token is ord
unsafe fn reregister_source(token: Token, interest: Ready) {
    EXECUTOR.with(move |executor: &Executor| {
        let index = index_from_source_token(token);
        let source = &executor.sources.borrow()[index];
        executor.poll.reregister(&source.evented, token, interest, PollOpt::oneshot()).expect("task registration failed");
    });
}

// panic when token is ord
unsafe fn drop_source(token: Token) {
    EXECUTOR.with(move |executor: &Executor| {
        let index = index_from_source_token(token);
        let source = &executor.sources.borrow()[index];
        executor.poll.deregister(&source.evented).expect("task registration failed");
        &executor.sources.borrow_mut().remove(index);
    });
}
fn main() {}