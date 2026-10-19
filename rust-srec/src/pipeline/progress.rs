use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProgressKind {
    Ffmpeg,
    Rclone,
    BaiduPcs,
    Compression,
    Copy,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            raw: serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobProgressUpdate {
    pub job_id: String,
    pub snapshot: JobProgressSnapshot,
}

#[derive(Clone)]
pub struct ProgressReporter {
    job_id: String,
    tx: mpsc::Sender<JobProgressUpdate>,
    /// `(files_done, files_total)` of the batch item in progress. Shared by
    /// every clone, so commands started from the job's context report it.
    batch_item: Arc<Mutex<Option<(u64, u64)>>>,
}

impl ProgressReporter {
    pub fn new(job_id: impl Into<String>, tx: mpsc::Sender<JobProgressUpdate>) -> Self {
        Self {
            job_id: job_id.into(),
            tx,
            batch_item: Arc::default(),
        }
    }

    pub fn noop(job_id: impl Into<String>) -> Self {
        let (tx, _rx) = mpsc::channel::<JobProgressUpdate>(1);
        Self::new(job_id, tx)
    }

    /// Marks later reports as coming from item `index` (zero-based) of a
    /// `total`-item batch: they carry the file position, and a reported
    /// percent covers the whole batch with each item as an equal share. A
    /// reported ETA covers only its item, so only the last item keeps it.
    pub fn set_batch_item(&self, index: usize, total: usize) {
        *self.batch_item.lock() = Some((index as u64, total as u64));
    }

    pub fn report(&self, mut snapshot: JobProgressSnapshot) {
        snapshot.updated_at = Utc::now();
        if let Some((done, total)) = *self.batch_item.lock() {
            snapshot.files_done = Some(done);
            snapshot.files_total = Some(total);
            snapshot.percent = snapshot
                .percent
                .map(|percent| (done as f32 * 100.0 + percent) / total as f32);
            if done + 1 < total {
                snapshot.eta_secs = None;
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

        reporter.clone().set_batch_item(1, 4);
        reporter.report(snapshot.clone());
        let batch = rx.try_recv().unwrap().snapshot;
        assert_eq!(
            (
                batch.files_done,
                batch.files_total,
                batch.percent,
                batch.eta_secs
            ),
            (Some(1), Some(4), Some(37.5), None)
        );

        reporter.set_batch_item(3, 4);
        reporter.report(snapshot);
        assert_eq!(rx.try_recv().unwrap().snapshot.eta_secs, Some(10.0));
    }
}
