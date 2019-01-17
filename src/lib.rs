#![feature(arbitrary_self_types)]
#![feature(futures_api)]
#![feature(fnbox)]

#[macro_use]
extern crate log;

pub mod executor;

pub mod fs;

pub mod fs_mio;

pub mod fs_future;