//! Output format configuration for mesio-cli
//!
//! This module provides the `OutputFormat` enum used to specify where output should be written.
//! The actual output strategies are implemented in `pipe_flv_strategy.rs` and `pipe_hls_strategy.rs`.

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
