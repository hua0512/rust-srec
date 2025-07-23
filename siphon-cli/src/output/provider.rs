use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use bytes::Bytes;
use tracing::{debug, info};

/// OutputFormat enum to specify the type of output
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Write to a file
    File,
    /// Write to stdout
    Stdout,
    /// Write to stderr
    Stderr,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "file" => Ok(OutputFormat::File),
            "stdout" => Ok(OutputFormat::Stdout),
            "stderr" => Ok(OutputFormat::Stderr),
            _ => Err(format!("Unknown output format: {s}")),
        }
    }
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

/// A trait defining the interface for output providers
pub trait OutputProvider: Send + Sync {
    /// Write bytes to the output
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize>;

    /// Flush any buffered data
    fn flush(&mut self) -> io::Result<()>;

    /// Get total bytes written so far
    fn bytes_written(&self) -> u64;

    /// Close the provider and perform any necessary cleanup
    fn close(&mut self) -> io::Result<()>;
}

/// A file-based output provider
pub struct FileOutputProvider {
    writer: BufWriter<File>,
    bytes_written: u64,
}

impl FileOutputProvider {
    /// Create a new file output provider
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let file = File::create(&path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            bytes_written: 0,
        })
    }
}

impl OutputProvider for FileOutputProvider {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let bytes_written = self.writer.write(bytes)?;
        self.bytes_written += bytes_written as u64;
        Ok(bytes_written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    fn close(&mut self) -> io::Result<()> {
        self.flush()
    }
}

/// A pipe-based output provider
pub struct PipeOutputProvider {
    writer: BufWriter<Box<dyn Write + Send + Sync>>,
    bytes_written: u64,
}

impl PipeOutputProvider {
    /// Create a new stdout output provider
    pub fn stdout() -> io::Result<Self> {
        Ok(Self {
            writer: BufWriter::new(Box::new(io::stdout())),
            bytes_written: 0,
        })
    }

    /// Create a new stderr output provider
    pub fn stderr() -> io::Result<Self> {
        Ok(Self {
            writer: BufWriter::new(Box::new(io::stderr())),
            bytes_written: 0,
        })
    }
}

impl OutputProvider for PipeOutputProvider {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let bytes_written = self.writer.write(bytes)?;
        self.bytes_written += bytes_written as u64;
        Ok(bytes_written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    fn close(&mut self) -> io::Result<()> {
        self.flush()
    }
}

/// Output manager that handles creating and managing output providers
pub struct OutputManager {
    provider: Box<dyn OutputProvider>,
}

impl OutputManager {
    /// Create a new output manager with a specific output format
    pub fn new(format: OutputFormat, output_path: Option<PathBuf>) -> io::Result<Self> {
        let provider: Box<dyn OutputProvider> = match format {
            OutputFormat::File => {
                let path = output_path.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Output path required for file output",
                    )
                })?;
                Box::new(FileOutputProvider::new(path)?)
            }
            OutputFormat::Stdout => Box::new(PipeOutputProvider::stdout()?),
            OutputFormat::Stderr => Box::new(PipeOutputProvider::stderr()?),
        };

        Ok(Self { provider })
    }

    /// Write data to the output provider with progress updates
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.provider.write(bytes)
    }

    /// Write bytes from the Bytes type
    pub fn write_bytes(&mut self, bytes: &Bytes) -> io::Result<usize> {
        self.write(bytes)
    }

    /// Flush the output
    pub fn flush(&mut self) -> io::Result<()> {
        self.provider.flush()
    }

    /// Close the output and finalize
    pub fn close(mut self) -> io::Result<u64> {
        self.flush()?;
        self.provider.close()?;

        // Progress updates are now handled by the event system

        Ok(self.provider.bytes_written())
    }
}

/// Create an output provider based on the format and configuration
pub fn create_output(
    format: OutputFormat,
    output_dir: &Path,
    base_name: &str,
    extension: &str,
) -> io::Result<OutputManager> {
    match format {
        OutputFormat::File => {
            // Ensure output directory exists
            std::fs::create_dir_all(output_dir)?;

            let path = output_dir.join(format!("{base_name}.{extension}"));
            info!("Creating file output: {}", path.display());

            OutputManager::new(format, Some(path))
        }
        OutputFormat::Stdout => {
            debug!("Creating stdout output");
            OutputManager::new(format, None)
        }
        OutputFormat::Stderr => {
            debug!("Creating stderr output");
            OutputManager::new(format, None)
        }
    }
}
