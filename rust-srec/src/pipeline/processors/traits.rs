//! Processor trait and related types.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

use crate::Result;
use crate::pipeline::job_queue::JobLogEntry;
use crate::pipeline::progress::ProgressReporter;

/// Type of processor (determines which worker pool handles it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProcessorType {
    /// CPU-bound processor (remux, thumbnail).
    Cpu,
    /// IO-bound processor (upload, file operations).
    Io,
}

/// Input for a processor.
#[derive(Debug, Clone)]
pub struct ProcessorInput {
    /// Input file paths.
    pub inputs: Vec<String>,
    /// Output file paths (if known/required upfront).
    pub outputs: Vec<String>,
    /// Additional configuration as JSON.
    pub config: Option<String>,
    /// Streamer ID.
    pub streamer_id: String,
    /// Session ID.
    pub session_id: String,
    /// Human-readable streamer name.
    pub streamer_name: Option<String>,
    /// Session/stream title.
    pub session_title: Option<String>,
    /// Platform name (e.g., "Twitch", "Huya").
    pub platform: Option<String>,
    /// When the job was originally created.
    /// Used for time-based placeholder expansion to ensure consistency across retries.
    pub created_at: DateTime<Utc>,
}

impl Default for ProcessorInput {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            config: None,
            streamer_id: String::new(),
            session_id: String::new(),
            streamer_name: None,
            session_title: None,
            platform: None,
            created_at: Utc::now(),
        }
    }
}

impl ProcessorInput {
    /// Create a new ProcessorInput with required fields.
    pub fn new(
        inputs: Vec<String>,
        outputs: Vec<String>,
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            inputs,
            outputs,
            config: None,
            streamer_id: streamer_id.into(),
            session_id: session_id.into(),
            streamer_name: None,
            session_title: None,
            platform: None,
            created_at: Utc::now(),
        }
    }

    /// Set the configuration.
    pub fn with_config(mut self, config: impl Into<String>) -> Self {
        self.config = Some(config.into());
        self
    }

    /// Set the streamer name.
    pub fn with_streamer_name(mut self, name: impl Into<String>) -> Self {
        self.streamer_name = Some(name.into());
        self
    }

    /// Set the session title.
    pub fn with_session_title(mut self, title: impl Into<String>) -> Self {
        self.session_title = Some(title.into());
        self
    }

    /// Set the platform.
    pub fn with_platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }

    /// Set the created_at timestamp.
    pub fn with_created_at(mut self, created_at: DateTime<Utc>) -> Self {
        self.created_at = created_at;
        self
    }
}

/// Processor context for emitting progress and other side-channel data.
#[derive(Clone)]
pub struct ProcessorContext {
    pub job_id: String,
    pub progress: ProgressReporter,
    pub log_sink: JobLogSink,
    pub cancellation_token: CancellationToken,
}

#[derive(Clone)]
pub struct JobLogSink {
    tx: tokio::sync::mpsc::Sender<JobLogEntry>,
    dropped: Arc<AtomicUsize>,
}

impl JobLogSink {
    pub fn new(tx: tokio::sync::mpsc::Sender<JobLogEntry>, dropped: Arc<AtomicUsize>) -> Self {
        Self { tx, dropped }
    }

    pub fn try_send(&self, entry: JobLogEntry) {
        if self.tx.try_send(entry).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped_count(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl ProcessorContext {
    pub fn noop(job_id: impl Into<String>) -> Self {
        let job_id = job_id.into();
        let (log_tx, _) = tokio::sync::mpsc::channel(100);
        let dropped = Arc::new(AtomicUsize::new(0));
        Self {
            job_id: job_id.clone(),
            progress: ProgressReporter::noop(job_id),
            log_sink: JobLogSink::new(log_tx, dropped),
            cancellation_token: CancellationToken::new(),
        }
    }

    pub fn new(
        job_id: impl Into<String>,
        progress: ProgressReporter,
        log_sink: JobLogSink,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            job_id: job_id.into(),
            progress,
            log_sink,
            cancellation_token,
        }
    }

    /// Emit a log entry.
    pub fn log(&self, entry: JobLogEntry) {
        self.log_sink.try_send(entry);
    }

    /// Emit an info log.
    pub fn info(&self, message: impl Into<String>) {
        self.log(JobLogEntry::info(message));
    }

    /// Emit a warning log.
    pub fn warn(&self, message: impl Into<String>) {
        self.log(JobLogEntry::warn(message));
    }

    /// Emit an error log.
    pub fn error(&self, message: impl Into<String>) {
        self.log(JobLogEntry::error(message));
    }
}

/// Output from a processor.
#[derive(Debug, Clone, Default)]
pub struct ProcessorOutput {
    /// Output file paths.
    pub outputs: Vec<String>,
    /// Processing duration in seconds.
    pub duration_secs: f64,
    /// Additional metadata as JSON.
    pub metadata: Option<String>,
    /// Intermediate artifacts created during processing.
    /// Used for observability and cleanup on failure.
    pub items_produced: Vec<String>,
    /// Input file size in bytes (for metrics).
    pub input_size_bytes: Option<u64>,
    /// Output file size in bytes (for metrics).
    pub output_size_bytes: Option<u64>,
    /// Input files that failed processing with their error messages.
    /// Used for partial failure reporting in multi-input jobs.
    /// Each tuple contains (input_path, error_message).
    pub failed_inputs: Vec<(String, String)>,
    /// Input files that were successfully processed.
    /// Used for partial failure reporting in multi-input jobs.
    pub succeeded_inputs: Vec<String>,
    /// Input files that were skipped (passed through) because the processor
    /// doesn't support them. These files are included in outputs for chaining.
    /// Each tuple contains (input_path, reason).
    pub skipped_inputs: Vec<(String, String)>,
    /// Execution logs captured during processing.
    pub logs: Vec<JobLogEntry>,
}

/// Trait for pipeline processors.
#[async_trait]
pub trait Processor: Send + Sync {
    /// Get the processor type.
    fn processor_type(&self) -> ProcessorType;

    /// Get the job types this processor can handle.
    fn job_types(&self) -> Vec<&'static str>;

    /// Check if this processor can handle a job type.
    fn can_process(&self, job_type: &str) -> bool {
        self.job_types().contains(&job_type)
    }

    /// Process the input and produce output.
    ///
    /// # Cancel Safety
    ///
    /// This method MUST be cancel-safe. The worker pool may cancel the future if the job times out
    /// or if the application is shutting down. Implementations should ensure that cancellation
    /// does not leave the system in an inconsistent state (e.g., partial files should be cleaned up).
    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput>;

    /// Get the processor name.
    fn name(&self) -> &'static str;

    /// Indicates if this processor supports multiple inputs in a single job (batch processing).
    ///
    /// When `true`, the processor can handle multiple input files in a single `process()` call.
    /// When `false` (default), the worker pool will split multi-input jobs into separate jobs.
    ///
    fn supports_batch_input(&self) -> bool {
        false
    }

    /// Indicates if this processor can produce multiple outputs from a single input (fan-out).
    ///
    /// When `true`, the processor may produce multiple output files from a single input.
    /// The worker pool will pass all outputs as inputs to the next pipeline step.
    ///
    fn supports_fan_out(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_processor_input() {
        let input = ProcessorInput {
            inputs: vec!["/input.flv".to_string()],
            outputs: vec!["/output.mp4".to_string()],
            config: None,
            streamer_id: "streamer-1".to_string(),
            session_id: "session-1".to_string(),
            streamer_name: Some("Test Streamer".to_string()),
            session_title: Some("Test Title".to_string()),
            platform: None,
            created_at: Utc::now(),
        };

        assert_eq!(input.inputs[0], "/input.flv");
        assert_eq!(input.streamer_name, Some("Test Streamer".to_string()));
    }

    #[test]
    fn test_processor_output() {
        let output = ProcessorOutput {
            outputs: vec!["/output.mp4".to_string()],
            duration_secs: 10.5,
            metadata: Some(r#"{"size": 1024}"#.to_string()),
            items_produced: vec!["/tmp/intermediate.mp4".to_string()],
            input_size_bytes: Some(1024),
            output_size_bytes: Some(512),
            failed_inputs: vec![("/failed.mp4".to_string(), "error message".to_string())],
            succeeded_inputs: vec!["/success.mp4".to_string()],
            skipped_inputs: vec![("/skipped.txt".to_string(), "unsupported format".to_string())],
            logs: vec![],
        };

        assert_eq!(output.duration_secs, 10.5);
        assert_eq!(output.items_produced.len(), 1);
        assert_eq!(output.input_size_bytes, Some(1024));
        assert_eq!(output.output_size_bytes, Some(512));
        assert_eq!(output.failed_inputs.len(), 1);
        assert_eq!(output.failed_inputs[0].0, "/failed.mp4");
        assert_eq!(output.failed_inputs[0].1, "error message");
        assert_eq!(output.succeeded_inputs.len(), 1);
        assert_eq!(output.succeeded_inputs[0], "/success.mp4");
        assert_eq!(output.skipped_inputs.len(), 1);
        assert_eq!(output.skipped_inputs[0].0, "/skipped.txt");
        assert_eq!(output.skipped_inputs[0].1, "unsupported format");
    }

    #[test]
    fn test_processor_output_default() {
        let output = ProcessorOutput::default();
        assert!(output.outputs.is_empty());
        assert_eq!(output.duration_secs, 0.0);
        assert!(output.metadata.is_none());
        assert!(output.items_produced.is_empty());
        assert!(output.input_size_bytes.is_none());
        assert!(output.output_size_bytes.is_none());
        assert!(output.failed_inputs.is_empty());
        assert!(output.succeeded_inputs.is_empty());
        assert!(output.skipped_inputs.is_empty());
    }
}
