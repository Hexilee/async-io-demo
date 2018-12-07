use mio::*;
use std::cell::RefCell;

const EXIT_TOKEN: Token = Token(0);

pub struct Executor {
    poll: Poll,
    handlers: RefCell<Vec<Handler>>,
    exit_readiness: SetReadiness,
}

struct Handler {
    token: Token,
    action: Box<Fn(&Poll, Event) -> Result<(), failure::Error>>,
}

impl Executor {
    pub fn new() -> Result<Self, failure::Error> {
        let poll = Poll::new()?;
        let (registration, exit_readiness) = Registration::new2();
        poll.register(&registration, EXIT_TOKEN, Ready::readable(), PollOpt::oneshot())?;
        Ok(Executor { poll, handlers: RefCell::new(Vec::new()), exit_readiness })
    }

    pub fn run(&self) -> Result<(), failure::Error> {
        let mut events = Events::with_capacity(1024);
        'outer: loop {
            self.poll.poll(&mut events, None)?;
            for event in events.iter() {
                if event.token() == EXIT_TOKEN {
                    break 'outer;
                }
                for handler in self.handlers.borrow().iter() {
                    if event.token() == handler.token {
                        (handler.action)(&self.poll, event)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn register<E, A>(&self, evented: &E,
                          token: Token,
                          interest: Ready,
                          opts: PollOpt,
                          action: A) -> Result<(), failure::Error>
        where E: ?Sized + Evented,
              A: Fn(&Poll, Event) -> Result<(), failure::Error> + 'static {
        self.poll.register(evented, token, interest, opts)?;
        self.handlers.borrow_mut().push(Handler { token, action: Box::new(action) });
        Ok(())
    }

    pub fn shutdown(&self) -> Result<(), failure::Error> {
        Ok(self.exit_readiness.set_readiness(Ready::readable())?)
    }
}
