use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProgressKind {
    Ffmpeg,
    Rclone,
    Compression,
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
}

impl ProgressReporter {
    pub fn new(job_id: impl Into<String>, tx: mpsc::Sender<JobProgressUpdate>) -> Self {
        Self {
            job_id: job_id.into(),
            tx,
        }
    }

    pub fn noop(job_id: impl Into<String>) -> Self {
        let (tx, _rx) = mpsc::channel::<JobProgressUpdate>(1);
        Self::new(job_id, tx)
    }

    pub fn report(&self, mut snapshot: JobProgressSnapshot) {
        snapshot.updated_at = Utc::now();
        let _ = self.tx.try_send(JobProgressUpdate {
            job_id: self.job_id.clone(),
            snapshot,
        });
    }
}
