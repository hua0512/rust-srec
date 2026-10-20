use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum JobPhase {
    /// FFmpeg wrote the last frame and is rewriting the output, such as
    /// moving an MP4's index to the front for fast start, or rclone sent
    /// every byte and waits for the destination to confirm the files in
    /// flight.
    Finalizing,
    /// The processor waits to try its work again after a failed attempt,
    /// such as an rclone or BaiduPCS-Go upload.
    Retrying,
    /// An upload prepares its files before sending them: none of the files it
    /// is working on has sent data yet. Before sending a file BaiduPCS-Go
    /// checks the Netdisk and hashes the file without progress output, and
    /// rclone may hash it for the destination; before its first transfer
    /// rclone lists the destination and compares the files already there.
    Preparing,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct JobProgressSnapshot {
    pub kind: ProgressKind,
    pub updated_at: DateTime<Utc>,
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
    /// The processor's own attempt within one job run, from 1, such as an
    /// rclone or BaiduPCS-Go upload retry; unrelated to the job's
    /// `retry_count`. Absent until the processor retries.
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
            updated_at: Utc::now(),
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
pub(crate) fn progress_file_name(path: impl AsRef<Path>) -> Option<String> {
    path.as_ref()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// What a reporter adds to every report.
#[derive(Default)]
struct ReporterState {
    /// `(files_done, files_total)` of the batch item in progress.
    batch_item: Option<(u64, u64)>,
    /// File name of the batch item in progress.
    current_file: Option<String>,
    /// `(attempt, max_attempts)` of the processor's own retries.
    attempt: Option<(u32, u32)>,
}

#[derive(Clone)]
pub struct ProgressReporter {
    job_id: String,
    tx: mpsc::Sender<JobProgressUpdate>,
    /// Shared by every clone, so commands started from the job's context
    /// report it.
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
    /// `total`-item batch, the file at `path`. They carry the file position
    /// and, unless they name their own file, the item's file name. A reported
    /// percent covers the whole batch with each item as an equal share. A
    /// reported ETA covers only its item, so only the last item keeps it.
    pub fn set_batch_item(&self, index: usize, total: usize, path: impl AsRef<Path>) {
        let mut state = self.state.lock();
        state.batch_item = Some((index as u64, total as u64));
        state.current_file = progress_file_name(path);
    }

    /// Marks later reports as coming from attempt `attempt` (1-based) of at
    /// most `max_attempts` that the processor makes within this job run. The
    /// worker pool creates a reporter for each job run, so a job retry starts
    /// with no attempt.
    pub fn set_attempt(&self, attempt: u32, max_attempts: u32) {
        self.state.lock().attempt = Some((attempt, max_attempts));
    }

    pub fn report(&self, mut snapshot: JobProgressSnapshot) {
        snapshot.updated_at = Utc::now();
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn phase_is_serialized_by_name_and_omitted_when_absent() {
        let mut snapshot = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
        let plain = serde_json::to_value(&snapshot).unwrap();
        for absent in ["phase", "current_file", "attempt", "max_attempts"] {
            assert!(plain.get(absent).is_none(), "{absent} is serialized");
        }

        snapshot.phase = Some(JobPhase::Finalizing);
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap()["phase"],
            "finalizing"
        );
    }
}
