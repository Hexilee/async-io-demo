#![feature(fnbox)]


#[cfg(test)]
mod tcp;

#[cfg(test)]
mod fs;

#[cfg(test)]
mod poll_timeout;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
