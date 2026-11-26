//! Filtered live session listing queries.

use jiff::Timestamp;
use sqlx::{QueryBuilder, Sqlite};

use crate::Result;
use crate::database::models::{LiveSessionDbModel, Pagination};
use crate::database::repositories::literal_substring_pattern;

use super::ApiQueries;

/// Filter criteria for querying sessions.
#[derive(Debug, Clone, Default)]
pub struct SessionFilters {
    /// Filter by streamer ID.
    pub streamer_id: Option<String>,
    /// Filter sessions started after this date.
    pub from_date: Option<Timestamp>,
    /// Filter sessions started before this date.
    pub to_date: Option<Timestamp>,
    /// Filter for active sessions only (sessions without an end_time).
    pub active_only: Option<bool>,
    /// Search query.
    pub search: Option<String>,
    /// When `false` (the default), exclude ended sessions whose
    /// `total_size_bytes == 0`, i.e. sessions that produced no retained
    /// segments.
    ///
    /// Setting this to `true` includes those rows for diagnostics and audit
    /// access via the `session_events` trail.
    ///
    /// Active sessions (`end_time IS NULL`) are always returned regardless of
    /// size: their `total_size_bytes` is legitimately 0 between LIVE detection
    /// and the first retained segment.
    pub include_empty: Option<bool>,
}

impl SessionFilters {
    /// Create a new empty filter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by streamer ID.
    pub fn with_streamer_id(mut self, streamer_id: impl Into<String>) -> Self {
        self.streamer_id = Some(streamer_id.into());
        self
    }

    /// Filter by date range.
    pub fn with_date_range(mut self, from: Option<Timestamp>, to: Option<Timestamp>) -> Self {
        self.from_date = from;
        self.to_date = to;
        self
    }

    /// Filter for active sessions only.
    pub fn with_active_only(mut self, active_only: bool) -> Self {
        self.active_only = Some(active_only);
        self
    }

    /// Filter by search query.
    pub fn with_search(mut self, search: impl Into<String>) -> Self {
        self.search = Some(search.into());
        self
    }
}

/// Append each session list predicate with its bound value. The query must
/// select from `live_sessions s LEFT JOIN streamers st`.
fn push_session_filters(query: &mut QueryBuilder<Sqlite>, filters: &SessionFilters) {
    let mut separator = " WHERE ";
    let mut next_separator = || std::mem::replace(&mut separator, " AND ");

    if let Some(streamer_id) = &filters.streamer_id {
        query
            .push(next_separator())
            .push("s.streamer_id = ")
            .push_bind(streamer_id);
    }
    if let Some(from_date) = &filters.from_date {
        query
            .push(next_separator())
            .push("s.start_time >= ")
            .push_bind(crate::database::time::datetime_to_ms(*from_date));
    }
    if let Some(to_date) = &filters.to_date {
        query
            .push(next_separator())
            .push("s.start_time <= ")
            .push_bind(crate::database::time::datetime_to_ms(*to_date));
    }
    if filters.active_only == Some(true) {
        query.push(next_separator()).push("s.end_time IS NULL");
    }

    // Hide ended sessions that produced no retained segments by default.
    // `total_size_bytes` grows only when a retained segment's output is
    // recorded, so `total_size_bytes > 0` means the session kept real bytes.
    // Active sessions (`end_time IS NULL`) are always kept: their size is 0
    // between LIVE detection and the first retained segment.
    if filters.include_empty != Some(true) {
        query
            .push(next_separator())
            .push("(s.total_size_bytes > 0 OR s.end_time IS NULL)");
    }

    // `COALESCE` keeps a session searchable by streamer name after its
    // `streamers` row is gone: the LEFT JOIN yields a NULL `st.name` and
    // the denormalized `s.streamer_name` takes over. While the streamer
    // exists `st.name` wins, so a rename is searchable immediately.
    if let Some(search) = &filters.search {
        let pattern = literal_substring_pattern(search);
        query
            .push(next_separator())
            .push("(COALESCE(st.name, s.streamer_name) LIKE ")
            .push_bind(pattern.clone())
            .push(r" ESCAPE '\' OR s.titles LIKE ")
            .push_bind(pattern.clone())
            .push(r" ESCAPE '\' OR s.id LIKE ")
            .push_bind(pattern)
            .push(r" ESCAPE '\')");
    }
}

impl ApiQueries {
    /// List sessions with optional filters and pagination.
    /// Returns a tuple of `(sessions, total_count)`.
    pub async fn list_sessions_filtered(
        &self,
        filters: &SessionFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<LiveSessionDbModel>, u64)> {
        const FROM: &str = "FROM live_sessions s LEFT JOIN streamers st ON s.streamer_id = st.id";

        let mut count_query = QueryBuilder::new("SELECT COUNT(s.id) as count ");
        count_query.push(FROM);
        push_session_filters(&mut count_query, filters);
        let total_count = count_query
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await? as u64;

        let mut data_query = QueryBuilder::new("SELECT s.* ");
        data_query.push(FROM);
        push_session_filters(&mut data_query, filters);
        data_query
            .push(" ORDER BY s.start_time DESC LIMIT ")
            .push_bind(pagination.limit as i64)
            .push(" OFFSET ")
            .push_bind(pagination.offset as i64);
        let sessions = data_query
            .build_query_as::<LiveSessionDbModel>()
            .fetch_all(&self.pool)
            .await?;

        Ok((sessions, total_count))
    }
}
