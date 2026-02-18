//! DanmakuFactory processor for converting danmu XML to ASS subtitles.
//!
//! Designed for paired/session DAG pipelines where `inputs[]` may contain a manifest + video + danmu files.
//! This processor:
//! - Selects danmu XML inputs (prefer manifest's `danmu_inputs` when present)
//! - Runs DanmakuFactory to generate `.ass` files
//! - Returns outputs that include the original inputs plus generated `.ass` paths for downstream steps

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{create_log_entry, parse_config_or_default};
use crate::Result;

fn default_true() -> bool {
    true
}

fn default_args_template() -> Vec<String> {
    vec![
        "-i".to_string(),
        "{input}".to_string(),
        "-o".to_string(),
        "{output}".to_string(),
    ]
}

fn default_false() -> bool {
    false
}

/// Configuration for DanmakuFactory conversion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DanmakuFactoryConfig {
    /// Path to DanmakuFactory binary (or command name in PATH).
    /// If omitted, uses env `DANMAKU_FACTORY_PATH` or defaults to `DanmakuFactory`.
    #[serde(default)]
    pub binary_path: Option<String>,

    /// Command args template. Supports `{input}` and `{output}` placeholders.
    #[serde(default = "default_args_template")]
    pub args: Vec<String>,

    /// Extra args appended after templated args.
    #[serde(default)]
    pub extra_args: Vec<String>,

    /// If true, overwrite existing output files.
    #[serde(default = "default_true")]
    pub overwrite: bool,

    /// If true, verifies the output file exists after the command succeeds.
    #[serde(default = "default_true")]
    pub verify_output_exists: bool,

    /// Prefer selecting XML inputs from the JSON manifest (looks for `danmu_inputs` array).
    #[serde(default = "default_true")]
    pub prefer_manifest: bool,

    /// Include original inputs in outputs for downstream chaining.
    #[serde(default = "default_true")]
    pub passthrough_inputs: bool,

    /// If true, delete selected source XML files after all conversions succeed.
    ///
    /// When enabled and `passthrough_inputs=true`, the deleted XML paths are NOT passed through
    /// in the job outputs (to avoid dangling paths); manifest/video inputs are still preserved.
    #[serde(default = "default_false")]
    pub delete_source_xml_on_success: bool,
}

impl Default for DanmakuFactoryConfig {
    fn default() -> Self {
        Self {
            binary_path: None,
            args: default_args_template(),
            extra_args: Vec::new(),
            overwrite: true,
            verify_output_exists: true,
            prefer_manifest: true,
            passthrough_inputs: true,
            delete_source_xml_on_success: false,
        }
    }
}

pub struct DanmakuFactoryProcessor;

impl DanmakuFactoryProcessor {
    pub fn new() -> Self {
        Self
    }

    fn resolve_binary_path(config: &DanmakuFactoryConfig) -> String {
        config
            .binary_path
            .clone()
            .or_else(|| std::env::var("DANMAKU_FACTORY_PATH").ok())
            .unwrap_or_else(|| "DanmakuFactory".to_string())
    }

    fn substitute_args(template: &[String], input: &str, output: &str) -> Vec<String> {
        template
            .iter()
            .map(|arg| arg.replace("{input}", input).replace("{output}", output))
            .collect()
    }

    fn default_ass_output_for_xml(xml_path: &str) -> String {
        let path = Path::new(xml_path);
        let parent = path.parent().unwrap_or(Path::new("."));
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        parent
            .join(format!("{}.ass", stem))
            .to_string_lossy()
            .to_string()
    }

    async fn select_danmu_xml_inputs(inputs: &[String], prefer_manifest: bool) -> Vec<String> {
        if prefer_manifest
            // Look for a JSON file in the inputs list (manifest is usually first).
            && let Some(manifest_path) = inputs
                .iter()
                .find(|p| p.to_lowercase().ends_with(".json"))
            && let Ok(text) = tokio::fs::read_to_string(manifest_path).await
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(array) = value.get("danmu_inputs").and_then(|v| v.as_array())
        {
            let mut out = Vec::new();
            for item in array {
                if let Some(s) = item.as_str() {
                    out.push(s.to_string());
                }
            }
            if !out.is_empty() {
                return out;
            }
        }

        // Fallback: any `.xml` inputs.
        inputs
            .iter()
            .filter(|p| p.to_lowercase().ends_with(".xml"))
            .cloned()
            .collect()
    }

    fn build_outputs_for_chaining(
        inputs: &[String],
        ass_outputs: &[String],
        passthrough: bool,
        exclude_passthrough: &HashSet<String>,
    ) -> Vec<String> {
        let mut seen = HashSet::<String>::new();
        let mut out = Vec::new();

        if passthrough {
            for path in inputs {
                if exclude_passthrough.contains(path) {
                    continue;
                }
                if seen.insert(path.clone()) {
                    out.push(path.clone());
                }
            }
        }

        for path in ass_outputs {
            if seen.insert(path.clone()) {
                out.push(path.clone());
            }
        }

        out
    }
}

impl Default for DanmakuFactoryProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for DanmakuFactoryProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        // Keep specific names for clarity/back-compat, but also provide a short alias.
        vec!["danmaku_factory", "danmu_to_ass", "danmu"]
    }

    fn name(&self) -> &'static str {
        "DanmakuFactoryProcessor"
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
        let mut logs = Vec::new();

        let config: DanmakuFactoryConfig = parse_config_or_default(
            input.config.as_deref(),
            ctx,
            "danmaku_factory",
            Some(&mut logs),
        );

        if input.inputs.is_empty() {
            return Err(crate::Error::PipelineError(
                "No input files specified for danmaku_factory".to_string(),
            ));
        }

        let xml_inputs = Self::select_danmu_xml_inputs(&input.inputs, config.prefer_manifest).await;
        let xml_count = xml_inputs.len();

        if xml_count == 0 {
            let duration = start.elapsed().as_secs_f64();
            info!("No danmu XML inputs found; passing through");
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Info,
                "No danmu XML inputs found; passing through",
            ));
            return Ok(ProcessorOutput {
                outputs: Self::build_outputs_for_chaining(
                    &input.inputs,
                    &[],
                    config.passthrough_inputs,
                    &HashSet::new(),
                ),
                duration_secs: duration,
                metadata: Some(
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "no_danmu_xml_inputs",
                    })
                    .to_string(),
                ),
                ..Default::default()
            });
        }

        // Batch output mapping contract applies to selected XML inputs.
        let ass_outputs: Vec<String> = if input.outputs.is_empty() {
            xml_inputs
                .iter()
                .map(|p| Self::default_ass_output_for_xml(p))
                .collect()
        } else if input.outputs.len() == xml_count {
            input.outputs.clone()
        } else {
            return Err(crate::Error::PipelineError(format!(
                "danmaku_factory batch job requires outputs to be empty or have the same length as selected XML inputs (xml_inputs={}, outputs={})",
                xml_count,
                input.outputs.len()
            )));
        };

        let binary = Self::resolve_binary_path(&config);
        let mut items_produced = Vec::new();
        let mut duration_secs = 0.0;
        let mut delete_warnings: Vec<String> = Vec::new();

        info!(
            "DanmakuFactory converting {} XML files (passthrough_inputs={})",
            xml_count, config.passthrough_inputs
        );

        for (xml_path, ass_path) in xml_inputs.iter().zip(ass_outputs.iter()) {
            let xml = PathBuf::from(xml_path);
            let ass = PathBuf::from(ass_path);

            if !xml.exists() {
                return Err(crate::Error::PipelineError(format!(
                    "DanmakuFactory input XML does not exist: {}",
                    xml.display()
                )));
            }

            if ass.exists() && !config.overwrite {
                return Err(crate::Error::PipelineError(format!(
                    "DanmakuFactory output already exists and overwrite is disabled: {}",
                    ass.display()
                )));
            }

            let mut cmd = Command::new(&binary);
            let mut args = Self::substitute_args(&config.args, xml_path, ass_path);
            args.extend(config.extra_args.iter().cloned());
            cmd.args(&args).env("LC_ALL", "C");

            debug!("DanmakuFactory args: {:?}", args);

            let command_output = crate::pipeline::processors::utils::run_command_with_logs(
                &mut cmd,
                Some(ctx.log_sink.clone()),
            )
            .await?;

            duration_secs += command_output.duration;
            logs.extend(command_output.logs);

            if !command_output.status.success() {
                return Err(crate::Error::PipelineError(format!(
                    "DanmakuFactory failed with exit code {:?} for input {}",
                    command_output.status.code(),
                    xml.display()
                )));
            }

            if config.verify_output_exists && !ass.exists() {
                return Err(crate::Error::PipelineError(format!(
                    "DanmakuFactory reported success but output file was not created: {}",
                    ass.display()
                )));
            }

            items_produced.push(ass_path.clone());
        }

        let mut removed_xml_count = 0usize;
        let mut failed_remove_xml_count = 0usize;
        if config.delete_source_xml_on_success {
            for xml_path in &xml_inputs {
                match tokio::fs::remove_file(xml_path).await {
                    Ok(()) => {
                        removed_xml_count = removed_xml_count.saturating_add(1);
                    }
                    Err(e) => {
                        failed_remove_xml_count = failed_remove_xml_count.saturating_add(1);
                        let msg = format!("Failed to remove source XML {}: {}", xml_path, e);
                        warn!("{}", msg);
                        delete_warnings.push(msg.clone());
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Warn,
                            msg,
                        ));
                    }
                }
            }
        }

        let duration = start.elapsed().as_secs_f64().max(duration_secs);

        let mut exclude_passthrough = HashSet::new();
        if config.delete_source_xml_on_success && config.passthrough_inputs {
            // Avoid returning dangling paths when we deleted the sources.
            exclude_passthrough.extend(xml_inputs.iter().cloned());
        }

        // For downstream steps we keep original inputs (manifest/video) and append produced `.ass`.
        let outputs = Self::build_outputs_for_chaining(
            &input.inputs,
            &ass_outputs,
            config.passthrough_inputs,
            &exclude_passthrough,
        );

        Ok(ProcessorOutput {
            outputs,
            duration_secs: duration,
            metadata: Some(
                serde_json::json!({
                    "xml_inputs": xml_count,
                    "ass_outputs": ass_outputs.len(),
                    "passthrough_inputs": config.passthrough_inputs,
                    "delete_source_xml_on_success": config.delete_source_xml_on_success,
                    "removed_xml_count": removed_xml_count,
                    "failed_remove_xml_count": failed_remove_xml_count,
                    "delete_warnings": delete_warnings,
                })
                .to_string(),
            ),
            items_produced,
            input_size_bytes: None,
            output_size_bytes: None,
            failed_inputs: vec![],
            succeeded_inputs: xml_inputs,
            skipped_inputs: vec![],
            logs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_ass_output_for_xml() {
        let out = DanmakuFactoryProcessor::default_ass_output_for_xml("/a/b/c.xml");
        assert_eq!(Path::new(&out).file_name().unwrap(), "c.ass");
    }

    #[tokio::test]
    async fn test_selects_manifest_danmu_inputs() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("segment_inputs.json");
        let xml1 = temp.path().join("a.xml");
        let xml2 = temp.path().join("b.xml");

        tokio::fs::write(&xml1, "<i></i>").await.unwrap();
        tokio::fs::write(&xml2, "<i></i>").await.unwrap();
        tokio::fs::write(
            &manifest,
            serde_json::json!({
                "danmu_inputs": [xml1.to_string_lossy().to_string()]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let inputs = vec![
            manifest.to_string_lossy().to_string(),
            xml1.to_string_lossy().to_string(),
            xml2.to_string_lossy().to_string(),
        ];

        let selected = DanmakuFactoryProcessor::select_danmu_xml_inputs(&inputs, true).await;
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0], xml1.to_string_lossy().to_string());
    }

    #[tokio::test]
    async fn test_outputs_mismatch_fails_before_running_command() {
        let processor = DanmakuFactoryProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec!["a.xml".to_string(), "b.xml".to_string()],
            outputs: vec!["out.ass".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let err = processor.process(&input, &ctx).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("outputs to be empty or have the same length")
        );
    }

    #[tokio::test]
    async fn test_no_xml_inputs_pass_through() {
        let processor = DanmakuFactoryProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec!["a.mp4".to_string(), "segment_inputs.json".to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();
        assert_eq!(output.outputs, input.inputs);
        let meta: serde_json::Value =
            serde_json::from_str(output.metadata.as_ref().unwrap()).unwrap();
        assert_eq!(meta["status"], "skipped");
    }

    #[test]
    fn test_build_outputs_filters_excluded_passthrough_inputs() {
        let inputs = vec![
            "manifest.json".to_string(),
            "video.mp4".to_string(),
            "danmu.xml".to_string(),
        ];
        let ass = vec!["danmu.ass".to_string()];
        let mut exclude = HashSet::new();
        exclude.insert("danmu.xml".to_string());

        let outputs =
            DanmakuFactoryProcessor::build_outputs_for_chaining(&inputs, &ass, true, &exclude);
        assert_eq!(outputs, vec!["manifest.json", "video.mp4", "danmu.ass"]);
    }
}
