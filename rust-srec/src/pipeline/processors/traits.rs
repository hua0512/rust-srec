//! Processor trait and related types.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;

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
    /// Input file path.
    pub input_path: String,
    /// Output file path.
    pub output_path: String,
    /// Additional configuration as JSON.
    pub config: Option<String>,
    /// Streamer ID.
    pub streamer_id: String,
    /// Session ID.
    pub session_id: String,
}

/// Output from a processor.
#[derive(Debug, Clone)]
pub struct ProcessorOutput {
    /// Output file path or result.
    pub output_path: String,
    /// Processing duration in seconds.
    pub duration_secs: f64,
    /// Additional metadata as JSON.
    pub metadata: Option<String>,
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
    async fn process(&self, input: &ProcessorInput) -> Result<ProcessorOutput>;

    /// Get the processor name.
    fn name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_processor_input() {
        let input = ProcessorInput {
            input_path: "/input.flv".to_string(),
            output_path: "/output.mp4".to_string(),
            config: None,
            streamer_id: "streamer-1".to_string(),
            session_id: "session-1".to_string(),
        };

        assert_eq!(input.input_path, "/input.flv");
        assert_eq!(input.output_path, "/output.mp4");
    }

    #[test]
    fn test_processor_output() {
        let output = ProcessorOutput {
            output_path: "/output.mp4".to_string(),
            duration_secs: 10.5,
            metadata: Some(r#"{"size": 1024}"#.to_string()),
        };

        assert_eq!(output.duration_secs, 10.5);
    }
}
