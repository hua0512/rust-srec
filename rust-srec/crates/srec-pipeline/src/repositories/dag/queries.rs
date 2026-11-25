//! Read queries, display-status calculations, and filter builders for DAG executions.

use sqlx::{QueryBuilder, Sqlite};

use super::SqlxDagRepository;
use srec_sqlite::models::dag::dag_definition_name;
use srec_sqlite::models::{
    DagDisplayCounts, DagExecutionDbModel, DagExecutionListSummary, DagExecutionStats,
    DagExecutionStatus, DagStepExecutionDbModel, DagStepWithJob,
};
use srec_sqlite::{DbError as Error, DbResult as Result};

/// SQL predicate, over `dag_execution AS dag`, that one of the DAG's jobs is
/// running. A materialized step is PROCESSING while its job is still queued,
/// so only the job row says whether a worker has picked it up.
macro_rules! dag_has_running_job_sql {
    () => {
        "EXISTS (SELECT 1 FROM job WHERE job.pipeline_id = dag.id AND job.status = 'PROCESSING')"
    };
}

/// SQL expression for a DAG's display status (see [`DagDisplayCounts`]).
const DAG_DISPLAY_STATUS_SQL: &str = concat!(
    "CASE WHEN dag.status IN ('PENDING', 'PROCESSING') THEN CASE WHEN ",
    dag_has_running_job_sql!(),
    " THEN 'PROCESSING' ELSE 'PENDING' END ELSE dag.status END"
);

/// Display-status counts, equivalent to grouping [`DAG_DISPLAY_STATUS_SQL`]
/// over every DAG. One statement keeps both halves on the same snapshot.
pub(super) const DAG_DISPLAY_COUNTS_SQL: &str = concat!(
    "SELECT status, COUNT(*) FROM dag_execution \
     WHERE status IN ('COMPLETED', 'FAILED', 'CANCELLED') GROUP BY status \
     UNION ALL \
     SELECT CASE WHEN ",
    dag_has_running_job_sql!(),
    " THEN 'PROCESSING' ELSE 'PENDING' END AS display_status, COUNT(*) \
     FROM dag_execution AS dag WHERE dag.status IN ('PENDING', 'PROCESSING') \
     GROUP BY display_status"
);

/// Average completed duration in milliseconds, read from the covering status
/// index rather than the DAG rows.
pub(super) const DAG_AVG_DURATION_SQL: &str = "SELECT AVG(completed_at - created_at) FROM dag_execution \
     WHERE status = 'COMPLETED' AND completed_at IS NOT NULL";

/// SQL predicate selecting DAGs whose display status is `status`. Terminal
/// statuses compare the stored column directly so they can use its index.
fn dag_display_status_condition(status: DagExecutionStatus) -> &'static str {
    match status {
        DagExecutionStatus::Pending => concat!(
            "dag.status IN ('PENDING', 'PROCESSING') AND NOT ",
            dag_has_running_job_sql!()
        ),
        DagExecutionStatus::Processing => concat!(
            "dag.status IN ('PENDING', 'PROCESSING') AND ",
            dag_has_running_job_sql!()
        ),
        DagExecutionStatus::Completed => "dag.status = 'COMPLETED'",
        DagExecutionStatus::Failed => "dag.status = 'FAILED'",
        DagExecutionStatus::Cancelled => "dag.status = 'CANCELLED'",
    }
}

/// Filters for listing DAG executions by display status.
#[derive(Debug, Clone, Copy, Default)]
pub struct DagListFilter<'a> {
    /// Display status (see [`DagDisplayCounts`]).
    pub status: Option<DagExecutionStatus>,
    pub session_id: Option<&'a str>,
    /// Case-insensitive substring of the DAG's ID, name, session ID, streamer
    /// ID, or streamer name. Blank text matches every DAG.
    pub search: Option<&'a str>,
}

/// Append display-list predicates and their binds in the same operation.
fn push_dag_list_filter(query: &mut QueryBuilder<Sqlite>, filter: &DagListFilter<'_>) {
    let mut separator = " WHERE ";
    let mut next_separator = || std::mem::replace(&mut separator, " AND ");
    if let Some(status) = filter.status {
        query
            .push(next_separator())
            .push(dag_display_status_condition(status));
    }
    if let Some(session_id) = filter.session_id {
        query
            .push(next_separator())
            .push("dag.session_id = ")
            .push_bind(session_id);
    }
    if let Some(search) = filter
        .search
        .map(str::trim)
        .filter(|search| !search.is_empty())
    {
        let pattern = srec_sqlite::repositories::literal_substring_pattern(search);
        query
            .push(next_separator())
            .push("(dag.id LIKE ")
            .push_bind(pattern.clone())
            .push(r" ESCAPE '\' OR json_extract(dag.dag_definition, '$.name') LIKE ")
            .push_bind(pattern.clone())
            .push(r" ESCAPE '\' OR dag.session_id LIKE ")
            .push_bind(pattern.clone())
            .push(r" ESCAPE '\' OR dag.streamer_id LIKE ")
            .push_bind(pattern.clone())
            .push(r" ESCAPE '\' OR EXISTS (SELECT 1 FROM streamers")
            .push(" WHERE streamers.id = dag.streamer_id AND streamers.name LIKE ")
            .push_bind(pattern)
            .push(r" ESCAPE '\'))");
    }
}

fn push_dag_execution_filters(
    query: &mut QueryBuilder<Sqlite>,
    status: Option<DagExecutionStatus>,
    session_id: Option<&str>,
) {
    let mut separator = " WHERE ";
    let mut next_separator = || std::mem::replace(&mut separator, " AND ");
    if let Some(status) = status {
        query
            .push(next_separator())
            .push("status = ")
            .push_bind(status.as_str());
    }
    if let Some(session_id) = session_id {
        query
            .push(next_separator())
            .push("session_id = ")
            .push_bind(session_id);
    }
}

fn parse_display_status(dag_id: &str, status: &str) -> Result<DagExecutionStatus> {
    DagExecutionStatus::parse(status)
        .ok_or_else(|| Error::Database(format!("DAG {dag_id} has unknown status {status}")))
}

#[derive(sqlx::FromRow)]
struct DagSummaryRow {
    id: String,
    dag_definition: String,
    display_status: String,
    streamer_id: Option<String>,
    session_id: Option<String>,
    total_steps: i32,
    completed_steps: i32,
    failed_steps: i32,
    created_at: i64,
    updated_at: i64,
}

impl SqlxDagRepository {
    pub(super) async fn query_dag(&self, id: &str) -> Result<DagExecutionDbModel> {
        sqlx::query_as::<_, DagExecutionDbModel>("SELECT * FROM dag_execution WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("DAG execution", id))
    }

    pub(super) async fn query_list_dags(
        &self,
        status: Option<DagExecutionStatus>,
        session_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionDbModel>> {
        let mut query = QueryBuilder::new("SELECT * FROM dag_execution");
        push_dag_execution_filters(&mut query, status, session_id);
        query
            .push(" ORDER BY created_at DESC LIMIT ")
            .push_bind(limit)
            .push(" OFFSET ")
            .push_bind(offset);
        Ok(query
            .build_query_as::<DagExecutionDbModel>()
            .fetch_all(&self.pool)
            .await?)
    }

    pub(super) async fn query_count_dags(
        &self,
        status: Option<DagExecutionStatus>,
        session_id: Option<&str>,
    ) -> Result<u64> {
        let mut query = QueryBuilder::new("SELECT COUNT(*) FROM dag_execution");
        push_dag_execution_filters(&mut query, status, session_id);
        let count = query
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await?;
        Ok(count as u64)
    }

    pub(super) async fn query_list_dag_summaries(
        &self,
        filter: &DagListFilter<'_>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionListSummary>> {
        let mut query = QueryBuilder::new(format!(
            "SELECT dag.id, dag.dag_definition, {DAG_DISPLAY_STATUS_SQL} AS display_status, \
             dag.streamer_id, dag.session_id, dag.total_steps, dag.completed_steps, \
             dag.failed_steps, dag.created_at, dag.updated_at \
             FROM dag_execution AS dag"
        ));
        push_dag_list_filter(&mut query, filter);
        query
            .push(" ORDER BY dag.created_at DESC LIMIT ")
            .push_bind(limit)
            .push(" OFFSET ")
            .push_bind(offset);
        let rows = query
            .build_query_as::<DagSummaryRow>()
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter()
            .map(|row| {
                let display_status = parse_display_status(&row.id, &row.display_status)?;
                Ok(DagExecutionListSummary {
                    name: dag_definition_name(&row.dag_definition),
                    id: row.id,
                    display_status,
                    streamer_id: row.streamer_id,
                    session_id: row.session_id,
                    total_steps: row.total_steps,
                    completed_steps: row.completed_steps,
                    failed_steps: row.failed_steps,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                })
            })
            .collect()
    }

    pub(super) async fn query_count_dags_by_display_status(
        &self,
        filter: &DagListFilter<'_>,
    ) -> Result<u64> {
        let mut query = QueryBuilder::new("SELECT COUNT(*) FROM dag_execution AS dag");
        push_dag_list_filter(&mut query, filter);
        let count = query
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await?;
        Ok(count as u64)
    }

    pub(super) async fn query_dag_display_status(&self, id: &str) -> Result<DagExecutionStatus> {
        let status: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT {DAG_DISPLAY_STATUS_SQL} FROM dag_execution AS dag WHERE dag.id = ?"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        let status = status.ok_or_else(|| Error::not_found("DAG execution", id))?;
        parse_display_status(id, &status)
    }

    pub(super) async fn query_dag_with_display_status(
        &self,
        id: &str,
    ) -> Result<(DagExecutionDbModel, DagExecutionStatus)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            #[sqlx(flatten)]
            dag: DagExecutionDbModel,
            display_status: String,
        }

        let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT dag.*, {DAG_DISPLAY_STATUS_SQL} AS display_status \
             FROM dag_execution AS dag WHERE dag.id = ?"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("DAG execution", id))?;
        let status = parse_display_status(id, &row.display_status)?;
        Ok((row.dag, status))
    }

    pub(super) async fn query_dag_display_counts(&self) -> Result<DagDisplayCounts> {
        // Retained history is almost entirely terminal; its counts come from
        // the status index, and only unfinished DAGs probe for a running job.
        let rows: Vec<(String, i64)> = sqlx::query_as(DAG_DISPLAY_COUNTS_SQL)
            .fetch_all(&self.pool)
            .await?;

        let avg_duration_ms: Option<f64> = sqlx::query_scalar(DAG_AVG_DURATION_SQL)
            .fetch_one(&self.pool)
            .await?;

        let mut counts = DagDisplayCounts {
            avg_duration_secs: avg_duration_ms.map(|ms| ms / 1000.0),
            ..Default::default()
        };
        for (status, count) in rows {
            let count = count as u64;
            match DagExecutionStatus::parse(&status) {
                Some(DagExecutionStatus::Pending) => counts.pending = count,
                Some(DagExecutionStatus::Processing) => counts.processing = count,
                Some(DagExecutionStatus::Completed) => counts.completed = count,
                Some(DagExecutionStatus::Failed) => counts.failed = count,
                Some(DagExecutionStatus::Cancelled) => counts.cancelled = count,
                None => {}
            }
        }
        Ok(counts)
    }

    pub(super) async fn query_step(&self, id: &str) -> Result<DagStepExecutionDbModel> {
        sqlx::query_as::<_, DagStepExecutionDbModel>(
            "SELECT * FROM dag_step_execution WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("DAG step execution", id))
    }

    pub(super) async fn query_step_by_dag_and_step_id(
        &self,
        dag_id: &str,
        step_id: &str,
    ) -> Result<DagStepExecutionDbModel> {
        sqlx::query_as::<_, DagStepExecutionDbModel>(
            "SELECT * FROM dag_step_execution WHERE dag_id = ? AND step_id = ?",
        )
        .bind(dag_id)
        .bind(step_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("DAG step", format!("{}/{}", dag_id, step_id)))
    }

    pub(super) async fn query_steps_by_dag(
        &self,
        dag_id: &str,
    ) -> Result<Vec<DagStepExecutionDbModel>> {
        // A DAG's steps are inserted together in definition order and usually
        // share created_at; rowid keeps that order instead of the index's.
        let steps = sqlx::query_as::<_, DagStepExecutionDbModel>(
            "SELECT * FROM dag_step_execution WHERE dag_id = ? ORDER BY created_at, rowid",
        )
        .bind(dag_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(steps)
    }

    pub(super) async fn query_steps_with_jobs_by_dag(
        &self,
        dag_id: &str,
    ) -> Result<Vec<DagStepWithJob>> {
        let steps = sqlx::query_as::<_, DagStepWithJob>(
            r#"
            SELECT s.*, j.status AS job_status, j.started_at AS job_started_at,
                   j.retry_after AS job_retry_after
            FROM dag_step_execution AS s
            LEFT JOIN job AS j ON j.id = s.job_id
            WHERE s.dag_id = ?
            ORDER BY s.created_at, s.rowid
            "#,
        )
        .bind(dag_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(steps)
    }

    pub(super) async fn query_dag_stats(&self, dag_id: &str) -> Result<DagExecutionStats> {
        #[derive(sqlx::FromRow)]
        struct StatusCount {
            status: String,
            count: i64,
        }

        let counts: Vec<StatusCount> = sqlx::query_as(
            r#"
            SELECT status, COUNT(*) as count
            FROM dag_step_execution
            WHERE dag_id = ?
            GROUP BY status
            "#,
        )
        .bind(dag_id)
        .fetch_all(&self.pool)
        .await?;

        let mut stats = DagExecutionStats::default();
        for StatusCount { status, count } in counts {
            match status.as_str() {
                "BLOCKED" => stats.blocked = count as u64,
                "PENDING" => stats.pending = count as u64,
                "PROCESSING" => stats.processing = count as u64,
                "COMPLETED" => stats.completed = count as u64,
                "FAILED" => stats.failed = count as u64,
                "CANCELLED" => stats.cancelled = count as u64,
                _ => {}
            }
        }

        Ok(stats)
    }

    pub(super) async fn query_running_step_jobs(
        &self,
        dag_ids: &[&str],
    ) -> Result<Vec<(String, String)>> {
        srec_sqlite::repositories::fetch_in_chunks(
            &self.pool,
            "SELECT step.dag_id, job.id \
             FROM dag_step_execution AS step \
             JOIN job ON job.id = step.job_id \
             WHERE step.status = 'PROCESSING' AND job.status = 'PROCESSING' \
               AND step.dag_id IN (",
            dag_ids,
            ")",
        )
        .await
    }
}
