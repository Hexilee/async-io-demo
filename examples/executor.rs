#![feature(arbitrary_self_types)]
#![feature(async_await)]
#![feature(await_macro)]
#![feature(futures_api)]
#![feature(fnbox)]
#![feature(pin)]

#[macro_use]
extern crate log;

use crossbeam_channel::{unbounded, Sender, Receiver};
use std::future::Future;
use std::io::{Read, Write, self};
use std::pin::Pin;
use std::task::{LocalWaker, Waker, UnsafeWake, self};
use std::borrow::{Borrow, BorrowMut};
use std::ptr::NonNull;
use std::cell::{RefCell, Cell};
use std::time::Duration;
use std::rc::Rc;
use std::net::SocketAddr;
use slab::Slab;
use mio::*;
use failure::Error;

const MAX_RESOURCE_NUM: usize = 1 << 31;
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
    poll: Poll,
    main_waker: InnerWaker,
    tasks: RefCell<Slab<Task>>,
    sources: RefCell<Slab<Source>>,
}

struct InnerWaker {
    awake_readiness: SetReadiness,
    awake_registration: Registration,
}

struct Source {
    task_waker: LocalWaker,
    evented: Box<dyn Evented>,
}

struct Task {
    waker: InnerWaker,
    inner_task: PinFuture<()>,
}

#[derive(Clone)]
struct TcpListener {
    inner: Rc<net::TcpListener>,
    accept_source_token: Option<Token>,
}

#[derive(Clone)]
struct TcpStream {
    inner: Rc<RefCell<net::TcpStream>>,
    read_source_token: Option<Token>,
    write_source_token: Option<Token>,
}

struct TcpAcceptState<'a> {
    listener: &'a mut TcpListener
}

struct StreamReadState<'a> {
    stream: &'a mut TcpStream,
}

struct StreamWriteState<'a> {
    stream: &'a mut TcpStream,
    data: Vec<u8>,
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
    fn gen_local_waker(&self) -> LocalWaker {
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

    fn main_waker(&self) -> LocalWaker {
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
        let main_waker = executor.main_waker();
        debug!("main_waker addr: {:p}", &main_waker);
        match pinned_task.as_mut().poll(&main_waker) {
            task::Poll::Ready(result) => {
                debug!("main task complete");
                return;
            },
            task::Poll::Pending => {
                debug!("main task pending");
                loop {
                    executor.poll.poll(&mut events, Some(Duration::from_millis(POLL_TIME_OUT_MILL))).expect("polling failed");
                    for event in events.iter() {
                        match event.token() {
                            MAIN_TASK_TOKEN => {
                                debug!("receive a main task event");
                                match pinned_task.as_mut().poll(&main_waker) {
                                    task::Poll::Ready(result) => return,
                                    task::Poll::Pending => {
                                        debug!("main task pending again");
                                        continue;
                                    }
                                }
                            }
                            token if is_source(token) => {
                                debug!("receive a source event: {:?}", token);
                                let index = unsafe { index_from_source_token(token) };
                                let source = &executor.sources.borrow_mut()[index];
                                source.task_waker.wake();
                            }

                            token if is_task(token) => {
                                debug!("receive a task event: {:?}", token);
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
        debug!("register source: {:?}", token);
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
        self.inner.register(poll, token, interest, opts)
    }

    fn reregister(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()> {
        self.inner.reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        self.inner.deregister(poll)
    }
}

impl Evented for TcpStream {
    fn register(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()> {
        let ref_cell: &RefCell<net::TcpStream> = self.inner.borrow();
        let stream = ref_cell.borrow();
        stream.register(poll, token, interest, opts)
    }

    fn reregister(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()> {
        let ref_cell: &RefCell<net::TcpStream> = self.inner.borrow();
        let stream = ref_cell.borrow();
        stream.reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        let ref_cell: &RefCell<net::TcpStream> = self.inner.borrow();
        let stream = ref_cell.borrow();
        stream.deregister(poll)
    }
}

impl TcpListener {
    fn new(listener: mio::net::TcpListener) -> TcpListener {
        TcpListener { inner: Rc::new(listener), accept_source_token: None }
    }

    fn poll_accept(&mut self, lw: &LocalWaker) -> task::Poll<io::Result<(TcpStream, SocketAddr)>> {
        debug!("waker addr: {:p}", lw);
        if let None = self.accept_source_token {
            self.accept_source_token = Some(register_source(self.clone(), lw.clone(), Ready::readable()));
        }
        match self.inner.accept() {
            Ok((stream, addr)) => {
                debug!("accept stream from: {}", addr);
                task::Poll::Ready(Ok((TcpStream::new(stream), addr)))
            }
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                debug!("accept would block");
                unsafe { reregister_source(self.accept_source_token.unwrap(), Ready::readable()) };
                task::Poll::Pending
            }
            Err(err) => task::Poll::Ready(Err(err))
        }
    }

    pub fn bind(addr: &SocketAddr) -> io::Result<TcpListener> {
        let l = mio::net::TcpListener::bind(addr)?;
        Ok(TcpListener::new(l))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    pub fn ttl(&self) -> io::Result<u32> {
        self.inner.ttl()
    }
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.set_ttl(ttl)
    }

    pub fn accept(&mut self) -> TcpAcceptState {
        TcpAcceptState { listener: self }
    }
}

impl TcpStream {
    pub fn new(connected: mio::net::TcpStream) -> TcpStream {
        TcpStream { inner: Rc::new(RefCell::new(connected)), read_source_token: None, write_source_token: None }
    }

    pub fn read(&mut self) -> StreamReadState {
        StreamReadState { stream: self }
    }

    pub fn write(&mut self, data: Vec<u8>) -> StreamWriteState {
        StreamWriteState { stream: self, data }
    }

    pub fn write_str(&mut self, data: &str) -> StreamWriteState {
        StreamWriteState { stream: self, data: data.as_bytes().to_vec() }
    }
}

impl TcpStream {
    fn read_poll(&mut self, lw: &LocalWaker) -> task::Poll<io::Result<Vec<u8>>> {
        if let None = self.read_source_token {
            self.read_source_token = Some(register_source(self.clone(), lw.clone(), Ready::readable()));
        }
        let mut ret = Vec::new();
        let ref_cell: &RefCell<net::TcpStream> = self.inner.borrow();
        match ref_cell.borrow_mut().read_to_end(&mut ret) {
            Ok(_) => task::Poll::Ready(Ok(ret)),
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                unsafe { reregister_source(self.read_source_token.unwrap(), Ready::readable()) };
                task::Poll::Pending
            }
            Err(err) => task::Poll::Ready(Err(err))
        }
    }

    fn write_poll(&mut self, data: &[u8], lw: &LocalWaker) -> task::Poll<io::Result<usize>> {
        if let None = self.write_source_token {
            self.write_source_token = Some(register_source(self.clone(), lw.clone(), Ready::writable()));
        }
        let ref_cell: &RefCell<net::TcpStream> = self.inner.borrow();
        match ref_cell.borrow_mut().write(data) {
            Ok(n) => task::Poll::Ready(Ok(n)),
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                unsafe { reregister_source(self.write_source_token.unwrap(), Ready::writable()) };
                task::Poll::Pending
            }
            Err(err) => task::Poll::Ready(Err(err))
        }
    }
}

impl<'a> Future for TcpAcceptState<'a> {
    type Output = io::Result<(TcpStream, SocketAddr)>;
    fn poll(mut self: Pin<&mut Self>, lw: &LocalWaker) -> task::Poll<<Self as Future>::Output> {
        self.listener.poll_accept(lw)
    }
}

impl<'a> Future for StreamReadState<'a> {
    type Output = io::Result<Vec<u8>>;
    fn poll(mut self: Pin<&mut Self>, lw: &LocalWaker) -> task::Poll<<Self as Future>::Output> {
        self.stream.read_poll(lw)
    }
}

impl<'a> Future for StreamWriteState<'a> {
    type Output = io::Result<usize>;
    fn poll(mut self: Pin<&mut Self>, lw: &LocalWaker) -> task::Poll<<Self as Future>::Output> {
        let data = self.data.clone();
        self.stream.write_poll(data.as_slice(), lw)
    }
}


fn main() {
    env_logger::init();
    block_on(async {
        let mut listener = TcpListener::bind(&"127.0.0.1:7878".parse().unwrap()).expect("TcpListener bind fail");
        info!("Listening on 127.0.0.1:7878");
        while let Ok((stream, addr)) = await!(listener.accept()) {
            info!("connection from {}", addr);
        }
    })
}