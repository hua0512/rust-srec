//! `staged_outputs` table model.
//!
//! One row per temporary file a pipeline attempt staged before publishing an
//! output. Rows of a live attempt are bookkeeping only; rows a restart found
//! still present carry `interrupted_at` and are the leftovers users can review.

use sqlx::FromRow;

/// One row from the `staged_outputs` table.
#[derive(Debug, Clone, FromRow)]
pub struct StagedOutputDbModel {
    /// UUID v4.
    pub id: String,
    /// The temporary file, a sibling of `output_path`.
    pub path: String,
    /// The output the temporary file would have been published as.
    pub output_path: String,
    /// Producing job. Not a foreign key: the job can be pruned first.
    pub job_id: String,
    /// The attempt that staged the file; finalizing it removes its rows.
    pub attempt_id: String,
    /// Milliseconds since Unix epoch (UTC).
    pub created_at: i64,
    /// When startup recovery found the row left by an earlier process.
    pub interrupted_at: Option<i64>,
}
