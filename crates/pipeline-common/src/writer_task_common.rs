use chrono::Utc;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tracing::debug;

/// Expands a filename template with optional sequence number.
/// Replaces `%i` with the sequence number if provided.
pub fn expand_filename_template(template: &str, sequence_number: Option<u32>) -> String {
    if let Some(seq) = sequence_number {
        template.replace("%i", &seq.to_string())
    } else {
        template.to_string()
    }
}

/// Action to take after writing an item.
#[derive(Debug, Clone, Copy)]
pub enum PostWriteAction {
    /// Do nothing.
    None,
    /// Close the current file.
    Close,
    /// Rotate the current file.
    Rotate,
}

/// Configuration for the writer task.
#[derive(Debug, Clone)]
pub struct WriterConfig {
    /// Base directory for output files.
    pub base_path: PathBuf,
    /// File name prefix.
    pub file_name_template: String,
    /// File name extension.
    pub file_extension: String,
    /// Custom data for the strategy.
    pub strategy_specific_config: Option<Arc<dyn std::any::Any + Send + Sync>>,
}

impl WriterConfig {
    pub fn new(base_path: PathBuf, file_name_template: String, file_extension: String) -> Self {
        Self {
            base_path,
            file_name_template,
            file_extension,
            strategy_specific_config: None,
        }
    }
}

/// State of the writer task.
#[derive(Debug, Default)]
pub struct WriterState {
    /// Current output file path.
    pub current_path: PathBuf,
    pub current_file_path: Option<PathBuf>,
    /// Number of items written to the current file.
    pub items_written_current_file: usize,
    /// Total number of items written across all files.
    pub items_written_total: usize,
    /// Number of bytes written to the current file.
    pub bytes_written_current_file: u64,
    /// Total number of bytes written across all files.
    pub bytes_written_total: u64,
    /// Timestamp when the current file was opened.
    pub current_file_opened_at: Option<chrono::DateTime<Utc>>,
    /// Sequence number for file naming.
    pub file_sequence_number: u32,
}

impl WriterState {
    pub fn reset_for_new_file(&mut self, new_path: PathBuf) {
        self.current_path = new_path.clone();
        self.current_file_path = Some(new_path);
        self.items_written_current_file = 0;
        self.bytes_written_current_file = 0;
        self.current_file_opened_at = Some(Utc::now());
    }
}

/// Error type for the writer task.
#[derive(Error, Debug)]
pub enum TaskError<StrategyError: std::error::Error + Send + Sync + 'static> {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("Strategy error: {0}")]
    Strategy(StrategyError),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("File rotation error: {0}")]
    Rotation(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Trait defining the strategy for formatting and writing data.
pub trait FormatStrategy<D>: Send + Sync + 'static {
    type Writer: Write;
    type StrategyError: std::error::Error + Send + Sync + 'static;

    /// Creates a new writer for the given path.
    /// This is typically a `BufWriter<File>` or similar.
    fn create_writer(&self, path: &Path) -> Result<Self::Writer, Self::StrategyError>;

    /// Writes a single data item to the writer.
    /// Should return the number of bytes written.
    fn write_item(
        &mut self,
        writer: &mut Self::Writer,
        item: &D,
    ) -> Result<u64, Self::StrategyError>;

    /// Determines if the current file should be rotated based on the state and config.
    fn should_rotate_file(&self, config: &WriterConfig, state: &WriterState) -> bool;

    /// Generates the path for the next output file.
    fn next_file_path(&self, config: &WriterConfig, state: &WriterState) -> PathBuf;

    /// Called when a new file is opened, before any items are written.
    /// Useful for writing headers or initial metadata.
    fn on_file_open(
        &mut self,
        writer: &mut Self::Writer,
        path: &Path,
        config: &WriterConfig,
        state: &WriterState,
    ) -> Result<u64, Self::StrategyError>;

    /// Called when a file is about to be closed.
    /// Useful for writing footers, final metadata, or flushing buffers.
    fn on_file_close(
        &mut self,
        writer: &mut Self::Writer,
        path: &Path,
        config: &WriterConfig,
        state: &WriterState,
    ) -> Result<u64, Self::StrategyError>;

    /// Optional: Called after an item has been successfully written.
    /// Can be used for logging or updating internal strategy state.
    fn after_item_written(
        &mut self,
        _item: &D,
        _bytes_written: u64,
        _state: &WriterState,
    ) -> Result<PostWriteAction, Self::StrategyError> {
        Ok(PostWriteAction::None)
    }

    /// Optional: Called if an error occurs during writing an item.
    fn on_write_error(&mut self, _error: &Self::StrategyError, _item: &D) {
        // Default: do nothing
    }
}

pub type FileOpCallback = Box<dyn Fn(&Path, u32) + Send + Sync>;

/// Generic writer task.
pub struct WriterTask<D, S: FormatStrategy<D>> {
    config: WriterConfig,
    state: WriterState,
    strategy: S,
    writer: Option<S::Writer>,
    on_file_open_callback: Option<FileOpCallback>,
    on_file_close_callback: Option<FileOpCallback>,
}

impl<D, S: FormatStrategy<D>> WriterTask<D, S> {
    pub fn new(config: WriterConfig, strategy: S) -> Self {
        std::fs::create_dir_all(&config.base_path).unwrap_or_else(|e| {
            eprintln!("Failed to create base path {:?}: {}", &config.base_path, e);
        });
        Self {
            config,
            state: WriterState::default(),
            strategy,
            writer: None,
            on_file_open_callback: None,
            on_file_close_callback: None,
        }
    }

    pub fn set_on_file_open_callback<F>(&mut self, callback: F)
    where
        F: Fn(&Path, u32) + Send + Sync + 'static,
    {
        self.on_file_open_callback = Some(Box::new(callback));
    }

    pub fn set_on_file_close_callback<F>(&mut self, callback: F)
    where
        F: Fn(&Path, u32) + Send + Sync + 'static,
    {
        self.on_file_close_callback = Some(Box::new(callback));
    }

    fn ensure_writer_open(&mut self) -> Result<(), TaskError<S::StrategyError>> {
        if self.writer.is_none() {
            self.open_initial_writer()?;
        } else if self.strategy.should_rotate_file(&self.config, &self.state) {
            self.rotate_file()?;
        }
        Ok(())
    }

    fn open_initial_writer(&mut self) -> Result<(), TaskError<S::StrategyError>> {
        // This should only be called when there is no writer.
        if self.writer.is_some() {
            return Err(TaskError::Internal(
                "Initial writer already exists".to_string(),
            ));
        }

        let initial_path = self.strategy.next_file_path(&self.config, &self.state);
        if let Some(parent) = initial_path.parent() {
            std::fs::create_dir_all(parent).map_err(TaskError::Io)?;
        }

        debug!("Creating initial writer for file: {:?}", initial_path);

        let mut new_writer = self
            .strategy
            .create_writer(&initial_path)
            .map_err(TaskError::Strategy)?;
        self.state.reset_for_new_file(initial_path.clone());

        debug!("Opening initial file: {:?}", initial_path);

        let bytes_opened = self
            .strategy
            .on_file_open(&mut new_writer, &initial_path, &self.config, &self.state)
            .map_err(TaskError::Strategy)?;
        self.state.bytes_written_current_file += bytes_opened;
        self.state.bytes_written_total += bytes_opened;

        if let Some(cb) = &self.on_file_open_callback {
            cb(&initial_path, self.state.file_sequence_number);
        }

        debug!("Initial writer opened for file: {:?}", initial_path);

        self.writer = Some(new_writer);
        Ok(())
    }

    fn rotate_file(&mut self) -> Result<(), TaskError<S::StrategyError>> {
        // close the existing writer
        if let Some(mut writer) = self.writer.take() {
            debug!(
                "Closing file for rotation: {:?}",
                self.state.current_file_path
            );
            if let Some(path) = &self.state.current_file_path {
                let bytes_closed = self
                    .strategy
                    .on_file_close(&mut writer, path, &self.config, &self.state)
                    .map_err(TaskError::Strategy)?;
                self.state.bytes_written_current_file += bytes_closed;
                self.state.bytes_written_total += bytes_closed;

                writer.flush().map_err(TaskError::Io)?;

                if let Some(cb) = &self.on_file_close_callback {
                    cb(path, self.state.file_sequence_number);
                }
            }
        } else {
            // This should not happen if called from ensure_writer_open
            return Err(TaskError::Internal(
                "rotate_file called without an open writer".to_string(),
            ));
        }

        // increment sequence number for the new file
        self.state.file_sequence_number += 1;

        // open the new writer
        let next_path = self.strategy.next_file_path(&self.config, &self.state);
        if let Some(parent) = next_path.parent() {
            std::fs::create_dir_all(parent).map_err(TaskError::Io)?;
        }

        debug!("Creating new writer for file (rotation): {:?}", next_path);

        let mut new_writer = self
            .strategy
            .create_writer(&next_path)
            .map_err(TaskError::Strategy)?;
        self.state.reset_for_new_file(next_path.clone());

        debug!("Opening new file after rotation: {:?}", next_path);

        let bytes_opened = self
            .strategy
            .on_file_open(&mut new_writer, &next_path, &self.config, &self.state)
            .map_err(TaskError::Strategy)?;
        self.state.bytes_written_current_file += bytes_opened;
        self.state.bytes_written_total += bytes_opened;

        if let Some(cb) = &self.on_file_open_callback {
            cb(&next_path, self.state.file_sequence_number);
        }

        debug!("Writer opened for file: {:?}", next_path);

        self.writer = Some(new_writer);
        Ok(())
    }

    pub fn process_item(&mut self, item: D) -> Result<(), TaskError<S::StrategyError>> {
        self.ensure_writer_open()?;

        if let Some(writer) = self.writer.as_mut() {
            match self.strategy.write_item(writer, &item) {
                Ok(bytes_written) => {
                    self.state.items_written_current_file += 1;
                    self.state.items_written_total += 1;
                    self.state.bytes_written_current_file += bytes_written;
                    self.state.bytes_written_total += bytes_written;
                    let post_write_action = self
                        .strategy
                        .after_item_written(&item, bytes_written, &self.state)
                        .map_err(TaskError::Strategy)?;
                    match post_write_action {
                        PostWriteAction::None => {}
                        PostWriteAction::Close => {
                            self.close()?;
                        }
                        PostWriteAction::Rotate => {
                            self.rotate_file()?;
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    self.strategy.on_write_error(&e, &item);
                    Err(TaskError::Strategy(e))
                }
            }
        } else {
            Err(TaskError::Internal(
                "Writer not open after ensure_writer_open call".to_string(),
            ))
        }
    }

    pub fn flush(&mut self) -> Result<(), TaskError<S::StrategyError>> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush().map_err(TaskError::Io)?;
        }
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), TaskError<S::StrategyError>> {
        if let Some(mut writer) = self.writer.take() {
            if let Some(path) = &self.state.current_file_path {
                let bytes_closed = self
                    .strategy
                    .on_file_close(&mut writer, path, &self.config, &self.state)
                    .map_err(TaskError::Strategy)?;
                self.state.bytes_written_current_file += bytes_closed;
                self.state.bytes_written_total += bytes_closed;
                writer.flush().map_err(TaskError::Io)?;
                if let Some(cb) = &self.on_file_close_callback {
                    cb(path, self.state.file_sequence_number);
                }
            }
        }
        self.state.current_file_path = None;
        Ok(())
    }

    pub fn get_current_file_path(&self) -> Option<&PathBuf> {
        self.state.current_file_path.as_ref()
    }

    pub fn get_state(&self) -> &WriterState {
        &self.state
    }

    /// Returns a reference to the configuration.
    pub fn config(&self) -> &WriterConfig {
        &self.config
    }

    /// Returns a reference to the strategy.
    pub fn strategy(&self) -> &S {
        &self.strategy
    }

    /// Returns a mutable reference to the strategy.
    pub fn strategy_mut(&mut self) -> &mut S {
        &mut self.strategy
    }
}

/// A default file-based strategy for convenience.
/// This can be used directly or as a template for more complex strategies.
pub struct DefaultFileStrategy;

#[derive(Error, Debug)]
pub enum DefaultStrategyError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

impl<D: Send + Sync + 'static> FormatStrategy<D> for DefaultFileStrategy {
    type Writer = BufWriter<File>;
    type StrategyError = DefaultStrategyError;

    fn create_writer(&self, path: &Path) -> Result<Self::Writer, Self::StrategyError> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true) // Or append(true) depending on desired behavior for existing files
            .open(path)?;
        Ok(BufWriter::new(file))
    }

    fn write_item(
        &mut self,
        _writer: &mut Self::Writer,
        _item: &D,
    ) -> Result<u64, Self::StrategyError> {
        // This default strategy doesn't know how to write arbitrary D.
        // Users should implement this for their specific D.
        // For demonstration, let's assume D is `Vec<u8>` and write it.
        // if let Some(bytes_item) = (&item as &dyn std::any::Any).downcast_ref::<Vec<u8>>() {
        //     writer.write_all(bytes_item)?;
        //     Ok(bytes_item.len() as u64)
        // } else {
        //     Err(DefaultStrategyError::Io(io::Error::new(io::ErrorKind::InvalidInput, "DefaultFileStrategy cannot write this data type")))
        // }
        // The above is commented out as D is generic. This method MUST be implemented by concrete strategies.
        panic!(
            "DefaultFileStrategy::write_item must be implemented by a concrete strategy or this strategy should not be used directly with WriterTask::process_item"
        );
    }

    fn should_rotate_file(&self, _config: &WriterConfig, _state: &WriterState) -> bool {
        false
    }

    fn next_file_path(&self, config: &WriterConfig, state: &WriterState) -> PathBuf {
        let filename = expand_filename_template(
            &config.file_name_template,
            Some(state.file_sequence_number + 1),
        );
        config
            .base_path
            .join(format!("{}.{}", filename, config.file_extension))
    }

    fn on_file_open(
        &mut self,
        _writer: &mut Self::Writer,
        _path: &Path,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        Ok(0) // No header by default
    }

    fn on_file_close(
        &mut self,
        _writer: &mut Self::Writer,
        _path: &Path,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        Ok(0) // No footer by default
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // Dummy data type for testing
    struct TestData(String);

    #[derive(Debug)]
    struct TestStrategyError(String);
    impl std::fmt::Display for TestStrategyError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "TestStrategyError: {}", self.0)
        }
    }
    impl std::error::Error for TestStrategyError {}

    struct TestStrategy {
        item_count_to_rotate: usize,
        header_content: Option<String>,
        footer_content: Option<String>,
        items_written_for_rotation_check: usize,
    }

    impl FormatStrategy<TestData> for TestStrategy {
        type Writer = BufWriter<File>;
        type StrategyError = TestStrategyError;

        fn create_writer(&self, path: &Path) -> Result<Self::Writer, Self::StrategyError> {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)
                .map(BufWriter::new)
                .map_err(|e| TestStrategyError(format!("Failed to create writer: {e}")))
        }

        fn write_item(
            &mut self,
            writer: &mut Self::Writer,
            item: &TestData,
        ) -> Result<u64, Self::StrategyError> {
            self.items_written_for_rotation_check += 1;
            let bytes = item.0.as_bytes();
            writer
                .write_all(bytes)
                .and_then(|_| writer.write_all(b"\n"))
                .map_err(|e| TestStrategyError(format!("Failed to write item: {e}")))?;
            Ok((bytes.len() + 1) as u64)
        }

        fn should_rotate_file(&self, _config: &WriterConfig, state: &WriterState) -> bool {
            // Use items_written_current_file from state for actual rotation logic
            state.items_written_current_file >= self.item_count_to_rotate
        }

        fn next_file_path(&self, config: &WriterConfig, state: &WriterState) -> PathBuf {
            let filename = expand_filename_template(
                &config.file_name_template,
                Some(state.file_sequence_number),
            );
            config
                .base_path
                .join(format!("{}.{}", filename, config.file_extension))
        }

        fn on_file_open(
            &mut self,
            writer: &mut Self::Writer,
            _path: &Path,
            _config: &WriterConfig,
            _state: &WriterState,
        ) -> Result<u64, Self::StrategyError> {
            if let Some(header) = &self.header_content {
                writer
                    .write_all(header.as_bytes())
                    .map_err(|e| TestStrategyError(e.to_string()))?;
                writer
                    .write_all(b"\n")
                    .map_err(|e| TestStrategyError(e.to_string()))?;
                return Ok((header.len() + 1) as u64);
            }
            Ok(0)
        }

        fn on_file_close(
            &mut self,
            writer: &mut Self::Writer,
            _path: &Path,
            _config: &WriterConfig,
            _state: &WriterState,
        ) -> Result<u64, Self::StrategyError> {
            if let Some(footer) = &self.footer_content {
                writer
                    .write_all(footer.as_bytes())
                    .map_err(|e| TestStrategyError(e.to_string()))?;
                writer
                    .write_all(b"\n")
                    .map_err(|e| TestStrategyError(e.to_string()))?;
                return Ok((footer.len() + 1) as u64);
            }
            Ok(0)
        }
    }

    #[test]
    fn test_writer_task_basic_write_and_close() {
        let dir = tempdir().unwrap();
        let config = WriterConfig {
            base_path: dir.path().to_path_buf(),
            file_name_template: "test_basic_%i".to_string(),
            file_extension: "txt".to_string(),
            ..WriterConfig::new(
                dir.path().to_path_buf(),
                "test".to_string(),
                "txt".to_string(),
            )
        };
        let strategy = TestStrategy {
            item_count_to_rotate: 5,
            header_content: Some("HEADER".to_string()),
            footer_content: Some("FOOTER".to_string()),
            items_written_for_rotation_check: 0,
        };
        let mut task = WriterTask::new(config.clone(), strategy);

        task.process_item(TestData("item1".to_string())).unwrap();
        task.process_item(TestData("item2".to_string())).unwrap();
        task.close().unwrap();

        let expected_file_path = config.base_path.join("test_basic_0.txt");
        assert!(expected_file_path.exists());
        let content = fs::read_to_string(expected_file_path).unwrap();
        assert_eq!(content, "HEADER\nitem1\nitem2\nFOOTER\n");
        assert_eq!(task.get_state().items_written_total, 2);
    }

    #[test]
    fn test_writer_task_rotation() {
        let dir = tempdir().unwrap();
        let config = WriterConfig {
            base_path: dir.path().to_path_buf(),
            file_name_template: "test_rotate_%i".to_string(),
            file_extension: "log".to_string(),
            ..WriterConfig::new(
                dir.path().to_path_buf(),
                "test_rotate_%i".to_string(),
                "log".to_string(),
            )
        };
        // Strategy will rotate after 2 items based on its internal counter fed to should_rotate_file
        let strategy = TestStrategy {
            item_count_to_rotate: 2,
            header_content: None,
            footer_content: None,
            items_written_for_rotation_check: 0,
        };
        let mut task = WriterTask::new(config.clone(), strategy);

        task.process_item(TestData("data1".to_string())).unwrap(); // File 1
        task.process_item(TestData("data2".to_string())).unwrap(); // File 1, rotate after this
        task.process_item(TestData("data3".to_string())).unwrap(); // File 2
        task.process_item(TestData("data4".to_string())).unwrap(); // File 2, rotate after this
        task.process_item(TestData("data5".to_string())).unwrap(); // File 3
        task.close().unwrap();

        let file1_path = config.base_path.join("test_rotate_0.log");
        let file2_path = config.base_path.join("test_rotate_1.log");
        let file3_path = config.base_path.join("test_rotate_2.log");

        assert!(file1_path.exists());
        assert!(file2_path.exists());
        assert!(file3_path.exists());

        let content1 = fs::read_to_string(file1_path).unwrap();
        let content2 = fs::read_to_string(file2_path).unwrap();
        let content3 = fs::read_to_string(file3_path).unwrap();

        assert_eq!(content1, "data1\ndata2\n");
        assert_eq!(content2, "data3\ndata4\n");
        assert_eq!(content3, "data5\n");

        assert_eq!(task.get_state().items_written_total, 5);
        assert_eq!(task.get_state().file_sequence_number, 2);
    }
}
