//! `upload_records` table model.
//!
//! One row per (upload job, local file): the durable outcome of an upload
//! attempt. Rows are written only at terminal job transitions
//! (`JobQueue::complete` on success, worker-pool failure synthesis on error);
//! live upload state travels through `job_execution_progress` and the upload
//! WebSocket events instead.

use sqlx::FromRow;

/// One row from the `upload_records` table.
///
/// `(job_id, local_path)` is unique: `JobQueue::retry_job` reuses the job id,
/// so a retried upload upserts over the earlier FAILED rows rather than
/// accumulating duplicates.
#[derive(Debug, Clone, FromRow)]
pub struct UploadRecordDbModel {
    /// UUID v4.
    pub id: String,
    /// Producing job. NULL after the job row is pruned (`ON DELETE SET NULL`);
    /// the record itself is pruned separately by
    /// `MaintenanceRepository::prune_upload_records_before`.
    pub job_id: Option<String>,
    pub streamer_id: Option<String>,
    pub session_id: Option<String>,
    /// Producing processor kind (`"rclone"` today). Free-form so future
    /// uploaders reuse the table without a schema change.
    pub uploader: String,
    pub local_path: String,
    /// Expanded remote destination (e.g. `remote:bucket/streamer/file.mp4`).
    /// NULL when the destination could not be computed (failure synthesis).
    pub remote_path: Option<String>,
    /// Public HTTP URL for the uploaded file, derived by the processor
    /// (`RcloneConfig::public_url_mode`). NULL when no URL mode is
    /// configured or the derivation failed.
    pub public_url: Option<String>,
    /// One of [`upload_status::ALL`]; the table CHECK constraint enforces it.
    pub status: String,
    pub size_bytes: Option<i64>,
    pub error: Option<String>,
    /// Milliseconds since Unix epoch (UTC).
    pub created_at: i64,
    pub updated_at: i64,
    /// Set only for `status = 'COMPLETED'`.
    pub completed_at: Option<i64>,
}

/// Status discriminator values, kept in one place so the writer, repository,
/// API, and migration CHECK constraint cannot drift apart.
pub mod upload_status {
    pub const COMPLETED: &str = "COMPLETED";
    pub const FAILED: &str = "FAILED";
    pub const SKIPPED: &str = "SKIPPED";

    /// Every accepted status value, in the order the migration's CHECK lists
    /// them. Used by tests to assert every variant survives a round-trip.
    pub const ALL: &[&str] = &[COMPLETED, FAILED, SKIPPED];
}
