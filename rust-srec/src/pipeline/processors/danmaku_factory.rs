//! DanmakuFactory processor for converting danmu XML to ASS subtitles.
//!
//! Designed for paired/session DAG pipelines where `inputs[]` may contain video and danmu files.
//! This processor:
//! - Selects danmu XML inputs (the session pairing's danmu files when the job carries one)
//! - Runs DanmakuFactory to generate `.ass` files, staged and published together
//! - Returns outputs that include the original inputs plus generated `.ass` paths for downstream steps

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::outputs::OutputBatch;
use super::paths::{CasePolicy, lexical_absolute, spelling_equal};
use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{create_log_entry, parse_config_or_default};
use crate::Result;
use crate::pipeline::manifest::PipelineInputManifest;

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

    /// Convert only the danmu files recorded by the session pairing when the
    /// job carries one, instead of every `.xml` input.
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

    fn is_xml(path: &str) -> bool {
        path.to_lowercase().ends_with(".xml")
    }

    /// XML inputs to convert. With a session pairing, the recorded danmu paths
    /// are resolved against the job's `.xml` inputs by spelling, then by stem so
    /// a file an upstream step relocated still converts; when none resolves,
    /// every `.xml` input is converted, as without a pairing.
    fn select_danmu_xml_inputs(
        inputs: &[String],
        manifest: Option<&PipelineInputManifest>,
        prefer_manifest: bool,
    ) -> Vec<String> {
        let xml_inputs: Vec<&String> = inputs.iter().filter(|p| Self::is_xml(p)).collect();
        if prefer_manifest && let Some(manifest) = manifest {
            let mut selected = Vec::new();
            let mut seen = HashSet::<&str>::new();
            for danmu in manifest.danmu_inputs() {
                let danmu_abs = lexical_absolute(danmu);
                let resolved = xml_inputs
                    .iter()
                    .find(|input| {
                        spelling_equal(&lexical_absolute(input), &danmu_abs, CasePolicy::Windows)
                    })
                    .or_else(|| {
                        let stem = Self::stem_key(danmu)?;
                        xml_inputs
                            .iter()
                            .find(|input| Self::stem_key(input).is_some_and(|key| key == stem))
                    });
                if let Some(input) = resolved
                    && seen.insert(input.as_str())
                {
                    selected.push((*input).clone());
                }
            }
            if !selected.is_empty() {
                return selected;
            }
        }

        xml_inputs.into_iter().cloned().collect()
    }

    fn stem_key(path: &str) -> Option<String> {
        Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_lowercase())
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
        // All three job type names select the same XML-to-ASS processor.
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

        let xml_inputs = Self::select_danmu_xml_inputs(
            &input.inputs,
            input.manifest.as_deref(),
            config.prefer_manifest,
        );
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
        let plan = super::planning::OutputPlan::selected(&xml_inputs, &input.outputs)
            .map_err(|_| crate::Error::PipelineError(format!("danmaku_factory batch job requires outputs to be empty or have the same length as selected XML inputs (xml_inputs={}, outputs={})", xml_count, input.outputs.len())))?;
        let ass_outputs: Vec<String> = plan
            .items()
            .map(|(xml, output)| {
                output
                    .map(str::to_owned)
                    .unwrap_or_else(|| Self::default_ass_output_for_xml(xml))
            })
            .collect();

        let binary = Self::resolve_binary_path(&config);
        let mut duration_secs = 0.0;
        let mut delete_warnings: Vec<String> = Vec::new();

        info!(
            "DanmakuFactory converting {} XML files (passthrough_inputs={})",
            xml_count, config.passthrough_inputs
        );

        // Every `.ass` is written to a staged temporary path and published only
        // after the whole batch converted, so a failure or timeout mid-batch
        // leaves no truncated subtitle at a destination a retry would refuse
        // to overwrite.
        let mut batch = OutputBatch::new(&input.inputs);
        let mut produced = Vec::new();
        let mut converted_xml = Vec::new();

        for (xml_path, ass_path) in xml_inputs.iter().zip(ass_outputs.iter()) {
            let xml = PathBuf::from(xml_path);
            let ass = PathBuf::from(ass_path);

            if !super::utils::try_exists(&xml).await? {
                return Err(crate::Error::PipelineError(format!(
                    "DanmakuFactory input XML does not exist: {}",
                    xml.display()
                )));
            }

            if !config.overwrite && super::utils::try_exists(&ass).await? {
                return Err(crate::Error::PipelineError(format!(
                    "DanmakuFactory output already exists and overwrite is disabled: {}",
                    ass.display()
                )));
            }

            let temp_path = batch.stage(&ass, config.overwrite).await?;
            let temp = temp_path.to_string_lossy().into_owned();

            let mut cmd = Command::new(&binary);
            let mut args = Self::substitute_args(&config.args, xml_path, &temp);
            args.extend(config.extra_args.iter().cloned());
            cmd.args(&args);

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

            if !super::utils::try_exists(&temp_path).await? {
                if config.verify_output_exists {
                    return Err(crate::Error::PipelineError(format!(
                        "DanmakuFactory reported success but output file was not created: {}",
                        ass.display()
                    )));
                }
                let msg = format!(
                    "DanmakuFactory produced no output for {}; nothing to publish",
                    xml.display()
                );
                warn!("{}", msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Warn,
                    msg,
                ));
                batch.discard(&temp_path);
                continue;
            }

            produced.push(ass_path.clone());
            converted_xml.push(xml_path.clone());
        }

        batch.commit().await?;

        // Only an XML whose subtitle was published may be deleted; one whose
        // conversion produced nothing is the only danmu left for that segment.
        let mut removed_xml_count = 0usize;
        let mut failed_remove_xml_count = 0usize;
        if config.delete_source_xml_on_success {
            for xml_path in &converted_xml {
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
            exclude_passthrough.extend(converted_xml.iter().cloned());
        }

        // For downstream steps we keep original inputs (video/xml) and append published `.ass`.
        let outputs = Self::build_outputs_for_chaining(
            &input.inputs,
            &produced,
            config.passthrough_inputs,
            &exclude_passthrough,
        );

        Ok(ProcessorOutput {
            outputs,
            duration_secs: duration,
            metadata: Some(
                serde_json::json!({
                    "xml_inputs": xml_count,
                    "ass_outputs": produced.len(),
                    "passthrough_inputs": config.passthrough_inputs,
                    "delete_source_xml_on_success": config.delete_source_xml_on_success,
                    "removed_xml_count": removed_xml_count,
                    "failed_remove_xml_count": failed_remove_xml_count,
                    "delete_warnings": delete_warnings,
                })
                .to_string(),
            ),
            items_produced: produced,
            input_size_bytes: None,
            output_size_bytes: None,
            failed_inputs: vec![],
            succeeded_inputs: converted_xml,
            skipped_inputs: vec![],
            uploads: vec![],
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

    fn manifest(danmu: &[&str]) -> PipelineInputManifest {
        use crate::pipeline::manifest::{ManifestScope, ManifestSegment};
        PipelineInputManifest::new(
            "session",
            "streamer",
            ManifestScope::Segment { index: 0 },
            vec![ManifestSegment {
                segment_index: 0,
                video: vec!["/rec/a.mp4".to_string()],
                danmu: danmu.iter().map(|path| path.to_string()).collect(),
            }],
        )
    }

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    #[test]
    fn session_pairing_selects_recorded_danmu_by_spelling_then_stem() {
        let inputs = strings(&["/rec/a.mp4", "/rec/a.xml", "/rec/b.xml", "/rec/c.ass"]);
        assert_eq!(
            DanmakuFactoryProcessor::select_danmu_xml_inputs(
                &inputs,
                Some(&manifest(&["/rec/a.xml"])),
                true
            ),
            vec!["/rec/a.xml"]
        );

        // An upstream step moved the recording: the stem still resolves it.
        let relocated = strings(&["/archive/a.mp4", "/archive/A.xml", "/archive/b.xml"]);
        assert_eq!(
            DanmakuFactoryProcessor::select_danmu_xml_inputs(
                &relocated,
                Some(&manifest(&["/rec/a.xml", "/rec/a.xml"])),
                true
            ),
            vec!["/archive/A.xml"]
        );

        // Nothing recorded is present, no pairing, or pairing disabled: every `.xml` input.
        for (manifest, prefer) in [
            (Some(manifest(&["/rec/zzz.xml"])), true),
            (None, true),
            (Some(manifest(&["/rec/a.xml"])), false),
        ] {
            assert_eq!(
                DanmakuFactoryProcessor::select_danmu_xml_inputs(
                    &inputs,
                    manifest.as_ref(),
                    prefer
                ),
                vec!["/rec/a.xml", "/rec/b.xml"]
            );
        }
    }

    /// A stand-in DanmakuFactory that writes its `-o` argument, failing on the
    /// second XML so the first staged subtitle must be discarded with the batch.
    fn fake_danmaku_factory(dir: &Path) -> String {
        #[cfg(windows)]
        let (name, script) = (
            "DanmakuFactory.cmd",
            concat!(
                "@echo off\r\nsetlocal\r\n:args\r\nif \"%~1\"==\"\" goto run\r\n",
                "if \"%~1\"==\"-o\" set \"output=%~2\"\r\n",
                "if \"%~1\"==\"-i\" set \"input=%~2\"\r\n",
                "shift\r\ngoto args\r\n:run\r\n",
                "echo [Script Info]> \"%output%\"\r\n",
                "if not \"%input:fail=%\"==\"%input%\" exit /b 3\r\nexit /b 0\r\n",
            ),
        );
        #[cfg(not(windows))]
        let (name, script) = (
            "DanmakuFactory.sh",
            concat!(
                "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n",
                "    -o) output=\"$2\"; shift;;\n",
                "    -i) input=\"$2\"; shift;;\n",
                "  esac\n  shift\ndone\n",
                "printf '[Script Info]' > \"$output\"\n",
                "case \"$input\" in *fail*) exit 3;; esac\nexit 0\n",
            ),
        );
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn staged_subtitles_are_published_together_or_not_at_all() {
        let temp = TempDir::new().unwrap();
        let binary = fake_danmaku_factory(temp.path());
        let good = temp.path().join("good.xml");
        let fail = temp.path().join("fail.xml");
        tokio::fs::write(&good, "<i></i>").await.unwrap();
        tokio::fs::write(&fail, "<i></i>").await.unwrap();
        let processor = DanmakuFactoryProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let failing = ProcessorInput {
            inputs: strings(&[&good.to_string_lossy(), &fail.to_string_lossy()]),
            config: Some(serde_json::json!({ "binary_path": binary }).to_string()),
            ..Default::default()
        };
        let error = processor.process(&failing, &ctx).await.unwrap_err();
        assert!(error.to_string().contains("exit code"), "{error}");
        assert!(!good.with_extension("ass").exists());
        assert!(!fail.with_extension("ass").exists());

        let succeeding = ProcessorInput {
            inputs: strings(&[&good.to_string_lossy()]),
            config: Some(serde_json::json!({ "binary_path": binary }).to_string()),
            ..Default::default()
        };
        let output = processor.process(&succeeding, &ctx).await.unwrap();
        let ass = good.with_extension("ass");
        assert_eq!(
            tokio::fs::read_to_string(&ass).await.unwrap(),
            "[Script Info]"
        );
        assert_eq!(output.items_produced, vec![ass.to_string_lossy()]);
        assert_eq!(
            output.outputs,
            vec![
                good.to_string_lossy().into_owned(),
                ass.to_string_lossy().into_owned()
            ]
        );
        let leftovers: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
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
            inputs: vec!["a.mp4".to_string(), "cover.jpg".to_string()],
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
            "cover.jpg".to_string(),
            "video.mp4".to_string(),
            "danmu.xml".to_string(),
        ];
        let ass = vec!["danmu.ass".to_string()];
        let mut exclude = HashSet::new();
        exclude.insert("danmu.xml".to_string());

        let outputs =
            DanmakuFactoryProcessor::build_outputs_for_chaining(&inputs, &ass, true, &exclude);
        assert_eq!(outputs, vec!["cover.jpg", "video.mp4", "danmu.ass"]);
    }
}
