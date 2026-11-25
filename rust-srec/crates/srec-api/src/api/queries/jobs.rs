//! Filtered job listing and count queries.

use jiff::Timestamp;
use sqlx::{QueryBuilder, Sqlite};

use crate::Result;
use crate::database::models::{JobDbModel, JobStatus, Pagination};
use crate::database::repositories::literal_substring_pattern;

use super::ApiQueries;

/// Filter criteria for querying jobs.
#[derive(Debug, Clone, Default)]
pub struct JobFilters {
    /// Filter by job status.
    pub status: Option<JobStatus>,
    /// Filter by any of several job statuses, for queries a single `status`
    /// cannot express. Combined with `status` by `AND`, so setting both narrows
    /// to their intersection.
    ///
    /// An empty list means "no status filter", not "no rows", matching how
    /// `job_types` treats an empty list.
    pub statuses: Option<Vec<JobStatus>>,
    /// Filter by streamer ID.
    pub streamer_id: Option<String>,
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by pipeline ID.
    pub pipeline_id: Option<String>,
    /// Filter jobs created after this date.
    pub from_date: Option<Timestamp>,
    /// Filter jobs created before this date.
    pub to_date: Option<Timestamp>,
    /// Filter by job type.
    pub job_type: Option<String>,
    /// Filter by multiple job types.
    pub job_types: Option<Vec<String>>,
    /// Search query.
    pub search: Option<String>,
}

impl JobFilters {
    /// Create a new empty filter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by status.
    pub fn with_status(mut self, status: JobStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Filter by any of `statuses`.
    pub fn with_statuses(mut self, statuses: impl IntoIterator<Item = JobStatus>) -> Self {
        self.statuses = Some(statuses.into_iter().collect());
        self
    }

    /// Filter by streamer ID.
    pub fn with_streamer_id(mut self, streamer_id: impl Into<String>) -> Self {
        self.streamer_id = Some(streamer_id.into());
        self
    }

    /// Filter by session ID.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Filter by pipeline ID.
    pub fn with_pipeline_id(mut self, pipeline_id: impl Into<String>) -> Self {
        self.pipeline_id = Some(pipeline_id.into());
        self
    }

    /// Filter by date range.
    pub fn with_date_range(mut self, from: Option<Timestamp>, to: Option<Timestamp>) -> Self {
        self.from_date = from;
        self.to_date = to;
        self
    }

    /// Filter by search query.
    pub fn with_search(mut self, search: impl Into<String>) -> Self {
        self.search = Some(search.into());
        self
    }
}

/// Append each optional predicate with its bound value for count and page queries.
fn push_job_filters(query: &mut QueryBuilder<Sqlite>, filters: &JobFilters) {
    let mut separator = " WHERE ";
    let mut next_separator = || std::mem::replace(&mut separator, " AND ");

    if let Some(status) = filters.status {
        query
            .push(next_separator())
            .push("status = ")
            .push_bind(status.as_str());
    }
    if let Some(statuses) = &filters.statuses
        && !statuses.is_empty()
    {
        query.push(next_separator()).push("status IN (");
        let mut values = query.separated(", ");
        for status in statuses {
            values.push_bind(status.as_str());
        }
        values.push_unseparated(")");
    }
    if let Some(streamer_id) = &filters.streamer_id {
        query
            .push(next_separator())
            .push("streamer_id = ")
            .push_bind(streamer_id);
    }
    if let Some(session_id) = &filters.session_id {
        query
            .push(next_separator())
            .push("session_id = ")
            .push_bind(session_id);
    }
    if let Some(pipeline_id) = &filters.pipeline_id {
        query
            .push(next_separator())
            .push("pipeline_id = ")
            .push_bind(pipeline_id);
    }
    if let Some(from_date) = filters.from_date {
        query
            .push(next_separator())
            .push("created_at >= ")
            .push_bind(crate::database::time::datetime_to_ms(from_date));
    }
    if let Some(to_date) = filters.to_date {
        query
            .push(next_separator())
            .push("created_at <= ")
            .push_bind(crate::database::time::datetime_to_ms(to_date));
    }
    if let Some(job_type) = &filters.job_type {
        query
            .push(next_separator())
            .push("job_type = ")
            .push_bind(job_type);
    }
    if let Some(job_types) = &filters.job_types
        && !job_types.is_empty()
    {
        query.push(next_separator()).push("job_type IN (");
        let mut values = query.separated(", ");
        for job_type in job_types {
            values.push_bind(job_type);
        }
        values.push_unseparated(")");
    }
    if let Some(search) = &filters.search {
        let pattern = literal_substring_pattern(search);
        query
            .push(next_separator())
            .push("(id LIKE ")
            .push_bind(pattern.clone())
            .push(r" ESCAPE '\' OR session_id LIKE ")
            .push_bind(pattern.clone())
            .push(r" ESCAPE '\' OR streamer_id LIKE ")
            .push_bind(pattern.clone())
            .push(r" ESCAPE '\' OR job_type LIKE ")
            .push_bind(pattern)
            .push(r" ESCAPE '\')");
    }
}

impl ApiQueries {
    /// List jobs with optional filters and pagination, returning `(jobs, total_count)`.
    pub async fn list_jobs_filtered(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<JobDbModel>, u64)> {
        let total_count = self.count_jobs(filters).await?;
        let jobs = self.list_jobs_page_filtered(filters, pagination).await?;
        Ok((jobs, total_count))
    }

    /// Count jobs matching `filters`, without fetching any rows.
    pub async fn count_jobs(&self, filters: &JobFilters) -> Result<u64> {
        let mut query = QueryBuilder::new("SELECT COUNT(*) as count FROM job");
        push_job_filters(&mut query, filters);
        Ok(query
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await? as u64)
    }

    /// List jobs with optional filters and pagination, without running a `COUNT(*)`.
    pub async fn list_jobs_page_filtered(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<Vec<JobDbModel>> {
        let mut query = QueryBuilder::new("SELECT * FROM job");
        push_job_filters(&mut query, filters);
        query
            .push(" ORDER BY priority DESC, created_at DESC LIMIT ")
            .push_bind(i64::from(pagination.limit))
            .push(" OFFSET ")
            .push_bind(i64::from(pagination.offset));
        Ok(query
            .build_query_as::<JobDbModel>()
            .fetch_all(&self.pool)
            .await?)
    }
}
