//! Copy/Move processor for file operations.
//!
//! This processor handles copying and moving files to different locations
//! with directory creation et integrity verification.
//!
//! Supports placeholder expansion in destination paths:
//! - `{streamer}` - Streamer name
//! - `{title}` - Session title
//! - `{streamer_id}` - Streamer ID
//! - `{session_id}` - Session ID
//! - `{platform}` - Platform name
//! - Time placeholders: `%Y`, `%m`, `%d`, `%H`, `%M`, `%S`, etc.

use async_trait::async_trait;
use regex::RegexSet;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::fs;
use tracing::{debug, error, info};

use super::outputs::TempOutputGuard;
use super::traits::{
    Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType, TimeAnchor,
};
use super::utils::create_log_entry;
use crate::Result;
use crate::pipeline::{JobProgressSnapshot, ProgressKind, ProgressReporter};
use crate::utils::filename::{expand_placeholders, expand_placeholders_at};
use crate::utils::text::format_bytes;

mod native;

/// Default value for create_dirs option.
fn default_true() -> bool {
    true
}

/// Operation type for copy/move processor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CopyMoveOperation {
    /// Copy the file to destination (keeps original).
    #[default]
    Copy,
    /// Move the file to destination (removes original).
    Move,
}

/// Configuration for copy/move operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyMoveConfig {
    /// Operation type: "copy" or "move".
    #[serde(default)]
    pub operation: CopyMoveOperation,

    /// Destination directory for output files (supports placeholder expansion).
    ///
    /// This is required at runtime; the processor will return an error if it is missing.
    pub destination: Option<String>,

    /// Optional timestamp source for time placeholder expansion.
    ///
    /// `None` expands time placeholders using the local time at execution.
    #[serde(default)]
    pub time_anchor: Option<TimeAnchor>,

    /// Whether to create destination directories if they don't exist.
    #[serde(default = "default_true")]
    pub create_dirs: bool,

    /// Whether to verify file integrity after copy using size comparison.
    #[serde(default = "default_true")]
    pub verify_integrity: bool,

    /// Whether to overwrite existing files at destination.
    #[serde(default)]
    pub overwrite: bool,

    /// Regex patterns for excluding inputs.
    ///
    /// Patterns are matched against both the full input path and the filename.
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

impl Default for CopyMoveConfig {
    fn default() -> Self {
        Self {
            operation: CopyMoveOperation::Copy,
            destination: None,
            time_anchor: None,
            create_dirs: true,
            verify_integrity: true,
            overwrite: false,
            exclude_patterns: Vec::new(),
        }
    }
}

/// Minimum gap between copy progress reports. A finished copy always reports.
const COPY_PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// Byte progress for one file copy, throttled before it reaches the job's
/// progress channel, which drops reports rather than blocking the copy.
struct CopyProgress {
    reporter: ProgressReporter,
    bytes_total: u64,
    /// One-based position of this file among the job's inputs.
    file_index: usize,
    file_count: usize,
    file: String,
    started: Instant,
    last_report: Option<Instant>,
}

impl CopyProgress {
    fn new(
        reporter: ProgressReporter,
        bytes_total: u64,
        file_index: usize,
        file_count: usize,
        file: &str,
    ) -> Self {
        Self {
            reporter,
            bytes_total,
            file_index,
            file_count,
            file: file.to_owned(),
            started: Instant::now(),
            last_report: None,
        }
    }

    fn update(&mut self, bytes_done: u64) {
        self.update_at(bytes_done, Instant::now());
    }

    fn update_at(&mut self, bytes_done: u64, now: Instant) {
        if bytes_done < self.bytes_total
            && self
                .last_report
                .is_some_and(|last| now.saturating_duration_since(last) < COPY_PROGRESS_INTERVAL)
        {
            return;
        }
        self.last_report = Some(now);
        self.reporter
            .report(self.snapshot(bytes_done, now.saturating_duration_since(self.started)));
    }

    fn snapshot(&self, bytes_done: u64, elapsed: Duration) -> JobProgressSnapshot {
        let mut snapshot = JobProgressSnapshot::new(ProgressKind::Copy);
        snapshot.bytes_done = Some(bytes_done);
        snapshot.bytes_total = Some(self.bytes_total);
        // A source that grows during the copy is read to its end; keep the
        // percentage within bounds.
        snapshot.percent = Some(if self.bytes_total == 0 {
            100.0
        } else {
            (bytes_done.min(self.bytes_total) as f64 / self.bytes_total as f64 * 100.0) as f32
        });
        let seconds = elapsed.as_secs_f64();
        if seconds > 0.0 && bytes_done > 0 {
            let speed = bytes_done as f64 / seconds;
            snapshot.speed_bytes_per_sec = Some(speed);
            snapshot.eta_secs = Some(self.bytes_total.saturating_sub(bytes_done) as f64 / speed);
        }
        snapshot.raw = serde_json::json!({
            "file_index": self.file_index,
            "file_count": self.file_count,
            "file": self.file,
        });
        snapshot
    }
}

/// Processor for copying and moving files.
///
/// Handles file copy and move operations with:
/// - Directory creation
/// - Integrity verification via size comparison
/// - Disk space error reporting
pub struct CopyMoveProcessor;

impl CopyMoveProcessor {
    /// Create a new copy/move processor.
    pub fn new() -> Self {
        Self
    }

    /// Copy `source` to `dest` through a temporary sibling, verify the byte
    /// count, and rename the copy into place. Cancellation therefore leaves at
    /// most a `.tmp-*` file, never a partial file under the final destination
    /// name, which would block a retried move when `overwrite` is disabled. The
    /// blocking copy owns the temporary-file guard, so cleanup never races it.
    ///
    /// Returns the bytes copied, or a user-facing error message.
    async fn copy_via_tmp(
        source: &Path,
        dest: &Path,
        source_size: u64,
        config: &CopyMoveConfig,
        ctx: &ProcessorContext,
        progress: CopyProgress,
    ) -> std::result::Result<u64, String> {
        if ctx.cancellation_token.is_cancelled() {
            return Err("Copy cancelled".into());
        }
        let temp = TempOutputGuard::new(dest);
        let tmp_dest = temp.path().to_path_buf();
        let source = source.to_owned();
        let cancel = ctx.cancellation_token.child_token();
        let _cancel_on_drop = cancel.clone().drop_guard();
        // Workers drain the log collector before releasing their I/O admission.
        // Retaining its sender contains a cancelled native copy until it exits,
        // so a retry cannot exceed the configured number of I/O workers.
        let completion = ctx.log_sink.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _completion = completion;
            let mut staged_output = temp;
            // Error returns drop the guard before `_completion`, so a partial
            // copy is removed before the worker can release its I/O admission.
            staged_output.cleanup_on_current_thread = true;
            if cancel.is_cancelled() {
                return Err(std::io::Error::other("Copy cancelled"));
            }
            let mut progress = progress;
            let copied = native::copy_file(
                &source,
                staged_output.path(),
                || cancel.is_cancelled(),
                |copied| progress.update(copied),
            )?;
            if cancel.is_cancelled() {
                return Err(std::io::Error::other("Copy cancelled"));
            }
            staged_output.cleanup_on_current_thread = false;
            Ok::<_, std::io::Error>((copied, staged_output))
        })
        .await
        .map_err(|error| format!("Copy worker failed: {error}"))?;

        // Keep the staged output owned through verification and publication.
        let (bytes_copied, _temp) = match result {
            Ok(result) => result,
            Err(e) => {
                return Err(if e.kind() == ErrorKind::StorageFull {
                    format!(
                        "Insufficient disk space while copying. Required: {}",
                        format_bytes(source_size)
                    )
                } else {
                    format!("Failed to copy file: {}", e)
                });
            }
        };

        if ctx.cancellation_token.is_cancelled() {
            return Err("Copy cancelled".into());
        }

        if config.verify_integrity && bytes_copied != source_size {
            return Err(format!(
                "File integrity check failed. Source size: {}, Destination size: {}",
                source_size, bytes_copied
            ));
        }

        if let Err(e) = fs::rename(&tmp_dest, dest).await {
            // `rename` replaces an existing file on supported platforms; a
            // filesystem that reports a collision instead gets the same
            // `overwrite` handling as the direct-rename move path.
            let renamed = if e.kind() == ErrorKind::AlreadyExists && config.overwrite {
                match fs::remove_file(dest).await {
                    Ok(()) => fs::rename(&tmp_dest, dest).await,
                    Err(remove_err) => Err(remove_err),
                }
            } else {
                Err(e)
            };
            if let Err(e) = renamed {
                return Err(format!("Failed to move copy into place: {}", e));
            }
        }

        Ok(bytes_copied)
    }

    /// Whether `source_path` is excluded by the configured patterns, matched
    /// against the full path and the file name alike.
    fn is_excluded(exclude_set: Option<&RegexSet>, source_path: &str, filename: &str) -> bool {
        exclude_set.is_some_and(|set| set.is_match(source_path) || set.is_match(filename))
    }
}

impl Default for CopyMoveProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for CopyMoveProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Io
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["copy_move"]
    }

    fn name(&self) -> &'static str {
        "CopyMoveProcessor"
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

        // Initialize logs vector
        let mut logs = Vec::new();

        // Parse strictly (like RcloneProcessor) rather than via
        // parse_config_or_default: falling back to CopyMoveConfig::default()
        // on a malformed config clears `destination`, so the job would fail
        // anyway — but with a misleading "No destination directory specified"
        // error that hides the actual parse problem (e.g. a misspelled
        // `time_anchor` variant).
        let config: CopyMoveConfig = match input.config.as_deref() {
            Some(s) => serde_json::from_str(s).map_err(|e| {
                crate::Error::Validation(format!("Invalid copy/move config JSON: {e}"))
            })?,
            None => CopyMoveConfig::default(),
        };

        if input.inputs.is_empty() {
            return Err(crate::Error::PipelineError(
                "No input files specified for copy/move".to_string(),
            ));
        }

        // Get destination template from config
        let dest_template = config.destination.as_deref().ok_or_else(|| {
            crate::Error::PipelineError(
                "No destination directory specified for copy/move operation".to_string(),
            )
        })?;

        // Expand placeholders in destination path. `time_anchor: None`
        // uses the local time at execution via `expand_placeholders`.
        let dest_dir = if let Some(anchor) = config.time_anchor {
            expand_placeholders_at(
                dest_template,
                &input.streamer_id,
                &input.session_id,
                input.streamer_name.as_deref(),
                input.session_title.as_deref(),
                input.platform.as_deref(),
                Some(anchor.reference_time(input).timestamp_millis()),
            )
        } else {
            expand_placeholders(
                dest_template,
                &input.streamer_id,
                &input.session_id,
                input.streamer_name.as_deref(),
                input.session_title.as_deref(),
                input.platform.as_deref(),
            )
        };

        debug!(
            template = %dest_template,
            expanded = %dest_dir,
            time_anchor = ?config.time_anchor,
            "CopyMove: Placeholder expansion result"
        );

        let dest_dir_path = Path::new(&dest_dir);

        let exclude_set = if config.exclude_patterns.is_empty() {
            None
        } else {
            Some(RegexSet::new(&config.exclude_patterns).map_err(|e| {
                crate::Error::PipelineError(format!("Invalid exclude_patterns regex: {}", e))
            })?)
        };

        let dest_dir_exists = fs::try_exists(dest_dir_path).await.map_err(|e| {
            crate::Error::PipelineError(format!(
                "Failed to check destination directory '{}': {}",
                dest_dir, e
            ))
        })?;

        // Create destination directory if needed
        if !dest_dir_exists {
            if config.create_dirs {
                let log_msg = format!("Creating destination directory: {}", dest_dir);
                debug!("{}", log_msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Debug,
                    log_msg,
                ));

                crate::utils::fs::ensure_dir_all_with_op(
                    "creating destination directory",
                    dest_dir_path,
                )
                .await
                .map_err(|e| {
                    crate::Error::PipelineError(format!(
                        "Failed to create destination directory '{}': {}",
                        dest_dir, e
                    ))
                })?;
            } else {
                return Err(crate::Error::PipelineError(format!(
                    "Destination directory does not exist: {}",
                    dest_dir
                )));
            }
        }

        // Every input lands directly under the destination, so two inputs with
        // the same file name (fan-in from two branches) would silently replace
        // each other. Refuse the whole job before any transfer.
        let mut destinations: HashMap<std::path::PathBuf, &String> = HashMap::new();
        for source_path in &input.inputs {
            let Some(filename) = Path::new(source_path).file_name() else {
                continue;
            };
            if Self::is_excluded(
                exclude_set.as_ref(),
                source_path,
                &filename.to_string_lossy(),
            ) {
                continue;
            }
            let dest = dest_dir_path.join(filename);
            if let Some(earlier) = destinations.insert(dest.clone(), source_path) {
                return Err(crate::Error::PipelineError(format!(
                    "Inputs {} and {} would both be written to {}; rename one or send them to different destinations",
                    earlier,
                    source_path,
                    dest.display()
                )));
            }
        }

        let mut outputs = Vec::with_capacity(input.inputs.len());
        let mut items_produced = Vec::with_capacity(input.inputs.len());
        let mut succeeded_inputs = Vec::with_capacity(input.inputs.len());
        let mut failed_inputs: Vec<(String, String)> = Vec::new();
        let mut skipped_inputs: Vec<(String, String)> = Vec::new();
        let mut total_input_size: u64 = 0;
        let mut total_output_size: u64 = 0;

        for (file_index, source_path) in input.inputs.iter().enumerate() {
            if let Some(exclude_set) = &exclude_set
                && exclude_set.is_match(source_path)
            {
                let msg = format!("Skipping excluded input: {}", source_path);
                debug!("{}", msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Debug,
                    msg,
                ));
                skipped_inputs.push((source_path.clone(), "excluded".to_string()));
                continue;
            }

            let source = Path::new(source_path);

            // Get filename from source
            let Some(filename) = source.file_name() else {
                let error_msg = format!("Failed to get filename from source: {}", source_path);
                error!("{}", error_msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Error,
                    &error_msg,
                ));
                failed_inputs.push((source_path.clone(), error_msg));
                continue;
            };

            if let Some(exclude_set) = &exclude_set
                && exclude_set.is_match(&filename.to_string_lossy())
            {
                let msg = format!("Skipping excluded input: {}", source_path);
                debug!("{}", msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Debug,
                    msg,
                ));
                skipped_inputs.push((source_path.clone(), "excluded".to_string()));
                continue;
            }

            let dest = dest_dir_path.join(filename);

            let log_msg = format!(
                "{} {} -> {}",
                if config.operation == CopyMoveOperation::Copy {
                    "Copying"
                } else {
                    "Moving"
                },
                source_path,
                dest.display()
            );
            info!("{}", log_msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Info,
                log_msg,
            ));

            // Get source file size (also validates existence)
            let source_size = match fs::metadata(source).await {
                Ok(m) => m.len(),
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    // A retried move may reference a source consumed by an
                    // earlier attempt; a file already present at the
                    // destination confirms that move completed. This must
                    // run before the `overwrite` check below, which would
                    // otherwise reject the destination file as a collision.
                    //
                    // Gated on `ctx.is_retry`, which the worker loop sets only
                    // when this job has been attempted before: without an
                    // earlier attempt to resume, a same-named file at the
                    // destination belongs to something else, and adopting it
                    // would report a move that never happened.
                    if ctx.is_retry
                        && config.operation == CopyMoveOperation::Move
                        && let Ok(dest_meta) = fs::metadata(&dest).await
                        && dest_meta.is_file()
                    {
                        let msg = format!(
                            "Source already moved to {}; treating as completed",
                            dest.display()
                        );
                        info!("{}", msg);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Info,
                            msg,
                        ));
                        let dest_path = dest.to_string_lossy().into_owned();
                        total_input_size += dest_meta.len();
                        total_output_size += dest_meta.len();
                        outputs.push(dest_path.clone());
                        items_produced.push(dest_path);
                        succeeded_inputs.push(source_path.clone());
                        continue;
                    }

                    let error_msg = format!("Source file does not exist: {}", source_path);
                    error!("{}", error_msg);
                    logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Error,
                        &error_msg,
                    ));
                    failed_inputs.push((source_path.clone(), error_msg));
                    continue;
                }
                Err(e) => {
                    let error_msg = format!("Failed to get source file metadata: {}", e);
                    error!("{}", error_msg);
                    logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Error,
                        &error_msg,
                    ));
                    failed_inputs.push((source_path.clone(), error_msg));
                    continue;
                }
            };

            // A retry may find this input's transfer already complete: a copy whose
            // job failed on another input, or a cross-device move interrupted after
            // the copy but before the source was removed. Same-size destination
            // files are adopted so the retry resumes instead of rejecting them as
            // collisions; a first attempt never adopts, for the reason above.
            if ctx.is_retry
                && let Ok(dest_meta) = fs::metadata(&dest).await
                && dest_meta.is_file()
                && dest_meta.len() == source_size
            {
                // A destination that resolves to the input itself (the destination
                // template names the recording's own directory) is "in place": a
                // move must not remove the only copy. The identity check opens both
                // files, so an error leaves the input untouched for a later retry
                // rather than being read as "distinct" and deleting it.
                let identity = tokio::task::spawn_blocking({
                    let source = source.to_path_buf();
                    let dest = dest.clone();
                    move || same_file::is_same_file(&source, &dest)
                })
                .await;
                let in_place = match identity {
                    Ok(Ok(in_place)) => in_place,
                    Ok(Err(e)) => {
                        let error_msg = format!(
                            "Could not tell whether destination {} is the input itself: {}",
                            dest.display(),
                            e
                        );
                        error!("{}", error_msg);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Error,
                            &error_msg,
                        ));
                        failed_inputs.push((source_path.clone(), error_msg));
                        continue;
                    }
                    Err(e) => {
                        let error_msg = format!(
                            "Could not tell whether destination {} is the input itself: {}",
                            dest.display(),
                            e
                        );
                        error!("{}", error_msg);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Error,
                            &error_msg,
                        ));
                        failed_inputs.push((source_path.clone(), error_msg));
                        continue;
                    }
                };
                if config.operation == CopyMoveOperation::Move
                    && !in_place
                    && let Err(e) = fs::remove_file(source).await
                {
                    let error_msg = format!(
                        "Destination {} already holds this file but the source could not be removed: {}",
                        dest.display(),
                        e
                    );
                    error!("{}", error_msg);
                    logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Error,
                        &error_msg,
                    ));
                    failed_inputs.push((source_path.clone(), error_msg));
                    continue;
                }
                let msg = if in_place {
                    format!(
                        "Destination {} is the input itself; treating as completed without removing it",
                        dest.display()
                    )
                } else {
                    format!(
                        "Destination {} already holds this file from an earlier attempt; treating as completed",
                        dest.display()
                    )
                };
                info!("{}", msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Info,
                    msg,
                ));
                let dest_path = dest.to_string_lossy().into_owned();
                total_input_size += source_size;
                total_output_size += dest_meta.len();
                outputs.push(dest_path.clone());
                items_produced.push(dest_path);
                succeeded_inputs.push(source_path.clone());
                continue;
            }

            // Check if destination exists and handle overwrite
            if !config.overwrite {
                match fs::try_exists(&dest).await {
                    Ok(true) => {
                        let error_msg = format!(
                            "Destination file already exists and overwrite is disabled: {}",
                            dest.display()
                        );
                        error!("{}", error_msg);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Error,
                            &error_msg,
                        ));
                        failed_inputs.push((source_path.clone(), error_msg));
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        let error_msg =
                            format!("Failed to check destination file existence: {}", e);
                        error!("{}", error_msg);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Error,
                            &error_msg,
                        ));
                        failed_inputs.push((source_path.clone(), error_msg));
                        continue;
                    }
                }
            }

            // Same-filesystem moves are renames and report no progress.
            let progress = CopyProgress::new(
                ctx.progress.clone(),
                source_size,
                file_index + 1,
                input.inputs.len(),
                source_path,
            );
            let dest_size = match config.operation {
                CopyMoveOperation::Copy => {
                    match Self::copy_via_tmp(source, &dest, source_size, &config, ctx, progress)
                        .await
                    {
                        Ok(bytes) => bytes,
                        Err(error_msg) => {
                            error!("{}", error_msg);
                            logs.push(create_log_entry(
                                crate::pipeline::job_queue::LogLevel::Error,
                                &error_msg,
                            ));
                            failed_inputs.push((source_path.clone(), error_msg));
                            continue;
                        }
                    }
                }
                CopyMoveOperation::Move => {
                    // First try a cheap rename (fast-path on the same filesystem).
                    match fs::rename(source, &dest).await {
                        Ok(()) => source_size,
                        Err(e) if e.kind() == ErrorKind::AlreadyExists && config.overwrite => {
                            // A filesystem that reports a collision instead of replacing.
                            if let Err(e) = fs::remove_file(&dest).await {
                                let error_msg = format!(
                                    "Failed to remove destination file for overwrite: {}",
                                    e
                                );
                                error!("{}", error_msg);
                                logs.push(create_log_entry(
                                    crate::pipeline::job_queue::LogLevel::Error,
                                    &error_msg,
                                ));
                                failed_inputs.push((source_path.clone(), error_msg));
                                continue;
                            }

                            if let Err(e) = fs::rename(source, &dest).await {
                                let error_msg = format!("Failed to move file: {}", e);
                                error!("{}", error_msg);
                                logs.push(create_log_entry(
                                    crate::pipeline::job_queue::LogLevel::Error,
                                    &error_msg,
                                ));
                                failed_inputs.push((source_path.clone(), error_msg));
                                continue;
                            }

                            source_size
                        }
                        Err(e) if e.kind() == ErrorKind::CrossesDevices => {
                            // Cross-filesystem move: fall back to copy + verify,
                            // then remove the source.
                            let bytes_copied = match Self::copy_via_tmp(
                                source,
                                &dest,
                                source_size,
                                &config,
                                ctx,
                                progress,
                            )
                            .await
                            {
                                Ok(bytes) => bytes,
                                Err(error_msg) => {
                                    error!("{}", error_msg);
                                    logs.push(create_log_entry(
                                        crate::pipeline::job_queue::LogLevel::Error,
                                        &error_msg,
                                    ));
                                    failed_inputs.push((source_path.clone(), error_msg));
                                    continue;
                                }
                            };

                            if let Err(e) = fs::remove_file(source).await {
                                let error_msg =
                                    format!("Failed to remove source file after move: {}", e);
                                error!("{}", error_msg);
                                logs.push(create_log_entry(
                                    crate::pipeline::job_queue::LogLevel::Error,
                                    error_msg,
                                ));
                                // Don't mark as failed since copy succeeded
                            }

                            bytes_copied
                        }
                        Err(e) => {
                            let error_msg = format!("Failed to move file: {}", e);
                            error!("{}", error_msg);
                            logs.push(create_log_entry(
                                crate::pipeline::job_queue::LogLevel::Error,
                                &error_msg,
                            ));
                            failed_inputs.push((source_path.clone(), error_msg));
                            continue;
                        }
                    }
                }
            };

            let dest_path = dest.to_string_lossy().into_owned();

            total_input_size += source_size;
            total_output_size += dest_size;
            outputs.push(dest_path.clone());
            items_produced.push(dest_path);
            succeeded_inputs.push(source_path.clone());
        }

        let duration = start.elapsed().as_secs_f64();

        let success_msg = format!(
            "{} completed in {:.2}s: {} inputs ({} succeeded, {} failed, {} skipped)",
            if config.operation == CopyMoveOperation::Copy {
                "Copy"
            } else {
                "Move"
            },
            duration,
            input.inputs.len(),
            succeeded_inputs.len(),
            failed_inputs.len(),
            skipped_inputs.len()
        );
        info!("{}", success_msg);
        logs.push(create_log_entry(
            crate::pipeline::job_queue::LogLevel::Info,
            success_msg,
        ));

        // If all inputs failed, return error
        if succeeded_inputs.is_empty() && !failed_inputs.is_empty() {
            return Err(crate::Error::PipelineError(format!(
                "All {} input files failed to copy/move",
                failed_inputs.len()
            )));
        }

        Ok(ProcessorOutput {
            outputs,
            duration_secs: duration,
            metadata: Some(
                serde_json::json!({
                    "operation": format!("{:?}", config.operation),
                    "destination_dir": dest_dir,
                    "total_files": input.inputs.len(),
                    "succeeded": succeeded_inputs.len(),
                    "failed": failed_inputs.len(),
                    "skipped": skipped_inputs.len(),
                    "total_size_bytes": total_output_size,
                })
                .to_string(),
            ),
            items_produced,
            input_size_bytes: Some(total_input_size),
            output_size_bytes: Some(total_output_size),
            failed_inputs,
            succeeded_inputs,
            skipped_inputs,
            uploads: vec![],
            logs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::utc_datetime;
    use super::*;
    use tempfile::TempDir;

    fn no_progress() -> CopyProgress {
        CopyProgress::new(ProgressReporter::noop("copy"), 0, 1, 1, "source")
    }

    fn progress_channel(
        bytes_total: u64,
    ) -> (
        CopyProgress,
        tokio::sync::mpsc::Receiver<crate::pipeline::progress::JobProgressUpdate>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let progress = CopyProgress::new(
            ProgressReporter::new("copy", tx),
            bytes_total,
            1,
            1,
            "source",
        );
        (progress, rx)
    }

    #[test]
    fn copy_progress_snapshots_carry_bytes_rate_and_file_position() {
        let (mut progress, _rx) = progress_channel(1000);
        progress.file_index = 2;
        progress.file_count = 4;

        let snapshot = progress.snapshot(250, Duration::from_secs(5));
        assert_eq!(snapshot.kind, ProgressKind::Copy);
        assert_eq!(snapshot.bytes_done, Some(250));
        assert_eq!(snapshot.bytes_total, Some(1000));
        assert_eq!(snapshot.percent, Some(25.0));
        assert_eq!(snapshot.speed_bytes_per_sec, Some(50.0));
        assert_eq!(snapshot.eta_secs, Some(15.0));
        assert_eq!(
            snapshot.raw,
            serde_json::json!({"file_index": 2, "file_count": 4, "file": "source"})
        );

        // No elapsed time or no bytes yet gives no rate rather than infinity.
        let starting = progress.snapshot(0, Duration::from_secs(1));
        assert_eq!(
            (starting.speed_bytes_per_sec, starting.eta_secs),
            (None, None)
        );
        assert_eq!(
            progress.snapshot(10, Duration::ZERO).speed_bytes_per_sec,
            None
        );
        // A source that grew past its starting size stays at 100%.
        assert_eq!(
            progress.snapshot(1200, Duration::from_secs(1)).percent,
            Some(100.0)
        );

        let (empty, _rx) = progress_channel(0);
        assert_eq!(empty.snapshot(0, Duration::ZERO).percent, Some(100.0));
    }

    #[test]
    fn copy_progress_is_throttled_but_always_reports_the_finished_copy() {
        let (mut progress, mut rx) = progress_channel(100);
        let start = progress.started;
        let mut reported = || {
            let mut bytes = Vec::new();
            while let Ok(update) = rx.try_recv() {
                bytes.push(update.snapshot.bytes_done.unwrap());
            }
            bytes
        };

        progress.update_at(0, start);
        progress.update_at(10, start + Duration::from_millis(100));
        progress.update_at(20, start + COPY_PROGRESS_INTERVAL);
        progress.update_at(
            30,
            start + COPY_PROGRESS_INTERVAL + Duration::from_millis(1),
        );
        // The finished copy reports even inside the interval.
        progress.update_at(
            100,
            start + COPY_PROGRESS_INTERVAL + Duration::from_millis(2),
        );

        assert_eq!(reported(), vec![0, 20, 100]);
    }

    #[test]
    fn cancelled_copy_retains_worker_completion_until_blocking_work_exits() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        std::fs::write(&source, b"contents").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();

        runtime.block_on(async {
            let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
            let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
            let blocker = tokio::task::spawn_blocking(move || {
                started_tx.send(()).unwrap();
                // Bound the helper even if an assertion fails before release.
                release_rx.recv_timeout(std::time::Duration::from_secs(5))
            });
            started_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(1);
            let ctx = ProcessorContext::new(
                "copy-completion",
                crate::pipeline::ProgressReporter::noop("copy-completion"),
                crate::pipeline::processors::JobLogSink::new(
                    log_tx,
                    std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                ),
                tokio_util::sync::CancellationToken::new(),
            );
            let config = CopyMoveConfig::default();
            let mut copying = Box::pin(CopyMoveProcessor::copy_via_tmp(
                &source,
                &destination,
                8,
                &config,
                &ctx,
                no_progress(),
            ));
            assert!(futures::poll!(&mut copying).is_pending());
            drop(copying);
            drop(ctx);

            // The worker's collector must still wait for the queued copy, even
            // though the processor future and its context have been dropped.
            assert!(matches!(
                log_rx.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ));
            release_tx.send(()).unwrap();
            blocker.await.unwrap().unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(5), log_rx.recv())
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(!destination.exists());
            assert_eq!(std::fs::read(&source).unwrap(), b"contents");
        });
    }

    #[tokio::test]
    async fn already_cancelled_copy_does_not_create_or_replace_a_destination() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::write(&source, b"new contents").await.unwrap();
        fs::write(&destination, b"original contents").await.unwrap();
        let ctx = ProcessorContext::noop("cancelled-copy");
        ctx.cancellation_token.cancel();

        let result = CopyMoveProcessor::copy_via_tmp(
            &source,
            &destination,
            12,
            &CopyMoveConfig {
                overwrite: true,
                ..Default::default()
            },
            &ctx,
            no_progress(),
        )
        .await;

        assert_eq!(result.unwrap_err(), "Copy cancelled");
        assert_eq!(fs::read(&destination).await.unwrap(), b"original contents");
        assert_eq!(fs::read(&source).await.unwrap(), b"new contents");
        let mut entries = fs::read_dir(directory.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            assert!(!entry.file_name().to_string_lossy().contains(".tmp-"));
        }
    }

    #[test]
    fn test_copy_move_processor_type() {
        let processor = CopyMoveProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Io);
    }

    #[test]
    fn test_copy_move_processor_job_types() {
        let processor = CopyMoveProcessor::new();
        assert!(processor.can_process("copy_move"));
        assert!(!processor.can_process("copy"));
        assert!(!processor.can_process("move"));
        assert!(!processor.can_process("remux"));
    }

    #[test]
    fn test_copy_move_processor_name() {
        let processor = CopyMoveProcessor::new();
        assert_eq!(processor.name(), "CopyMoveProcessor");
    }

    #[test]
    fn test_copy_move_config_parse() {
        let json = r#"{
            "operation": "move",
            "destination": "/dest/file.mp4",
            "create_dirs": true,
            "verify_integrity": false,
            "overwrite": true,
            "exclude_patterns": ["\\.tmp$"]
        }"#;

        let config: CopyMoveConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.operation, CopyMoveOperation::Move);
        assert_eq!(config.destination, Some("/dest/file.mp4".to_string()));
        assert!(config.create_dirs);
        assert!(!config.verify_integrity);
        assert!(config.overwrite);
        assert_eq!(config.exclude_patterns, vec!["\\.tmp$"]);
        assert_eq!(config.time_anchor, None);
    }

    #[test]
    fn test_copy_move_config_parse_time_anchor() {
        let config: CopyMoveConfig =
            serde_json::from_str(r#"{"destination": "/dest", "time_anchor": "session_start"}"#)
                .unwrap();
        assert_eq!(config.time_anchor, Some(TimeAnchor::SessionStart));
    }

    /// A malformed config must fail with the parse error itself, not fall
    /// back to defaults and then error about the (discarded) destination.
    #[tokio::test]
    async fn test_copy_move_invalid_config_fails_with_parse_error() {
        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec!["/input.flv".to_string()],
            config: Some(r#"{"destination": "/dest", "time_anchor": "session-start"}"#.to_string()),
            ..Default::default()
        };

        let err = processor.process(&input, &ctx).await.unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("Invalid copy/move config JSON"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("unknown variant"),
            "unexpected error: {message}"
        );
    }

    #[tokio::test]
    async fn test_copy_operation() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_dir = temp_dir.path().join("output");

        // Create source file
        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify copy succeeded
        let dest_path = dest_dir.join("source.txt");
        assert!(dest_path.exists());
        assert!(source_path.exists()); // Source should still exist
        assert_eq!(
            output.outputs,
            vec![dest_path.to_string_lossy().to_string()]
        );
        assert!(output.input_size_bytes.is_some());
        assert!(output.output_size_bytes.is_some());
        assert_eq!(output.input_size_bytes, output.output_size_bytes);
    }

    #[tokio::test]
    async fn test_exclude_patterns_skip_inputs() {
        let temp_dir = TempDir::new().unwrap();
        let keep_path = temp_dir.path().join("keep.txt");
        let skip_path = temp_dir.path().join("skip.tmp");
        let dest_dir = temp_dir.path().join("output");

        fs::write(&keep_path, "keep").await.unwrap();
        fs::write(&skip_path, "skip").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![
                keep_path.to_string_lossy().to_string(),
                skip_path.to_string_lossy().to_string(),
            ],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy(),
                    "exclude_patterns": [r"\.tmp$"],
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        let keep_dest = dest_dir.join("keep.txt");
        let skip_dest = dest_dir.join("skip.tmp");
        assert!(keep_dest.exists());
        assert!(!skip_dest.exists());
        assert_eq!(
            output.outputs,
            vec![keep_dest.to_string_lossy().to_string()]
        );
        assert!(output.failed_inputs.is_empty());
        assert_eq!(
            output.succeeded_inputs,
            vec![keep_path.to_string_lossy().to_string()]
        );
        assert_eq!(
            output.skipped_inputs,
            vec![(
                skip_path.to_string_lossy().to_string(),
                "excluded".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn test_copy_move_session_start_anchor_expands_destination_date() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_root = temp_dir.path().join("output");
        fs::write(&source_path, "test content").await.unwrap();

        let session_start = utc_datetime(2024, 1, 1, 12, 0, 0);
        let created_at = utc_datetime(2024, 1, 2, 12, 0, 0);
        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": format!("{}/%Y/%m/%d", dest_root.to_string_lossy()),
                    "time_anchor": "session_start",
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "session-1".to_string(),
            session_start: Some(session_start),
            created_at,
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        let expected_dest = dest_root
            .join(pipeline_common::expand_path_template_at(
                "%Y/%m/%d",
                Some(session_start.timestamp_millis()),
            ))
            .join("source.txt");
        assert!(expected_dest.exists());
        assert_eq!(output.outputs.len(), 1);
        assert_eq!(
            fs::canonicalize(&output.outputs[0]).await.unwrap(),
            fs::canonicalize(expected_dest).await.unwrap()
        );
    }

    #[tokio::test]
    async fn test_exclude_patterns_invalid_regex_fails() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_dir = temp_dir.path().join("output");

        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy(),
                    "exclude_patterns": ["("],
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let err = processor.process(&input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("Invalid exclude_patterns regex"));
    }

    #[tokio::test]
    async fn test_move_operation() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_dir = temp_dir.path().join("output");

        // Create source file
        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "move",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify move succeeded
        let dest_path = dest_dir.join("source.txt");
        assert!(dest_path.exists());
        assert!(!source_path.exists()); // Source should be removed
        assert_eq!(
            output.outputs,
            vec![dest_path.to_string_lossy().to_string()]
        );
    }

    #[tokio::test]
    async fn test_create_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_dir = temp_dir.path().join("subdir1/subdir2");

        // Create source file
        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy(),
                    "create_dirs": true
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify directories were created and copy succeeded
        let dest_path = dest_dir.join("source.txt");
        assert!(dest_path.exists());
        assert!(dest_dir.exists());
        assert_eq!(
            output.outputs,
            vec![dest_path.to_string_lossy().to_string()]
        );
    }

    #[tokio::test]
    async fn test_overwrite_disabled_by_default() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_dir = temp_dir.path();

        // Create source and destination files
        fs::write(&source_path, "source content").await.unwrap();
        // Destination file has same name as source
        fs::write(dest_dir.join("source.txt"), "existing content")
            .await
            .unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            // `overwrite` is omitted: clobbering an existing destination must
            // require an explicit opt-in, so `CopyMoveConfig`'s serde default
            // has to be `false`. `test_overwrite_enabled` covers the opt-in.
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy(),
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;

        // In batch mode, single file failure results in partial success
        // Since this is the only file, it should fail completely
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("failed to copy/move"));
    }

    #[tokio::test]
    async fn test_overwrite_enabled() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_dir = temp_dir.path().join("output");

        // Create dest dir and existing file
        fs::create_dir_all(&dest_dir).await.unwrap();
        fs::write(dest_dir.join("source.txt"), "old content")
            .await
            .unwrap();

        // Create source file
        fs::write(&source_path, "new content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy(),
                    "overwrite": true
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify overwrite succeeded
        let dest_path = dest_dir.join("source.txt");
        assert!(dest_path.exists());
        let content = fs::read_to_string(&dest_path).await.unwrap();
        assert_eq!(content, "new content");
        assert_eq!(
            output.outputs,
            vec![dest_path.to_string_lossy().to_string()]
        );
    }

    #[tokio::test]
    async fn test_source_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("nonexistent.txt");
        let dest_dir = temp_dir.path().join("output");

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;

        // Should fail because source doesn't exist (single file = total failure)
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("failed to copy/move"));
    }

    #[tokio::test]
    async fn test_no_input_file() {
        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "destination": "/dest"
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;

        // Should fail because no input file specified
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("No input file"));
    }

    #[tokio::test]
    async fn test_no_destination() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        fs::write(&source_path, "test").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;

        // Should fail because no destination specified
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("No destination"));
    }

    #[tokio::test]
    async fn test_placeholder_expansion() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("video.mp4");

        // Create source file
        fs::write(&source_path, "test video content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");

        // Use placeholders in destination using platform-native path separator
        let dest_template = temp_dir
            .path()
            .join("{platform}")
            .join("{streamer}")
            .join("{title}")
            .to_string_lossy()
            .to_string();

        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_template
                })
                .to_string(),
            ),
            streamer_id: "streamer123".to_string(),
            session_id: "session456".to_string(),
            streamer_name: Some("TestStreamer".to_string()),
            session_title: Some("LiveStream".to_string()),
            platform: Some("Twitch".to_string()),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify placeholders were expanded - check via output existence
        assert_eq!(output.outputs.len(), 1);
        assert!(output.outputs[0].contains("Twitch"));
        assert!(output.outputs[0].contains("TestStreamer"));
        assert!(output.outputs[0].contains("LiveStream"));
        assert!(output.outputs[0].contains("video.mp4"));

        // Verify file actually exists at output path
        let output_path = Path::new(&output.outputs[0]);
        assert!(output_path.exists());
    }

    #[tokio::test]
    async fn test_batch_copy() {
        let temp_dir = TempDir::new().unwrap();
        let source1 = temp_dir.path().join("file1.txt");
        let source2 = temp_dir.path().join("file2.txt");
        let dest_dir = temp_dir.path().join("output");

        // Create source files
        fs::write(&source1, "content 1").await.unwrap();
        fs::write(&source2, "content 2").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![
                source1.to_string_lossy().to_string(),
                source2.to_string_lossy().to_string(),
            ],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify both files copied
        assert!(dest_dir.join("file1.txt").exists());
        assert!(dest_dir.join("file2.txt").exists());
        assert_eq!(output.outputs.len(), 2);
        assert_eq!(output.succeeded_inputs.len(), 2);
        assert!(output.failed_inputs.is_empty());
    }

    #[tokio::test]
    async fn test_move_retry_treats_absent_source_with_destination_as_moved() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("moved.txt");
        let dest_dir = temp_dir.path().join("output");
        // Simulates a retried job: the earlier attempt already moved the
        // source, so only the destination file exists.
        fs::create_dir_all(&dest_dir).await.unwrap();
        fs::write(dest_dir.join("moved.txt"), "content")
            .await
            .unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test").with_retry(true);
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            config: Some(
                serde_json::json!({
                    "operation": "move",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        assert_eq!(
            output.succeeded_inputs,
            vec![source_path.to_string_lossy().to_string()]
        );
        assert!(output.failed_inputs.is_empty());
        assert_eq!(
            output.outputs,
            vec![dest_dir.join("moved.txt").to_string_lossy().to_string()]
        );
    }

    /// A retried copy or move whose destination already holds a same-size file
    /// resumes past it instead of failing on the overwrite check; a move also
    /// finishes by removing the source it left behind. A first attempt keeps
    /// rejecting the collision.
    #[tokio::test]
    async fn test_retry_adopts_a_complete_destination_for_copy_and_move() {
        for operation in ["copy", "move"] {
            let temp_dir = TempDir::new().unwrap();
            let source_path = temp_dir.path().join("done.txt");
            let pending_path = temp_dir.path().join("pending.txt");
            let dest_dir = temp_dir.path().join("output");
            fs::create_dir_all(&dest_dir).await.unwrap();
            fs::write(&source_path, "content").await.unwrap();
            fs::write(&pending_path, "later").await.unwrap();
            fs::write(dest_dir.join("done.txt"), "content")
                .await
                .unwrap();
            let input = ProcessorInput {
                inputs: vec![
                    source_path.to_string_lossy().to_string(),
                    pending_path.to_string_lossy().to_string(),
                ],
                config: Some(
                    serde_json::json!({
                        "operation": operation,
                        "destination": dest_dir.to_string_lossy()
                    })
                    .to_string(),
                ),
                ..Default::default()
            };
            let processor = CopyMoveProcessor::new();

            let first_attempt = processor
                .process(&input, &ProcessorContext::noop("test"))
                .await
                .unwrap();
            assert_eq!(first_attempt.failed_inputs.len(), 1, "{operation}");
            assert!(
                first_attempt.failed_inputs[0]
                    .1
                    .contains("overwrite is disabled")
            );
            assert_eq!(
                first_attempt.succeeded_inputs,
                vec![pending_path.to_string_lossy().to_string()]
            );

            fs::write(&pending_path, "later").await.unwrap();
            let retry = processor
                .process(&input, &ProcessorContext::noop("test").with_retry(true))
                .await
                .unwrap();
            assert!(
                retry.failed_inputs.is_empty(),
                "{operation}: {:?}",
                retry.failed_inputs
            );
            assert_eq!(retry.succeeded_inputs, input.inputs, "{operation}");
            assert_eq!(
                retry.outputs,
                vec![
                    dest_dir.join("done.txt").to_string_lossy().to_string(),
                    dest_dir.join("pending.txt").to_string_lossy().to_string(),
                ]
            );
            assert_eq!(
                fs::read_to_string(dest_dir.join("done.txt")).await.unwrap(),
                "content"
            );
            assert_eq!(
                source_path.exists(),
                operation == "copy",
                "{operation}: a move removes the source it left behind"
            );
        }
    }

    /// Two inputs that share a file name would overwrite each other under the
    /// destination; the job is refused before anything is transferred.
    #[tokio::test]
    async fn test_duplicate_destinations_are_refused_before_any_transfer() {
        let temp_dir = TempDir::new().unwrap();
        let first = temp_dir.path().join("a").join("rec.mp4");
        let second = temp_dir.path().join("b").join("rec.mp4");
        let excluded = temp_dir.path().join("c").join("rec.mp4");
        for path in [&first, &second, &excluded] {
            fs::create_dir_all(path.parent().unwrap()).await.unwrap();
            fs::write(path, "content").await.unwrap();
        }
        let dest_dir = temp_dir.path().join("output");
        let config = |exclude: &str| {
            serde_json::json!({
                "operation": "move",
                "destination": dest_dir.to_string_lossy(),
                "exclude_patterns": [exclude],
            })
            .to_string()
        };
        let processor = CopyMoveProcessor::new();

        let error = processor
            .process(
                &ProcessorInput {
                    inputs: vec![
                        first.to_string_lossy().to_string(),
                        second.to_string_lossy().to_string(),
                    ],
                    config: Some(config("never-matches")),
                    ..Default::default()
                },
                &ProcessorContext::noop("test"),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("would both be written to"),
            "{error}"
        );
        assert!(first.exists() && second.exists(), "nothing was moved");
        assert!(!dest_dir.join("rec.mp4").exists());

        // An excluded input does not count as a collision. The pattern accepts
        // either separator so the directory matches on Windows too.
        let output = processor
            .process(
                &ProcessorInput {
                    inputs: vec![
                        first.to_string_lossy().to_string(),
                        excluded.to_string_lossy().to_string(),
                    ],
                    config: Some(config(r"[\\/]c[\\/]")),
                    ..Default::default()
                },
                &ProcessorContext::noop("test"),
            )
            .await
            .unwrap();
        assert_eq!(output.succeeded_inputs, vec![first.to_string_lossy()]);
        assert_eq!(output.skipped_inputs.len(), 1);
        assert!(dest_dir.join("rec.mp4").exists());
    }

    /// A destination template that resolves to the input's own directory makes
    /// source and destination the same file; a retried move must keep it.
    #[tokio::test]
    async fn test_retry_of_an_in_place_move_keeps_the_only_copy() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("rec.mp4");
        fs::write(&source_path, "content").await.unwrap();
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            config: Some(
                serde_json::json!({
                    "operation": "move",
                    "destination": temp_dir.path().to_string_lossy()
                })
                .to_string(),
            ),
            ..Default::default()
        };
        let processor = CopyMoveProcessor::new();

        let first_attempt = processor
            .process(&input, &ProcessorContext::noop("test"))
            .await
            .unwrap_err();
        assert!(
            first_attempt.to_string().contains("failed to copy/move"),
            "the first attempt rejects the collision: {first_attempt}"
        );
        assert_eq!(fs::read_to_string(&source_path).await.unwrap(), "content");

        let retry = processor
            .process(&input, &ProcessorContext::noop("test").with_retry(true))
            .await
            .unwrap();
        assert!(retry.failed_inputs.is_empty(), "{:?}", retry.failed_inputs);
        assert_eq!(retry.outputs, input.inputs);
        assert_eq!(
            fs::read_to_string(&source_path).await.unwrap(),
            "content",
            "the recording is never removed"
        );
    }

    /// Without an earlier attempt to resume, a same-named file at the
    /// destination belongs to something else and must not be adopted as this
    /// move's output.
    #[tokio::test]
    async fn test_first_attempt_move_does_not_adopt_unrelated_destination_file() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("moved.txt");
        let dest_dir = temp_dir.path().join("output");
        fs::create_dir_all(&dest_dir).await.unwrap();
        fs::write(dest_dir.join("moved.txt"), "unrelated")
            .await
            .unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            config: Some(
                serde_json::json!({
                    "operation": "move",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            ..Default::default()
        };

        let err = processor.process(&input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("failed to copy/move"));
    }

    #[tokio::test]
    async fn test_move_fails_when_source_and_destination_are_both_missing() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("missing.txt");
        let dest_dir = temp_dir.path().join("output");
        fs::create_dir_all(&dest_dir).await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            config: Some(
                serde_json::json!({
                    "operation": "move",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            ..Default::default()
        };

        let err = processor.process(&input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("failed to copy/move"));
    }

    /// The absent-source resume only applies to moves: a copy never
    /// consumes its source, so a missing source is a real error even when
    /// a same-named destination file exists.
    #[tokio::test]
    async fn test_copy_missing_source_with_existing_destination_still_fails() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("missing.txt");
        let dest_dir = temp_dir.path().join("output");
        fs::create_dir_all(&dest_dir).await.unwrap();
        fs::write(dest_dir.join("missing.txt"), "unrelated")
            .await
            .unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            ..Default::default()
        };

        let err = processor.process(&input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("failed to copy/move"));
    }

    #[tokio::test]
    async fn test_move_retry_resumes_mixed_batch() {
        let temp_dir = TempDir::new().unwrap();
        let already_moved = temp_dir.path().join("already-moved.txt");
        let pending = temp_dir.path().join("pending.txt");
        let dest_dir = temp_dir.path().join("output");
        fs::create_dir_all(&dest_dir).await.unwrap();
        fs::write(dest_dir.join("already-moved.txt"), "moved earlier")
            .await
            .unwrap();
        fs::write(&pending, "still local").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test").with_retry(true);
        let input = ProcessorInput {
            inputs: vec![
                already_moved.to_string_lossy().to_string(),
                pending.to_string_lossy().to_string(),
            ],
            config: Some(
                serde_json::json!({
                    "operation": "move",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        assert_eq!(output.succeeded_inputs.len(), 2);
        assert!(output.failed_inputs.is_empty());
        assert!(!pending.exists());
        assert!(dest_dir.join("pending.txt").exists());
        assert!(dest_dir.join("already-moved.txt").exists());
    }

    #[tokio::test]
    async fn test_copy_via_tmp_creates_destination_without_leaving_tmp() {
        let temp_dir = TempDir::new().unwrap();
        let source = temp_dir.path().join("source.txt");
        let dest = temp_dir.path().join("dest.txt");
        fs::write(&source, "content").await.unwrap();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(16);
        let progress = CopyProgress::new(
            ProgressReporter::new("copy", progress_tx),
            7,
            2,
            3,
            &source.to_string_lossy(),
        );

        let bytes = CopyMoveProcessor::copy_via_tmp(
            &source,
            &dest,
            7,
            &CopyMoveConfig::default(),
            &ProcessorContext::noop("copy"),
            progress,
        )
        .await
        .unwrap();

        assert_eq!(bytes, 7);
        // Every platform reports the finished copy, whatever came before it.
        let mut last = None;
        while let Ok(update) = progress_rx.try_recv() {
            last = Some(update.snapshot);
        }
        let last = last.expect("a final copy progress report");
        assert_eq!(last.kind, ProgressKind::Copy);
        assert_eq!(
            (last.bytes_done, last.bytes_total, last.percent),
            (Some(7), Some(7), Some(100.0))
        );
        assert_eq!(
            last.raw,
            serde_json::json!({"file_index": 2, "file_count": 3, "file": source.to_string_lossy()})
        );
        assert_eq!(fs::read_to_string(&dest).await.unwrap(), "content");
        let mut entries = fs::read_dir(temp_dir.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            assert!(
                !entry.file_name().to_string_lossy().contains(".tmp-"),
                "temporary copy left behind: {:?}",
                entry.file_name()
            );
        }
    }
}
