//! ASS burn-in processor (renders `.ass` subtitles into video frames).
//!
//! Intended for paired/session DAG pipelines where `inputs[]` may include:
//! - a JSON manifest (`*_inputs.json`)
//! - video files
//! - danmu XML files
//! - generated `.ass` subtitle files
//!
//! This processor is manifest-aware (prefers `video_inputs` + `danmu_inputs`) and batch-safe.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, info};

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{create_log_entry, get_extension, is_video, parse_config_or_default};
use crate::Result;

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_crf() -> u8 {
    23
}

fn default_preset() -> String {
    "veryfast".to_string()
}

/// How to match a video input with an ASS subtitle file.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssMatchStrategy {
    /// Prefer the manifest: map `video_inputs[i]` to `danmu_inputs[i]` (xml) and then expect `*.ass`.
    #[default]
    Manifest,
    /// Match by file stem: `video_stem.ass` (or `video_stem_danmaku.ass`).
    Stem,
}

/// Configuration for ASS burn-in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssBurnInConfig {
    /// Path to ffmpeg binary.
    /// If omitted, uses env `FFMPEG_PATH` or defaults to `ffmpeg`.
    #[serde(default)]
    pub ffmpeg_path: Option<String>,

    /// Matching strategy for pairing videos with `.ass`.
    #[serde(default)]
    pub match_strategy: AssMatchStrategy,

    /// If true, require an ASS file for every video input; otherwise videos without ASS are passed through.
    #[serde(default = "default_true")]
    pub require_ass: bool,

    /// If true, include original inputs in outputs for downstream chaining; otherwise output only burned videos.
    #[serde(default = "default_true")]
    pub passthrough_inputs: bool,

    /// If true, exclude `.ass` inputs from passthrough outputs (useful when treating `.ass` as intermediate).
    #[serde(default = "default_false")]
    pub exclude_ass_from_passthrough: bool,

    /// Output extension override (default keeps the original video extension).
    #[serde(default)]
    pub output_extension: Option<String>,

    /// Video encoder for burn-in output (defaults to libx264).
    #[serde(default)]
    pub video_codec: Option<String>,

    /// Audio codec (defaults to `copy`).
    #[serde(default)]
    pub audio_codec: Option<String>,

    /// CRF for x264/x265-like encoders.
    #[serde(default = "default_crf")]
    pub crf: u8,

    /// Encoder preset (for x264/x265-like encoders).
    #[serde(default = "default_preset")]
    pub preset: String,

    /// If true, overwrite existing outputs.
    #[serde(default = "default_true")]
    pub overwrite: bool,

    /// Optional fonts dir for libass.
    #[serde(default)]
    pub fonts_dir: Option<String>,

    /// If true, delete source video files that were successfully burned-in after all conversions succeed.
    #[serde(default = "default_false")]
    pub delete_source_videos_on_success: bool,

    /// If true, delete matched source `.ass` files for successfully burned-in videos after all conversions succeed.
    #[serde(default = "default_false")]
    pub delete_source_ass_on_success: bool,
}

impl Default for AssBurnInConfig {
    fn default() -> Self {
        Self {
            ffmpeg_path: None,
            match_strategy: AssMatchStrategy::default(),
            require_ass: true,
            passthrough_inputs: true,
            exclude_ass_from_passthrough: false,
            output_extension: None,
            video_codec: Some("libx264".to_string()),
            audio_codec: Some("copy".to_string()),
            crf: default_crf(),
            preset: default_preset(),
            overwrite: true,
            fonts_dir: None,
            delete_source_videos_on_success: false,
            delete_source_ass_on_success: false,
        }
    }
}

pub struct AssBurnInProcessor;

impl AssBurnInProcessor {
    pub fn new() -> Self {
        Self
    }

    fn resolve_ffmpeg_path(config: &AssBurnInConfig) -> String {
        config
            .ffmpeg_path
            .clone()
            .or_else(|| std::env::var("FFMPEG_PATH").ok())
            .unwrap_or_else(|| "ffmpeg".to_string())
    }

    fn paths_equal(a: &str, b: &str) -> bool {
        if cfg!(windows) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    }

    fn escape_filter_path(value: &str) -> String {
        // FFmpeg filter option escaping: backslash and colon are special.
        // We also escape single quotes since we wrap values in single quotes.
        value
            .replace('\\', "\\\\")
            .replace(':', "\\:")
            .replace('\'', "\\'")
    }

    fn make_subtitles_filter(ass_path: &str, fonts_dir: Option<&str>) -> String {
        // Use the `subtitles` filter (works with .ass and supports fontsdir).
        let filename = Self::escape_filter_path(ass_path);
        if let Some(fonts) = fonts_dir {
            let fonts = Self::escape_filter_path(fonts);
            format!("subtitles=filename='{}':fontsdir='{}'", filename, fonts)
        } else {
            format!("subtitles=filename='{}'", filename)
        }
    }

    fn determine_output_path_for_video(
        input_path: &str,
        config: &AssBurnInConfig,
        output_override: Option<&str>,
    ) -> Result<String> {
        let input_abs = Self::make_absolute(input_path);

        if let Some(out) = output_override.filter(|s| !s.is_empty()) {
            let out_abs = Self::make_absolute(out);
            if Self::paths_equal(&input_abs, &out_abs) {
                return Err(crate::Error::PipelineError(
                    "ASS burn-in output path must not be the same as the input path".to_string(),
                ));
            }
            return Ok(out.to_string());
        }

        let input_path_obj = Path::new(input_path);
        let parent = input_path_obj.parent().unwrap_or(Path::new("."));
        let stem = input_path_obj
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let ext = config
            .output_extension
            .as_deref()
            .or_else(|| input_path_obj.extension().and_then(|s| s.to_str()))
            .unwrap_or("mp4");

        let candidate = parent
            .join(format!("{}_burnin.{}", stem, ext))
            .to_string_lossy()
            .to_string();
        let candidate_abs = Self::make_absolute(&candidate);
        if Self::paths_equal(&input_abs, &candidate_abs) {
            return Ok(parent
                .join(format!("{}_burnin2.{}", stem, ext))
                .to_string_lossy()
                .to_string());
        }
        Ok(candidate)
    }

    async fn parse_manifest_pairs(inputs: &[String]) -> Option<(Vec<String>, Vec<String>)> {
        let manifest_path = inputs
            .iter()
            .find(|p| p.to_lowercase().ends_with(".json"))?;
        let text = tokio::fs::read_to_string(manifest_path).await.ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        let videos = value.get("video_inputs")?.as_array()?;
        let danmus = value.get("danmu_inputs")?.as_array()?;
        let video_inputs: Vec<String> = videos
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        let danmu_inputs: Vec<String> = danmus
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        Some((video_inputs, danmu_inputs))
    }

    fn build_ass_index(inputs: &[String]) -> HashMap<String, String> {
        // Map lowercased file stem -> full path for `.ass` inputs.
        let mut map = HashMap::new();
        for path in inputs {
            if path.to_lowercase().ends_with(".ass")
                && let Some(stem) = Path::new(path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
            {
                map.insert(stem.to_lowercase(), path.clone());
            }
        }
        map
    }

    fn match_ass_for_video(
        video_path: &str,
        strategy: AssMatchStrategy,
        manifest_map: Option<&HashMap<String, String>>,
        ass_by_stem: &HashMap<String, String>,
    ) -> Option<String> {
        match strategy {
            AssMatchStrategy::Manifest => {
                if let Some(map) = manifest_map {
                    let video_abs = Self::make_absolute(video_path);
                    if let Some(ass) = map.get(&video_abs.to_lowercase()) {
                        return Some(ass.clone());
                    }
                }
                // fallback to stem matching if manifest mapping not available
                Self::match_ass_for_video(video_path, AssMatchStrategy::Stem, None, ass_by_stem)
            }
            AssMatchStrategy::Stem => {
                let stem = Path::new(video_path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())?;
                let stem_key = stem.to_lowercase();
                if let Some(ass) = ass_by_stem.get(&stem_key) {
                    return Some(ass.clone());
                }
                let alt = format!("{}_danmaku", stem_key);
                ass_by_stem.get(&alt).cloned()
            }
        }
    }

    fn build_passthrough_outputs(
        inputs: &[String],
        exclude_ass: bool,
        exclude_paths: &HashSet<String>,
    ) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::<String>::new();
        for p in inputs {
            if exclude_paths.contains(p) {
                continue;
            }
            if exclude_ass && p.to_lowercase().ends_with(".ass") {
                continue;
            }
            if seen.insert(p.clone()) {
                out.push(p.clone());
            }
        }
        out
    }

    fn make_absolute(path: &str) -> String {
        let p = Path::new(path);
        if p.is_absolute() {
            return path.to_string();
        }
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(p).to_string_lossy().to_string(),
            Err(_) => path.to_string(),
        }
    }
}

impl Default for AssBurnInProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for AssBurnInProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["ass_burnin", "burn_ass", "burn_subtitles"]
    }

    fn name(&self) -> &'static str {
        "AssBurnInProcessor"
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

        let config: AssBurnInConfig =
            parse_config_or_default(input.config.as_deref(), ctx, "ass_burnin", Some(&mut logs));

        if input.inputs.is_empty() {
            return Err(crate::Error::PipelineError(
                "No input files specified for ass_burnin".to_string(),
            ));
        }

        let ffmpeg = Self::resolve_ffmpeg_path(&config);

        // Build candidate video list: prefer manifest.video_inputs when available and strategy=Manifest.
        let mut manifest_video_to_ass: Option<HashMap<String, String>> = None;
        if config.match_strategy == AssMatchStrategy::Manifest
            && let Some((video_inputs, danmu_inputs)) =
                Self::parse_manifest_pairs(&input.inputs).await
        {
            let mut map = HashMap::new();
            for (video, danmu) in video_inputs.iter().zip(danmu_inputs.iter()) {
                let video_abs = Self::make_absolute(video);
                // Assume danmu is xml and ass is alongside with same stem by default.
                let ass_candidate = PathBuf::from(danmu)
                    .with_extension("ass")
                    .to_string_lossy()
                    .to_string();
                map.insert(video_abs.to_lowercase(), ass_candidate);
            }
            if !map.is_empty() {
                manifest_video_to_ass = Some(map);
            }
        }

        let ass_by_stem = Self::build_ass_index(&input.inputs);

        // Determine which inputs are videos.
        let mut video_inputs = Vec::new();
        for p in &input.inputs {
            let ext = get_extension(p).unwrap_or_default();
            if is_video(&ext) {
                video_inputs.push(p.clone());
            }
        }

        if video_inputs.is_empty() {
            let duration = start.elapsed().as_secs_f64();
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Info,
                "No video inputs found; passing through",
            ));
            return Ok(ProcessorOutput {
                outputs: if config.passthrough_inputs {
                    Self::build_passthrough_outputs(
                        &input.inputs,
                        config.exclude_ass_from_passthrough,
                        &HashSet::new(),
                    )
                } else {
                    vec![]
                },
                duration_secs: duration,
                metadata: Some(
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "no_video_inputs",
                    })
                    .to_string(),
                ),
                ..Default::default()
            });
        }

        // Output mapping contract: map outputs[] against video_inputs (not full inputs list).
        let output_paths: Vec<String> = if input.outputs.is_empty() {
            video_inputs
                .iter()
                .map(|v| Self::determine_output_path_for_video(v, &config, None))
                .collect::<Result<Vec<_>>>()?
        } else if input.outputs.len() == video_inputs.len() {
            video_inputs
                .iter()
                .zip(input.outputs.iter())
                .map(|(v, out)| Self::determine_output_path_for_video(v, &config, Some(out)))
                .collect::<Result<Vec<_>>>()?
        } else {
            return Err(crate::Error::PipelineError(format!(
                "ass_burnin batch job requires outputs to be empty or have the same length as selected video inputs (videos={}, outputs={})",
                video_inputs.len(),
                input.outputs.len()
            )));
        };

        let mut produced = Vec::new();
        let mut succeeded_inputs = Vec::new();
        let mut skipped_inputs = Vec::new();
        let mut matched_ass_for_succeeded = Vec::new();
        let mut total_duration = 0.0;

        for (idx, video_path) in video_inputs.iter().enumerate() {
            let output_path = &output_paths[idx];

            if !Path::new(video_path).exists() {
                return Err(crate::Error::PipelineError(format!(
                    "Video input does not exist: {}",
                    video_path
                )));
            }

            let ass_path = Self::match_ass_for_video(
                video_path,
                config.match_strategy,
                manifest_video_to_ass.as_ref(),
                &ass_by_stem,
            );

            let Some(ass_path) = ass_path else {
                if config.require_ass {
                    return Err(crate::Error::PipelineError(format!(
                        "No ASS subtitle found for video input: {}",
                        video_path
                    )));
                }
                skipped_inputs.push((video_path.clone(), "no matching .ass subtitle".to_string()));
                continue;
            };

            if !Path::new(&ass_path).exists() {
                if config.require_ass {
                    return Err(crate::Error::PipelineError(format!(
                        "Matched ASS subtitle does not exist for video {}: {}",
                        video_path, ass_path
                    )));
                }
                skipped_inputs.push((
                    video_path.clone(),
                    "matched .ass subtitle missing".to_string(),
                ));
                continue;
            }

            if Path::new(output_path).exists() && !config.overwrite {
                return Err(crate::Error::PipelineError(format!(
                    "Output already exists and overwrite is disabled: {}",
                    output_path
                )));
            }

            let filter = Self::make_subtitles_filter(&ass_path, config.fonts_dir.as_deref());

            let mut args: Vec<String> = Vec::new();
            if config.overwrite {
                args.push("-y".to_string());
            }
            args.extend([
                "-hide_banner".to_string(),
                "-nostats".to_string(),
                "-loglevel".to_string(),
                "info".to_string(),
                "-progress".to_string(),
                "pipe:1".to_string(),
                "-i".to_string(),
                video_path.clone(),
                "-vf".to_string(),
                filter,
            ]);

            let vcodec = config.video_codec.as_deref().unwrap_or("libx264");
            let acodec = config.audio_codec.as_deref().unwrap_or("copy");
            args.extend(["-c:v".to_string(), vcodec.to_string()]);
            if vcodec.contains("264") || vcodec.contains("265") {
                args.extend(["-crf".to_string(), config.crf.to_string()]);
                args.extend(["-preset".to_string(), config.preset.clone()]);
            }
            args.extend(["-c:a".to_string(), acodec.to_string()]);

            args.push(output_path.clone());

            info!("Burning ASS into {} -> {}", video_path, output_path);
            debug!("FFmpeg args: {:?}", args);

            let mut cmd = Command::new(&ffmpeg);
            cmd.args(&args).env("LC_ALL", "C");

            let command_output = crate::pipeline::processors::utils::run_ffmpeg_with_progress(
                &mut cmd,
                &ctx.progress,
                Some(ctx.log_sink.clone()),
            )
            .await?;

            total_duration += command_output.duration;
            logs.extend(command_output.logs);

            if !command_output.status.success() {
                return Err(crate::Error::PipelineError(format!(
                    "ffmpeg burn-in failed for {}",
                    video_path
                )));
            }

            produced.push(output_path.clone());
            succeeded_inputs.push(video_path.clone());
            matched_ass_for_succeeded.push(ass_path);
        }

        // Delete sources only after all burn-ins have succeeded (best-effort).
        let mut deleted_paths = HashSet::<String>::new();
        let mut removed_video_count = 0usize;
        let mut failed_remove_video_count = 0usize;
        if config.delete_source_videos_on_success {
            for video in &succeeded_inputs {
                match tokio::fs::remove_file(video).await {
                    Ok(()) => {
                        deleted_paths.insert(video.clone());
                        removed_video_count = removed_video_count.saturating_add(1);
                    }
                    Err(e) => {
                        failed_remove_video_count = failed_remove_video_count.saturating_add(1);
                        let msg = format!("Failed to remove source video {}: {}", video, e);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Warn,
                            msg,
                        ));
                    }
                }
            }
        }

        let mut removed_ass_count = 0usize;
        let mut failed_remove_ass_count = 0usize;
        if config.delete_source_ass_on_success {
            let mut unique = HashSet::<String>::new();
            for ass in &matched_ass_for_succeeded {
                if !unique.insert(ass.clone()) {
                    continue;
                }
                match tokio::fs::remove_file(ass).await {
                    Ok(()) => {
                        deleted_paths.insert(ass.clone());
                        removed_ass_count = removed_ass_count.saturating_add(1);
                    }
                    Err(e) => {
                        failed_remove_ass_count = failed_remove_ass_count.saturating_add(1);
                        let msg = format!("Failed to remove source ass {}: {}", ass, e);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Warn,
                            msg,
                        ));
                    }
                }
            }
        }

        // Outputs for chaining: either pass through non-video inputs and append produced videos, or return produced only.
        let mut outputs = Vec::new();
        if config.passthrough_inputs {
            outputs.extend(Self::build_passthrough_outputs(
                &input.inputs,
                config.exclude_ass_from_passthrough,
                &deleted_paths,
            ));
        }
        // Append produced burn-in videos (dedup).
        let mut seen = outputs.iter().cloned().collect::<HashSet<_>>();
        for p in &produced {
            if seen.insert(p.clone()) {
                outputs.push(p.clone());
            }
        }

        Ok(ProcessorOutput {
            outputs,
            duration_secs: start.elapsed().as_secs_f64().max(total_duration),
            metadata: Some(
                serde_json::json!({
                    "videos": video_inputs.len(),
                    "produced": produced.len(),
                    "require_ass": config.require_ass,
                    "match_strategy": format!("{:?}", config.match_strategy),
                    "delete_source_videos_on_success": config.delete_source_videos_on_success,
                    "delete_source_ass_on_success": config.delete_source_ass_on_success,
                    "removed_video_count": removed_video_count,
                    "failed_remove_video_count": failed_remove_video_count,
                    "removed_ass_count": removed_ass_count,
                    "failed_remove_ass_count": failed_remove_ass_count,
                })
                .to_string(),
            ),
            items_produced: produced,
            input_size_bytes: None,
            output_size_bytes: None,
            failed_inputs: vec![],
            succeeded_inputs,
            skipped_inputs,
            logs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_escape_filter_path_windows_sensitive() {
        let s = "C:\\a\\b:c'ass";
        let escaped = AssBurnInProcessor::escape_filter_path(s);
        assert!(escaped.contains("\\\\"));
        assert!(escaped.contains("\\:"));
        assert!(escaped.contains("\\'"));
    }

    #[test]
    fn test_make_subtitles_filter() {
        let f = AssBurnInProcessor::make_subtitles_filter("a.ass", None);
        assert!(f.contains("subtitles="));
        assert!(f.contains("a.ass"));
    }

    #[tokio::test]
    async fn test_manifest_pair_mapping_builds_video_map() {
        let temp = TempDir::new().unwrap();
        let manifest = temp.path().join("segment_inputs.json");
        let video = temp.path().join("seg.mp4");
        let xml = temp.path().join("seg.xml");
        tokio::fs::write(
            &manifest,
            serde_json::json!({
                "video_inputs": [video.to_string_lossy().to_string()],
                "danmu_inputs": [xml.to_string_lossy().to_string()]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let inputs = vec![
            manifest.to_string_lossy().to_string(),
            video.to_string_lossy().to_string(),
            xml.to_string_lossy().to_string(),
        ];

        let (videos, danmus) = AssBurnInProcessor::parse_manifest_pairs(&inputs)
            .await
            .unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(danmus.len(), 1);
    }

    #[tokio::test]
    async fn test_outputs_len_mismatch_errors() {
        let processor = AssBurnInProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec!["a.mp4".to_string(), "b.mp4".to_string()],
            outputs: vec!["out.mp4".to_string()],
            config: Some(serde_json::json!({ "require_ass": false }).to_string()),
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

    #[test]
    fn test_passthrough_filters_deleted_paths_and_ass() {
        let inputs = vec![
            "manifest.json".to_string(),
            "video.mp4".to_string(),
            "subtitle.ass".to_string(),
        ];
        let mut exclude = HashSet::new();
        exclude.insert("video.mp4".to_string());

        let outputs = AssBurnInProcessor::build_passthrough_outputs(&inputs, true, &exclude);
        assert_eq!(outputs, vec!["manifest.json"]);
    }
}
