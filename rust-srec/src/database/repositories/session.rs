//! Session repository.

use crate::database::begin_immediate;
use crate::database::models::{
    DanmuStatisticsDbModel, LiveSessionDbModel, MediaOutputDbModel, MediaOutputTypeSummary,
    OutputFilters, Pagination, SessionFilters, SessionSegmentDbModel,
};
use crate::database::retry::retry_on_sqlite_busy;
use crate::{Error, Result};
use async_trait::async_trait;
use sqlx::SqlitePool;

/// Session repository trait.
#[async_trait]
pub trait SessionRepository: Send + Sync {
    // Live Sessions
    async fn get_session(&self, id: &str) -> Result<LiveSessionDbModel>;
    /// Fetch several sessions in one round trip. Ids with no row are skipped.
    async fn get_sessions_by_ids(&self, ids: &[String]) -> Result<Vec<LiveSessionDbModel>> {
        let mut sessions = Vec::with_capacity(ids.len());
        for id in ids {
            match self.get_session(id).await {
                Ok(session) => sessions.push(session),
                Err(Error::NotFound { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(sessions)
    }
    async fn get_active_session_for_streamer(
        &self,
        streamer_id: &str,
    ) -> Result<Option<LiveSessionDbModel>>;
    async fn list_sessions_for_streamer(
        &self,
        streamer_id: &str,
        limit: i32,
    ) -> Result<Vec<LiveSessionDbModel>>;
    async fn create_session(&self, session: &LiveSessionDbModel) -> Result<()>;
    async fn end_session(&self, id: &str, end_time: i64) -> Result<()>;
    async fn resume_session(&self, id: &str) -> Result<()>;
    async fn update_session_titles(&self, id: &str, titles: &str) -> Result<()>;
    async fn delete_session(&self, id: &str) -> Result<()>;
    async fn delete_sessions_batch(&self, ids: &[String]) -> Result<u64>;

    // Filtering and pagination
    /// List sessions with optional filters and pagination.
    /// Returns a tuple of (sessions, total_count).
    async fn list_sessions_filtered(
        &self,
        filters: &SessionFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<LiveSessionDbModel>, u64)>;

    /// List ended sessions that have persisted artifacts and whose session-complete pipeline was
    /// never dispatched.
    async fn list_ended_sessions_pending_pipeline_recovery(
        &self,
        pagination: &Pagination,
    ) -> Result<Vec<LiveSessionDbModel>>;

    /// Record that `session_id` no longer needs its session-complete pipeline dispatched.
    async fn mark_session_complete_dispatched(&self, session_id: &str) -> Result<()>;

    // Media Outputs
    async fn get_media_output(&self, id: &str) -> Result<MediaOutputDbModel>;
    async fn get_media_outputs_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<MediaOutputDbModel>>;
    async fn get_media_outputs_for_sessions(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<MediaOutputDbModel>> {
        let mut outputs = Vec::new();
        for session_id in session_ids {
            outputs.extend(self.get_media_outputs_for_session(session_id).await?);
        }
        Ok(outputs)
    }
    async fn create_media_output(&self, output: &MediaOutputDbModel) -> Result<()>;
    async fn delete_media_output(&self, id: &str) -> Result<()>;

    /// Persist the retained video output and its session-segment index as one
    /// atomic operation.
    async fn create_segment_output(
        &self,
        output: &MediaOutputDbModel,
        segment: &SessionSegmentDbModel,
    ) -> Result<()>;

    /// Get the count of media outputs for a session.
    async fn get_output_count(&self, session_id: &str) -> Result<u32>;

    /// List media outputs with optional filters and pagination.
    /// Returns a tuple of (outputs, total_count).
    async fn list_outputs_filtered(
        &self,
        filters: &OutputFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<MediaOutputDbModel>, u64)>;

    /// Count and total size of the media outputs matching `filters`, grouped by
    /// `file_type`. Types with no matching rows are absent from the result.
    async fn summarize_outputs_filtered(
        &self,
        filters: &OutputFilters,
    ) -> Result<Vec<MediaOutputTypeSummary>>;

    async fn create_session_segment(&self, segment: &SessionSegmentDbModel) -> Result<()>;
    async fn list_session_segments_for_session(
        &self,
        session_id: &str,
        limit: i32,
    ) -> Result<Vec<SessionSegmentDbModel>>;

    async fn list_session_segments_page(
        &self,
        session_id: &str,
        pagination: &Pagination,
    ) -> Result<Vec<SessionSegmentDbModel>>;
    async fn next_session_segment_index(&self, session_id: &str) -> Result<u32>;

    // Danmu Statistics
    async fn get_danmu_statistics(
        &self,
        session_id: &str,
    ) -> Result<Option<DanmuStatisticsDbModel>>;
    async fn get_danmu_statistics_for_sessions(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<DanmuStatisticsDbModel>> {
        let mut statistics = Vec::new();
        for session_id in session_ids {
            if let Some(session_statistics) = self.get_danmu_statistics(session_id).await? {
                statistics.push(session_statistics);
            }
        }
        Ok(statistics)
    }
    /// Message totals only, for callers that need a per-session count rather than
    /// the full statistics.
    ///
    /// Exists because the aggregate JSON columns are large — the activity
    /// timeseries alone is tens of kilobytes per row — and a session list that
    /// only renders a count should not read them.
    async fn get_danmu_counts_for_sessions(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<(String, i64)>> {
        let statistics = self.get_danmu_statistics_for_sessions(session_ids).await?;
        Ok(statistics
            .into_iter()
            .map(|stats| (stats.session_id, stats.total_danmus))
            .collect())
    }
    async fn create_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()>;
    /// Insert or replace the statistics row for `stats.session_id`
    /// (`danmu_statistics.session_id` is UNIQUE; `stats.id` is only used when
    /// the row does not exist yet).
    async fn upsert_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()>;

    // Danmu aggregator checkpoints. Kept apart from the statistics accessors
    // because the blob is large and only read when a collection starts.

    /// Compressed `AggregatorState` for `session_id`, if a checkpoint exists.
    async fn get_danmu_aggregator_state(&self, session_id: &str) -> Result<Option<Vec<u8>>> {
        let _ = session_id;
        Ok(None)
    }
    /// Store the latest checkpoint for `session_id`, replacing any previous one.
    async fn upsert_danmu_aggregator_state(
        &self,
        session_id: &str,
        version: i64,
        state: &[u8],
    ) -> Result<()> {
        let _ = (session_id, version, state);
        Ok(())
    }
    /// Discard the checkpoint for `session_id`.
    async fn delete_danmu_aggregator_state(&self, session_id: &str) -> Result<()> {
        let _ = session_id;
        Ok(())
    }
}

/// Danmu-statistics writes. Both statements list the same columns in the same
/// order; `create_danmu_statistics` and `upsert_danmu_statistics` bind in that
/// order, so the three must be edited together.
const DANMU_STATISTICS_INSERT: &str = "INSERT INTO danmu_statistics (\
     id, session_id, total_danmus, unique_talkers, chat_count, gift_count, \
     duration_secs, start_time, end_time, rate_bucket_secs, \
     danmu_rate_timeseries, top_talkers, top_gifters, top_gifts, word_frequency\
     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

/// `id` is only used when the row does not exist yet; `session_id` is UNIQUE.
const DANMU_STATISTICS_UPSERT: &str = "INSERT INTO danmu_statistics (\
     id, session_id, total_danmus, unique_talkers, chat_count, gift_count, \
     duration_secs, start_time, end_time, rate_bucket_secs, \
     danmu_rate_timeseries, top_talkers, top_gifters, top_gifts, word_frequency\
     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
     ON CONFLICT(session_id) DO UPDATE SET \
     total_danmus = excluded.total_danmus, \
     unique_talkers = excluded.unique_talkers, \
     chat_count = excluded.chat_count, \
     gift_count = excluded.gift_count, \
     duration_secs = excluded.duration_secs, \
     start_time = excluded.start_time, \
     end_time = excluded.end_time, \
     rate_bucket_secs = excluded.rate_bucket_secs, \
     danmu_rate_timeseries = excluded.danmu_rate_timeseries, \
     top_talkers = excluded.top_talkers, \
     top_gifters = excluded.top_gifters, \
     top_gifts = excluded.top_gifts, \
     word_frequency = excluded.word_frequency";

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
            conditions.push("(m.file_path LIKE ? OR m.session_id LIKE ? OR m.file_type LIKE ?)");
            let pattern = format!("%{search}%");
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

/// SQLx implementation of SessionRepository.
pub struct SqlxSessionRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
}

impl SqlxSessionRepository {
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl SessionRepository for SqlxSessionRepository {
    async fn get_session(&self, id: &str) -> Result<LiveSessionDbModel> {
        sqlx::query_as::<_, LiveSessionDbModel>("SELECT * FROM live_sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("LiveSession", id))
    }

    async fn get_sessions_by_ids(&self, ids: &[String]) -> Result<Vec<LiveSessionDbModel>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder =
            sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT * FROM live_sessions WHERE id IN (");
        let mut separated = builder.separated(", ");
        for id in ids {
            separated.push_bind(id);
        }
        separated.push_unseparated(")");

        Ok(builder
            .build_query_as::<LiveSessionDbModel>()
            .fetch_all(&self.pool)
            .await?)
    }

    async fn get_active_session_for_streamer(
        &self,
        streamer_id: &str,
    ) -> Result<Option<LiveSessionDbModel>> {
        let session = sqlx::query_as::<_, LiveSessionDbModel>(
            "SELECT * FROM live_sessions WHERE streamer_id = ? AND end_time IS NULL ORDER BY start_time DESC LIMIT 1",
        )
        .bind(streamer_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(session)
    }

    async fn list_sessions_for_streamer(
        &self,
        streamer_id: &str,
        limit: i32,
    ) -> Result<Vec<LiveSessionDbModel>> {
        let sessions = sqlx::query_as::<_, LiveSessionDbModel>(
            "SELECT * FROM live_sessions WHERE streamer_id = ? ORDER BY start_time DESC LIMIT ?",
        )
        .bind(streamer_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(sessions)
    }

    async fn create_session(&self, session: &LiveSessionDbModel) -> Result<()> {
        retry_on_sqlite_busy("create_session", || async {
            sqlx::query(
                r#"
                INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time, titles, total_size_bytes)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&session.id)
            .bind(&session.streamer_id)
            .bind(&session.streamer_name)
            .bind(session.start_time)
            .bind(session.end_time)
            .bind(&session.titles)
            .bind(session.total_size_bytes)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn end_session(&self, id: &str, end_time: i64) -> Result<()> {
        retry_on_sqlite_busy("end_session", || async {
            sqlx::query(
                r#"
                UPDATE live_sessions 
                SET end_time = ?,
                    total_size_bytes = (SELECT COALESCE(SUM(size_bytes), 0) FROM media_outputs WHERE session_id = ?)
                WHERE id = ?
                "#,
            )
            .bind(end_time)
            .bind(id)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn resume_session(&self, id: &str) -> Result<()> {
        retry_on_sqlite_busy("resume_session", || async {
            sqlx::query("UPDATE live_sessions SET end_time = NULL WHERE id = ?")
                .bind(id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn update_session_titles(&self, id: &str, titles: &str) -> Result<()> {
        retry_on_sqlite_busy("update_session_titles", || async {
            sqlx::query("UPDATE live_sessions SET titles = ? WHERE id = ?")
                .bind(titles)
                .bind(id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        retry_on_sqlite_busy("delete_session", || async {
            sqlx::query("DELETE FROM live_sessions WHERE id = ?")
                .bind(id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn delete_sessions_batch(&self, ids: &[String]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }

        retry_on_sqlite_busy("delete_sessions_batch", || async {
            // Build a query with multiple placeholders: DELETE FROM live_sessions WHERE id IN (?, ?, ...)
            let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let sql = format!("DELETE FROM live_sessions WHERE id IN ({})", placeholders);

            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for id in ids {
                query = query.bind(id);
            }

            let result = query.execute(&self.write_pool).await?;
            Ok(result.rows_affected())
        })
        .await
    }

    async fn get_media_output(&self, id: &str) -> Result<MediaOutputDbModel> {
        sqlx::query_as::<_, MediaOutputDbModel>("SELECT * FROM media_outputs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("MediaOutput", id))
    }

    async fn get_media_outputs_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<MediaOutputDbModel>> {
        let outputs = sqlx::query_as::<_, MediaOutputDbModel>(
            "SELECT * FROM media_outputs WHERE session_id = ? ORDER BY created_at",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(outputs)
    }

    async fn get_media_outputs_for_sessions(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<MediaOutputDbModel>> {
        if session_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT * FROM media_outputs WHERE session_id IN (",
        );
        let mut separated = builder.separated(", ");
        for session_id in session_ids {
            separated.push_bind(session_id);
        }
        separated.push_unseparated(") ORDER BY session_id, created_at");

        Ok(builder
            .build_query_as::<MediaOutputDbModel>()
            .fetch_all(&self.pool)
            .await?)
    }

    async fn create_media_output(&self, output: &MediaOutputDbModel) -> Result<()> {
        retry_on_sqlite_busy("create_media_output", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;

            sqlx::query(
                r#"
                INSERT INTO media_outputs (id, session_id, parent_media_output_id, file_path, file_type, size_bytes, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&output.id)
            .bind(&output.session_id)
            .bind(&output.parent_media_output_id)
            .bind(&output.file_path)
            .bind(&output.file_type)
            .bind(output.size_bytes)
            .bind(output.created_at)
            .execute(&mut *tx)
            .await?;

            // Update session total size
            sqlx::query(
                "UPDATE live_sessions SET total_size_bytes = total_size_bytes + ? WHERE id = ?",
            )
            .bind(output.size_bytes)
            .bind(&output.session_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn create_segment_output(
        &self,
        output: &MediaOutputDbModel,
        segment: &SessionSegmentDbModel,
    ) -> Result<()> {
        if output.session_id != segment.session_id || output.file_path != segment.file_path {
            return Err(Error::Other(
                "media output and session segment must identify the same file".to_string(),
            ));
        }

        retry_on_sqlite_busy("create_segment_output", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;

            sqlx::query(
                r#"
                INSERT INTO media_outputs (id, session_id, parent_media_output_id, file_path, file_type, size_bytes, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&output.id)
            .bind(&output.session_id)
            .bind(&output.parent_media_output_id)
            .bind(&output.file_path)
            .bind(&output.file_type)
            .bind(output.size_bytes)
            .bind(output.created_at)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "UPDATE live_sessions SET total_size_bytes = total_size_bytes + ? WHERE id = ?",
            )
            .bind(output.size_bytes)
            .bind(&output.session_id)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                r#"
                INSERT INTO session_segments (
                    id,
                    session_id,
                    segment_index,
                    file_path,
                    duration_secs,
                    size_bytes,
                    split_reason_code,
                    split_reason_details_json,
                    created_at,
                    completed_at,
                    persisted_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&segment.id)
            .bind(&segment.session_id)
            .bind(segment.segment_index)
            .bind(&segment.file_path)
            .bind(segment.duration_secs)
            .bind(segment.size_bytes)
            .bind(&segment.split_reason_code)
            .bind(&segment.split_reason_details_json)
            .bind(segment.created_at)
            .bind(segment.completed_at)
            .bind(segment.persisted_at)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn create_session_segment(&self, segment: &SessionSegmentDbModel) -> Result<()> {
        retry_on_sqlite_busy("create_session_segment", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;

            sqlx::query(
                r#"
                INSERT INTO session_segments (
                    id,
                    session_id,
                    segment_index,
                    file_path,
                    duration_secs,
                    size_bytes,
                    split_reason_code,
                    split_reason_details_json,
                    created_at,
                    completed_at,
                    persisted_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&segment.id)
            .bind(&segment.session_id)
            .bind(segment.segment_index)
            .bind(&segment.file_path)
            .bind(segment.duration_secs)
            .bind(segment.size_bytes)
            .bind(&segment.split_reason_code)
            .bind(&segment.split_reason_details_json)
            .bind(segment.created_at)
            .bind(segment.completed_at)
            .bind(segment.persisted_at)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn list_session_segments_for_session(
        &self,
        session_id: &str,
        limit: i32,
    ) -> Result<Vec<SessionSegmentDbModel>> {
        let limit = limit.clamp(1, 10000);
        let segments = sqlx::query_as::<_, SessionSegmentDbModel>(
            "SELECT * FROM session_segments WHERE session_id = ? ORDER BY created_at DESC, persisted_at DESC LIMIT ?",
        )
        .bind(session_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(segments)
    }

    async fn list_session_segments_page(
        &self,
        session_id: &str,
        pagination: &Pagination,
    ) -> Result<Vec<SessionSegmentDbModel>> {
        let limit = i32::try_from(pagination.limit)
            .unwrap_or(i32::MAX)
            .clamp(1, 10_000);
        let offset = i32::try_from(pagination.offset).unwrap_or(0).max(0);
        let segments = sqlx::query_as::<_, SessionSegmentDbModel>(
            "SELECT * FROM session_segments WHERE session_id = ? ORDER BY created_at DESC, persisted_at DESC LIMIT ? OFFSET ?",
        )
        .bind(session_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(segments)
    }

    async fn next_session_segment_index(&self, session_id: &str) -> Result<u32> {
        let next: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(segment_index), -1) + 1 FROM session_segments WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await?;

        u32::try_from(next).map_err(|_| {
            Error::Database(format!(
                "next session segment index {} for session {} is outside u32 range",
                next, session_id
            ))
        })
    }

    async fn delete_media_output(&self, id: &str) -> Result<()> {
        // Get output info before deletion to update session size
        let output = self.get_media_output(id).await?;

        retry_on_sqlite_busy("delete_media_output", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;

            sqlx::query("DELETE FROM media_outputs WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;

            // Update session total size
            sqlx::query(
                "UPDATE live_sessions SET total_size_bytes = total_size_bytes - ? WHERE id = ?",
            )
            .bind(output.size_bytes)
            .bind(&output.session_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn get_danmu_statistics(
        &self,
        session_id: &str,
    ) -> Result<Option<DanmuStatisticsDbModel>> {
        let stats = sqlx::query_as::<_, DanmuStatisticsDbModel>(
            "SELECT * FROM danmu_statistics WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(stats)
    }

    async fn get_danmu_statistics_for_sessions(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<DanmuStatisticsDbModel>> {
        if session_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT * FROM danmu_statistics WHERE session_id IN (",
        );
        let mut separated = builder.separated(", ");
        for session_id in session_ids {
            separated.push_bind(session_id);
        }
        separated.push_unseparated(") ORDER BY session_id");

        Ok(builder
            .build_query_as::<DanmuStatisticsDbModel>()
            .fetch_all(&self.pool)
            .await?)
    }

    async fn create_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()> {
        retry_on_sqlite_busy("create_danmu_statistics", || async {
            sqlx::query(DANMU_STATISTICS_INSERT)
                .bind(&stats.id)
                .bind(&stats.session_id)
                .bind(stats.total_danmus)
                .bind(stats.unique_talkers)
                .bind(stats.chat_count)
                .bind(stats.gift_count)
                .bind(stats.duration_secs)
                .bind(stats.start_time)
                .bind(stats.end_time)
                .bind(stats.rate_bucket_secs)
                .bind(&stats.danmu_rate_timeseries)
                .bind(&stats.top_talkers)
                .bind(&stats.top_gifters)
                .bind(&stats.top_gifts)
                .bind(&stats.word_frequency)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn upsert_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()> {
        retry_on_sqlite_busy("upsert_danmu_statistics", || async {
            sqlx::query(DANMU_STATISTICS_UPSERT)
                .bind(&stats.id)
                .bind(&stats.session_id)
                .bind(stats.total_danmus)
                .bind(stats.unique_talkers)
                .bind(stats.chat_count)
                .bind(stats.gift_count)
                .bind(stats.duration_secs)
                .bind(stats.start_time)
                .bind(stats.end_time)
                .bind(stats.rate_bucket_secs)
                .bind(&stats.danmu_rate_timeseries)
                .bind(&stats.top_talkers)
                .bind(&stats.top_gifters)
                .bind(&stats.top_gifts)
                .bind(&stats.word_frequency)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn get_danmu_counts_for_sessions(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<(String, i64)>> {
        if session_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT session_id, total_danmus FROM danmu_statistics WHERE session_id IN (",
        );
        let mut separated = builder.separated(", ");
        for session_id in session_ids {
            separated.push_bind(session_id);
        }
        separated.push_unseparated(") ORDER BY session_id");

        Ok(builder
            .build_query_as::<(String, i64)>()
            .fetch_all(&self.pool)
            .await?)
    }

    async fn get_danmu_aggregator_state(&self, session_id: &str) -> Result<Option<Vec<u8>>> {
        let state: Option<(Vec<u8>,)> =
            sqlx::query_as("SELECT state FROM danmu_aggregator_state WHERE session_id = ?")
                .bind(session_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(state.map(|(state,)| state))
    }

    async fn upsert_danmu_aggregator_state(
        &self,
        session_id: &str,
        version: i64,
        state: &[u8],
    ) -> Result<()> {
        retry_on_sqlite_busy("upsert_danmu_aggregator_state", || async {
            sqlx::query(
                "INSERT INTO danmu_aggregator_state (session_id, version, updated_at, state) \
                 VALUES (?, ?, ?, ?) \
                 ON CONFLICT(session_id) DO UPDATE SET \
                 version = excluded.version, \
                 updated_at = excluded.updated_at, \
                 state = excluded.state",
            )
            .bind(session_id)
            .bind(version)
            .bind(chrono::Utc::now().timestamp_millis())
            .bind(state)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn delete_danmu_aggregator_state(&self, session_id: &str) -> Result<()> {
        retry_on_sqlite_busy("delete_danmu_aggregator_state", || async {
            sqlx::query("DELETE FROM danmu_aggregator_state WHERE session_id = ?")
                .bind(session_id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn list_sessions_filtered(
        &self,
        filters: &SessionFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<LiveSessionDbModel>, u64)> {
        // Build dynamic WHERE clause
        let mut conditions: Vec<String> = Vec::new();

        if filters.streamer_id.is_some() {
            conditions.push("s.streamer_id = ?".to_string());
        }
        if filters.from_date.is_some() {
            conditions.push("s.start_time >= ?".to_string());
        }
        if filters.to_date.is_some() {
            conditions.push("s.start_time <= ?".to_string());
        }
        if filters.active_only == Some(true) {
            conditions.push("s.end_time IS NULL".to_string());
        }

        // Hide ended sessions that produced no retained segments by default.
        // `total_size_bytes` is incremented in the `media_outputs` insert
        // path (`session.rs::add_session_output_with_*`) which only runs
        // for segments that passed the small-segment guard in
        // `services::container`. So `total_size_bytes > 0` is a reliable
        // signal that the session retained real bytes. Active sessions
        // (`end_time IS NULL`) are always kept — their size is 0 in the
        // window between LIVE detection and the first retained segment.
        if filters.include_empty != Some(true) {
            conditions.push("(s.total_size_bytes > 0 OR s.end_time IS NULL)".to_string());
        }

        // `COALESCE` keeps a session searchable by streamer name after its
        // `streamers` row is gone: the LEFT JOIN yields a NULL `st.name` and
        // the denormalized `s.streamer_name` takes over. While the streamer
        // exists `st.name` wins, so a rename is searchable immediately.
        if filters.search.is_some() {
            conditions.push(
                "(COALESCE(st.name, s.streamer_name) LIKE ? OR s.titles LIKE ? OR s.id LIKE ?)"
                    .to_string(),
            );
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        // Count query with JOIN to support search by streamer name
        let count_sql = format!(
            "SELECT COUNT(s.id) as count FROM live_sessions s \
             LEFT JOIN streamers st ON s.streamer_id = st.id \
             {}",
            where_clause
        );

        // Data query with pagination, ordered by start_time descending
        // Join with streamers table to filter by streamer name if needed
        let data_sql = format!(
            "SELECT s.* FROM live_sessions s \
             LEFT JOIN streamers st ON s.streamer_id = st.id \
             {} ORDER BY s.start_time DESC LIMIT ? OFFSET ?",
            where_clause
        );

        // Execute count query
        let mut count_query = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql));

        // Bind parameters for count query (excluding active_only which is a static condition)
        if let Some(streamer_id) = &filters.streamer_id {
            count_query = count_query.bind(streamer_id);
        }
        if let Some(from_date) = &filters.from_date {
            count_query = count_query.bind(from_date.timestamp_millis());
        }
        if let Some(to_date) = &filters.to_date {
            count_query = count_query.bind(to_date.timestamp_millis());
        }
        if let Some(search) = &filters.search {
            let pattern = format!("%{}%", search);
            count_query = count_query
                .bind(pattern.clone())
                .bind(pattern.clone())
                .bind(pattern);
        }

        let total_count = count_query.fetch_one(&self.pool).await? as u64;

        // Execute data query
        let mut data_query = sqlx::query_as::<_, LiveSessionDbModel>(sqlx::AssertSqlSafe(data_sql));

        // Bind parameters for data query
        if let Some(streamer_id) = &filters.streamer_id {
            data_query = data_query.bind(streamer_id);
        }
        if let Some(from_date) = &filters.from_date {
            data_query = data_query.bind(from_date.timestamp_millis());
        }
        if let Some(to_date) = &filters.to_date {
            data_query = data_query.bind(to_date.timestamp_millis());
        }
        if let Some(search) = &filters.search {
            let pattern = format!("%{}%", search);
            data_query = data_query
                .bind(pattern.clone())
                .bind(pattern.clone())
                .bind(pattern);
        }

        // Bind pagination parameters
        data_query = data_query.bind(pagination.limit as i64);
        data_query = data_query.bind(pagination.offset as i64);

        let sessions = data_query.fetch_all(&self.pool).await?;

        Ok((sessions, total_count))
    }

    async fn list_ended_sessions_pending_pipeline_recovery(
        &self,
        pagination: &Pagination,
    ) -> Result<Vec<LiveSessionDbModel>> {
        // `session_complete_dispatched` is the authoritative signal;
        // `segment_source = 'session_complete'` covers the window where
        // `run_session_complete_pipeline` published the DAG but had not yet marked the session.
        //
        // `streamer_id IS NOT NULL` excludes sessions whose streamer was
        // deleted: `PipelineManager::recover_pipeline_coordinator_state` needs
        // a streamer id to resolve a session-complete pipeline from
        // `ConfigService::get_config_for_streamer`, and running one for a
        // streamer the user removed would re-upload and re-notify.
        let sessions = sqlx::query_as::<_, LiveSessionDbModel>(
            r#"
            SELECT session.*
            FROM live_sessions AS session
            WHERE session.end_time IS NOT NULL
              AND session.streamer_id IS NOT NULL
              AND session.session_complete_dispatched = 0
              AND (
                EXISTS (
                    SELECT 1 FROM session_segments
                    WHERE session_id = session.id
                )
                OR EXISTS (
                    SELECT 1 FROM media_outputs
                    WHERE session_id = session.id
                )
              )
              AND NOT EXISTS (
                SELECT 1 FROM dag_execution
                WHERE session_id = session.id
                  AND segment_source = 'session_complete'
              )
            ORDER BY session.start_time DESC
            LIMIT ? OFFSET ?
            "#,
        )
        .bind(pagination.limit as i64)
        .bind(pagination.offset as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(sessions)
    }

    async fn mark_session_complete_dispatched(&self, session_id: &str) -> Result<()> {
        retry_on_sqlite_busy("mark_session_complete_dispatched", || async {
            sqlx::query("UPDATE live_sessions SET session_complete_dispatched = 1 WHERE id = ?")
                .bind(session_id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn get_output_count(&self, session_id: &str) -> Result<u32> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM media_outputs WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&self.pool)
                .await?;

        Ok(count as u32)
    }

    async fn list_outputs_filtered(
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

    async fn summarize_outputs_filtered(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::{
        LiveSessionDbModel, MediaFileType, MediaOutputDbModel, Pagination, StreamerDbModel,
    };
    use crate::database::repositories::{SqlxStreamerRepository, StreamerRepository as _};
    use crate::database::{init_pool_with_size, run_migrations};
    use std::collections::HashMap;

    async fn setup_test_repo() -> SqlxSessionRepository {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let mut streamer = StreamerDbModel::new(
            "Streamer One",
            "https://example.com/streamer-1",
            "platform-twitch",
        );
        streamer.id = "streamer-1".to_string();
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();

        let mut session = LiveSessionDbModel::new("streamer-1");
        session.id = "session-1".to_string();
        SqlxSessionRepository::new(pool.clone(), pool.clone())
            .create_session(&session)
            .await
            .unwrap();

        SqlxSessionRepository::new(pool.clone(), pool)
    }

    #[tokio::test]
    async fn test_create_and_list_session_segment_with_lifecycle_timestamps() {
        let repo = setup_test_repo().await;
        let segment = SessionSegmentDbModel::new(
            "session-1",
            7,
            "/tmp/segment-007.ts",
            9.5,
            2048,
            crate::database::models::SessionSegmentLifecycle::new(
                Some(1_700_000_000_000),
                Some(1_700_000_009_500),
            ),
            crate::database::models::SessionSegmentSplitReason::new(
                Some("manual".to_string()),
                Some("{\"kind\":\"manual\"}".to_string()),
            ),
        );

        repo.create_session_segment(&segment).await.unwrap();

        let listed = repo
            .list_session_segments_page("session-1", &Pagination::new(10, 0))
            .await
            .unwrap();

        assert_eq!(listed.len(), 1);
        let saved = &listed[0];
        assert_eq!(saved.session_id, "session-1");
        assert_eq!(saved.segment_index, 7);
        assert_eq!(saved.created_at, Some(1_700_000_000_000));
        assert_eq!(saved.completed_at, Some(1_700_000_009_500));
        assert!(saved.persisted_at >= 1_700_000_000_000);
    }

    #[tokio::test]
    async fn create_segment_output_rolls_back_media_and_size_when_segment_insert_fails() {
        let repo = setup_test_repo().await;
        let mut existing = SessionSegmentDbModel::new(
            "session-1",
            0,
            "/tmp/existing.ts",
            1.0,
            1,
            Default::default(),
            Default::default(),
        );
        existing.id = "duplicate-segment-id".to_string();
        repo.create_session_segment(&existing).await.unwrap();

        let output =
            MediaOutputDbModel::new("session-1", "/tmp/final.ts", MediaFileType::Video, 2048);
        let mut final_segment = SessionSegmentDbModel::new(
            "session-1",
            1,
            "/tmp/final.ts",
            2.0,
            2048,
            Default::default(),
            Default::default(),
        );
        final_segment.id = existing.id.clone();

        assert!(
            repo.create_segment_output(&output, &final_segment)
                .await
                .is_err()
        );
        assert_eq!(repo.get_output_count("session-1").await.unwrap(), 0);
        let total_size: i64 =
            sqlx::query_scalar("SELECT total_size_bytes FROM live_sessions WHERE id = ?")
                .bind("session-1")
                .fetch_one(&repo.pool)
                .await
                .unwrap();
        assert_eq!(total_size, 0);
    }

    #[tokio::test]
    async fn next_session_segment_index_returns_zero_without_segments() {
        let repo = setup_test_repo().await;

        let next = repo.next_session_segment_index("session-1").await.unwrap();

        assert_eq!(next, 0);
    }

    #[tokio::test]
    async fn next_session_segment_index_returns_max_plus_one() {
        let repo = setup_test_repo().await;
        let segment = SessionSegmentDbModel::new(
            "session-1",
            0,
            "/tmp/segment-000.ts",
            9.5,
            2048,
            crate::database::models::SessionSegmentLifecycle::default(),
            crate::database::models::SessionSegmentSplitReason::default(),
        );
        repo.create_session_segment(&segment).await.unwrap();

        let next = repo.next_session_segment_index("session-1").await.unwrap();

        assert_eq!(next, 1);
    }

    #[tokio::test]
    async fn next_session_segment_index_uses_max_when_duplicates_exist() {
        let repo = setup_test_repo().await;
        for (index, path) in [
            (0, "/tmp/segment-000-a.ts"),
            (0, "/tmp/segment-000-b.ts"),
            (3, "/tmp/segment-003.ts"),
        ] {
            let segment = SessionSegmentDbModel::new(
                "session-1",
                index,
                path,
                9.5,
                2048,
                crate::database::models::SessionSegmentLifecycle::default(),
                crate::database::models::SessionSegmentSplitReason::default(),
            );
            repo.create_session_segment(&segment).await.unwrap();
        }

        let next = repo.next_session_segment_index("session-1").await.unwrap();

        assert_eq!(next, 4);
    }

    #[tokio::test]
    async fn ended_pipeline_recovery_candidates_require_artifact_and_no_session_dag() {
        use crate::database::models::{DagExecutionDbModel, DagPipelineDefinition};
        use crate::database::repositories::{DagRepository as _, SqlxDagRepository};

        let repo = setup_test_repo().await;
        let ended_at = crate::database::time::now_ms();
        repo.end_session("session-1", ended_at).await.unwrap();
        assert!(
            repo.list_ended_sessions_pending_pipeline_recovery(&Pagination::new(10, 0))
                .await
                .unwrap()
                .is_empty()
        );

        repo.create_session_segment(&SessionSegmentDbModel::new(
            "session-1",
            0,
            "/tmp/segment-000.ts",
            1.0,
            1024,
            Default::default(),
            Default::default(),
        ))
        .await
        .unwrap();
        let candidates = repo
            .list_ended_sessions_pending_pipeline_recovery(&Pagination::new(10, 0))
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, "session-1");

        // An untagged session-scoped DAG is a thumbnail run and says nothing about
        // session-complete dispatch, so the session stays a candidate.
        let definition = DagPipelineDefinition::new("session-complete", Vec::new());
        let new_session_dag = |segment_source: Option<&str>| {
            let mut dag = DagExecutionDbModel::new(
                &definition,
                Some("streamer-1".to_string()),
                Some("session-1".to_string()),
            );
            dag.segment_source = segment_source.map(str::to_string);
            dag
        };
        SqlxDagRepository::new(repo.pool.clone(), repo.write_pool.clone())
            .create_dag(&new_session_dag(None))
            .await
            .unwrap();
        assert_eq!(
            repo.list_ended_sessions_pending_pipeline_recovery(&Pagination::new(10, 0))
                .await
                .unwrap()
                .len(),
            1
        );

        repo.mark_session_complete_dispatched("session-1")
            .await
            .unwrap();
        assert!(
            repo.list_ended_sessions_pending_pipeline_recovery(&Pagination::new(10, 0))
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A published session-complete DAG suppresses recovery on its own, covering the window
    /// where `run_session_complete_pipeline` committed the DAG but had not yet marked the row.
    #[tokio::test]
    async fn published_session_complete_dag_suppresses_recovery_without_the_marker() {
        use crate::database::models::{DagExecutionDbModel, DagPipelineDefinition};
        use crate::database::repositories::{DagRepository as _, SqlxDagRepository};

        let repo = setup_test_repo().await;
        repo.end_session("session-1", crate::database::time::now_ms())
            .await
            .unwrap();
        repo.create_session_segment(&SessionSegmentDbModel::new(
            "session-1",
            0,
            "/tmp/segment-000.ts",
            1.0,
            1024,
            Default::default(),
            Default::default(),
        ))
        .await
        .unwrap();

        let definition = DagPipelineDefinition::new("session-complete", Vec::new());
        let mut dag = DagExecutionDbModel::new(
            &definition,
            Some("streamer-1".to_string()),
            Some("session-1".to_string()),
        );
        dag.segment_source = Some("session_complete".to_string());
        SqlxDagRepository::new(repo.pool.clone(), repo.write_pool.clone())
            .create_dag(&dag)
            .await
            .unwrap();

        assert!(
            repo.list_ended_sessions_pending_pipeline_recovery(&Pagination::new(10, 0))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn batch_session_metadata_queries_return_requested_rows() {
        let repo = setup_test_repo().await;
        let mut second_streamer = StreamerDbModel::new(
            "Streamer Two",
            "https://example.com/streamer-2",
            "platform-twitch",
        );
        second_streamer.id = "streamer-2".to_string();
        SqlxStreamerRepository::new(repo.pool.clone(), repo.write_pool.clone())
            .create_streamer(&second_streamer)
            .await
            .unwrap();

        let mut second_session = LiveSessionDbModel::new("streamer-2");
        second_session.id = "session-2".to_string();
        repo.create_session(&second_session).await.unwrap();

        // A session outside `session_ids`: its rows must be filtered out, so a
        // batch query missing its `WHERE session_id IN (...)` fails the counts.
        // Ended, because live_sessions allows one active session per streamer.
        let mut excluded_session = LiveSessionDbModel::new("streamer-2");
        excluded_session.id = "session-3".to_string();
        excluded_session.end_time = Some(1_700_000_100_000);
        repo.create_session(&excluded_session).await.unwrap();

        for output in [
            MediaOutputDbModel::new("session-1", "/video.mp4", MediaFileType::Video, 10),
            MediaOutputDbModel::new("session-1", "/thumbnail.jpg", MediaFileType::Thumbnail, 2),
            MediaOutputDbModel::new("session-2", "/audio.m4a", MediaFileType::Audio, 3),
            MediaOutputDbModel::new("session-3", "/excluded.mp4", MediaFileType::Video, 4),
        ] {
            repo.create_media_output(&output).await.unwrap();
        }

        let mut statistics = DanmuStatisticsDbModel::new("session-1");
        statistics.total_danmus = 42;
        repo.create_danmu_statistics(&statistics).await.unwrap();

        let mut excluded_statistics = DanmuStatisticsDbModel::new("session-3");
        excluded_statistics.total_danmus = 7;
        repo.create_danmu_statistics(&excluded_statistics)
            .await
            .unwrap();

        let session_ids = vec!["session-1".to_string(), "session-2".to_string()];
        let outputs = repo
            .get_media_outputs_for_sessions(&session_ids)
            .await
            .unwrap();
        let statistics = repo
            .get_danmu_statistics_for_sessions(&session_ids)
            .await
            .unwrap();

        assert_eq!(outputs.len(), 3);
        assert!(
            outputs
                .iter()
                .all(|output| output.session_id != "session-3")
        );
        assert_eq!(statistics.len(), 1);
        assert_eq!(statistics[0].total_danmus, 42);
        assert!(
            repo.get_media_outputs_for_sessions(&[])
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Seeds `session-1` with one output per `MediaFileType` plus a second
    /// video, giving every type a distinct size so a summary that mixes up
    /// groups cannot still sum to the right totals.
    async fn setup_output_filter_repo() -> SqlxSessionRepository {
        let repo = setup_test_repo().await;
        for output in [
            MediaOutputDbModel::new("session-1", "/one.mp4", MediaFileType::Video, 100),
            MediaOutputDbModel::new("session-1", "/two.mp4", MediaFileType::Video, 200),
            MediaOutputDbModel::new("session-1", "/one.jpg", MediaFileType::Thumbnail, 10),
            MediaOutputDbModel::new("session-1", "/one.xml", MediaFileType::DanmuXml, 1),
        ] {
            repo.create_media_output(&output).await.unwrap();
        }
        repo
    }

    #[tokio::test]
    async fn list_outputs_filtered_narrows_to_one_file_type() {
        let repo = setup_output_filter_repo().await;

        let filters = OutputFilters::new().with_file_type(MediaFileType::Video.as_str());
        let (outputs, total) = repo
            .list_outputs_filtered(&filters, &Pagination::new(10, 0))
            .await
            .unwrap();

        assert_eq!(total, 2);
        assert_eq!(outputs.len(), 2);
        assert!(
            outputs
                .iter()
                .all(|output| output.file_type == MediaFileType::Video.as_str())
        );
    }

    #[tokio::test]
    async fn list_outputs_filtered_applies_file_type_and_search_together() {
        let repo = setup_output_filter_repo().await;

        // The search alone matches `/one.mp4`, `/one.jpg` and `/one.xml`, so a
        // dropped or misordered bind would show up as more than one row.
        let filters = OutputFilters::new()
            .with_file_type(MediaFileType::Video.as_str())
            .with_search("/one.");
        let (outputs, total) = repo
            .list_outputs_filtered(&filters, &Pagination::new(10, 0))
            .await
            .unwrap();

        assert_eq!(total, 1);
        assert_eq!(outputs[0].file_path, "/one.mp4");
    }

    #[tokio::test]
    async fn summarize_outputs_filtered_groups_counts_and_sizes_by_type() {
        let repo = setup_output_filter_repo().await;

        let summaries = repo
            .summarize_outputs_filtered(&OutputFilters::new())
            .await
            .unwrap();

        let by_type: HashMap<&str, &MediaOutputTypeSummary> = summaries
            .iter()
            .map(|summary| (summary.file_type.as_str(), summary))
            .collect();
        assert_eq!(by_type.len(), 3);
        assert_eq!(by_type["VIDEO"].count, 2);
        assert_eq!(by_type["VIDEO"].size_bytes, 300);
        assert_eq!(by_type["THUMBNAIL"].count, 1);
        assert_eq!(by_type["THUMBNAIL"].size_bytes, 10);
        assert_eq!(by_type["DANMU_XML"].count, 1);
        assert_eq!(by_type["DANMU_XML"].size_bytes, 1);
        // No AUDIO rows exist, so that type is absent rather than zero.
        assert!(!by_type.contains_key("AUDIO"));
    }

    /// Deleting a streamer leaves its recordings behind: the
    /// `live_sessions.streamer_id` foreign key is `ON DELETE SET NULL`, so the
    /// session rows and their `media_outputs` / `session_segments` children
    /// survive, and `streamer_name` keeps the label for the API responses.
    #[tokio::test]
    async fn deleting_a_streamer_keeps_its_sessions_and_outputs() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let streamer_repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        let mut streamer = StreamerDbModel::new(
            "Streamer One",
            "https://example.com/streamer-1",
            "platform-twitch",
        );
        streamer.id = "streamer-1".to_string();
        streamer_repo.create_streamer(&streamer).await.unwrap();

        let repo = SqlxSessionRepository::new(pool.clone(), pool.clone());

        let mut ended = LiveSessionDbModel::new("streamer-1").with_streamer_name("Streamer One");
        ended.id = "session-ended".to_string();
        repo.create_session(&ended).await.unwrap();
        repo.end_session("session-ended", crate::database::time::now_ms())
            .await
            .unwrap();

        let mut active = LiveSessionDbModel::new("streamer-1").with_streamer_name("Streamer One");
        active.id = "session-active".to_string();
        repo.create_session(&active).await.unwrap();

        for session_id in ["session-ended", "session-active"] {
            repo.create_media_output(&MediaOutputDbModel::new(
                session_id,
                format!("/tmp/{session_id}.mp4"),
                MediaFileType::Video,
                4096,
            ))
            .await
            .unwrap();
            repo.create_session_segment(&SessionSegmentDbModel::new(
                session_id,
                0,
                format!("/tmp/{session_id}.mp4"),
                12.0,
                4096,
                crate::database::models::SessionSegmentLifecycle::new(None, None),
                crate::database::models::SessionSegmentSplitReason::new(None, None),
            ))
            .await
            .unwrap();
        }

        // The physical delete is what the reaper runs once a retirement
        // acknowledges, and it only touches rows carrying the marker.
        streamer_repo
            .mark_streamer_deleted("streamer-1")
            .await
            .unwrap();
        streamer_repo
            .delete_marked_streamer("streamer-1")
            .await
            .unwrap();

        for session_id in ["session-ended", "session-active"] {
            let session = repo.get_session(session_id).await.unwrap();
            assert_eq!(
                session.streamer_id, None,
                "{session_id} must be orphaned, not deleted"
            );
            assert_eq!(session.streamer_name.as_deref(), Some("Streamer One"));
            assert_eq!(repo.get_output_count(session_id).await.unwrap(), 1);
            assert_eq!(
                repo.list_session_segments_page(session_id, &Pagination::new(10, 0))
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }

        // `trg_live_session_orphan_ends` closes the row the foreign-key action
        // orphaned, so nothing reports it as still recording.
        assert!(
            repo.get_session("session-active")
                .await
                .unwrap()
                .end_time
                .is_some(),
            "an orphaned session must not stay active"
        );
        let (active_sessions, _) = repo
            .list_sessions_filtered(
                &SessionFilters::new().with_active_only(true),
                &Pagination::new(10, 0),
            )
            .await
            .unwrap();
        assert!(active_sessions.is_empty());
    }

    /// The session list keeps searching by streamer name after the streamer is
    /// deleted: `list_sessions_filtered` coalesces the joined `streamers.name`
    /// with the denormalized `live_sessions.streamer_name`.
    #[tokio::test]
    async fn orphaned_sessions_are_still_searchable_by_streamer_name() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let streamer_repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        let mut streamer = StreamerDbModel::new(
            "Streamer One",
            "https://example.com/streamer-1",
            "platform-twitch",
        );
        streamer.id = "streamer-1".to_string();
        streamer_repo.create_streamer(&streamer).await.unwrap();

        let repo = SqlxSessionRepository::new(pool.clone(), pool.clone());
        let mut session = LiveSessionDbModel::new("streamer-1").with_streamer_name("Streamer One");
        session.id = "session-1".to_string();
        session.total_size_bytes = 4096;
        repo.create_session(&session).await.unwrap();
        repo.end_session("session-1", crate::database::time::now_ms())
            .await
            .unwrap();
        repo.create_media_output(&MediaOutputDbModel::new(
            "session-1",
            "/tmp/session-1.mp4",
            MediaFileType::Video,
            4096,
        ))
        .await
        .unwrap();

        // The physical delete is what the reaper runs once a retirement
        // acknowledges, and it only touches rows carrying the marker.
        streamer_repo
            .mark_streamer_deleted("streamer-1")
            .await
            .unwrap();
        streamer_repo
            .delete_marked_streamer("streamer-1")
            .await
            .unwrap();

        let (found, total) = repo
            .list_sessions_filtered(
                &SessionFilters::new().with_search("Streamer One"),
                &Pagination::new(10, 0),
            )
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "session-1");
        assert_eq!(found[0].streamer_id, None);
    }

    /// Startup pipeline recovery skips orphaned sessions:
    /// `PipelineManager` resolves a session-complete pipeline through
    /// `ConfigService::get_config_for_streamer`, which needs a streamer id.
    #[tokio::test]
    async fn pipeline_recovery_skips_sessions_whose_streamer_was_deleted() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let streamer_repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        let mut streamer = StreamerDbModel::new(
            "Streamer One",
            "https://example.com/streamer-1",
            "platform-twitch",
        );
        streamer.id = "streamer-1".to_string();
        streamer_repo.create_streamer(&streamer).await.unwrap();

        let repo = SqlxSessionRepository::new(pool.clone(), pool.clone());
        let mut session = LiveSessionDbModel::new("streamer-1").with_streamer_name("Streamer One");
        session.id = "session-1".to_string();
        repo.create_session(&session).await.unwrap();
        repo.create_media_output(&MediaOutputDbModel::new(
            "session-1",
            "/tmp/session-1.mp4",
            MediaFileType::Video,
            4096,
        ))
        .await
        .unwrap();
        repo.end_session("session-1", crate::database::time::now_ms())
            .await
            .unwrap();

        let pending = repo
            .list_ended_sessions_pending_pipeline_recovery(&Pagination::new(10, 0))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1, "owned session is a recovery candidate");

        // The physical delete is what the reaper runs once a retirement
        // acknowledges, and it only touches rows carrying the marker.
        streamer_repo
            .mark_streamer_deleted("streamer-1")
            .await
            .unwrap();
        streamer_repo
            .delete_marked_streamer("streamer-1")
            .await
            .unwrap();

        let pending = repo
            .list_ended_sessions_pending_pipeline_recovery(&Pagination::new(10, 0))
            .await
            .unwrap();
        assert!(pending.is_empty(), "orphaned session must not be recovered");
    }

    #[tokio::test]
    async fn summarize_outputs_filtered_honours_the_search_filter() {
        let repo = setup_output_filter_repo().await;

        let summaries = repo
            .summarize_outputs_filtered(&OutputFilters::new().with_search("/two."))
            .await
            .unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].file_type, "VIDEO");
        assert_eq!(summaries[0].count, 1);
        assert_eq!(summaries[0].size_bytes, 200);
    }
}
