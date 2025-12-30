//! Telegram upload processor using the `tdl` CLI (https://github.com/iyear/tdl).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{create_log_entry, get_extension, is_image, is_media, run_command_with_logs};
use crate::Result;
use crate::pipeline::job_queue::LogLevel;

fn default_max_retries() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TdlUploadConfig {
    /// Optional override for the `tdl` binary path. If not set, uses the `TDL_PATH`
    /// environment variable, then falls back to `tdl`.
    #[serde(default)]
    pub tdl_path: Option<String>,

    /// Working directory to run `tdl` in.
    ///
    /// This is useful when `tdl` stores its session/config relative to the current directory,
    /// or when you want an explicit, persistent location for its state.
    #[serde(default)]
    pub working_dir: Option<String>,

    /// Extra environment variables to set for `tdl` (e.g., to control where it stores session data).
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Base arguments passed to `tdl` for each upload.
    ///
    /// - If any arg contains `{input}`, it is replaced with the file path.
    /// - Otherwise, the file path is appended as the last argument.
    ///
    /// Additional placeholders:
    /// - `{streamer}` `{title}` `{platform}` (human-readable, may be empty)
    /// - `{streamer_id}` `{session_id}` (always present)
    /// - `{filename}` `{basename}` (from input file path)
    pub args: Vec<String>,

    /// Upload all inputs regardless of file type.
    ///
    /// When `true`, every input path is uploaded unless its extension is excluded via
    /// `excluded_extensions`.
    #[serde(default)]
    pub upload_all: bool,

    /// Optional allowlist of extensions to upload (case-insensitive, without the dot).
    /// When set, only files with an extension in this list will be uploaded.
    ///
    /// If not set, the default filter is "media" (video/audio), with optional `include_images`.
    #[serde(default)]
    pub allowed_extensions: Option<Vec<String>>,

    /// Extensions to skip (case-insensitive, without the dot), even if they match allow rules.
    #[serde(default)]
    pub excluded_extensions: Vec<String>,

    /// Include image files (jpg/png/webp/...) when `allowed_extensions` is not set.
    #[serde(default)]
    pub include_images: bool,

    /// Include files with no extension.
    #[serde(default)]
    pub include_no_extension: bool,

    /// When enabled, do not execute `tdl`; just log what would run.
    #[serde(default)]
    pub dry_run: bool,

    /// Retry count per file when `tdl` fails.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Continue uploading other files if one fails.
    /// When `false` (default), the processor fails the whole job on the first failure.
    #[serde(default)]
    pub continue_on_error: bool,
}

pub struct TdlUploadProcessor;

impl TdlUploadProcessor {
    pub fn new() -> Self {
        Self
    }

    fn normalize_ext_list(list: &[String]) -> Vec<String> {
        list.iter()
            .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn should_upload(cfg: &TdlUploadConfig, file_path: &str) -> (bool, Option<String>) {
        let ext = get_extension(file_path);

        if cfg.upload_all {
            if let Some(ext) = ext {
                let excluded = Self::normalize_ext_list(&cfg.excluded_extensions);
                if excluded.iter().any(|e| e == &ext) {
                    return (
                        false,
                        Some(format!("excluded extension .{} (skipped)", ext)),
                    );
                }
            }
            return (true, None);
        }

        if ext.is_none() {
            if cfg.include_no_extension {
                return (true, None);
            }
            return (false, Some("no extension (skipped)".to_string()));
        }

        let ext = ext.unwrap();
        let excluded = Self::normalize_ext_list(&cfg.excluded_extensions);
        if excluded.iter().any(|e| e == &ext) {
            return (
                false,
                Some(format!("excluded extension .{} (skipped)", ext)),
            );
        }

        if let Some(allowed) = cfg.allowed_extensions.as_ref() {
            let allowed = Self::normalize_ext_list(allowed);
            if allowed.iter().any(|e| e == &ext) {
                return (true, None);
            }
            return (
                false,
                Some(format!("extension .{} not allowed (skipped)", ext)),
            );
        }

        if is_media(&ext) {
            return (true, None);
        }
        if cfg.include_images && is_image(&ext) {
            return (true, None);
        }

        (false, Some("non-media input (skipped)".to_string()))
    }

    fn tdl_path(cfg: &TdlUploadConfig) -> String {
        cfg.tdl_path
            .clone()
            .or_else(|| std::env::var("TDL_PATH").ok())
            .unwrap_or_else(|| "tdl".to_string())
    }

    fn expand_arg(arg: &str, input: &ProcessorInput, file_path: &str) -> String {
        let streamer = input.streamer_name.as_deref().unwrap_or_default();
        let title = input.session_title.as_deref().unwrap_or_default();
        let platform = input.platform.as_deref().unwrap_or_default();

        let (filename, basename) = {
            let p = std::path::Path::new(file_path);
            let filename = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
            let basename = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            (filename, basename)
        };

        arg.replace("{input}", file_path)
            .replace("{streamer}", streamer)
            .replace("{title}", title)
            .replace("{platform}", platform)
            .replace("{streamer_id}", &input.streamer_id)
            .replace("{session_id}", &input.session_id)
            .replace("{filename}", filename)
            .replace("{basename}", basename)
    }

    fn build_args(cfg: &TdlUploadConfig, input: &ProcessorInput, file_path: &str) -> Vec<String> {
        let mut args: Vec<String> = cfg
            .args
            .iter()
            .map(|a| Self::expand_arg(a, input, file_path))
            .collect();

        if !cfg.args.iter().any(|a| a.contains("{input}")) {
            args.push(file_path.to_string());
        }

        args
    }
}

impl Default for TdlUploadProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for TdlUploadProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Io
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["tdl", "telegram"]
    }

    fn name(&self) -> &'static str {
        "TdlUploadProcessor"
    }

    fn supports_batch_input(&self) -> bool {
        true
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        let cfg_str = input.config.as_deref().ok_or_else(|| {
            crate::Error::Validation("No config specified for tdl upload processor".to_string())
        })?;
        let cfg: TdlUploadConfig = serde_json::from_str(cfg_str)
            .map_err(|e| crate::Error::Validation(format!("Invalid tdl config JSON: {}", e)))?;
        if cfg.args.is_empty() {
            return Err(crate::Error::Validation(
                "tdl config must include non-empty 'args'".to_string(),
            ));
        }

        let tdl_path = Self::tdl_path(&cfg);
        let mut logs = Vec::new();

        let mut succeeded_inputs = Vec::new();
        let mut failed_inputs: Vec<(String, String)> = Vec::new();
        let mut skipped_inputs: Vec<(String, String)> = Vec::new();

        let mut output_paths = Vec::with_capacity(input.inputs.len());
        let mut total_uploaded_bytes: u64 = 0;

        for file_path in &input.inputs {
            let (upload, reason) = Self::should_upload(&cfg, file_path);
            if !upload {
                skipped_inputs.push((
                    file_path.clone(),
                    reason.unwrap_or_else(|| "skipped".to_string()),
                ));
                output_paths.push(file_path.clone());
                continue;
            }

            let file_size = tokio::fs::metadata(file_path).await.ok().map(|m| m.len());
            if let Some(size) = file_size {
                total_uploaded_bytes = total_uploaded_bytes.saturating_add(size);
            }

            let args = Self::build_args(&cfg, input, file_path);
            debug!(job_id = %ctx.job_id, tdl_path, ?args, "tdl upload command");

            if cfg.dry_run {
                logs.push(create_log_entry(
                    LogLevel::Info,
                    format!("dry_run: {} {}", tdl_path, args.join(" ")),
                ));
                succeeded_inputs.push(file_path.clone());
                output_paths.push(file_path.clone());
                continue;
            }

            let mut attempt: u32 = 0;
            let max_attempts = cfg.max_retries.saturating_add(1).max(1);

            loop {
                attempt = attempt.saturating_add(1);

                let mut command = Command::new(&tdl_path);
                command.args(&args);
                if let Some(dir) = cfg.working_dir.as_deref()
                    && !dir.trim().is_empty()
                {
                    command.current_dir(dir);
                }
                if !cfg.env.is_empty() {
                    command.envs(cfg.env.iter());
                }
                command.kill_on_drop(true);

                let command_output = run_command_with_logs(&mut command, None).await?;
                logs.extend(command_output.logs);

                if command_output.status.success() {
                    succeeded_inputs.push(file_path.clone());
                    output_paths.push(file_path.clone());
                    break;
                }

                let err_msg = format!(
                    "tdl exited with code {:?} (attempt {}/{})",
                    command_output.status.code(),
                    attempt,
                    max_attempts
                );
                warn!(
                    job_id = %ctx.job_id,
                    file = %file_path,
                    attempt,
                    max_attempts,
                    "{}", err_msg
                );

                if attempt >= max_attempts {
                    failed_inputs.push((file_path.clone(), err_msg.clone()));
                    if !cfg.continue_on_error {
                        return Err(crate::Error::PipelineError(format!(
                            "tdl upload failed for {}: {}",
                            file_path, err_msg
                        )));
                    }
                    // Continue to the next file.
                    output_paths.push(file_path.clone());
                    break;
                }
            }
        }

        let duration = start.elapsed().as_secs_f64();
        info!(
            job_id = %ctx.job_id,
            succeeded = succeeded_inputs.len(),
            failed = failed_inputs.len(),
            skipped = skipped_inputs.len(),
            "tdl upload finished"
        );

        Ok(ProcessorOutput {
            outputs: output_paths,
            duration_secs: duration,
            metadata: None,
            items_produced: vec![],
            input_size_bytes: Some(total_uploaded_bytes),
            output_size_bytes: None,
            failed_inputs,
            succeeded_inputs,
            skipped_inputs,
            logs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_args_appends_input_when_missing_placeholder() {
        let cfg = TdlUploadConfig {
            tdl_path: None,
            working_dir: None,
            env: HashMap::new(),
            args: vec!["upload".to_string(), "--to".to_string(), "@c".to_string()],
            upload_all: false,
            allowed_extensions: None,
            excluded_extensions: vec![],
            include_images: false,
            include_no_extension: false,
            dry_run: true,
            max_retries: 0,
            continue_on_error: false,
        };
        let input = ProcessorInput {
            inputs: vec!["/in.mp4".to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "s1".to_string(),
            session_id: "sess1".to_string(),
            streamer_name: Some("Streamer".to_string()),
            session_title: Some("Title".to_string()),
            platform: Some("X".to_string()),
        };

        let args = TdlUploadProcessor::build_args(&cfg, &input, "/in.mp4");
        assert_eq!(args.last().unwrap(), "/in.mp4");
    }

    #[test]
    fn test_expand_arg_placeholders() {
        let input = ProcessorInput {
            inputs: vec!["/dir/file.mp4".to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "sid".to_string(),
            session_id: "sess".to_string(),
            streamer_name: Some("Alice".to_string()),
            session_title: Some("Hello".to_string()),
            platform: Some("Twitch".to_string()),
        };

        let out = TdlUploadProcessor::expand_arg(
            "{streamer}:{title}:{platform}:{streamer_id}:{session_id}:{filename}:{basename}:{input}",
            &input,
            "/dir/file.mp4",
        );
        assert!(out.contains("Alice:Hello:Twitch:sid:sess:file.mp4:file:/dir/file.mp4"));
    }

    #[test]
    fn test_should_upload_allowlist_overrides_default() {
        let cfg = TdlUploadConfig {
            tdl_path: None,
            working_dir: None,
            env: HashMap::new(),
            args: vec!["upload".to_string()],
            upload_all: false,
            allowed_extensions: Some(vec!["json".to_string()]),
            excluded_extensions: vec![],
            include_images: false,
            include_no_extension: false,
            dry_run: true,
            max_retries: 0,
            continue_on_error: false,
        };

        assert!(TdlUploadProcessor::should_upload(&cfg, "/a/b/c.json").0);
        assert!(!TdlUploadProcessor::should_upload(&cfg, "/a/b/c.mp4").0);
    }

    #[test]
    fn test_should_upload_upload_all() {
        let cfg = TdlUploadConfig {
            tdl_path: None,
            working_dir: None,
            env: HashMap::new(),
            args: vec!["upload".to_string()],
            upload_all: true,
            allowed_extensions: None,
            excluded_extensions: vec!["tmp".to_string()],
            include_images: false,
            include_no_extension: false,
            dry_run: true,
            max_retries: 0,
            continue_on_error: false,
        };

        assert!(TdlUploadProcessor::should_upload(&cfg, "/a/b/c.weird").0);
        assert!(TdlUploadProcessor::should_upload(&cfg, "/a/b/c").0); // no extension
        assert!(!TdlUploadProcessor::should_upload(&cfg, "/a/b/c.tmp").0);
    }
}
