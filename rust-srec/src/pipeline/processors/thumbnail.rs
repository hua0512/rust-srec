//! Thumbnail processor for extracting video thumbnails.

use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, error, info};

use super::traits::{Processor, ProcessorInput, ProcessorOutput, ProcessorType};
use crate::Result;

/// Configuration for thumbnail extraction.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ThumbnailConfig {
    /// Timestamp to extract thumbnail from (in seconds).
    #[serde(default = "default_timestamp")]
    pub timestamp_secs: f64,
    /// Output width (height auto-calculated to maintain aspect ratio).
    #[serde(default = "default_width")]
    pub width: u32,
    /// Output quality (1-31, lower is better).
    #[serde(default = "default_quality")]
    pub quality: u32,
}

fn default_timestamp() -> f64 {
    10.0
}

fn default_width() -> u32 {
    320
}

fn default_quality() -> u32 {
    2
}

impl Default for ThumbnailConfig {
    fn default() -> Self {
        Self {
            timestamp_secs: default_timestamp(),
            width: default_width(),
            quality: default_quality(),
        }
    }
}

/// Processor for extracting thumbnails from video files.
pub struct ThumbnailProcessor {
    /// Path to ffmpeg binary.
    ffmpeg_path: String,
}

impl ThumbnailProcessor {
    /// Create a new thumbnail processor.
    pub fn new() -> Self {
        Self {
            ffmpeg_path: std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string()),
        }
    }

    /// Create with a custom ffmpeg path.
    pub fn with_ffmpeg_path(path: impl Into<String>) -> Self {
        Self {
            ffmpeg_path: path.into(),
        }
    }
}

impl Default for ThumbnailProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for ThumbnailProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["thumbnail"]
    }

    fn name(&self) -> &'static str {
        "ThumbnailProcessor"
    }

    async fn process(&self, input: &ProcessorInput) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();
        
        // Parse config or use defaults
        let config: ThumbnailConfig = if let Some(ref config_str) = input.config {
            serde_json::from_str(config_str)
                .unwrap_or_default()
        } else {
            ThumbnailConfig::default()
        };

        info!(
            "Extracting thumbnail from {} at {:.2}s",
            input.input_path, config.timestamp_secs
        );

        // Build ffmpeg command
        let mut cmd = Command::new(&self.ffmpeg_path);
        cmd.args([
            "-y",
            "-hide_banner",
            "-ss", &format!("{:.2}", config.timestamp_secs),
            "-i", &input.input_path,
            "-vframes", "1",
            "-vf", &format!("scale={}:-1", config.width),
            "-q:v", &config.quality.to_string(),
            &input.output_path,
        ])
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        let child = cmd.spawn()
            .map_err(|e| crate::Error::Other(format!("Failed to spawn ffmpeg: {}", e)))?;

        let output = child.wait_with_output().await
            .map_err(|e| crate::Error::Other(format!("Failed to wait for ffmpeg: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("ffmpeg failed: {}", stderr);
            return Err(crate::Error::Other(format!(
                "ffmpeg failed with exit code: {}",
                output.status.code().unwrap_or(-1)
            )));
        }

        let duration = start.elapsed().as_secs_f64();
        
        debug!("ffmpeg output: {}", String::from_utf8_lossy(&output.stderr));
        
        info!(
            "Thumbnail extracted in {:.2}s: {}",
            duration, input.output_path
        );

        Ok(ProcessorOutput {
            output_path: input.output_path.clone(),
            duration_secs: duration,
            metadata: Some(serde_json::json!({
                "timestamp_secs": config.timestamp_secs,
                "width": config.width,
            }).to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thumbnail_processor_type() {
        let processor = ThumbnailProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Cpu);
    }

    #[test]
    fn test_thumbnail_processor_job_types() {
        let processor = ThumbnailProcessor::new();
        assert!(processor.can_process("thumbnail"));
        assert!(!processor.can_process("upload"));
    }

    #[test]
    fn test_thumbnail_config_default() {
        let config = ThumbnailConfig::default();
        assert_eq!(config.timestamp_secs, 10.0);
        assert_eq!(config.width, 320);
        assert_eq!(config.quality, 2);
    }

    #[test]
    fn test_thumbnail_config_parse() {
        let json = r#"{"timestamp_secs": 30.0, "width": 640}"#;
        let config: ThumbnailConfig = serde_json::from_str(json).unwrap();
        
        assert_eq!(config.timestamp_secs, 30.0);
        assert_eq!(config.width, 640);
        assert_eq!(config.quality, 2); // default
    }

    #[test]
    fn test_thumbnail_processor_name() {
        let processor = ThumbnailProcessor::new();
        assert_eq!(processor.name(), "ThumbnailProcessor");
    }
}
