//! Session repository.

use crate::begin_immediate;
use crate::models::{
    DanmuStatisticsDbModel, LiveSessionDbModel, MediaFileType, MediaOutputDbModel, Pagination,
    SessionOutputSummary, SessionSegmentDbModel,
};
use crate::retry::retry_on_sqlite_busy;
use crate::{DbError as Error, DbResult as Result};
use async_trait::async_trait;
use sqlx::{Sqlite, SqlitePool};

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
    async fn update_session_titles(&self, id: &str, titles: &str) -> Result<()>;
    async fn delete_session(&self, id: &str) -> Result<()>;
    async fn delete_sessions_batch(&self, ids: &[String]) -> Result<u64>;

    /// List active (unended) sessions ordered newest-first, including sessions
    /// that have not yet produced retained segments.
    async fn list_active_sessions(
        &self,
        pagination: &Pagination,
    ) -> Result<Vec<LiveSessionDbModel>>;

    /// List ended sessions whose final pipeline has not been published or skipped.
    async fn list_ended_sessions_pending_pipeline_recovery(
        &self,
        pagination: &Pagination,
    ) -> Result<Vec<LiveSessionDbModel>>;

    /// Record that `session_id` no longer needs its session-complete pipeline dispatched.
    async fn mark_session_complete_dispatched(&self, session_id: &str) -> Result<()>;

    /// Sessions whose open recording or unfinished pipeline can still need the
    /// streamer's configuration. Completed history need not be polled by a reaper.
    async fn list_sessions_for_retirement(
        &self,
        streamer_id: &str,
    ) -> Result<Vec<LiveSessionDbModel>> {
        self.list_sessions_for_streamer(streamer_id, i32::MAX).await
    }

    /// Whether the end was handled and its final pipeline was published or
    /// explicitly skipped. An existing final DAG covers a failed receipt write.
    async fn session_pipeline_settled(&self, _session_id: &str) -> Result<bool> {
        // An adapter without durable receipts cannot acknowledge retirement.
        Ok(false)
    }

    // Media Outputs
    async fn get_media_output(&self, id: &str) -> Result<MediaOutputDbModel>;
    async fn get_media_outputs_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<MediaOutputDbModel>>;
    /// Counts all output types and chooses the earliest thumbnail. Sessions without
    /// outputs are omitted. SQL storage uses insertion order to break timestamp ties.
    async fn summarize_session_outputs(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<SessionOutputSummary>> {
        let mut summaries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for session_id in session_ids {
            if !seen.insert(session_id) {
                continue;
            }
            let outputs = self.get_media_outputs_for_session(session_id).await?;
            if outputs.is_empty() {
                continue;
            }
            summaries.push(SessionOutputSummary {
                session_id: session_id.clone(),
                output_count: i64::try_from(outputs.len()).unwrap_or(i64::MAX),
                thumbnail_id: outputs
                    .into_iter()
                    .find(|output| output.file_type == MediaFileType::Thumbnail.as_str())
                    .map(|output| output.id),
            });
        }
        Ok(summaries)
    }

    async fn first_thumbnail_id(&self, session_id: &str) -> Result<Option<String>> {
        Ok(self
            .get_media_outputs_for_session(session_id)
            .await?
            .into_iter()
            .find(|output| output.file_type == MediaFileType::Thumbnail.as_str())
            .map(|output| output.id))
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
        let mut counts = Vec::new();
        for session_id in session_ids {
            if let Some(stats) = self.get_danmu_statistics(session_id).await? {
                counts.push((stats.session_id, stats.total_danmus));
            }
        }
        Ok(counts)
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
    async fn list_sessions_for_retirement(
        &self,
        streamer_id: &str,
    ) -> Result<Vec<LiveSessionDbModel>> {
        Ok(sqlx::query_as(
            "SELECT session.* FROM live_sessions session WHERE session.streamer_id = ? AND (
                session.end_time IS NULL OR session.session_complete_dispatched = 0
                OR EXISTS(SELECT 1 FROM job WHERE session_id = session.id AND status IN ('PENDING','PROCESSING'))
                OR EXISTS(SELECT 1 FROM dag_execution WHERE session_id = session.id AND status IN ('PENDING','PROCESSING'))
            ) ORDER BY session.start_time, session.id",
        ).bind(streamer_id).fetch_all(&self.pool).await?)
    }

    async fn session_pipeline_settled(&self, session_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT end_time IS NOT NULL AND (session_complete_dispatched = 1 OR EXISTS(
                SELECT 1 FROM dag_execution WHERE session_id = live_sessions.id AND segment_source = 'session_complete'
            )) FROM live_sessions WHERE id = ?",
        ).bind(session_id).fetch_optional(&self.pool).await?.unwrap_or(true))
    }

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

        super::fetch_in_chunks(
            &self.pool,
            "SELECT * FROM live_sessions WHERE id IN (",
            &super::unique_lookup_ids(ids),
            ")",
        )
        .await
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
            let mut connection = self.write_pool.acquire().await?;
            super::session_tx::writes::insert_session(&mut connection, session).await?;
            Ok(())
        })
        .await
    }

    async fn end_session(&self, id: &str, end_time: i64) -> Result<()> {
        retry_on_sqlite_busy("end_session", || async {
            let mut connection = self.write_pool.acquire().await?;
            super::session_tx::writes::end_session(
                &mut connection,
                id,
                end_time,
                super::session_tx::writes::EndCondition::Any,
            )
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
            let mut tx = crate::begin_immediate(&self.write_pool).await?;
            let status: Option<(bool, bool, bool)> = sqlx::query_as(
                "SELECT \
                    end_time IS NOT NULL, \
                    EXISTS(SELECT 1 FROM job j WHERE j.session_id = live_sessions.id AND j.status IN ('PENDING', 'PROCESSING')), \
                    EXISTS(SELECT 1 FROM dag_execution d WHERE d.session_id = live_sessions.id AND d.status IN ('PENDING', 'PROCESSING')) \
                 FROM live_sessions WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;

            let Some((is_ended, has_active_job, has_active_dag)) = status else {
                return Err(Error::not_found("Session", id));
            };
            if !is_ended {
                return Err(Error::conflict(format!(
                    "Cannot delete active session '{id}' while it is still live"
                )));
            }
            if has_active_job || has_active_dag {
                return Err(Error::conflict(format!(
                    "Cannot delete session '{id}' while pipeline jobs are pending or processing"
                )));
            }

            sqlx::query("DELETE FROM live_sessions WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn delete_sessions_batch(&self, ids: &[String]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }

        let unique_ids = super::unique_lookup_ids(ids);
        retry_on_sqlite_busy("delete_sessions_batch", || async {
            let mut tx = crate::begin_immediate(&self.write_pool).await?;

            for chunk in unique_ids.chunks(super::LOOKUP_BATCH_SIZE) {
                let mut check = sqlx::QueryBuilder::<Sqlite>::new(
                    "SELECT id, end_time IS NULL FROM live_sessions WHERE id IN (",
                );
                let mut sep = check.separated(", ");
                for id in chunk {
                    sep.push_bind(*id);
                }
                sep.push_unseparated(
                    ") AND (end_time IS NULL \
                        OR EXISTS(SELECT 1 FROM job j WHERE j.session_id = live_sessions.id AND j.status IN ('PENDING', 'PROCESSING')) \
                        OR EXISTS(SELECT 1 FROM dag_execution d WHERE d.session_id = live_sessions.id AND d.status IN ('PENDING', 'PROCESSING'))) \
                     LIMIT 1",
                );
                if let Some((blocking_id, is_live)) = check
                    .build_query_as::<(String, bool)>()
                    .fetch_optional(&mut *tx)
                    .await?
                {
                    if is_live {
                        return Err(Error::conflict(format!(
                            "Cannot delete active session '{blocking_id}' while it is still live"
                        )));
                    }
                    return Err(Error::conflict(format!(
                        "Cannot delete session '{blocking_id}' while pipeline jobs are pending or processing"
                    )));
                }
            }

            let mut total_deleted = 0_u64;
            for chunk in unique_ids.chunks(super::LOOKUP_BATCH_SIZE) {
                let mut del =
                    sqlx::QueryBuilder::<Sqlite>::new("DELETE FROM live_sessions WHERE id IN (");
                let mut sep = del.separated(", ");
                for id in chunk {
                    sep.push_bind(*id);
                }
                sep.push_unseparated(")");
                let result = del.build().execute(&mut *tx).await?;
                total_deleted += result.rows_affected();
            }

            tx.commit().await?;
            Ok(total_deleted)
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

    async fn summarize_session_outputs(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<SessionOutputSummary>> {
        if session_ids.is_empty() {
            return Ok(Vec::new());
        }
        // The existing (session_id, created_at) index covers the count and
        // supplies insertion order (rowid) for equal thumbnail timestamps.
        super::fetch_in_chunks(
            &self.pool,
            "SELECT m.session_id, COUNT(*) AS output_count, \
             (SELECT t.id FROM media_outputs t WHERE t.session_id = m.session_id \
              AND t.file_type = 'THUMBNAIL' ORDER BY t.created_at, t.rowid LIMIT 1) AS thumbnail_id \
             FROM media_outputs m WHERE m.session_id IN (",
            &super::unique_lookup_ids(session_ids),
            ") GROUP BY m.session_id",
        )
        .await
    }

    async fn first_thumbnail_id(&self, session_id: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM media_outputs WHERE session_id = ? AND file_type = 'THUMBNAIL' \
             ORDER BY created_at, rowid LIMIT 1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn create_media_output(&self, output: &MediaOutputDbModel) -> Result<()> {
        retry_on_sqlite_busy("create_media_output", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;

            super::session_tx::writes::insert_media_output(&mut tx, output).await?;

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

            super::session_tx::writes::insert_media_output(&mut tx, output).await?;

            super::session_tx::writes::insert_segment(&mut tx, segment).await?;

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn create_session_segment(&self, segment: &SessionSegmentDbModel) -> Result<()> {
        retry_on_sqlite_busy("create_session_segment", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;

            super::session_tx::writes::insert_segment(&mut tx, segment).await?;

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
        retry_on_sqlite_busy("delete_media_output", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;

            super::session_tx::SessionTxOps::delete_media_output(&mut tx, id).await?;

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

        super::fetch_in_chunks(
            &self.pool,
            "SELECT session_id, total_danmus FROM danmu_statistics WHERE session_id IN (",
            &super::unique_lookup_ids(session_ids),
            ") ORDER BY session_id",
        )
        .await
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
            .bind(jiff::Timestamp::now().as_millisecond())
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

    async fn list_active_sessions(
        &self,
        pagination: &Pagination,
    ) -> Result<Vec<LiveSessionDbModel>> {
        let sessions = sqlx::query_as::<_, LiveSessionDbModel>(
            "SELECT * FROM live_sessions WHERE end_time IS NULL ORDER BY start_time DESC LIMIT ? OFFSET ?",
        )
        .bind(i64::from(pagination.limit))
        .bind(i64::from(pagination.offset))
        .fetch_all(&self.pool)
        .await?;
        Ok(sessions)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        LiveSessionDbModel, MediaFileType, MediaOutputDbModel, Pagination, StreamerDbModel,
    };
    use crate::repositories::{SqlxStreamerRepository, StreamerRepository as _};
    use crate::{init_pool_with_size, run_migrations};

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
                crate::models::SessionSegmentLifecycle::default(),
                crate::models::SessionSegmentSplitReason::default(),
            );
            repo.create_session_segment(&segment).await.unwrap();
        }

        let next = repo.next_session_segment_index("session-1").await.unwrap();

        assert_eq!(next, 4);
    }

    #[tokio::test]
    async fn ended_pipeline_recovery_includes_empty_sessions_until_explicitly_settled() {
        use crate::models::{DagExecutionDbModel, DagPipelineDefinition};
        use crate::repositories::{DagRepository as _, SqlxDagRepository};

        let repo = setup_test_repo().await;
        let ended_at = crate::time::now_ms();
        repo.end_session("session-1", ended_at).await.unwrap();
        assert_eq!(
            repo.list_ended_sessions_pending_pipeline_recovery(&Pagination::new(10, 0))
                .await
                .unwrap()
                .len(),
            1
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
        use crate::models::{DagExecutionDbModel, DagPipelineDefinition};
        use crate::repositories::{DagRepository as _, SqlxDagRepository};

        let repo = setup_test_repo().await;
        repo.end_session("session-1", crate::time::now_ms())
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
    async fn session_output_projection_preserves_counts_and_thumbnail_selection() {
        let repo = setup_test_repo().await;
        let mut empty = LiveSessionDbModel::new("streamer-1");
        empty.id = "empty".to_string();
        empty.end_time = Some(1);
        repo.create_session(&empty).await.unwrap();
        for (id, kind, created_at) in [
            ("video", MediaFileType::Video, 0),
            ("late", MediaFileType::Thumbnail, 20),
            ("z-first", MediaFileType::Thumbnail, 10),
            ("a-second", MediaFileType::Thumbnail, 10),
            ("audio", MediaFileType::Audio, 10),
            ("danmu", MediaFileType::DanmuXml, 10),
        ] {
            let mut output = MediaOutputDbModel::new("session-1", id, kind, 1);
            output.id = id.to_string();
            output.created_at = created_at;
            repo.create_media_output(&output).await.unwrap();
        }
        for ids in [
            vec![],
            vec!["empty".to_string(), "absent".to_string()],
            vec![
                "session-1".to_string(),
                "session-1".to_string(),
                "empty".to_string(),
            ],
        ] {
            let summaries = repo.summarize_session_outputs(&ids).await.unwrap();
            if !ids.iter().any(|id| id == "session-1") {
                assert!(summaries.is_empty());
                continue;
            }
            assert_eq!(summaries.len(), 1);
            assert_eq!(summaries[0].output_count, 6);
            assert_eq!(summaries[0].thumbnail_id.as_deref(), Some("z-first"));
        }
        assert_eq!(
            repo.first_thumbnail_id("session-1")
                .await
                .unwrap()
                .as_deref(),
            Some("z-first")
        );
        assert!(repo.first_thumbnail_id("empty").await.unwrap().is_none());
        repo.delete_media_output("z-first").await.unwrap();
        assert_eq!(
            repo.first_thumbnail_id("session-1")
                .await
                .unwrap()
                .as_deref(),
            Some("a-second")
        );
        for id in ["a-second", "late"] {
            repo.delete_media_output(id).await.unwrap();
        }
        let summaries = repo
            .summarize_session_outputs(&["session-1".to_string()])
            .await
            .unwrap();
        assert_eq!(summaries[0].output_count, 3);
        assert_eq!(summaries[0].thumbnail_id, None);
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
        repo.end_session("session-ended", crate::time::now_ms())
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
                crate::models::SessionSegmentLifecycle::new(None, None),
                crate::models::SessionSegmentSplitReason::new(None, None),
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
        let active_sessions = repo
            .list_active_sessions(&Pagination::new(10, 0))
            .await
            .unwrap();
        assert!(active_sessions.is_empty());
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
        repo.end_session("session-1", crate::time::now_ms())
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
}
