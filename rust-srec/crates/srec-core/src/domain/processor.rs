//! Shared processor and backpressure contracts.
//!
//! Collects the storage-free types and traits exchanged between pipeline
//! orchestration, processors, downloaders, and external upload tools:
//!
//! - [`DownloadLimitAdjuster`] — backpressure port implemented by the download
//!   manager without coupling orchestration to the downloader module.
//! - [`LogLevel`], [`JobLogEntry`] — structured execution log entries.
//! - [`ManifestScope`], [`ManifestSegment`], [`PipelineInputManifest`] —
//!   per-segment video/danmu pairing manifest.
//! - [`ProgressKind`], [`JobPhase`], [`JobProgressSnapshot`],
//!   [`JobProgressUpdate`], [`ProgressReporter`] — job progress reporting.
//! - [`temp_path_for`], [`is_temp_path_for`], [`remove_leftover`],
//!   [`StagedOutputSink`], [`StagedOutputTracker`] — staged temporary file
//!   tracking decoupled from SQLite persistence.
//! - [`LoginMaterial`], [`BaiduPcsAuthenticator`] — stored BaiduPCS-Go
//!   authentication contract.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use jiff::Timestamp;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::warn;

pub use crate::domain::filename::expand_placeholders_at;
use crate::domain::proxy::ProxyTarget;

// ============================================================================
// Backpressure port
// ============================================================================

/// Applies a temporary reduction while preserving the configured download limit.
pub trait DownloadLimitAdjuster: Send + Sync {
    /// Apply a factor, or release the reduction with `None`. Return the
    /// configured and effective normal-slot limits from the same serialized
    /// update.
    fn set_throttle_factor(&self, factor: Option<f32>) -> (usize, usize);
}

// ============================================================================
// Execution logs
// ============================================================================

/// Log level for job execution logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Uppercase name stored in the `job_execution_logs.level` column and returned by the API.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

/// A single log entry for job execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobLogEntry {
    /// Timestamp of the log entry.
    pub timestamp: Timestamp,
    /// Log level.
    pub level: LogLevel,
    /// Log message.
    pub message: String,
    // Shared by clones in streamed and returned logs; equality of timestamps or text does
    // not identify an occurrence. Owned by the entries so retained snapshots stay bounded
    // without a queue-wide history, even after their database rows have been trimmed.
    #[serde(skip)]
    pub persisted_for: Arc<Mutex<HashSet<Arc<str>>>>,
}

impl JobLogEntry {
    /// Create a new log entry with the current timestamp.
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            timestamp: Timestamp::now(),
            level,
            message: message.into(),
            persisted_for: Arc::default(),
        }
    }

    pub fn mark_persisted(&self, job_id: Arc<str>) {
        self.persisted_for.lock().insert(job_id);
    }

    /// Create an info log entry.
    pub fn info(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Info, message)
    }

    /// Create a warning log entry.
    pub fn warn(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Warn, message)
    }

    /// Create an error log entry.
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Error, message)
    }

    /// Create a debug log entry.
    pub fn debug(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Debug, message)
    }
}

// ============================================================================
// Pipeline input manifest
// ============================================================================

/// Which dispatch produced the manifest.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManifestScope {
    /// Session-complete pipeline: every segment of the session.
    Session,
    /// Paired-segment pipeline for one segment.
    Segment { index: u32 },
}

/// Inputs one segment contributed, in the order the coordinator collected them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSegment {
    pub segment_index: u32,
    pub video: Vec<String>,
    pub danmu: Vec<String>,
}

/// Per-segment pairing of the video and danmu inputs handed to a paired-segment
/// or session-complete pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineInputManifest {
    /// Schema version of the stored JSON; readers reject nothing today.
    pub version: u32,
    pub session_id: String,
    pub streamer_id: String,
    pub scope: ManifestScope,
    /// Ascending by `segment_index`.
    pub segments: Vec<ManifestSegment>,
}

impl PipelineInputManifest {
    pub const VERSION: u32 = 1;

    /// Segments are sorted by index so flattened views follow recording order
    /// regardless of the order the caller collected them.
    pub fn new(
        session_id: impl Into<String>,
        streamer_id: impl Into<String>,
        scope: ManifestScope,
        mut segments: Vec<ManifestSegment>,
    ) -> Self {
        segments.sort_by_key(|segment| segment.segment_index);
        Self {
            version: Self::VERSION,
            session_id: session_id.into(),
            streamer_id: streamer_id.into(),
            scope,
            segments,
        }
    }

    /// Every video path in segment order.
    pub fn video_inputs(&self) -> impl Iterator<Item = &str> {
        self.segments
            .iter()
            .flat_map(|segment| segment.video.iter().map(String::as_str))
    }

    /// Every danmu path in segment order.
    pub fn danmu_inputs(&self) -> impl Iterator<Item = &str> {
        self.segments
            .iter()
            .flat_map(|segment| segment.danmu.iter().map(String::as_str))
    }

    /// Danmu paths recorded for the segment that produced `video`.
    ///
    /// The segment is found by exact spelling first, then by file stem so a
    /// relocated or remuxed copy of the video still resolves. Returns an empty
    /// slice when no segment lists the video.
    pub fn danmu_for_video(&self, video: &str) -> &[String] {
        if let Some(segment) = self
            .segments
            .iter()
            .find(|segment| segment.video.iter().any(|candidate| candidate == video))
        {
            return &segment.danmu;
        }
        let Some(stem) = file_stem(video) else {
            return &[];
        };
        self.segments
            .iter()
            .find(|segment| {
                segment
                    .video
                    .iter()
                    .any(|candidate| file_stem(candidate).is_some_and(|other| other == stem))
            })
            .map(|segment| segment.danmu.as_slice())
            .unwrap_or(&[])
    }

    /// Every `(video, danmu)` combination within a segment, in segment order.
    /// A segment with videos but no danmu yields nothing.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.segments.iter().flat_map(|segment| {
            segment.video.iter().flat_map(move |video| {
                segment
                    .danmu
                    .iter()
                    .map(move |danmu| (video.as_str(), danmu.as_str()))
            })
        })
    }

    pub fn is_empty(&self) -> bool {
        self.segments
            .iter()
            .all(|segment| segment.video.is_empty() && segment.danmu.is_empty())
    }
}

fn file_stem(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
}

// ============================================================================
// Progress reporting
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgressKind {
    Ffmpeg,
    Rclone,
    BaiduPcs,
    Compression,
    Copy,
    Danmaku,
}

/// A stage of a job that is not plain work towards its percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobPhase {
    Finalizing,
    Retrying,
    Preparing,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobProgressSnapshot {
    pub kind: ProgressKind,
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_done: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_bytes_per_sec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_time_ms: Option<u64>,
    /// Media length in milliseconds that the FFmpeg run writes; the total for `out_time_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// FFmpeg processing speed as a multiple of real time, such as 2.3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_x: Option<f64>,
    /// Files finished so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_done: Option<u64>,
    /// Files the job counts towards its progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_total: Option<u64>,
    /// Absent while the job does plain work towards `percent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<JobPhase>,
    /// Name of the file in progress, without its directories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    /// The processor's own attempt within one job run, from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// Attempts the processor makes at most within this job run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub raw: serde_json::Value,
}

impl JobProgressSnapshot {
    pub fn new(kind: ProgressKind) -> Self {
        Self {
            kind,
            updated_at: Timestamp::now(),
            percent: None,
            bytes_done: None,
            bytes_total: None,
            speed_bytes_per_sec: None,
            eta_secs: None,
            out_time_ms: None,
            duration_ms: None,
            speed_x: None,
            files_done: None,
            files_total: None,
            phase: None,
            current_file: None,
            attempt: None,
            max_attempts: None,
            raw: serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobProgressUpdate {
    pub job_id: String,
    pub snapshot: JobProgressSnapshot,
}

/// Name of the file at `path`, without its directories.
pub fn progress_file_name(path: impl AsRef<Path>) -> Option<String> {
    path.as_ref()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

#[derive(Default)]
struct ReporterState {
    batch_item: Option<(u64, u64)>,
    current_file: Option<String>,
    attempt: Option<(u32, u32)>,
}

#[derive(Clone)]
pub struct ProgressReporter {
    job_id: String,
    tx: mpsc::Sender<JobProgressUpdate>,
    state: Arc<Mutex<ReporterState>>,
}

impl ProgressReporter {
    pub fn new(job_id: impl Into<String>, tx: mpsc::Sender<JobProgressUpdate>) -> Self {
        Self {
            job_id: job_id.into(),
            tx,
            state: Arc::default(),
        }
    }

    pub fn noop(job_id: impl Into<String>) -> Self {
        let (tx, _rx) = mpsc::channel::<JobProgressUpdate>(1);
        Self::new(job_id, tx)
    }

    /// Marks later reports as coming from item `index` (zero-based) of a
    /// `total`-item batch, the file at `path`.
    pub fn set_batch_item(&self, index: usize, total: usize, path: impl AsRef<Path>) {
        let mut state = self.state.lock();
        state.batch_item = Some((index as u64, total as u64));
        state.current_file = progress_file_name(path);
    }

    /// Marks later reports as coming from attempt `attempt` (1-based) of at
    /// most `max_attempts` that the processor makes within this job run.
    pub fn set_attempt(&self, attempt: u32, max_attempts: u32) {
        self.state.lock().attempt = Some((attempt, max_attempts));
    }

    pub fn report(&self, mut snapshot: JobProgressSnapshot) {
        snapshot.updated_at = Timestamp::now();
        {
            let state = self.state.lock();
            if let Some((done, total)) = state.batch_item {
                snapshot.files_done = Some(done);
                snapshot.files_total = Some(total);
                snapshot.percent = snapshot
                    .percent
                    .map(|percent| (done as f32 * 100.0 + percent) / total as f32);
                if done + 1 < total {
                    snapshot.eta_secs = None;
                }
            }
            if snapshot.current_file.is_none() {
                snapshot.current_file.clone_from(&state.current_file);
            }
            if let Some((attempt, max_attempts)) = state.attempt {
                snapshot.attempt = Some(attempt);
                snapshot.max_attempts = Some(max_attempts);
            }
        }
        let _ = self.tx.try_send(JobProgressUpdate {
            job_id: self.job_id.clone(),
            snapshot,
        });
    }
}

// ============================================================================
// Staged temporary files contract
// ============================================================================

const TEMP_MARKER: &str = ".tmp-";

/// The temporary sibling a processor writes before publishing `output`.
pub fn temp_path_for(output: &Path) -> PathBuf {
    let mut name = output
        .file_stem()
        .map(std::ffi::OsString::from)
        .unwrap_or_default();
    name.push(format!("{TEMP_MARKER}{}", uuid::Uuid::new_v4()));
    if let Some(extension) = output.extension() {
        name.push(".");
        name.push(extension);
    }
    output.with_file_name(name)
}

/// Whether `path` has exactly the shape [`temp_path_for`] gives `output`.
pub fn is_temp_path_for(path: &Path, output: &Path) -> bool {
    if path.parent() != output.parent() {
        return false;
    }
    let (Some(name), Some(stem)) = (
        path.file_name().and_then(|name| name.to_str()),
        output.file_stem().and_then(|stem| stem.to_str()),
    ) else {
        return false;
    };
    let suffix = match output.extension() {
        Some(extension) => match extension.to_str() {
            Some(extension) => format!(".{extension}"),
            None => return false,
        },
        None => String::new(),
    };
    name.strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix(TEMP_MARKER))
        .and_then(|rest| rest.strip_suffix(suffix.as_str()))
        .is_some_and(|id| id.len() == 36 && uuid::Uuid::try_parse(id).is_ok())
}

/// Delete a leftover temporary file. A file that is already gone counts as
/// deleted; anything other than a regular file is refused.
pub fn remove_leftover(path: &Path) -> std::io::Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        #[cfg(windows)]
        Err(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied
                && metadata.permissions().readonly() =>
        {
            let mut permissions = metadata.permissions();
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "Windows-only attribute reset on a file that is about to be deleted"
            )]
            permissions.set_readonly(false);
            std::fs::set_permissions(path, permissions)?;
            std::fs::remove_file(path).map(|()| true)
        }
        Err(error) => Err(error),
    }
}

/// Storage-free sink for recording staged temporary outputs before creation
/// and forgetting them when an attempt finishes.
#[async_trait]
pub trait StagedOutputSink: Send + Sync {
    /// Record `temp` before the caller creates it.
    async fn record(&self, temp: &Path, output: &Path);

    /// Forget this attempt's staged paths once the worker finalizes it.
    async fn finish(&self);
}

/// Cloneable handle carried in processor contexts; defaults to a no-op sink.
#[derive(Clone, Default)]
pub struct StagedOutputTracker {
    sink: Option<Arc<dyn StagedOutputSink>>,
}

impl StagedOutputTracker {
    pub fn new(sink: Arc<dyn StagedOutputSink>) -> Self {
        Self { sink: Some(sink) }
    }

    pub async fn record(&self, temp: &Path, output: &Path) {
        if let Some(sink) = &self.sink {
            sink.record(temp, output).await;
        }
    }

    pub async fn finish(&self) {
        if let Some(sink) = &self.sink {
            sink.finish().await;
        }
    }
}

// ============================================================================
// BaiduPCS-Go authenticator contract
// ============================================================================

/// Login material in one of the two forms `BaiduPCS-Go login` accepts:
/// a full cookie string, or BDUSS with an optional STOKEN.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoginMaterial {
    pub cookies: Option<String>,
    pub bduss: Option<String>,
    pub stoken: Option<String>,
}

impl LoginMaterial {
    fn field(value: Option<&String>) -> Option<&str> {
        value.map(|v| v.trim()).filter(|v| !v.is_empty())
    }

    /// True when at least one accepted login form is present.
    pub fn is_usable(&self) -> bool {
        Self::field(self.cookies.as_ref()).is_some() || Self::field(self.bduss.as_ref()).is_some()
    }

    /// Command input for the supported no-argument REPL. Never pass this to process argv.
    pub fn login_input(&self) -> crate::CoreResult<Vec<u8>> {
        fn quoted(value: &str) -> crate::CoreResult<String> {
            if value.contains(['\r', '\n', '\0']) {
                return Err(crate::CoreError::validation(
                    "BaiduPCS-Go credentials must not contain line breaks or NUL",
                ));
            }
            Ok(format!(
                "\"{}\"",
                value.replace('\\', "\\\\").replace('\"', "\\\"")
            ))
        }
        let command = if let Some(cookies) = Self::field(self.cookies.as_ref()) {
            format!("login -cookies={}\nquit\n", quoted(cookies)?)
        } else {
            let bduss = Self::field(self.bduss.as_ref()).ok_or_else(|| {
                crate::CoreError::validation("No usable BaiduPCS-Go login material")
            })?;
            let mut command = format!("login -bduss={}", quoted(bduss)?);
            if let Some(stoken) = Self::field(self.stoken.as_ref()) {
                command.push_str(&format!(" -stoken={}", quoted(stoken)?));
            }
            command.push_str("\nquit\n");
            command
        };
        Ok(command.into_bytes())
    }

    /// Values that output scrubbing must mask out of any relayed CLI output.
    pub fn secrets(&self) -> Vec<&str> {
        [&self.cookies, &self.bduss, &self.stoken]
            .into_iter()
            .filter_map(|value| Self::field(value.as_ref()))
            .collect()
    }

    /// Parse a stored credential payload JSON blob. A malformed payload reads
    /// as "no usable material" rather than an error, so a hand-edited row only
    /// disables auto re-login.
    pub fn from_payload(payload: &str) -> Option<Self> {
        match serde_json::from_str::<Self>(payload) {
            Ok(material) if material.is_usable() => Some(material),
            Ok(_) => None,
            Err(e) => {
                warn!(error = %e, "Stored BaiduPCS-Go credential payload is not readable");
                None
            }
        }
    }
}

/// Stored-credential operations shared by API routes (save on remembered
/// login, delete on logout, `has_stored_credentials`) and `BaiduPcsProcessor`
/// (automatic re-login when a session is logged out or an upload failed).
#[async_trait]
pub trait BaiduPcsAuthenticator: Send + Sync {
    /// Usable stored material exists for this config dir. Lookup errors
    /// degrade to `false` so a DB hiccup only skips auto re-login instead
    /// of failing the caller.
    async fn has_credentials(&self, config_dir: Option<&str>) -> bool;

    /// Persist login material for this config dir, replacing any earlier row.
    async fn save_credentials(
        &self,
        config_dir: Option<&str>,
        material: &LoginMaterial,
    ) -> crate::CoreResult<()>;

    /// Forget the stored material for this config dir (no-op when absent).
    async fn delete_credentials(&self, config_dir: Option<&str>) -> crate::CoreResult<()>;

    /// Replay the stored material through `BaiduPCS-Go login`. Returns
    /// `Ok(false)` when nothing usable is stored or the CLI rejected the
    /// material.
    async fn relogin(
        &self,
        binary_path: &str,
        config_dir: Option<&str>,
        proxy: &ProxyTarget,
    ) -> crate::CoreResult<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PipelineInputManifest {
        PipelineInputManifest::new(
            "session",
            "streamer",
            ManifestScope::Session,
            vec![
                ManifestSegment {
                    segment_index: 2,
                    video: vec!["/rec/part2.mp4".to_owned(), "/rec/part2b.mp4".to_owned()],
                    danmu: vec!["/rec/part2.xml".to_owned()],
                },
                ManifestSegment {
                    segment_index: 0,
                    video: vec!["/rec/part0.mp4".to_owned()],
                    danmu: vec!["/rec/part0.xml".to_owned()],
                },
                ManifestSegment {
                    segment_index: 1,
                    video: vec!["/rec/part1.mp4".to_owned()],
                    danmu: Vec::new(),
                },
            ],
        )
    }

    #[test]
    fn serde_round_trip_keeps_scope_and_segment_order() {
        let manifest = manifest();
        assert_eq!(
            manifest
                .segments
                .iter()
                .map(|segment| segment.segment_index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        let json = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<PipelineInputManifest>(&json).unwrap(),
            manifest
        );

        let segment =
            PipelineInputManifest::new("s", "t", ManifestScope::Segment { index: 7 }, Vec::new());
        let value = serde_json::to_value(&segment).unwrap();
        assert_eq!(
            value["scope"],
            serde_json::json!({ "segment": { "index": 7 } })
        );
        assert_eq!(value["version"], PipelineInputManifest::VERSION);
        assert!(segment.is_empty());
        assert!(!manifest.is_empty());
    }

    #[test]
    fn danmu_for_video_matches_by_spelling_then_stem() {
        let manifest = manifest();
        assert_eq!(
            manifest.danmu_for_video("/rec/part2b.mp4"),
            &["/rec/part2.xml".to_owned()]
        );
        assert_eq!(
            manifest.danmu_for_video("/archive/PART0.flv"),
            &["/rec/part0.xml".to_owned()]
        );
        assert!(manifest.danmu_for_video("/rec/part1.mp4").is_empty());
        assert!(manifest.danmu_for_video("/rec/unknown.mp4").is_empty());
    }

    #[test]
    fn pairs_never_cross_segments() {
        let manifest = manifest();
        assert_eq!(
            manifest.pairs().collect::<Vec<_>>(),
            vec![
                ("/rec/part0.mp4", "/rec/part0.xml"),
                ("/rec/part2.mp4", "/rec/part2.xml"),
                ("/rec/part2b.mp4", "/rec/part2.xml"),
            ]
        );
    }

    #[test]
    fn batch_items_carry_their_position_and_a_job_wide_percent() {
        let (tx, mut rx) = mpsc::channel(4);
        let reporter = ProgressReporter::new("job", tx);
        let mut snapshot = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
        snapshot.percent = Some(50.0);
        snapshot.eta_secs = Some(10.0);

        reporter.clone().set_batch_item(1, 4, "/rec/a/seg_002.flv");
        reporter.report(snapshot.clone());
        let batch = rx.try_recv().unwrap().snapshot;
        assert_eq!(
            (
                batch.files_done,
                batch.files_total,
                batch.percent,
                batch.eta_secs,
                batch.current_file.as_deref()
            ),
            (Some(1), Some(4), Some(37.5), None, Some("seg_002.flv"))
        );

        reporter.set_batch_item(3, 4, "/rec/a/seg_004.flv");
        reporter.report(snapshot);
        assert_eq!(rx.try_recv().unwrap().snapshot.eta_secs, Some(10.0));
    }

    #[test]
    fn temp_paths_match_only_their_own_output() {
        let output = Path::new("/videos/stream.part1.mp4");
        let temp = temp_path_for(output);
        assert!(is_temp_path_for(&temp, output));
        assert!(!is_temp_path_for(&temp, Path::new("/videos/other.mp4")));
        assert!(!is_temp_path_for(
            &temp,
            Path::new("/elsewhere/stream.part1.mp4")
        ));
        assert!(!is_temp_path_for(output, output));

        let bare = Path::new("/videos/stream");
        assert!(is_temp_path_for(&temp_path_for(bare), bare));

        for name in [
            "stream.part1.tmp-not-a-uuid.mp4",
            "stream.part1.tmp-6d1c3d5e-8a3b-4c41-9d2e-1f0a9b7c2e11.mkv",
            "stream.part1.tmp-6d1c3d5e8a3b4c419d2e1f0a9b7c2e11.mp4",
            "stream.part1.mp4",
        ] {
            assert!(
                !is_temp_path_for(&Path::new("/videos").join(name), output),
                "{name}"
            );
        }
        assert!(is_temp_path_for(
            &Path::new("/videos").join("stream.part1.tmp-6d1c3d5e-8a3b-4c41-9d2e-1f0a9b7c2e11.mp4"),
            output
        ));
    }

    #[cfg(windows)]
    #[test]
    fn removing_a_read_only_leftover_clears_the_attribute() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("a.tmp-x.mp4");
        std::fs::write(&file, b"partial").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();

        assert!(remove_leftover(&file).unwrap());
        assert!(!file.exists());
    }

    #[test]
    fn login_material_builds_quoted_repl_input_and_refuses_line_breaks() {
        let material = LoginMaterial {
            cookies: Some("BDUSS=a; STOKEN=B".to_string()),
            bduss: Some("ignored".to_string()),
            stoken: Some("ignored".to_string()),
        };
        assert!(material.is_usable());
        assert_eq!(
            String::from_utf8(material.login_input().unwrap()).unwrap(),
            "login -cookies=\"BDUSS=a; STOKEN=B\"\nquit\n"
        );

        let material = LoginMaterial {
            cookies: None,
            bduss: Some("bd".to_string()),
            stoken: Some("ST".to_string()),
        };
        assert_eq!(
            String::from_utf8(material.login_input().unwrap()).unwrap(),
            "login -bduss=\"bd\" -stoken=\"ST\"\nquit\n"
        );
        assert_eq!(material.secrets(), vec!["bd", "ST"]);

        let empty = LoginMaterial {
            cookies: Some("  ".to_string()),
            bduss: None,
            stoken: Some("orphan-stoken".to_string()),
        };
        assert!(!empty.is_usable());
        assert!(empty.login_input().is_err());

        // The input is a script for BaiduPCS-Go's REPL on stdin: quotes and
        // backslashes stay inside the quoted value, and a value that could end
        // the line and start another command is refused.
        let quoting = LoginMaterial {
            cookies: Some(r#"BDUSS="a\b"#.to_string()),
            ..LoginMaterial::default()
        };
        assert_eq!(
            String::from_utf8(quoting.login_input().unwrap()).unwrap(),
            "login -cookies=\"BDUSS=\\\"a\\\\b\"\nquit\n"
        );
        for material in [
            LoginMaterial {
                cookies: Some("BDUSS=a\nquit".to_string()),
                ..LoginMaterial::default()
            },
            LoginMaterial {
                bduss: Some("bd\rquit".to_string()),
                ..LoginMaterial::default()
            },
            LoginMaterial {
                bduss: Some("bd".to_string()),
                stoken: Some("ST\0quit".to_string()),
                ..LoginMaterial::default()
            },
        ] {
            assert!(material.login_input().is_err(), "{material:?}");
        }
    }
}
