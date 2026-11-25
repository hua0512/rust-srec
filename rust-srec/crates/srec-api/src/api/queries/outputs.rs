//! Filtered media output listing and type-summary aggregation queries.

use crate::Result;
use crate::database::models::{MediaOutputDbModel, Pagination};
use crate::database::repositories::literal_substring_pattern;

use super::ApiQueries;

/// Filter criteria for querying media outputs.
#[derive(Debug, Clone, Default)]
pub struct OutputFilters {
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by streamer ID (requires join with `live_sessions`).
    pub streamer_id: Option<String>,
    /// Filter by `media_outputs.file_type`, i.e. a `MediaFileType::as_str` value
    /// such as `VIDEO` or `DANMU_XML` — not a container format.
    pub file_type: Option<String>,
    /// Search query.
    pub search: Option<String>,
}

impl OutputFilters {
    /// Create a new empty filter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by session ID.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Filter by streamer ID.
    pub fn with_streamer_id(mut self, streamer_id: impl Into<String>) -> Self {
        self.streamer_id = Some(streamer_id.into());
        self
    }

    /// Filter by media file type. Accepts a `MediaFileType::as_str` value.
    pub fn with_file_type(mut self, file_type: impl Into<String>) -> Self {
        self.file_type = Some(file_type.into());
        self
    }

    /// Filter by search query.
    pub fn with_search(mut self, search: impl Into<String>) -> Self {
        self.search = Some(search.into());
        self
    }
}

/// One row of [`ApiQueries::summarize_outputs_filtered`]: how many media
/// outputs of a given `file_type` match the filters, and their combined size.
#[derive(Debug, Clone)]
pub struct MediaOutputTypeSummary {
    /// A `MediaFileType::as_str` value, e.g. `VIDEO`.
    pub file_type: String,
    /// Number of matching outputs of this type.
    pub count: u64,
    /// Summed `media_outputs.size_bytes` of those outputs.
    pub size_bytes: u64,
}

/// The `FROM`/`WHERE` fragments for an `OutputFilters` query, plus the values to
/// bind in the order the placeholders appear.
///
/// `list_outputs_filtered` and `summarize_outputs_filtered` build three queries
/// between them off the same filters. They share this one builder so a new
/// filter cannot be added to one query's placeholders and missed in another's
/// binds — a mismatch SQLite reports only as wrong rows, not as an error.
struct OutputQueryClause {
    from_clause: &'static str,
    where_clause: String,
    binds: Vec<String>,
}

/// One `GROUP BY m.file_type` row of `summarize_outputs_filtered`.
#[derive(sqlx::FromRow)]
struct OutputTypeSummaryRow {
    file_type: String,
    count: i64,
    size_bytes: i64,
}

impl OutputQueryClause {
    fn build(filters: &OutputFilters) -> Self {
        let mut conditions: Vec<&'static str> = Vec::new();
        let mut binds: Vec<String> = Vec::new();

        if let Some(session_id) = &filters.session_id {
            conditions.push("m.session_id = ?");
            binds.push(session_id.clone());
        }
        if let Some(streamer_id) = &filters.streamer_id {
            conditions.push("s.streamer_id = ?");
            binds.push(streamer_id.clone());
        }
        if let Some(file_type) = &filters.file_type {
            conditions.push("m.file_type = ?");
            binds.push(file_type.clone());
        }
        if let Some(search) = &filters.search {
            conditions.push(r"(m.file_path LIKE ? ESCAPE '\' OR m.session_id LIKE ? ESCAPE '\' OR m.file_type LIKE ? ESCAPE '\')");
            let pattern = literal_substring_pattern(search);
            binds.extend([pattern.clone(), pattern.clone(), pattern]);
        }

        Self {
            // `s` is only in scope for the streamer_id condition, so the join is
            // added only when that filter is present.
            from_clause: if filters.streamer_id.is_some() {
                "media_outputs m INNER JOIN live_sessions s ON m.session_id = s.id"
            } else {
                "media_outputs m"
            },
            where_clause: if conditions.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", conditions.join(" AND "))
            },
            binds,
        }
    }
}

impl ApiQueries {
    /// List media outputs with optional filters and pagination.
    /// Returns a tuple of `(outputs, total_count)`.
    pub async fn list_outputs_filtered(
        &self,
        filters: &OutputFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<MediaOutputDbModel>, u64)> {
        let clause = OutputQueryClause::build(filters);

        let count_sql = format!(
            "SELECT COUNT(*) FROM {} {}",
            clause.from_clause, clause.where_clause
        );

        // Select only media_outputs columns to avoid ambiguity with the join.
        let data_sql = format!(
            "SELECT m.id, m.session_id, m.parent_media_output_id, m.file_path, m.file_type, m.size_bytes, m.created_at \
             FROM {} {} ORDER BY m.created_at DESC LIMIT ? OFFSET ?",
            clause.from_clause, clause.where_clause
        );

        let mut count_query = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql));
        for value in &clause.binds {
            count_query = count_query.bind(value);
        }
        // COUNT(*) is never negative, so the sign cannot be lost here.
        let total_count = count_query.fetch_one(&self.pool).await?.cast_unsigned();

        let mut data_query = sqlx::query_as::<_, MediaOutputDbModel>(sqlx::AssertSqlSafe(data_sql));
        for value in &clause.binds {
            data_query = data_query.bind(value);
        }
        data_query = data_query.bind(i64::from(pagination.limit));
        data_query = data_query.bind(i64::from(pagination.offset));

        let outputs = data_query.fetch_all(&self.pool).await?;

        Ok((outputs, total_count))
    }

    /// Count and sum `size_bytes` for every `media_outputs.file_type` matching
    /// `filters`, ordered alphabetically by `file_type`.
    pub async fn summarize_outputs_filtered(
        &self,
        filters: &OutputFilters,
    ) -> Result<Vec<MediaOutputTypeSummary>> {
        let clause = OutputQueryClause::build(filters);

        let sql = format!(
            "SELECT m.file_type AS file_type, COUNT(*) AS count, COALESCE(SUM(m.size_bytes), 0) AS size_bytes \
             FROM {} {} GROUP BY m.file_type ORDER BY m.file_type",
            clause.from_clause, clause.where_clause
        );

        let mut query = sqlx::query_as::<_, OutputTypeSummaryRow>(sqlx::AssertSqlSafe(sql));
        for value in &clause.binds {
            query = query.bind(value);
        }

        Ok(query
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| MediaOutputTypeSummary {
                file_type: row.file_type,
                // Clamped first: a stored negative `size_bytes` would otherwise
                // wrap to an enormous total rather than contributing nothing.
                count: row.count.max(0).cast_unsigned(),
                size_bytes: row.size_bytes.max(0).cast_unsigned(),
            })
            .collect())
    }
}
