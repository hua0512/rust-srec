//! Output format configuration for mesio-cli
//!
//! This module provides the `OutputFormat` enum used to specify where output should be written.
//! The actual output strategies are implemented in `pipe_flv_strategy.rs` and `pipe_hls_strategy.rs`.

use std::io::{self, Write};

use clap::ValueEnum;

/// OutputFormat enum to specify the type of output
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// Write to a file
    #[default]
    File,
    /// Write to stdout
    Stdout,
    /// Write to stderr
    Stderr,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::File => write!(f, "file"),
            OutputFormat::Stdout => write!(f, "stdout"),
            OutputFormat::Stderr => write!(f, "stderr"),
        }
    }
}

/// The standard stream that pipe output writes media to. Logs and progress
/// bars must stay off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PipeStream {
    #[default]
    Stdout,
    Stderr,
}

impl PipeStream {
    /// The stream `format` writes media to; none for file output.
    pub fn of(format: OutputFormat) -> Option<Self> {
        match format {
            OutputFormat::File => None,
            OutputFormat::Stdout => Some(Self::Stdout),
            OutputFormat::Stderr => Some(Self::Stderr),
        }
    }

    pub fn open(self) -> PipeWriter {
        match self {
            Self::Stdout => PipeWriter::Stdout(io::stdout()),
            Self::Stderr => PipeWriter::Stderr(io::stderr()),
        }
    }
}

/// A writer to the [`PipeStream`] it was opened for.
pub enum PipeWriter {
    Stdout(io::Stdout),
    Stderr(io::Stderr),
}

impl Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout(stdout) => stdout.write(buf),
            Self::Stderr(stderr) => stderr.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout(stdout) => stdout.flush(),
            Self::Stderr(stderr) => stderr.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_output_writes_media_to_the_stream_it_names() {
        assert_eq!(PipeStream::of(OutputFormat::File), None);
        assert_eq!(
            PipeStream::of(OutputFormat::Stdout),
            Some(PipeStream::Stdout)
        );
        assert_eq!(
            PipeStream::of(OutputFormat::Stderr),
            Some(PipeStream::Stderr)
        );
        assert!(matches!(PipeStream::Stdout.open(), PipeWriter::Stdout(_)));
        assert!(matches!(PipeStream::Stderr.open(), PipeWriter::Stderr(_)));
    }
}
