//! Complete step-row insertion shared by publication and standalone creation.

use sqlx::SqliteConnection;

use crate::Result;
use crate::database::models::{DagExecutionDbModel, DagStepExecutionDbModel};

const INSERT_DAG: &str = r#"
    INSERT INTO dag_execution (
        id, dag_definition, status, streamer_id, session_id,
        segment_index, segment_source, created_at, updated_at, completed_at, error,
        total_steps, completed_steps, failed_steps
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

const INSERT: &str = r#"
    INSERT INTO dag_step_execution (
        id, dag_id, step_id, job_id, status,
        depends_on_step_ids, outputs, created_at, updated_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

pub(super) enum InsertState<'a> {
    Stored,
    Unattached { status: &'a str },
}

pub(super) async fn insert_dag(
    connection: &mut SqliteConnection,
    dag: &DagExecutionDbModel,
) -> Result<()> {
    sqlx::query(INSERT_DAG)
        .bind(&dag.id)
        .bind(&dag.dag_definition)
        .bind(&dag.status)
        .bind(&dag.streamer_id)
        .bind(&dag.session_id)
        .bind(dag.segment_index)
        .bind(&dag.segment_source)
        .bind(dag.created_at)
        .bind(dag.updated_at)
        .bind(dag.completed_at)
        .bind(&dag.error)
        .bind(dag.total_steps)
        .bind(dag.completed_steps)
        .bind(dag.failed_steps)
        .execute(connection)
        .await?;
    Ok(())
}

pub(super) async fn insert_step(
    connection: &mut SqliteConnection,
    step: &DagStepExecutionDbModel,
    state: InsertState<'_>,
) -> Result<()> {
    let (job_id, status) = match state {
        InsertState::Stored => (step.job_id.as_deref(), step.status.as_str()),
        InsertState::Unattached { status } => (None, status),
    };
    sqlx::query(INSERT)
        .bind(&step.id)
        .bind(&step.dag_id)
        .bind(&step.step_id)
        .bind(job_id)
        .bind(status)
        .bind(&step.depends_on_step_ids)
        .bind(&step.outputs)
        .bind(step.created_at)
        .bind(step.updated_at)
        .execute(connection)
        .await?;
    Ok(())
}
