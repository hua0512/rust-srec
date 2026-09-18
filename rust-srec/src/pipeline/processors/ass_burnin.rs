//! ASS burn-in processor (renders `.ass` subtitles into video frames).
//!
//! Intended for paired/session DAG pipelines where `inputs[]` may include
//! video files, danmu XML files and generated `.ass` subtitle files. Videos
//! are paired with subtitles through the job's session pairing
//! (`ProcessorInput::manifest`) when present, otherwise by file stem. The
//! processor is batch-safe.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tokio::process::Command;
use tracing::{debug, info};

use super::outputs::{OutputBatch, output_size};
use super::paths::{CasePolicy, lexical_absolute, spelling_equal};
use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{create_log_entry, get_extension, is_video, parse_config_or_default};
use crate::Result;
use crate::pipeline::manifest::PipelineInputManifest;

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
    /// Session pairing: the subtitle recorded for the video's own segment when
    /// it is among the job's inputs, then any input with the video's stem, then
    /// the recorded subtitle on disk. Without a pairing only the stem index
    /// applies. The wire value stays `manifest` for saved presets.
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
    ///
    /// A subtitle is burned into one video per job. A later video that resolves
    /// to a subtitle already burned in this job (a second copy of the same
    /// recording) is passed through with a reason, whichever value this has.
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

    fn escape_filter_path(value: &str) -> String {
        // FFmpeg parses the filtergraph and then the option value. Escape each layer;
        // shell quoting is unnecessary because the filter is passed as one argv item.
        let option = value.chars().fold(String::new(), |mut escaped, character| {
            if matches!(character, '\\' | '\'' | ':') || character.is_ascii_whitespace() {
                escaped.push('\\');
            }
            escaped.push(character);
            escaped
        });
        option
            .chars()
            .fold(String::new(), |mut escaped, character| {
                if matches!(character, '\\' | '\'' | '[' | ']' | ',' | ';')
                    || character.is_ascii_whitespace()
                {
                    escaped.push('\\');
                }
                escaped.push(character);
                escaped
            })
    }

    fn make_subtitles_filter(ass_path: &str, fonts_dir: Option<&str>) -> String {
        // Use the `subtitles` filter (works with .ass and supports fontsdir).
        let filename = Self::escape_filter_path(ass_path);
        if let Some(fonts) = fonts_dir {
            let fonts = Self::escape_filter_path(fonts);
            format!("subtitles=filename={}:fontsdir={}", filename, fonts)
        } else {
            format!("subtitles=filename={}", filename)
        }
    }

    fn determine_output_path_for_video(
        input_path: &str,
        config: &AssBurnInConfig,
        output_override: Option<&str>,
    ) -> Result<String> {
        let input_abs = super::paths::lexical_absolute(input_path);

        if let Some(out) = output_override.filter(|s| !s.is_empty()) {
            let out_abs = super::paths::lexical_absolute(out);
            if super::paths::spelling_equal(&input_abs, &out_abs, super::paths::CasePolicy::Windows)
            {
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
        let candidate_abs = super::paths::lexical_absolute(&candidate);
        if super::paths::spelling_equal(
            &input_abs,
            &candidate_abs,
            super::paths::CasePolicy::Windows,
        ) {
            return Ok(parent
                .join(format!("{}_burnin2.{}", stem, ext))
                .to_string_lossy()
                .to_string());
        }
        Ok(candidate)
    }

    /// Subtitle candidates recorded for `video_path` by the session pairing:
    /// each danmu path of the video's segment with an `.ass` extension.
    fn manifest_ass_candidates(video_path: &str, manifest: &PipelineInputManifest) -> Vec<String> {
        manifest
            .danmu_for_video(video_path)
            .iter()
            .map(|danmu| {
                Path::new(danmu)
                    .with_extension("ass")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    /// Resolve the subtitle for one video. With session pairing the recorded
    /// candidate is looked for among the job's inputs first, which resolves a
    /// subtitle an upstream step relocated; then any input with the video's
    /// stem; then the candidate on disk. Inputs outrank a file next to the
    /// recording so a stale subtitle from an earlier run never wins over the
    /// one this job produced.
    async fn resolve_ass_for_video(
        video_path: &str,
        strategy: AssMatchStrategy,
        manifest: Option<&PipelineInputManifest>,
        inputs: &[String],
        ass_by_stem: &HashMap<String, String>,
    ) -> Result<Option<String>> {
        let candidates = match (strategy, manifest) {
            (AssMatchStrategy::Manifest, Some(manifest)) => {
                Self::manifest_ass_candidates(video_path, manifest)
            }
            _ => Vec::new(),
        };
        for candidate in &candidates {
            let candidate_abs = lexical_absolute(candidate);
            if let Some(input) = inputs.iter().find(|input| {
                spelling_equal(
                    &lexical_absolute(input),
                    &candidate_abs,
                    CasePolicy::Windows,
                )
            }) {
                return Ok(Some(input.clone()));
            }
        }
        if let Some(ass) = Self::match_ass_by_stem(video_path, ass_by_stem) {
            return Ok(Some(ass));
        }
        for candidate in candidates {
            if super::utils::try_exists(Path::new(&candidate)).await? {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// Key under which a subtitle is consumed once per job, so two videos with
    /// the same stem from different branches cannot both burn it.
    fn consumed_key(path: &str) -> String {
        let absolute = lexical_absolute(path);
        if cfg!(windows) {
            absolute.to_lowercase()
        } else {
            absolute
        }
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

    fn match_ass_by_stem(
        video_path: &str,
        ass_by_stem: &HashMap<String, String>,
    ) -> Option<String> {
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
        let plan = super::planning::OutputPlan::selected(&video_inputs, &input.outputs)
            .map_err(|_| crate::Error::PipelineError(format!("ass_burnin batch job requires outputs to be empty or have the same length as selected video inputs (videos={}, outputs={})", video_inputs.len(), input.outputs.len())))?;
        let output_paths = plan
            .items()
            .map(|(video, output)| Self::determine_output_path_for_video(video, &config, output))
            .collect::<Result<Vec<_>>>()?;

        let mut batch = OutputBatch::new(&input.inputs);
        for video in &video_inputs {
            batch.protect(Path::new(video));
        }
        let mut produced = Vec::new();
        let mut succeeded_inputs = Vec::new();
        let mut skipped_inputs = Vec::new();
        let mut matched_ass_for_succeeded = Vec::new();
        let mut consumed_ass = HashSet::<String>::new();
        let mut total_duration = 0.0;

        for (idx, video_path) in video_inputs.iter().enumerate() {
            let output_path = &output_paths[idx];

            if !super::utils::try_exists(Path::new(video_path)).await? {
                return Err(crate::Error::PipelineError(format!(
                    "Video input does not exist: {}",
                    video_path
                )));
            }

            let ass_path = Self::resolve_ass_for_video(
                video_path,
                config.match_strategy,
                input.manifest.as_deref(),
                &input.inputs,
                &ass_by_stem,
            )
            .await?;

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

            if !super::utils::try_exists(Path::new(&ass_path)).await? {
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

            // Only a subtitle that exists can be consumed: a missing match must
            // not mark later videos of the same recording as already burned.
            if !consumed_ass.insert(Self::consumed_key(&ass_path)) {
                let reason =
                    format!("subtitle {ass_path} was already burned into an earlier video");
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Warn,
                    format!("Skipping {video_path}: {reason}"),
                ));
                skipped_inputs.push((video_path.clone(), reason));
                continue;
            }

            if !config.overwrite && super::utils::try_exists(Path::new(output_path)).await? {
                return Err(crate::Error::PipelineError(format!(
                    "Output already exists and overwrite is disabled: {}",
                    output_path
                )));
            }

            batch.protect(Path::new(&ass_path));
            let temp_path = batch
                .stage(Path::new(output_path), config.overwrite)
                .await?;
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

            args.push(temp_path.to_string_lossy().into_owned());

            info!("Burning ASS into {} -> {}", video_path, output_path);
            debug!("FFmpeg args: {:?}", args);

            let mut cmd = Command::new(&ffmpeg);
            crate::utils::configure_ffmpeg_locale(&mut cmd);
            cmd.args(&args);

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

            output_size(&temp_path).await?;
            produced.push(output_path.clone());
            succeeded_inputs.push(video_path.clone());
            matched_ass_for_succeeded.push(ass_path);
        }

        batch.commit().await?;

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

    #[test]
    fn filter_values_escape_option_and_filtergraph_layers() {
        assert_eq!(AssBurnInProcessor::escape_filter_path("'"), r"\\\'");
        assert_eq!(AssBurnInProcessor::escape_filter_path(":"), r"\\:");
        assert_eq!(AssBurnInProcessor::escape_filter_path("[,]"), r"\[\,\]");
        assert_eq!(AssBurnInProcessor::escape_filter_path("\\"), r"\\\\");
    }

    #[tokio::test]
    #[ignore = "requires an installed FFmpeg binary with the subtitles filter"]
    async fn subtitle_filter_opens_paths_with_apostrophes_and_graph_delimiters() {
        let dir = tempfile::tempdir().unwrap();
        let subtitles = dir.path().join("Let's [play], now.srt");
        let fonts = dir.path().join("Font's [collection], now");
        tokio::fs::create_dir(&fonts).await.unwrap();
        tokio::fs::write(&subtitles, "1\n00:00:00,000 --> 00:00:01,000\nHello\n")
            .await
            .unwrap();
        let filter = AssBurnInProcessor::make_subtitles_filter(
            &subtitles.to_string_lossy(),
            Some(&fonts.to_string_lossy()),
        );
        let mut command = Command::new("ffmpeg");
        command
            .args([
                "-hide_banner",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=s=32x32:d=0.1",
                "-vf",
            ])
            .arg(filter)
            .args(["-frames:v", "1", "-f", "null", "-"]);
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            super::super::utils::run_command_with_logs(&mut command, None),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(output.status.success(), "{:?}", output.logs);
    }

    fn manifest(segments: &[(u32, &[&str], &[&str])]) -> PipelineInputManifest {
        use crate::pipeline::manifest::{ManifestScope, ManifestSegment};
        PipelineInputManifest::new(
            "session",
            "streamer",
            ManifestScope::Session,
            segments
                .iter()
                .map(|(index, video, danmu)| ManifestSegment {
                    segment_index: *index,
                    video: video.iter().map(|path| path.to_string()).collect(),
                    danmu: danmu.iter().map(|path| path.to_string()).collect(),
                })
                .collect(),
        )
    }

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    /// A segment without danmu leaves its own video unpaired and does not shift
    /// the pairs of later segments.
    #[tokio::test]
    async fn session_pairing_survives_a_missing_middle_danmu() {
        let temp = TempDir::new().unwrap();
        let path = |name: &str| temp.path().join(name).to_string_lossy().into_owned();
        let manifest = manifest(&[
            (0, &[&path("0.mp4")], &[&path("0.xml")]),
            (1, &[&path("1.mp4")], &[]),
            (2, &[&path("2.mp4")], &[&path("2.xml")]),
        ]);
        let inputs = strings(&[
            &path("0.mp4"),
            &path("1.mp4"),
            &path("2.mp4"),
            &path("0.ass"),
            &path("2.ass"),
        ]);
        let ass_by_stem = AssBurnInProcessor::build_ass_index(&inputs);
        for (video, expected) in [
            ("0.mp4", Some(path("0.ass"))),
            ("1.mp4", None),
            ("2.mp4", Some(path("2.ass"))),
        ] {
            let resolved = AssBurnInProcessor::resolve_ass_for_video(
                &path(video),
                AssMatchStrategy::Manifest,
                Some(&manifest),
                &inputs,
                &ass_by_stem,
            )
            .await
            .unwrap();
            assert_eq!(resolved, expected, "{video}");
        }
    }

    /// The `.ass` derived from the recorded danmu path may be absent when an
    /// upstream step wrote it elsewhere: a subtitle with the video's stem among
    /// the inputs is used, and it keeps winning over a stale file next to the
    /// danmu. Only when no input carries a subtitle is the file on disk used.
    #[tokio::test]
    async fn derived_ass_prefers_inputs_over_a_file_next_to_the_danmu() {
        let temp = TempDir::new().unwrap();
        let path = |name: &str| temp.path().join(name).to_string_lossy().into_owned();
        let manifest = manifest(&[(0, &[&path("rec.mp4")], &[&path("orig/rec.xml")])]);
        let with_ass = strings(&[&path("rec.mp4"), &path("out/rec.ass")]);
        let video_only = strings(&[&path("rec.mp4")]);
        async fn resolve(
            video: &str,
            inputs: &[String],
            manifest: Option<&PipelineInputManifest>,
        ) -> Option<String> {
            AssBurnInProcessor::resolve_ass_for_video(
                video,
                AssMatchStrategy::Manifest,
                manifest,
                inputs,
                &AssBurnInProcessor::build_ass_index(inputs),
            )
            .await
            .unwrap()
        }
        let video = path("rec.mp4");
        assert_eq!(
            resolve(&video, &with_ass, Some(&manifest)).await,
            Some(path("out/rec.ass"))
        );
        assert_eq!(resolve(&video, &video_only, Some(&manifest)).await, None);

        tokio::fs::create_dir(temp.path().join("orig"))
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("orig/rec.ass"), b"[Script Info]")
            .await
            .unwrap();
        assert_eq!(
            resolve(&video, &with_ass, Some(&manifest)).await,
            Some(path("out/rec.ass")),
            "a stale subtitle next to the danmu must not outrank the job's input"
        );
        assert_eq!(
            resolve(&video, &video_only, Some(&manifest)).await,
            Some(path("orig/rec.ass")),
            "with no subtitle among the inputs the recorded one on disk is used"
        );
        assert_eq!(
            resolve(&video, &with_ass, None).await,
            Some(path("out/rec.ass")),
            "without a pairing the stem index applies"
        );
    }

    /// Two videos from different branches that share a stem resolve to the same
    /// subtitle; only the first is burned and the second is skipped with a reason.
    /// A subtitle that is matched but missing on disk consumes nothing, so a
    /// later video of the same recording reports the missing file, not a burn
    /// that never happened.
    #[tokio::test]
    async fn duplicate_video_stems_consume_an_existing_subtitle_once() {
        let temp = TempDir::new().unwrap();
        let ffmpeg = super::super::output_tests::fake_success_ffmpeg(temp.path());
        let first = temp.path().join("a").join("rec.mp4");
        let second = temp.path().join("b").join("rec.mp4");
        let ass = temp.path().join("rec.ass");
        let lost_first = temp.path().join("a").join("lost.mp4");
        let lost_second = temp.path().join("b").join("lost.mp4");
        let lost_ass = temp.path().join("lost.ass");
        for video in [&first, &second, &lost_first, &lost_second] {
            tokio::fs::create_dir_all(video.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(video, b"video").await.unwrap();
        }
        tokio::fs::write(&ass, b"[Script Info]").await.unwrap();
        let input = ProcessorInput {
            inputs: strings(&[
                &lost_first.to_string_lossy(),
                &lost_second.to_string_lossy(),
                &first.to_string_lossy(),
                &second.to_string_lossy(),
                &ass.to_string_lossy(),
                &lost_ass.to_string_lossy(),
            ]),
            config: Some(
                serde_json::json!({
                    "ffmpeg_path": ffmpeg,
                    "match_strategy": "stem",
                    "passthrough_inputs": false,
                    "require_ass": false,
                })
                .to_string(),
            ),
            ..Default::default()
        };

        let output = AssBurnInProcessor::new()
            .process(&input, &ProcessorContext::noop("test"))
            .await
            .unwrap();
        assert_eq!(output.succeeded_inputs, vec![first.to_string_lossy()]);
        assert_eq!(
            output.outputs,
            vec![
                temp.path()
                    .join("a")
                    .join("rec_burnin.mp4")
                    .to_string_lossy()
            ]
        );
        assert_eq!(
            output
                .skipped_inputs
                .iter()
                .map(|(video, reason)| (video.clone(), reason.contains("already burned")))
                .collect::<Vec<_>>(),
            vec![
                (lost_first.to_string_lossy().into_owned(), false),
                (lost_second.to_string_lossy().into_owned(), false),
                (second.to_string_lossy().into_owned(), true),
            ]
        );
        assert!(!temp.path().join("b").join("rec_burnin.mp4").exists());
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
            "video.xml".to_string(),
            "video.mp4".to_string(),
            "subtitle.ass".to_string(),
        ];
        let mut exclude = HashSet::new();
        exclude.insert("video.mp4".to_string());

        let outputs = AssBurnInProcessor::build_passthrough_outputs(&inputs, true, &exclude);
        assert_eq!(outputs, vec!["video.xml"]);
    }
}
