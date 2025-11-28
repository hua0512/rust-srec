//! Execute command processor for running arbitrary shell commands.

use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use super::traits::{Processor, ProcessorInput, ProcessorOutput, ProcessorType};
use crate::Result;

/// Processor for executing arbitrary shell commands.
pub struct ExecuteCommandProcessor {
    /// Command timeout in seconds.
    timeout_secs: u64,
}

impl ExecuteCommandProcessor {
    /// Create a new execute command processor.
    pub fn new() -> Self {
        Self {
            timeout_secs: 3600, // 1 hour default
        }
    }

    /// Set the command timeout.
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Substitute variables in a command string.
    fn substitute_variables(command: &str, input: &ProcessorInput) -> String {
        command
            .replace("{input}", &input.input_path)
            .replace("{output}", &input.output_path)
            .replace("{streamer_id}", &input.streamer_id)
            .replace("{session_id}", &input.session_id)
    }
}

impl Default for ExecuteCommandProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for ExecuteCommandProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["execute", "command"]
    }

    fn name(&self) -> &'static str {
        "ExecuteCommandProcessor"
    }

    async fn process(&self, input: &ProcessorInput) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();
        
        // Get command from config
        let command = input.config.as_ref()
            .ok_or_else(|| crate::Error::Other("No command specified in config".to_string()))?;

        let command = Self::substitute_variables(command, input);
        
        info!("Executing command: {}", command);

        // Build command
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.args(["/C", &command]);
            c
        };

        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", &command]);
            c
        };

        cmd.stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let mut child = cmd.spawn()
            .map_err(|e| crate::Error::Other(format!("Failed to spawn command: {}", e)))?;

        // Read stdout and stderr
        let stdout_handle = if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            Some(tokio::spawn(async move {
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("stdout: {}", line);
                }
            }))
        } else {
            None
        };

        let stderr_handle = if let Some(stderr) = child.stderr.take() {
            let reader = BufReader::new(stderr);
            Some(tokio::spawn(async move {
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.contains("error") || line.contains("Error") {
                        warn!("stderr: {}", line);
                    } else {
                        debug!("stderr: {}", line);
                    }
                }
            }))
        } else {
            None
        };

        // Wait with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            child.wait(),
        ).await;

        // Wait for output readers
        if let Some(h) = stdout_handle {
            let _ = h.await;
        }
        if let Some(h) = stderr_handle {
            let _ = h.await;
        }

        let status = match result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return Err(crate::Error::Other(format!("Failed to wait for command: {}", e)));
            }
            Err(_) => {
                error!("Command timed out after {}s", self.timeout_secs);
                let _ = child.kill().await;
                return Err(crate::Error::Other("Command timed out".to_string()));
            }
        };

        if !status.success() {
            error!("Command exited with status: {}", status);
            return Err(crate::Error::Other(format!(
                "Command failed with exit code: {}",
                status.code().unwrap_or(-1)
            )));
        }

        let duration = start.elapsed().as_secs_f64();
        
        info!("Command completed in {:.2}s", duration);

        Ok(ProcessorOutput {
            output_path: input.output_path.clone(),
            duration_secs: duration,
            metadata: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_processor_type() {
        let processor = ExecuteCommandProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Cpu);
    }

    #[test]
    fn test_execute_processor_job_types() {
        let processor = ExecuteCommandProcessor::new();
        assert!(processor.can_process("execute"));
        assert!(processor.can_process("command"));
        assert!(!processor.can_process("upload"));
    }

    #[test]
    fn test_variable_substitution() {
        let input = ProcessorInput {
            input_path: "/input.flv".to_string(),
            output_path: "/output.mp4".to_string(),
            config: None,
            streamer_id: "streamer-1".to_string(),
            session_id: "session-1".to_string(),
        };

        let command = "echo {input} {output} {streamer_id}";
        let result = ExecuteCommandProcessor::substitute_variables(command, &input);
        
        assert_eq!(result, "echo /input.flv /output.mp4 streamer-1");
    }

    #[test]
    fn test_execute_processor_name() {
        let processor = ExecuteCommandProcessor::new();
        assert_eq!(processor.name(), "ExecuteCommandProcessor");
    }
}
