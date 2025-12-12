//! Processor trait and related types.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::pipeline::job_queue::JobLogEntry;

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
    /// Requirements: 6.2, 6.3
    pub items_produced: Vec<String>,
    /// Input file size in bytes (for metrics).
    /// Requirements: 6.2, 6.3
    pub input_size_bytes: Option<u64>,
    /// Output file size in bytes (for metrics).
    /// Requirements: 6.2, 6.3
    pub output_size_bytes: Option<u64>,
    /// Input files that failed processing with their error messages.
    /// Used for partial failure reporting in multi-input jobs.
    /// Each tuple contains (input_path, error_message).
    /// Requirements: 11.5
    pub failed_inputs: Vec<(String, String)>,
    /// Input files that were successfully processed.
    /// Used for partial failure reporting in multi-input jobs.
    /// Requirements: 11.5
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
    async fn process(&self, input: &ProcessorInput) -> Result<ProcessorOutput>;

    /// Get the processor name.
    fn name(&self) -> &'static str;

    /// Indicates if this processor supports multiple inputs in a single job (batch processing).
    ///
    /// When `true`, the processor can handle multiple input files in a single `process()` call.
    /// When `false` (default), the worker pool will split multi-input jobs into separate jobs.
    ///
    /// Requirements: 11.3, 11.4
    fn supports_batch_input(&self) -> bool {
        false
    }

    /// Indicates if this processor can produce multiple outputs from a single input (fan-out).
    ///
    /// When `true`, the processor may produce multiple output files from a single input.
    /// The worker pool will pass all outputs as inputs to the next pipeline step.
    ///
    /// Requirements: 11.3, 11.4
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
        };

        assert_eq!(input.inputs[0], "/input.flv");
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
