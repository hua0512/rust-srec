//! Execute command processor for running arbitrary shell commands.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use tokio::process::Command;
use tracing::debug;

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use crate::Result;
use crate::utils::filename::expand_placeholders;

/// Configuration for execute command processor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteConfig {
    /// The command to execute. Supports variable substitution:
    /// - `{input}` - first input file path
    /// - `{input0}`, `{input1}`, ... - Nth input file path
    /// - `{inputs_json}` - JSON array of all inputs
    /// - `{output}` - first output file path
    /// - `{output0}`, `{output1}`, ... - Nth output file path
    /// - `{outputs_json}` - JSON array of all outputs
    /// - `{streamer_id}` - streamer ID
    /// - `{session_id}` - session ID
    ///
    /// Placeholders path templates:
    /// - `{streamer}` - sanitized streamer name (falls back to streamer_id)
    /// - `{title}` - sanitized session title (falls back to empty)
    /// - `{platform}` - platform name (falls back to empty)
    /// - time placeholders like `%Y`, `%m`, `%d`, `%H`, `%M`, `%S`, `%t`, and `%%`
    pub command: String,

    /// Directory to scan for new files after command execution.
    /// If specified, the processor will detect files created during execution
    /// and include them in the outputs for pipeline chaining.
    #[serde(default)]
    pub scan_output_dir: Option<String>,

    /// File extension filter for scanning (e.g., "mp4", "mkv").
    /// Only files with this extension will be included in outputs.
    /// If not specified, all new files are included.
    #[serde(default)]
    pub scan_extension: Option<String>,
}

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
        let command = expand_placeholders(
            command,
            &input.streamer_id,
            &input.session_id,
            input.streamer_name.as_deref(),
            input.session_title.as_deref(),
            input.platform.as_deref(),
        );

        let input_path = input.inputs.first().map(|s| s.as_str()).unwrap_or("");
        let output_path = input.outputs.first().map(|s| s.as_str()).unwrap_or("");

        let inputs_json = serde_json::to_string(&input.inputs).unwrap_or_else(|_| "[]".to_string());
        let outputs_json =
            serde_json::to_string(&input.outputs).unwrap_or_else(|_| "[]".to_string());

        let mut expanded = command
            .replace("{input}", input_path)
            .replace("{output}", output_path)
            .replace("{inputs_json}", &inputs_json)
            .replace("{outputs_json}", &outputs_json)
            .replace("{streamer_id}", &input.streamer_id)
            .replace("{session_id}", &input.session_id);

        for (i, v) in input.inputs.iter().enumerate() {
            expanded = expanded.replace(&format!("{{input{i}}}"), v);
        }
        for (i, v) in input.outputs.iter().enumerate() {
            expanded = expanded.replace(&format!("{{output{i}}}"), v);
        }

        expanded
    }

    fn parse_config(input: &ProcessorInput) -> Result<ExecuteConfig> {
        let Some(config_str) = input.config.as_ref() else {
            return Err(crate::Error::Other(
                "No config specified for execute processor".to_string(),
            ));
        };

        let trimmed = config_str.trim_start();
        let looks_like_json = matches!(
            trimmed.as_bytes().first(),
            Some(b'{') | Some(b'[') | Some(b'"')
        );

        if !looks_like_json {
            return Ok(ExecuteConfig {
                command: config_str.clone(),
                scan_output_dir: None,
                scan_extension: None,
            });
        }

        let value: serde_json::Value = serde_json::from_str(config_str).map_err(|e| {
            crate::Error::Other(format!(
                "Invalid JSON for execute processor config: {e}. If you intended a raw command, \
                 pass it as a plain string (not starting with '{{', '[', or '\"') or as \
                 {{\"command\":\"...\"}}."
            ))
        })?;

        match value {
            serde_json::Value::Object(_) => serde_json::from_value(value).map_err(|e| {
                crate::Error::Other(format!(
                    "Invalid execute processor config object (expected {{\"command\": \"...\"}}): {e}"
                ))
            }),
            serde_json::Value::String(command) => Ok(ExecuteConfig {
                command,
                scan_output_dir: None,
                scan_extension: None,
            }),
            _ => Err(crate::Error::Other(
                "Execute processor config must be a JSON object or JSON string".to_string(),
            )),
        }
    }

    /// Scan a directory and return all file paths.
    async fn scan_directory(dir: &Path, extension_filter: Option<&str>) -> Vec<String> {
        let mut files = Vec::new();

        if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let is_file = entry
                    .file_type()
                    .await
                    .map(|t| t.is_file())
                    .unwrap_or(false);
                if is_file {
                    // Apply extension filter if specified
                    if let Some(ext_filter) = extension_filter {
                        if let Some(ext) = path.extension().and_then(|e| e.to_str())
                            && ext.eq_ignore_ascii_case(ext_filter)
                        {
                            files.push(path.to_string_lossy().to_string());
                        }
                    } else {
                        files.push(path.to_string_lossy().to_string());
                    }
                }
            }
        }

        files
    }

    /// Detect new files created in a directory by comparing before/after snapshots.
    async fn detect_new_files(
        before: &HashSet<String>,
        dir: &Path,
        extension_filter: Option<&str>,
    ) -> Vec<String> {
        let after: HashSet<String> = Self::scan_directory(dir, extension_filter)
            .await
            .into_iter()
            .collect();

        // Find files that exist now but didn't exist before
        let mut new_files: Vec<String> = after.difference(before).cloned().collect();
        new_files.sort();
        new_files
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

    fn supports_batch_input(&self) -> bool {
        // Execute is an arbitrary command runner and can consume many inputs in a single job.
        // This is important for session/paired pipelines where inputs are provided as a list.
        true
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        // Parse config - support JSON config, JSON string, and raw command string.
        // JSON object: {"command": "...", "scan_output_dir": "...", ...}
        // JSON string: "echo hello"
        // Raw string: echo hello
        let config = Self::parse_config(input)?;

        let command = Self::substitute_variables(&config.command, input);

        let _ = ctx.info(format!("Executing command: {}", command));

        // Take snapshot of output directory before execution (if scanning enabled)
        let before_snapshot: Option<HashSet<String>> = if let Some(ref dir) = config.scan_output_dir
        {
            let dir_path = Path::new(dir);
            let is_dir = tokio::fs::metadata(dir_path)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false);

            if !is_dir {
                // Create directory if it doesn't exist (or isn't a directory yet)
                if let Err(e) =
                    crate::utils::fs::ensure_dir_all_with_op("creating output directory", dir_path)
                        .await
                {
                    let _ = ctx.warn(format!("Failed to create output directory {}: {}", dir, e));
                }
            }

            Some(
                Self::scan_directory(dir_path, config.scan_extension.as_deref())
                    .await
                    .into_iter()
                    .collect(),
            )
        } else {
            None
        };

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

        // Ensure the child process is terminated if the job times out or the task is cancelled.
        cmd.kill_on_drop(true);

        // Execute command and capture logs (with timeout)
        let command_output_result = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            crate::pipeline::processors::utils::run_command_with_logs(
                &mut cmd,
                Some(ctx.log_sink.clone()),
            ),
        )
        .await;

        let command_output = match command_output_result {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                let _ = ctx.error(format!("Command timed out after {}s", self.timeout_secs));
                // Child process cleanup depends on implementation details of utils::run_command_with_logs
                // ideally that helper should handle cancellation/timeout cleanups if possible.
                // For now, we return timeout error.
                return Err(crate::Error::Other("Command timed out".to_string()));
            }
        };

        if !command_output.status.success() {
            // Find last error log
            let error_msg = command_output
                .logs
                .iter()
                .rfind(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                .map(|l| l.message.clone())
                .unwrap_or_else(|| "Command failed".to_string());

            let _ = ctx.error(format!(
                "Command failed with status: {}",
                command_output.status
            ));
            return Err(crate::Error::Other(format!(
                "Command failed with exit code: {} - {}",
                command_output.status.code().unwrap_or(-1),
                error_msg
            )));
        }

        let duration = command_output.duration;

        let _ = ctx.info(format!("Command completed in {:.2}s", duration));

        // Get file sizes for metrics if paths exist
        let input_path = input.inputs.first().map(|s| s.as_str()).unwrap_or("");
        let output_path = input.outputs.first().map(|s| s.as_str()).unwrap_or("");

        let input_size_bytes = if !input_path.is_empty() {
            tokio::fs::metadata(input_path).await.ok().map(|m| m.len())
        } else {
            None
        };
        let output_size_bytes = if !output_path.is_empty() {
            tokio::fs::metadata(output_path).await.ok().map(|m| m.len())
        } else {
            None
        };

        // Determine outputs for pipeline chaining
        // Priority:
        // 1. Scan output directory for new files (if configured)
        // 2. Use explicit outputs (if provided)
        // 3. Pass through inputs (fallback for chaining)
        let mut items_produced = Vec::new();
        let outputs = if let (Some(dir), Some(before)) = (&config.scan_output_dir, &before_snapshot)
        {
            let new_files =
                Self::detect_new_files(before, Path::new(dir), config.scan_extension.as_deref())
                    .await;

            if new_files.is_empty() {
                debug!(
                    "No new files detected in {}, falling back to explicit outputs or input passthrough",
                    dir
                );

                if !input.outputs.is_empty() {
                    items_produced = input.outputs.clone();
                    input.outputs.clone()
                } else {
                    input.inputs.clone()
                }
            } else {
                let _ = ctx.info(format!(
                    "Detected {} new files in output directory",
                    new_files.len()
                ));
                for file in &new_files {
                    debug!("  - {}", file);
                }
                items_produced = new_files.clone();
                new_files
            }
        } else if !input.outputs.is_empty() {
            // Use explicit outputs if provided.
            items_produced = input.outputs.clone();
            input.outputs.clone()
        } else {
            // Pass through inputs for chaining.
            input.inputs.clone()
        };

        Ok(ProcessorOutput {
            outputs,
            duration_secs: duration,
            metadata: Some(
                serde_json::json!({
                    "command": command,
                    "scan_output_dir": config.scan_output_dir,
                    "scan_extension": config.scan_extension,
                })
                .to_string(),
            ),
            items_produced,
            input_size_bytes,
            output_size_bytes,
            failed_inputs: vec![],
            succeeded_inputs: if input_path.is_empty() {
                vec![]
            } else {
                vec![input_path.to_string()]
            },
            skipped_inputs: vec![],
            logs: command_output.logs,
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
            inputs: vec!["/input.flv".to_string()],
            outputs: vec!["/output.mp4".to_string()],
            config: None,
            streamer_id: "streamer-1".to_string(),
            session_id: "session-1".to_string(),
            ..Default::default()
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

    /// Test that outputs pass through inputs when outputs is empty.
    /// This is critical for pipeline chaining where the next job
    /// receives outputs from the previous job as its inputs.
    #[tokio::test]
    async fn test_output_passthrough_for_chaining() {
        let processor = ExecuteCommandProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let config = serde_json::json!({
            "command": "echo test"
        });

        // Simulate a chained job where outputs is empty (as set by complete_with_next)
        let input = ProcessorInput {
            inputs: vec!["/path/to/video.mp4".to_string()],
            outputs: vec![], // Empty, as would be set by pipeline chaining
            config: Some(config.to_string()),
            streamer_id: "streamer-1".to_string(),
            session_id: "session-1".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await.unwrap();

        // Outputs should contain the inputs for proper chaining
        assert_eq!(result.outputs, vec!["/path/to/video.mp4".to_string()]);
    }

    /// Test that explicit outputs are preserved when provided.
    #[tokio::test]
    async fn test_explicit_outputs_preserved() {
        let processor = ExecuteCommandProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let config = serde_json::json!({
            "command": "echo test"
        });

        let input = ProcessorInput {
            inputs: vec!["/input.flv".to_string()],
            outputs: vec!["/output.mp4".to_string()],
            config: Some(config.to_string()),
            streamer_id: "streamer-1".to_string(),
            session_id: "session-1".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await.unwrap();

        // Explicit outputs should be preserved
        assert_eq!(result.outputs, vec!["/output.mp4".to_string()]);
    }

    #[test]
    fn test_execute_config_parse() {
        let json = r#"{
            "command": "ffmpeg -i {input} {output}",
            "scan_output_dir": "/output/dir",
            "scan_extension": "mp4"
        }"#;

        let config: ExecuteConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.command, "ffmpeg -i {input} {output}");
        assert_eq!(config.scan_output_dir, Some("/output/dir".to_string()));
        assert_eq!(config.scan_extension, Some("mp4".to_string()));
    }

    #[test]
    fn test_execute_config_minimal() {
        let json = r#"{"command": "echo hello"}"#;

        let config: ExecuteConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.command, "echo hello");
        assert!(config.scan_output_dir.is_none());
        assert!(config.scan_extension.is_none());
    }

    /// Test scan_directory helper function.
    #[tokio::test]
    async fn test_scan_directory_helper() {
        use tempfile::TempDir;
        use tokio::fs;

        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        // Create some test files
        fs::write(dir.join("video.mp4"), "test").await.unwrap();
        fs::write(dir.join("audio.mp3"), "test").await.unwrap();
        fs::write(dir.join("log.txt"), "test").await.unwrap();

        // Scan all files
        let all_files = ExecuteCommandProcessor::scan_directory(dir, None).await;
        assert_eq!(all_files.len(), 3);

        // Scan only .mp4 files
        let mp4_files = ExecuteCommandProcessor::scan_directory(dir, Some("mp4")).await;
        assert_eq!(mp4_files.len(), 1);
        assert!(mp4_files[0].contains("video.mp4"));

        // Scan only .txt files
        let txt_files = ExecuteCommandProcessor::scan_directory(dir, Some("txt")).await;
        assert_eq!(txt_files.len(), 1);
        assert!(txt_files[0].contains("log.txt"));
    }

    /// Test detect_new_files helper function.
    #[tokio::test]
    async fn test_detect_new_files_helper() {
        use tempfile::TempDir;
        use tokio::fs;

        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        // Create initial file
        fs::write(dir.join("existing.mp4"), "test").await.unwrap();

        // Take snapshot
        let before: HashSet<String> = ExecuteCommandProcessor::scan_directory(dir, None)
            .await
            .into_iter()
            .collect();

        // Create new files
        fs::write(dir.join("new1.mp4"), "test").await.unwrap();
        fs::write(dir.join("new2.txt"), "test").await.unwrap();

        // Detect new files (all)
        let new_files = ExecuteCommandProcessor::detect_new_files(&before, dir, None).await;
        assert_eq!(new_files.len(), 2);

        // Detect new files (only .mp4)
        let new_mp4 = ExecuteCommandProcessor::detect_new_files(&before, dir, Some("mp4")).await;
        assert_eq!(new_mp4.len(), 1);
        assert!(new_mp4[0].contains("new1.mp4"));
    }

    /// Test output directory scanning integration.
    #[tokio::test]
    async fn test_scan_output_directory_integration() {
        use tempfile::TempDir;
        use tokio::fs;

        let temp_dir = TempDir::new().unwrap();
        let output_dir = temp_dir.path().join("output");
        fs::create_dir_all(&output_dir).await.unwrap();

        let processor = ExecuteCommandProcessor::new();
        let ctx = ProcessorContext::noop("test");

        // Use echo which always succeeds
        let config = serde_json::json!({
            "command": "echo scanning test",
            "scan_output_dir": output_dir.to_string_lossy(),
        });

        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec![],
            config: Some(config.to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        // Simulate: create a file after taking snapshot but before checking
        // (In real usage, the command would create the file)
        // Since no files are created, it should fall back to input passthrough
        let result = processor.process(&input, &ctx).await.unwrap();

        // Should fall back to inputs when no new files detected
        assert_eq!(result.outputs, vec!["/input.mp4".to_string()]);
    }

    /// Test scan output directory fallback prefers explicit outputs over input passthrough.
    #[tokio::test]
    async fn test_scan_output_directory_fallback_prefers_explicit_outputs() {
        use tempfile::TempDir;
        use tokio::fs;

        let temp_dir = TempDir::new().unwrap();
        let output_dir = temp_dir.path().join("output");
        fs::create_dir_all(&output_dir).await.unwrap();

        let processor = ExecuteCommandProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let config = serde_json::json!({
            "command": "echo scanning test",
            "scan_output_dir": output_dir.to_string_lossy(),
        });

        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec!["/explicit.mp4".to_string()],
            config: Some(config.to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await.unwrap();
        assert_eq!(result.outputs, vec!["/explicit.mp4".to_string()]);
    }

    /// Test raw command string (for dynamic job creation).
    #[tokio::test]
    async fn test_raw_command_string() {
        let processor = ExecuteCommandProcessor::new();
        let ctx = ProcessorContext::noop("test");

        // Raw command string (not JSON) - for dynamic job creation
        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec![],
            config: Some("echo hello world".to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await.unwrap();

        // Should work and pass through inputs
        assert_eq!(result.outputs, vec!["/input.mp4".to_string()]);
    }

    /// Test JSON string config (for programmatic callers that always send JSON).
    #[tokio::test]
    async fn test_json_string_config() {
        let processor = ExecuteCommandProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec![],
            config: Some("\"echo hello world\"".to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await.unwrap();
        assert_eq!(result.outputs, vec!["/input.mp4".to_string()]);
    }

    #[test]
    fn test_substitute_variables_multiple_inputs() {
        let input = ProcessorInput {
            inputs: vec!["/in0.mp4".to_string(), "/in1.json".to_string()],
            outputs: vec!["/out0.mp4".to_string(), "/out1.json".to_string()],
            streamer_id: "s".to_string(),
            session_id: "sess".to_string(),
            ..Default::default()
        };

        let cmd = "echo {input} {input0} {input1} {output} {output1} {inputs_json} {outputs_json} {streamer_id} {session_id}";
        let out = ExecuteCommandProcessor::substitute_variables(cmd, &input);

        assert!(out.contains("/in0.mp4"));
        assert!(out.contains("/in1.json"));
        assert!(out.contains("/out0.mp4"));
        assert!(out.contains("/out1.json"));
        assert!(out.contains("[\"/in0.mp4\",\"/in1.json\"]"));
        assert!(out.contains("[\"/out0.mp4\",\"/out1.json\"]"));
        assert!(out.contains(" s "));
        assert!(out.contains(" sess"));
    }

    #[test]
    fn test_substitute_variables_rclone_style_placeholders() {
        let input = ProcessorInput {
            inputs: vec!["/in0.mp4".to_string()],
            outputs: vec![],
            streamer_id: "streamer-123".to_string(),
            session_id: "session-456".to_string(),
            streamer_name: Some("Streamer<Name>".to_string()),
            session_title: Some("Title:With:Colons".to_string()),
            platform: Some("Twitch".to_string()),
            config: None,
            ..Default::default()
        };

        let cmd = "echo {platform} {streamer} {title} {streamer_id} {session_id}";
        let out = ExecuteCommandProcessor::substitute_variables(cmd, &input);

        assert!(out.contains("Twitch"));
        assert!(out.contains("Streamer_Name_"));
        assert!(out.contains("Title_With_Colons"));
        assert!(out.contains("streamer-123"));
        assert!(out.contains("session-456"));
    }

    #[test]
    fn test_parse_config_rejects_invalid_json_object() {
        let input = ProcessorInput {
            inputs: vec![],
            outputs: vec![],
            config: Some(r#"{"command": 123}"#.to_string()),
            streamer_id: "s".to_string(),
            session_id: "sess".to_string(),
            ..Default::default()
        };

        let err = ExecuteCommandProcessor::parse_config(&input).unwrap_err();
        assert!(
            err.to_string()
                .contains("Invalid execute processor config object")
        );
    }

    /// Test missing config returns an error.
    #[tokio::test]
    async fn test_missing_config_error() {
        let processor = ExecuteCommandProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No config specified")
        );
    }
}
