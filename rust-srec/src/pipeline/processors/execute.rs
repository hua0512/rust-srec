//! Execute command processor for running arbitrary shell commands.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use tokio::process::Command;
use tracing::debug;

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use crate::Result;
use crate::utils::filename::sanitize_filename;

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
    ///
    /// Substituted values are quoted for the shell by `substitute_variables`,
    /// so a path or title containing spaces, quotes or `$` stays one literal
    /// word; the command may still use pipes, `&&` and redirects itself.
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

/// Shell that will interpret the command built by
/// `ExecuteCommandProcessor::substitute_variables`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    /// `sh -c <command>`.
    Posix,
    /// `cmd /C <command>`.
    Cmd,
}

/// Shell that `Processor::process` spawns; keeps the escaping applied by
/// `substitute_variables` in step with the interpreter that receives it.
const SHELL: ShellKind = if cfg!(windows) {
    ShellKind::Cmd
} else {
    ShellKind::Posix
};

/// Quoting context of a command template at a given byte offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteState {
    Unquoted,
    Single,
    Double,
}

/// A region the scan has entered and not yet left, carrying the `QuoteState`
/// to restore when it closes.
#[derive(Debug, Clone, Copy)]
enum Region {
    /// `$( ... )`: the body is parsed as a fresh command, so the quoting that
    /// surrounds the region does not reach into it.
    Subshell(QuoteState),
    /// `` `...` ``: as `Subshell`, and the shell additionally strips one layer
    /// of `\` escapes from the body before parsing it.
    Backtick(QuoteState),
}

/// Escape `value` so a POSIX shell reads it as literal text at a point where
/// the surrounding template is in `state`, nested inside `backtick_depth`
/// enclosing `` `...` `` regions.
fn escape_posix(value: &str, state: QuoteState, backtick_depth: usize) -> String {
    let mut escaped = match state {
        // Wrap the whole value so spaces and every metacharacter lose meaning.
        QuoteState::Unquoted => {
            let mut out = String::with_capacity(value.len() + 2);
            out.push('\'');
            for c in value.chars() {
                if c == '\'' {
                    out.push_str(r"'\''");
                } else {
                    out.push(c);
                }
            }
            out.push('\'');
            out
        }
        // Already inside `'...'`: close, emit an escaped quote, reopen, so the
        // template's own closing quote still lands where it was written.
        QuoteState::Single => value.replace('\'', r"'\''"),
        // Inside `"..."` only these four keep a special meaning.
        QuoteState::Double => {
            let mut out = String::with_capacity(value.len());
            for c in value.chars() {
                if matches!(c, '\\' | '"' | '$' | '`') {
                    out.push('\\');
                }
                out.push(c);
            }
            out
        }
    };

    // A `` `...` `` region ends at the first backtick the shell finds, whatever
    // quoting sits between, and one layer of `\` escapes is removed from the
    // body before it is parsed. Re-escape once per enclosing region so the word
    // built above arrives intact.
    for _ in 0..backtick_depth {
        let mut layered = String::with_capacity(escaped.len());
        for c in escaped.chars() {
            if matches!(c, '\\' | '`') {
                layered.push('\\');
            }
            layered.push(c);
        }
        escaped = layered;
    }

    escaped
}

/// Escape `value` for `cmd /C` at a point where the template is in `state`.
///
/// A `"` is doubled: cmd keeps reading the text as quoted, so `&`, `|`, `<` and
/// `>` stay inert, and the MS C runtime hands the child one literal quote.
/// Unlike `escape_posix` this is not airtight — cmd expands `%VAR%` (and
/// `!VAR!` where delayed expansion is on) before the command runs and offers no
/// escape for `%` on a `/C` command line, so a value naming a defined variable
/// still expands.
fn escape_cmd(value: &str, state: QuoteState) -> String {
    let escaped = value.replace('"', "\"\"");

    match state {
        // Quote so `&`, `|`, `<`, `>` and spaces stay inside the argument.
        QuoteState::Unquoted => format!("\"{escaped}\""),
        QuoteState::Single | QuoteState::Double => escaped,
    }
}

/// Replace every `{name}` in `template` that `resolve` recognises with its
/// value, escaped for the context the template is in at that occurrence.
///
/// The template is scanned once, left to right, so a value containing a quote
/// character cannot change the context seen by a later placeholder. Quote
/// characters written by the template are preserved verbatim, so a preset that
/// already writes `"{input}"` stays one double-quoted word instead of gaining
/// a second layer of quoting. On `ShellKind::Posix` the scan also follows
/// `$( ... )` and `` `...` ``, whose bodies the shell re-parses as commands in
/// their own right. Names `resolve` returns `None` for are left as written.
fn substitute_placeholders<'a>(
    template: &str,
    shell: ShellKind,
    resolve: impl Fn(&str) -> Option<&'a str>,
) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let posix = shell == ShellKind::Posix;
    let mut state = QuoteState::Unquoted;
    // Command-substitution regions still open, innermost last.
    let mut regions: Vec<Region> = Vec::new();
    let mut backtick_depth = 0usize;
    // Start of the run of template bytes not yet copied into `out`.
    let mut chunk_start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            // Skip past the escaped byte so `\"` does not flip `state`.
            b'\\' if posix && state != QuoteState::Single => {
                i = bytes.len().min(i + 2);
            }
            b'\'' if posix && state != QuoteState::Double => {
                state = if state == QuoteState::Single {
                    QuoteState::Unquoted
                } else {
                    QuoteState::Single
                };
                i += 1;
            }
            b'"' if state != QuoteState::Single => {
                state = if state == QuoteState::Double {
                    QuoteState::Unquoted
                } else {
                    QuoteState::Double
                };
                i += 1;
            }
            b'$' if posix && state != QuoteState::Single && bytes.get(i + 1) == Some(&b'(') => {
                regions.push(Region::Subshell(state));
                state = QuoteState::Unquoted;
                i += 2;
            }
            b')' if posix && state != QuoteState::Single => {
                if let Some(Region::Subshell(outer)) = regions.last().copied() {
                    regions.pop();
                    state = outer;
                }
                i += 1;
            }
            b'`' if posix && state != QuoteState::Single => {
                if let Some(Region::Backtick(outer)) = regions.last().copied() {
                    regions.pop();
                    backtick_depth -= 1;
                    state = outer;
                } else {
                    regions.push(Region::Backtick(state));
                    backtick_depth += 1;
                    state = QuoteState::Unquoted;
                }
                i += 1;
            }
            b'{' => {
                let name_start = i + 1;
                let resolved = template[name_start..].find('}').and_then(|offset| {
                    resolve(&template[name_start..name_start + offset])
                        .map(|value| (value, name_start + offset + 1))
                });

                match resolved {
                    Some((value, after)) => {
                        out.push_str(&template[chunk_start..i]);
                        // An empty value carries no shell syntax, so it is
                        // spliced in as nothing rather than as an empty word.
                        if !value.is_empty() {
                            out.push_str(&match shell {
                                ShellKind::Posix => escape_posix(value, state, backtick_depth),
                                ShellKind::Cmd => escape_cmd(value, state),
                            });
                        }
                        i = after;
                        chunk_start = after;
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }

    out.push_str(&template[chunk_start..]);
    out
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

    /// Substitute variables in a command string for the shell that will run it.
    fn substitute_variables(command: &str, input: &ProcessorInput) -> String {
        Self::substitute_variables_for(SHELL, command, input)
    }

    /// Expand the template's own `%Y`-style placeholders, then splice in the
    /// `{...}` values escaped for `shell`.
    ///
    /// Values carry platform-supplied text (a recording path built from the
    /// stream title, `{title}`, `{streamer}`), so they are treated as data
    /// rather than as command syntax: time placeholders belong to the template
    /// and are expanded first, which leaves a `%` arriving with a value literal
    /// and keeps `escape_cmd`'s doubled `%%` intact.
    fn substitute_variables_for(shell: ShellKind, command: &str, input: &ProcessorInput) -> String {
        let template = pipeline_common::expand_path_template(command);

        let input_path = input.inputs.first().map(|s| s.as_str()).unwrap_or("");
        let output_path = input.outputs.first().map(|s| s.as_str()).unwrap_or("");

        let inputs_json = serde_json::to_string(&input.inputs).unwrap_or_else(|_| "[]".to_string());
        let outputs_json =
            serde_json::to_string(&input.outputs).unwrap_or_else(|_| "[]".to_string());

        // Same fallbacks as utils::filename::expand_placeholders: `{streamer}`
        // degrades to the id, `{title}` to empty, both filesystem-sanitized.
        let streamer_display = input
            .streamer_name
            .as_deref()
            .map(sanitize_filename)
            .unwrap_or_else(|| input.streamer_id.clone());
        let title_display = input
            .session_title
            .as_deref()
            .map(sanitize_filename)
            .unwrap_or_default();

        substitute_placeholders(&template, shell, |name| match name {
            "input" => Some(input_path),
            "output" => Some(output_path),
            "inputs_json" => Some(inputs_json.as_str()),
            "outputs_json" => Some(outputs_json.as_str()),
            "streamer_id" => Some(input.streamer_id.as_str()),
            "session_id" => Some(input.session_id.as_str()),
            "streamer" => Some(streamer_display.as_str()),
            "title" => Some(title_display.as_str()),
            "platform" => Some(input.platform.as_deref().unwrap_or("")),
            // `{inputN}` / `{outputN}` past the end of the list stay literal.
            _ => name
                .strip_prefix("input")
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| input.inputs.get(index))
                .or_else(|| {
                    name.strip_prefix("output")
                        .and_then(|index| index.parse::<usize>().ok())
                        .and_then(|index| input.outputs.get(index))
                })
                .map(String::as_str),
        })
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

        ctx.info(format!("Executing command: {}", command));

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
                    ctx.warn(format!("Failed to create output directory {}: {}", dir, e));
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
            use std::os::windows::process::CommandExt;

            let mut c = Command::new("cmd");
            c.args(["/S", "/C"]);
            // `/S` makes cmd strip exactly the outer quote pair instead of
            // applying its multi-quote rule, which would otherwise eat the
            // first and last quote of a command that quotes its own program
            // path. `raw_arg` keeps std's argument quoting from rewriting the
            // `""` pairs escape_cmd emits into `\"`, which cmd does not
            // unescape.
            c.as_std_mut().raw_arg(format!("\"{command}\""));
            c
        };

        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", &command]);
            c
        };

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
                ctx.error(format!("Command timed out after {}s", self.timeout_secs));
                // run_command_with_logs enables kill_on_drop, so dropping its
                // future here also terminates the child.
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

            ctx.error(format!(
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

        ctx.info(format!("Command completed in {:.2}s", duration));

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
                ctx.info(format!(
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
            uploads: vec![],
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
        let result =
            ExecuteCommandProcessor::substitute_variables_for(ShellKind::Posix, command, &input);

        assert_eq!(result, "echo '/input.flv' '/output.mp4' 'streamer-1'");
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
        let out = ExecuteCommandProcessor::substitute_variables_for(ShellKind::Posix, cmd, &input);

        assert_eq!(
            out,
            "echo '/in0.mp4' '/in0.mp4' '/in1.json' '/out0.mp4' '/out1.json' \
             '[\"/in0.mp4\",\"/in1.json\"]' '[\"/out0.mp4\",\"/out1.json\"]' 's' 'sess'"
        );
    }

    #[test]
    fn test_substitute_variables_leaves_unknown_placeholders() {
        let input = ProcessorInput {
            inputs: vec!["/in0.mp4".to_string()],
            outputs: vec![],
            ..Default::default()
        };

        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo {input3} {nope} {",
            &input,
        );

        assert_eq!(out, "echo {input3} {nope} {");
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
        let out = ExecuteCommandProcessor::substitute_variables_for(ShellKind::Posix, cmd, &input);

        assert_eq!(
            out,
            "echo 'Twitch' 'Streamer_Name_' 'Title_With_Colons' 'streamer-123' 'session-456'"
        );
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

    /// A recording path built from a platform-supplied title can carry every
    /// shell metacharacter `utils::filename::sanitize_filename` leaves alone.
    const HOSTILE: &str = r#"/rec/a b$(id);`id`'q"&.mp4"#;

    fn hostile_input() -> ProcessorInput {
        ProcessorInput {
            inputs: vec![HOSTILE.to_string()],
            outputs: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn test_posix_escaping_unquoted_placeholder() {
        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo {input}",
            &hostile_input(),
        );

        assert_eq!(out, r#"echo '/rec/a b$(id);`id`'\''q"&.mp4'"#);
    }

    #[test]
    fn test_posix_escaping_inside_double_quotes() {
        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo \"{input}\"",
            &hostile_input(),
        );

        assert_eq!(out, r#"echo "/rec/a b\$(id);\`id\`'q\"&.mp4""#);
    }

    #[test]
    fn test_posix_escaping_inside_single_quotes() {
        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo '{input}'",
            &hostile_input(),
        );

        assert_eq!(out, r#"echo '/rec/a b$(id);`id`'\''q"&.mp4'"#);
    }

    /// A backslash-escaped quote is not a quote, so the placeholder after it is
    /// still unquoted and must be wrapped by `escape_posix`.
    #[test]
    fn test_posix_escaped_quote_does_not_open_a_quoted_region() {
        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo \\\"{input}\\\"",
            &hostile_input(),
        );

        assert_eq!(out, r#"echo \"'/rec/a b$(id);`id`'\''q"&.mp4'\""#);
    }

    /// The quote a value contributes must not change the state the next
    /// placeholder is escaped for.
    #[test]
    fn test_posix_value_quote_does_not_leak_into_later_placeholders() {
        let input = ProcessorInput {
            inputs: vec![HOSTILE.to_string()],
            outputs: vec!["/out.mp4".to_string()],
            streamer_id: "s1".to_string(),
            ..Default::default()
        };

        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo \"{input}\" {output} '{streamer_id}'",
            &input,
        );

        assert_eq!(
            out,
            r#"echo "/rec/a b\$(id);\`id\`'q\"&.mp4" '/out.mp4' 's1'"#
        );
    }

    /// Time placeholders belong to the template, so a `%` arriving with a value
    /// stays literal.
    #[test]
    fn test_percent_in_value_is_not_expanded_as_a_time_placeholder() {
        let input = ProcessorInput {
            inputs: vec!["/rec/100%Y.mp4".to_string()],
            ..Default::default()
        };

        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo {input}",
            &input,
        );

        assert_eq!(out, "echo '/rec/100%Y.mp4'");
    }

    /// A placeholder inside `$( ... )` is re-parsed by the subshell as a fresh
    /// command, so the enclosing double quotes do not protect it.
    #[test]
    fn test_posix_escaping_inside_command_substitution() {
        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo \"$(dirname {input})\"",
            &hostile_input(),
        );

        assert_eq!(out, r#"echo "$(dirname '/rec/a b$(id);`id`'\''q"&.mp4')""#);
    }

    /// The template's quoting resumes once the subshell closes.
    #[test]
    fn test_posix_quoting_resumes_after_command_substitution() {
        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo \"$(dirname {input})\" {input}",
            &hostile_input(),
        );

        assert_eq!(
            out,
            r#"echo "$(dirname '/rec/a b$(id);`id`'\''q"&.mp4')" '/rec/a b$(id);`id`'\''q"&.mp4'"#
        );
    }

    /// A `` `...` `` region ends at the first backtick the shell finds, so
    /// single-quoting the value is not enough on its own.
    #[test]
    fn test_posix_escaping_inside_backticks() {
        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo `dirname {input}`",
            &hostile_input(),
        );

        assert_eq!(out, r#"echo `dirname '/rec/a b$(id);\`id\`'\\''q"&.mp4'`"#);
    }

    #[test]
    fn test_posix_escaping_inside_backticks_within_double_quotes() {
        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo \"`dirname {input}`\"",
            &hostile_input(),
        );

        assert_eq!(
            out,
            r#"echo "`dirname '/rec/a b$(id);\`id\`'\\''q"&.mp4'`""#
        );
    }

    /// An absent output or title contributes no token, as before quoting.
    #[test]
    fn test_empty_values_are_spliced_in_as_nothing() {
        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Posix,
            "echo {output} {title}",
            &ProcessorInput::default(),
        );

        assert_eq!(out, "echo  ");
    }

    #[test]
    fn test_cmd_escaping_doubles_quotes_and_leaves_percent() {
        let input = ProcessorInput {
            inputs: vec![r#"C:\rec\a b&whoami%PATH%"x.mp4"#.to_string()],
            ..Default::default()
        };

        // Unquoted: escape_cmd supplies the quotes.
        assert_eq!(
            ExecuteCommandProcessor::substitute_variables_for(
                ShellKind::Cmd,
                "echo {input}",
                &input
            ),
            r#"echo "C:\rec\a b&whoami%PATH%""x.mp4""#
        );

        // Inside the template's own quotes: no second layer.
        assert_eq!(
            ExecuteCommandProcessor::substitute_variables_for(
                ShellKind::Cmd,
                "echo \"{input}\"",
                &input
            ),
            r#"echo "C:\rec\a b&whoami%PATH%""x.mp4""#
        );

        // cmd has no single-quote syntax, so the placeholder is still unquoted.
        assert_eq!(
            ExecuteCommandProcessor::substitute_variables_for(
                ShellKind::Cmd,
                "echo '{input}'",
                &input
            ),
            r#"echo '"C:\rec\a b&whoami%PATH%""x.mp4"'"#
        );
    }

    /// Doubling rather than dropping `"` keeps `{inputs_json}` parseable once
    /// cmd and the C runtime have removed one layer of quoting.
    #[test]
    fn test_cmd_escaping_keeps_json_placeholders_intact() {
        let input = ProcessorInput {
            inputs: vec![r"C:\rec\a.mp4".to_string()],
            ..Default::default()
        };

        let out = ExecuteCommandProcessor::substitute_variables_for(
            ShellKind::Cmd,
            "echo {inputs_json}",
            &input,
        );

        assert_eq!(out, r#"echo "[""C:\\rec\\a.mp4""]""#);
    }

    /// End-to-end through `sh -c`: a path full of metacharacters reaches the
    /// child as one literal argument.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_hostile_input_path_reaches_the_child_verbatim() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let hostile = temp_dir.path().join(r#"a b$(id);`id`'q"&.mp4"#);
        let hostile = hostile.to_string_lossy().to_string();
        let captured = temp_dir.path().join("captured.txt");

        let processor = ExecuteCommandProcessor::new();
        let ctx = ProcessorContext::noop("test");

        for template in [
            "printf %s {input}",
            "printf %s \"{input}\"",
            "printf %s '{input}'",
            "printf %s \"$(printf %s {input})\"",
            "printf %s \"`printf %s {input}`\"",
            "printf %s \"$(printf %s \"`printf %s {input}`\")\"",
        ] {
            let config = serde_json::json!({
                "command": format!("{template} > {}", captured.display()),
            });

            let input = ProcessorInput {
                inputs: vec![hostile.clone()],
                outputs: vec![],
                config: Some(config.to_string()),
                ..Default::default()
            };

            processor.process(&input, &ctx).await.unwrap();

            assert_eq!(
                tokio::fs::read_to_string(&captured).await.unwrap(),
                hostile,
                "template {template} did not pass the path through literally"
            );
        }
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
