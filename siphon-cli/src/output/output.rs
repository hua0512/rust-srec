use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use tracing::{debug, info};

use crate::utils::progress::ProgressManager;

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
            _ => Err(format!("Unknown output format: {}", s)),
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

    /// Get the path if this is a file provider, None otherwise
    fn path(&self) -> Option<&Path>;

    /// Get total bytes written so far
    fn bytes_written(&self) -> u64;

    /// Close the provider and perform any necessary cleanup
    fn close(&mut self) -> io::Result<()>;
}

/// A file-based output provider
pub struct FileOutputProvider {
    writer: BufWriter<File>,
    path: PathBuf,
    bytes_written: u64,
}

impl FileOutputProvider {
    /// Create a new file output provider
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let file = File::create(&path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            path,
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

    fn path(&self) -> Option<&Path> {
        Some(&self.path)
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

    fn path(&self) -> Option<&Path> {
        None
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
    progress_manager: Option<ProgressManager>,
    update_interval: usize,
    update_counter: usize,
}

impl OutputManager {
    /// Create a new output manager with a specific output format
    pub fn new(
        format: OutputFormat,
        output_path: Option<PathBuf>,
        progress_manager: Option<ProgressManager>,
    ) -> io::Result<Self> {
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

        Ok(Self {
            provider,
            progress_manager,
            update_interval: 100, // Update progress every 100 writes
            update_counter: 0,
        })
    }

    /// Write data to the output provider with progress updates
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = self.provider.write(bytes);

        // Update progress if we have a progress manager
        if let Some(ref pm) = self.progress_manager {
            self.update_counter += 1;
            if self.update_counter >= self.update_interval {
                pm.update_main_progress(self.provider.bytes_written());

                // Update file progress if it exists
                if let Some(file_pb) = pm.get_file_progress() {
                    file_pb.set_position(self.provider.bytes_written());
                }

                self.update_counter = 0;
            }
        }

        result
    }

    /// Write bytes from the Bytes type
    pub fn write_bytes(&mut self, bytes: &Bytes) -> io::Result<usize> {
        self.write(bytes)
    }

    /// Flush the output
    pub fn flush(&mut self) -> io::Result<()> {
        self.provider.flush()
    }

    /// Get the path of the output (if it's a file)
    pub fn path(&self) -> Option<&Path> {
        self.provider.path()
    }

    /// Get total bytes written
    pub fn bytes_written(&self) -> u64 {
        self.provider.bytes_written()
    }

    /// Set a new progress manager
    pub fn set_progress_manager(&mut self, progress_manager: ProgressManager) {
        self.progress_manager = Some(progress_manager);
    }

    /// Close the output and finalize
    pub fn close(mut self) -> io::Result<u64> {
        self.flush()?;
        self.provider.close()?;

        // Final progress update
        if let Some(ref pm) = self.progress_manager {
            pm.update_main_progress(self.provider.bytes_written());

            // Final file progress update
            if let Some(file_pb) = pm.get_file_progress() {
                file_pb.set_position(self.provider.bytes_written());
            }
        }

        Ok(self.provider.bytes_written())
    }
}

/// Create an output provider based on the format and configuration
pub fn create_output(
    format: OutputFormat,
    output_dir: &Path,
    base_name: &str,
    extension: &str,
    progress_manager: Option<ProgressManager>,
) -> io::Result<OutputManager> {
    match format {
        OutputFormat::File => {
            // Ensure output directory exists
            std::fs::create_dir_all(output_dir)?;

            let path = output_dir.join(format!("{}.{}", base_name, extension));
            info!("Creating file output: {}", path.display());

            OutputManager::new(format, Some(path), progress_manager)
        }
        OutputFormat::Stdout => {
            debug!("Creating stdout output");
            OutputManager::new(format, None, progress_manager)
        }
        OutputFormat::Stderr => {
            debug!("Creating stderr output");
            OutputManager::new(format, None, progress_manager)
        }
    }
}

// Helper function for async code that needs to write to the output manager
pub async fn write_stream_to_output<E>(
    stream: &mut (impl futures::Stream<Item = Result<Bytes, E>> + Unpin),
    output: Arc<Mutex<OutputManager>>,
) -> Result<u64, io::Error>
where
    E: std::error::Error,
{
    use futures::StreamExt;

    let mut total_bytes = 0u64;

    while let Some(result) = stream.next().await {
        match result {
            Ok(bytes) => {
                let bytes_len = bytes.len();
                // Write to output manager through mutex lock
                output
                    .lock()
                    .map_err(|e| {
                        io::Error::new(io::ErrorKind::Other, format!("Mutex error: {}", e))
                    })?
                    .write(&bytes)?;

                total_bytes += bytes_len as u64;
            }
            Err(e) => {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("Stream error: {}", e),
                ));
            }
        }
    }

    // Final flush
    output
        .lock()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Mutex error: {}", e)))?
        .flush()?;

    Ok(total_bytes)
}
