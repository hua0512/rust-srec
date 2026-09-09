//! Complete job row writes. Callers own retries, timestamps and transactions.

use sqlx::SqliteConnection;

use crate::Result;
use crate::database::models::{JobDbModel, JobStatus};

const INSERT: &str = r#"
    INSERT INTO job (
        id, job_type, status, config, state, created_at, updated_at,
        input, outputs, priority, streamer_id, session_id,
        started_at, completed_at, error, retry_count,
        pipeline_id, execution_info, duration_secs, queue_wait_secs, dag_step_execution_id
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

const UPDATE: &str = r#"
    UPDATE job SET
        job_type = ?, status = ?, config = ?, state = ?, updated_at = ?,
        input = ?, outputs = ?, priority = ?, streamer_id = ?, session_id = ?,
        started_at = ?, completed_at = ?, error = ?, retry_count = ?,
        pipeline_id = ?, execution_info = ?, duration_secs = ?, queue_wait_secs = ?,
        dag_step_execution_id = ?
    WHERE id = ? AND (? IS NULL OR status = ?)
"#;

pub(crate) enum UpdateCondition {
    Any,
    Status(JobStatus),
}

pub(crate) async fn insert_job(connection: &mut SqliteConnection, job: &JobDbModel) -> Result<()> {
    sqlx::query(INSERT)
        .bind(&job.id)
        .bind(&job.job_type)
        .bind(&job.status)
        .bind(&job.config)
        .bind(&job.state)
        .bind(job.created_at)
        .bind(job.updated_at)
        .bind(&job.input)
        .bind(&job.outputs)
        .bind(job.priority)
        .bind(&job.streamer_id)
        .bind(&job.session_id)
        .bind(job.started_at)
        .bind(job.completed_at)
        .bind(&job.error)
        .bind(job.retry_count)
        .bind(&job.pipeline_id)
        .bind(&job.execution_info)
        .bind(job.duration_secs)
        .bind(job.queue_wait_secs)
        .bind(&job.dag_step_execution_id)
        .execute(connection)
        .await?;
    Ok(())
}

pub(crate) async fn update_job(
    connection: &mut SqliteConnection,
    job: &JobDbModel,
    updated_at: i64,
    condition: UpdateCondition,
) -> Result<u64> {
    let expected = match condition {
        UpdateCondition::Any => None,
        UpdateCondition::Status(status) => Some(status.as_str()),
    };
    let result = sqlx::query(UPDATE)
        .bind(&job.job_type)
        .bind(&job.status)
        .bind(&job.config)
        .bind(&job.state)
        .bind(updated_at)
        .bind(&job.input)
        .bind(&job.outputs)
        .bind(job.priority)
        .bind(&job.streamer_id)
        .bind(&job.session_id)
        .bind(job.started_at)
        .bind(job.completed_at)
        .bind(&job.error)
        .bind(job.retry_count)
        .bind(&job.pipeline_id)
        .bind(&job.execution_info)
        .bind(job.duration_secs)
        .bind(job.queue_wait_secs)
        .bind(&job.dag_step_execution_id)
        .bind(&job.id)
        .bind(expected)
        .bind(expected)
        .execute(connection)
        .await?;
    Ok(result.rows_affected())
}
