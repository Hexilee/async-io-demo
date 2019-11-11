use failure::Error;
use mio::*;
use slab::Slab;
use std::borrow::Borrow;
use std::cell::RefCell;
use std::future::Future;
use std::io::{self, Read, Write};
use std::mem::transmute;
use std::net::SocketAddr;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{self, Context, RawWaker, RawWakerVTable, Waker};

const MAX_RESOURCE_NUM: usize = 1 << 31;
const MAIN_TASK_TOKEN: Token = Token(MAX_RESOURCE_NUM);
const EVENT_CAP: usize = 1024;
//const POLL_TIME_OUT_MILL: u64 = 100;
const RAW_WAKER_V_TABLE: RawWakerVTable = RawWakerVTable::new(clone_raw, wake, wake, drop_raw);

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

type PinFuture<T> = Pin<Box<dyn Future<Output = T>>>;

struct Executor {
    poll: Poll,
    main_awaker: Awaker,
    tasks: RefCell<Slab<Task>>,
    sources: RefCell<Slab<Source>>,
}

struct Awaker {
    awake_readiness: SetReadiness,
    awake_registration: Registration,
}

struct Source {
    waker: Waker,
    evented: Box<dyn Evented>,
}

struct Task {
    waker: Awaker,
    inner_task: PinFuture<Result<(), Error>>,
}

#[derive(Clone)]
pub struct TcpListener {
    inner: Rc<net::TcpListener>,
    accept_source_token: Option<Token>,
}

#[derive(Clone)]
pub struct TcpStream {
    inner: Rc<RefCell<net::TcpStream>>,
    source_token: Option<Token>,
    readiness: Ready,
}

pub struct TcpAcceptState<'a> {
    listener: &'a mut TcpListener,
}

pub struct StreamReadState<'a> {
    stream: &'a mut TcpStream,
}

pub struct StreamWriteState<'a> {
    stream: &'a mut TcpStream,
    data: Vec<u8>,
}

fn clone_raw(ptr: *const ()) -> RawWaker {
    RawWaker::new(ptr, &RAW_WAKER_V_TABLE)
}

fn drop_raw(_ptr: *const ()) {}

fn wake(ptr: *const ()) {
    let readiness: SetReadiness = unsafe { transmute(ptr) };
    readiness.set_readiness(Ready::readable()).unwrap();
}

impl Awaker {
    fn gen_waker(&self) -> Waker {
        unsafe {
            Waker::from_raw(RawWaker::new(
                transmute(self.awake_readiness.clone()),
                &RAW_WAKER_V_TABLE,
            ))
        }
    }
}

impl Executor {
    pub fn new() -> Result<Self, Error> {
        let poll = Poll::new()?;
        let (awake_registration, awake_readiness) = Registration::new2();
        poll.register(
            &awake_registration,
            MAIN_TASK_TOKEN,
            Ready::all(),
            PollOpt::edge(),
        )?;
        Ok(Executor {
            poll,
            main_awaker: Awaker {
                awake_registration,
                awake_readiness,
            },
            tasks: RefCell::new(Slab::new()),
            sources: RefCell::new(Slab::new()),
        })
    }
}

thread_local! {
    static EXECUTOR: Executor = Executor::new().expect("initializing executor failed!")
}

pub fn block_on<R, F>(main_task: F) -> Result<R, Error>
where
    R: Sized,
    F: Future<Output = R>,
{
    EXECUTOR.with(move |executor: &Executor| -> Result<R, Error> {
        let mut pinned_task = Box::pin(main_task);
        let mut events = Events::with_capacity(EVENT_CAP);
        match pinned_task
            .as_mut()
            .poll(&mut Context::from_waker(&executor.main_awaker.gen_waker()))
        {
            task::Poll::Ready(result) => {
                debug!("main task complete");
                Ok(result)
            }
            task::Poll::Pending => {
                debug!("main task pending");
                loop {
                    // executor.poll.poll(&mut events, Some(Duration::from_millis(POLL_TIME_OUT_MILL))).expect("polling failed");
                    executor.poll.poll(&mut events, None)?;
                    debug!("events empty: {}", events.is_empty());
                    for event in events.iter() {
                        debug!("get event: {:?}", event.token());
                        match event.token() {
                            MAIN_TASK_TOKEN => {
                                debug!("receive a main task event");
                                match pinned_task.as_mut().poll(&mut Context::from_waker(
                                    &executor.main_awaker.gen_waker(),
                                )) {
                                    task::Poll::Ready(result) => {
                                        return Ok(result);
                                    }
                                    task::Poll::Pending => {
                                        debug!("main task pending again");
                                        continue;
                                    }
                                }
                            }
                            token if is_source(token) => {
                                debug!("receive a source event: {:?}", token);
                                let index = unsafe { index_from_source_token(token) };
                                debug!("source: Index({})", index);
                                let source = &executor.sources.borrow()[index];
                                debug!("source addr: {:p}", source);
                                source.waker.wake_by_ref();
                            }

                            token if is_task(token) => {
                                debug!("receive a task event: {:?}", token);
                                let index = unsafe { index_from_task_token(token) };
                                let mut tasks = executor.tasks.borrow_mut();
                                let task = &mut tasks[index];
                                match task
                                    .inner_task
                                    .as_mut()
                                    .poll(&mut Context::from_waker(&task.waker.gen_waker()))
                                {
                                    task::Poll::Ready(result) => {
                                        debug!("task({:?}) complete", token);
                                        executor.poll.deregister(&task.waker.awake_registration)?;
                                        tasks.remove(index);
                                        result?;
                                    }
                                    task::Poll::Pending => {
                                        debug!("task({:?}) pending", token);
                                        continue;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    })
}

pub fn spawn<F: Future<Output = Result<(), Error>> + 'static>(task: F) -> Result<(), Error> {
    EXECUTOR.with(move |executor: &Executor| {
        let (awake_registration, awake_readiness) = Registration::new2();
        let index = executor.tasks.borrow_mut().insert(Task {
            inner_task: Box::pin(task),
            waker: Awaker {
                awake_readiness,
                awake_registration,
            },
        });
        let token = get_task_token(index);
        let mut tasks = executor.tasks.borrow_mut();
        let task = &mut tasks[index];
        debug!("task({:?}) spawn", token);
        match task
            .inner_task
            .as_mut()
            .poll(&mut Context::from_waker(&task.waker.gen_waker()))
        {
            task::Poll::Ready(_) => {
                debug!("task({:?}) complete when spawn", token);
                tasks.remove(index);
            }
            task::Poll::Pending => {
                executor.poll.register(
                    &task.waker.awake_registration,
                    token,
                    Ready::all(),
                    PollOpt::edge(),
                )?;
                debug!("task({:?}) pending", token);
            }
        };
        Ok(())
    })
}

pub fn register_source<T: Evented + 'static>(
    evented: T,
    waker: Waker,
    interest: Ready,
) -> Result<Token, Error> {
    EXECUTOR.with(move |executor: &Executor| {
        let index = executor.sources.borrow_mut().insert(Source {
            waker,
            evented: Box::new(evented),
        });
        debug!("new sources: Index({})", index);
        let token = get_source_token(index);
        let source = &executor.sources.borrow()[index];
        executor
            .poll
            .register(&source.evented, token, interest, PollOpt::edge())?;
        debug!("register source: {:?}", token);
        Ok(token)
    })
}

// panic when token is ord
unsafe fn reregister_source(token: Token, interest: Ready) -> Result<(), Error> {
    EXECUTOR.with(move |executor: &Executor| {
        let index = index_from_source_token(token);
        debug!("reregister source: Index({})", index);
        let source = &executor.sources.borrow()[index];
        executor
            .poll
            .reregister(&source.evented, token, interest, PollOpt::edge())?;
        debug!("source addr: {:p}", source);
        Ok(())
    })
}

// panic when token is ord
unsafe fn drop_source(token: Token) -> Result<(), Error> {
    EXECUTOR.with(move |executor: &Executor| {
        let index = index_from_source_token(token);
        let mut sources = executor.sources.borrow_mut();
        let source = &sources[index];
        executor.poll.deregister(&source.evented)?;
        sources.remove(index);
        Ok(())
    })
}

impl Evented for TcpListener {
    fn register(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        self.inner.register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        self.inner.reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        self.inner.deregister(poll)
    }
}

impl Evented for TcpStream {
    fn register(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        let ref_cell: &RefCell<net::TcpStream> = self.inner.borrow();
        let stream = ref_cell.borrow();
        stream.register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
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
        TcpListener {
            inner: Rc::new(listener),
            accept_source_token: None,
        }
    }

    fn poll_accept(&mut self, waker: Waker) -> task::Poll<Result<(TcpStream, SocketAddr), Error>> {
        if self.accept_source_token.is_none() {
            self.accept_source_token = Some(
                match register_source(self.clone(), waker, Ready::readable()) {
                    Ok(token) => token,
                    Err(err) => return task::Poll::Ready(Err(err)),
                },
            )
        };
        match self.inner.accept() {
            Ok((stream, addr)) => {
                debug!("accept stream from: {}", addr);
                task::Poll::Ready(Ok((TcpStream::new(stream), addr)))
            }
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                debug!("accept would block");
                task::Poll::Pending
            }
            Err(err) => task::Poll::Ready(Err(err.into())),
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
        TcpStream {
            inner: Rc::new(RefCell::new(connected)),
            source_token: None,
            readiness: Ready::empty(),
        }
    }

    pub fn read(&mut self) -> StreamReadState {
        StreamReadState { stream: self }
    }

    pub fn write(&mut self, data: Vec<u8>) -> StreamWriteState {
        StreamWriteState { stream: self, data }
    }

    pub fn write_str(&mut self, data: &str) -> StreamWriteState {
        StreamWriteState {
            stream: self,
            data: data.as_bytes().to_vec(),
        }
    }

    pub fn close(self) {
        if let Some(token) = self.source_token {
            unsafe { drop_source(token).unwrap() };
        }
    }
}

impl TcpStream {
    fn read_poll(&mut self, waker: Waker) -> task::Poll<Result<Vec<u8>, Error>> {
        match self.source_token {
            None => {
                self.readiness = Ready::readable();
                self.source_token =
                    Some(match register_source(self.clone(), waker, self.readiness) {
                        Ok(token) => token,
                        Err(err) => return task::Poll::Ready(Err(err)),
                    })
            }

            Some(token) => {
                if !self.readiness.is_readable() {
                    self.readiness |= Ready::readable();
                    if let Err(err) = unsafe { reregister_source(token, self.readiness) } {
                        return task::Poll::Ready(Err(err));
                    }
                }
            }
        }
        let mut ret = [0u8; 1024];
        let ref_cell: &RefCell<net::TcpStream> = self.inner.borrow();
        match ref_cell.borrow_mut().read(&mut ret) {
            Ok(n) => {
                debug!("stream read {} bytes", n);
                task::Poll::Ready(Ok(ret[..n].to_vec()))
            }
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                debug!("stream read pending");
                task::Poll::Pending
            }
            Err(err) => task::Poll::Ready(Err(err.into())),
        }
    }

    fn write_poll(&mut self, data: &[u8], waker: Waker) -> task::Poll<Result<usize, Error>> {
        match self.source_token {
            None => {
                self.readiness = Ready::writable();
                self.source_token =
                    Some(match register_source(self.clone(), waker, self.readiness) {
                        Ok(token) => token,
                        Err(err) => return task::Poll::Ready(Err(err)),
                    })
            }

            Some(token) => {
                if !self.readiness.is_writable() {
                    self.readiness |= Ready::writable();
                    if let Err(err) = unsafe { reregister_source(token, self.readiness) } {
                        return task::Poll::Ready(Err(err));
                    };
                }
            }
        }
        let ref_cell: &RefCell<net::TcpStream> = self.inner.borrow();
        match ref_cell.borrow_mut().write(data) {
            Ok(n) => task::Poll::Ready(Ok(n)),
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => task::Poll::Pending,
            Err(err) => task::Poll::Ready(Err(err.into())),
        }
    }
}

impl<'a> Future for TcpAcceptState<'a> {
    type Output = Result<(TcpStream, SocketAddr), Error>;
    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> task::Poll<<Self as Future>::Output> {
        self.listener.poll_accept(cx.waker().clone())
    }
}

impl<'a> Future for StreamReadState<'a> {
    type Output = Result<Vec<u8>, Error>;
    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> task::Poll<<Self as Future>::Output> {
        self.stream.read_poll(cx.waker().clone())
    }
}

impl<'a> Future for StreamWriteState<'a> {
    type Output = Result<usize, Error>;
    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> task::Poll<<Self as Future>::Output> {
        let data = self.data.clone();
        self.stream.write_poll(data.as_slice(), cx.waker().clone())
    }
}
