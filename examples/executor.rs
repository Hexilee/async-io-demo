#![feature(arbitrary_self_types)]
#![feature(futures_api)]
#![feature(fnbox)]
#![feature(pin)]

use crossbeam_channel::{unbounded, Sender, Receiver};
use std::future::Future;
use std::fs::File;
use std::io::{Read, Write, self};
use std::boxed::FnBox;
use std::pin::Pin;
use std::task::{LocalWaker, Waker, UnsafeWake, self};
use std::thread;
use std::borrow::{Borrow, BorrowMut};
use std::ptr::NonNull;
use std::cell::{RefCell, Cell};
use std::time::Duration;
use std::rc::Rc;
use std::net::SocketAddr;
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
unsafe fn index_from_task_token(token: Token) -> usize {
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

#[derive(Clone)]
struct TcpListener(Rc<net::TcpListener>);

#[derive(Clone)]
struct TcpStream {
    inner: Rc<net::TcpStream>
}

struct TcpAcceptState<'a> {
    listener: &'a mut TcpListener
}

struct StreamReadState<'a> {
    stream: &'a mut TcpStream
}

struct StreamWriteState<'a> {
    stream: &'a mut TcpStream
}

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
                                        executor.poll.deregister(&task.waker.awake_registration).expect("task deregister failed");
                                        executor.tasks.borrow_mut().remove(index);
                                    }
                                    task::Poll::Pending => {
                                        task.waker.awake_readiness.set_readiness(Ready::empty()).expect("readiness setting empty failed");
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    executor.main_waker.awake_readiness.set_readiness(Ready::empty()).expect("main readiness setting empty failed");
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

impl Evented for TcpListener {
    fn register(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()> {
        self.0.register(poll, token, interest, opts)
    }

    fn reregister(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()> {
        self.0.reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        self.0.deregister(poll)
    }
}

impl Evented for TcpStream {
    fn register(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()> {
        self.0.register(poll, token, interest, opts)
    }

    fn reregister(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()> {
        self.0.reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        self.0.deregister(poll)
    }
}

impl TcpListener {
    pub fn bind(addr: &SocketAddr) -> io::Result<TcpListener> {
        let l = mio::net::TcpListener::bind(addr)?;
        Ok(TcpListener::new(l))
    }

    fn new(listener: mio::net::TcpListener) -> TcpListener {
        TcpListener(Rc::new(listener))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr()
    }
    pub fn ttl(&self) -> io::Result<u32> {
        self.0.ttl()
    }
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.0.set_ttl(ttl)
    }
    fn poll_accept(&self, lw: &LocalWaker) -> task::Poll<io::Result<(TcpStream, SocketAddr)>> {
        let token = register_source(self.clone(), lw.clone(), Ready::readable());
        match self.0.accept() {
            Ok((stream, addr)) => task::Poll::Ready(Ok((TcpStream::new(stream), addr))),
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                task::Poll::Pending
            }
            Err(err) => task::Poll::Ready(Err(err))
        }
    }
}

impl TcpStream {
    pub(crate) fn new(connected: mio::net::TcpStream) -> TcpStream {
        TcpStream(Rc::new(connected))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.0.peer_addr()
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        self.0.nodelay()
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.0.set_nodelay(nodelay)
    }

    pub fn recv_buffer_size(&self) -> io::Result<usize> {
        self.0.recv_buffer_size()
    }

    pub fn set_recv_buffer_size(&self, size: usize) -> io::Result<()> {
        self.0.set_recv_buffer_size(size)
    }

    pub fn send_buffer_size(&self) -> io::Result<usize> {
        self.0.send_buffer_size()
    }

    pub fn set_send_buffer_size(&self, size: usize) -> io::Result<()> {
        self.0.set_send_buffer_size(size)
    }

    pub fn keepalive(&self) -> io::Result<Option<Duration>> {
        self.0.keepalive()
    }

    pub fn set_keepalive(&self, keepalive: Option<Duration>) -> io::Result<()> {
        self.0.set_keepalive(keepalive)
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.0.ttl()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.0.set_ttl(ttl)
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        self.0.linger()
    }

    pub fn set_linger(&self, dur: Option<Duration>) -> io::Result<()> {
        self.0.set_linger(dur)
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&mut self.0).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&mut self.0).flush()
    }
}

impl <'a> Future for TcpAcceptState<'a> {
    type Output = io::Result<(TcpStream, SocketAddr)>;
    fn poll(self: Pin<&mut Self>, lw: &LocalWaker) -> task::Poll<<Self as Future>::Output> {
        self.listener.poll_accept(lw)
    }
}

fn main() {}