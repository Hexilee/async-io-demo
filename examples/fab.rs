#![feature(generators, generator_trait)]

use std::ops::{Generator, GeneratorState};

fn main() {
    let mut gen = fab(5);
    loop {
        match unsafe { gen.resume() } {
            GeneratorState::Yielded(value) => println!("yield {}", value),
            GeneratorState::Complete(ret) => {
                println!("return {}", ret);
                break;
            }
        }
    }
}

fn fab(mut n: u64) -> impl Generator<Yield=u64, Return=u64> {
    move || {
        let mut last = 0u64;
        let mut current = 1;
        yield last;
        while n > 0 {
            yield current;
            let tmp = last;
            last = current;
            current = tmp + last;
            n -= 1;
        }
        return last;
    }
}