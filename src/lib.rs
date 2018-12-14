#![feature(arbitrary_self_types)]
#![feature(futures_api)]
#![feature(pin)]
#![feature(fnbox)]

#[macro_use]
extern crate log;

pub mod executor;

pub mod fs;