//! Standalone, std-only compatibility helper. Copies bytes; never interprets them.

use std::io::{self, Write};

fn main() -> io::Result<()> {
    if std::env::args_os().len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "copy helper takes no arguments",
        ));
    }
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    io::copy(&mut input, &mut output)?;
    output.flush()
}
