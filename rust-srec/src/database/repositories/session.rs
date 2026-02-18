//! Session repository.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::database::models::{
    DanmuStatisticsDbModel, LiveSessionDbModel, MediaOutputDbModel, OutputFilters, Pagination,
    SessionFilters,
};
use crate::database::retry::retry_on_sqlite_busy;
use crate::{Error, Result};

/// Session repository trait.
#[async_trait]
pub trait SessionRepository: Send + Sync {
    // Live Sessions
    async fn get_session(&self, id: &str) -> Result<LiveSessionDbModel>;
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

    // Media Outputs
    async fn get_media_output(&self, id: &str) -> Result<MediaOutputDbModel>;
    async fn get_media_outputs_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<MediaOutputDbModel>>;
    async fn create_media_output(&self, output: &MediaOutputDbModel) -> Result<()>;
    async fn delete_media_output(&self, id: &str) -> Result<()>;

    /// Get the count of media outputs for a session.
    async fn get_output_count(&self, session_id: &str) -> Result<u32>;

    /// List media outputs with optional filters and pagination.
    /// Returns a tuple of (outputs, total_count).
    async fn list_outputs_filtered(
        &self,
        filters: &OutputFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<MediaOutputDbModel>, u64)>;

    // Danmu Statistics
    async fn get_danmu_statistics(
        &self,
        session_id: &str,
    ) -> Result<Option<DanmuStatisticsDbModel>>;
    async fn create_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()>;
    async fn update_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()>;
    async fn upsert_danmu_statistics(
        &self,
        session_id: &str,
        total_danmus: i64,
        danmu_rate_timeseries: Option<&str>,
        top_talkers: Option<&str>,
        word_frequency: Option<&str>,
    ) -> Result<()>;
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
                INSERT INTO live_sessions (id, streamer_id, start_time, end_time, titles, danmu_statistics_id, total_size_bytes)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&session.id)
            .bind(&session.streamer_id)
            .bind(session.start_time)
            .bind(session.end_time)
            .bind(&session.titles)
            .bind(&session.danmu_statistics_id)
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

            let mut query = sqlx::query(&sql);
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

    async fn create_media_output(&self, output: &MediaOutputDbModel) -> Result<()> {
        retry_on_sqlite_busy("create_media_output", || async {
            let mut conn = self.write_pool.acquire().await?;
            sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

            let result: Result<()> = async {
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
                .execute(&mut *conn)
                .await?;

                // Update session total size
                sqlx::query(
                    "UPDATE live_sessions SET total_size_bytes = total_size_bytes + ? WHERE id = ?",
                )
                .bind(output.size_bytes)
                .bind(&output.session_id)
                .execute(&mut *conn)
                .await?;

                Ok(())
            }
            .await;

            match result {
                Ok(()) => {
                    sqlx::query("COMMIT").execute(&mut *conn).await?;
                    Ok(())
                }
                Err(err) => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                    Err(err)
                }
            }
        })
        .await
    }

    async fn delete_media_output(&self, id: &str) -> Result<()> {
        // Get output info before deletion to update session size
        let output = self.get_media_output(id).await?;

        retry_on_sqlite_busy("delete_media_output", || async {
            let mut conn = self.write_pool.acquire().await?;
            sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

            let result: Result<()> = async {
                sqlx::query("DELETE FROM media_outputs WHERE id = ?")
                    .bind(id)
                    .execute(&mut *conn)
                    .await?;

                // Update session total size
                sqlx::query(
                    "UPDATE live_sessions SET total_size_bytes = total_size_bytes - ? WHERE id = ?",
                )
                .bind(output.size_bytes)
                .bind(&output.session_id)
                .execute(&mut *conn)
                .await?;

                Ok(())
            }
            .await;

            match result {
                Ok(()) => {
                    sqlx::query("COMMIT").execute(&mut *conn).await?;
                    Ok(())
                }
                Err(err) => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                    Err(err)
                }
            }
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
            sqlx::query(
                r#"
                INSERT INTO danmu_statistics (id, session_id, total_danmus, danmu_rate_timeseries, top_talkers, word_frequency)
                VALUES (?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&stats.id)
            .bind(&stats.session_id)
            .bind(stats.total_danmus)
            .bind(&stats.danmu_rate_timeseries)
            .bind(&stats.top_talkers)
            .bind(&stats.word_frequency)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn update_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()> {
        retry_on_sqlite_busy("update_danmu_statistics", || async {
            sqlx::query(
                r#"
                UPDATE danmu_statistics SET
                    total_danmus = ?,
                    danmu_rate_timeseries = ?,
                    top_talkers = ?,
                    word_frequency = ?
                WHERE id = ?
                "#,
            )
            .bind(stats.total_danmus)
            .bind(&stats.danmu_rate_timeseries)
            .bind(&stats.top_talkers)
            .bind(&stats.word_frequency)
            .bind(&stats.id)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn upsert_danmu_statistics(
        &self,
        session_id: &str,
        total_danmus: i64,
        danmu_rate_timeseries: Option<&str>,
        top_talkers: Option<&str>,
        word_frequency: Option<&str>,
    ) -> Result<()> {
        retry_on_sqlite_busy("upsert_danmu_statistics", || async {
            sqlx::query(
                r#"
                INSERT INTO danmu_statistics (id, session_id, total_danmus, danmu_rate_timeseries, top_talkers, word_frequency)
                VALUES (?, ?, ?, ?, ?, ?)
                ON CONFLICT(session_id) DO UPDATE SET
                    total_danmus = excluded.total_danmus,
                    danmu_rate_timeseries = excluded.danmu_rate_timeseries,
                    top_talkers = excluded.top_talkers,
                    word_frequency = excluded.word_frequency
                "#,
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(session_id)
            .bind(total_danmus)
            .bind(danmu_rate_timeseries)
            .bind(top_talkers)
            .bind(word_frequency)
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

        if filters.search.is_some() {
            conditions.push("(st.name LIKE ? OR s.titles LIKE ? OR s.id LIKE ?)".to_string());
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
        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);

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
        let mut data_query = sqlx::query_as::<_, LiveSessionDbModel>(&data_sql);

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
        // Determine if we need to join with live_sessions for streamer_id filter
        let needs_join = filters.streamer_id.is_some();

        // Build dynamic WHERE clause
        let mut conditions: Vec<String> = Vec::new();

        if filters.session_id.is_some() {
            conditions.push("m.session_id = ?".to_string());
        }
        if filters.streamer_id.is_some() {
            conditions.push("s.streamer_id = ?".to_string());
        }

        if filters.search.is_some() {
            conditions.push(
                "(m.file_path LIKE ? OR m.session_id LIKE ? OR m.file_type LIKE ?)".to_string(),
            );
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        // Build FROM clause with optional join
        let from_clause = if needs_join {
            "media_outputs m INNER JOIN live_sessions s ON m.session_id = s.id"
        } else {
            "media_outputs m"
        };

        // Count query
        let count_sql = format!("SELECT COUNT(*) FROM {} {}", from_clause, where_clause);

        // Data query with pagination, ordered by created_at descending
        // Select only media_outputs columns to avoid ambiguity
        let data_sql = format!(
            "SELECT m.id, m.session_id, m.parent_media_output_id, m.file_path, m.file_type, m.size_bytes, m.created_at \
             FROM {} {} ORDER BY m.created_at DESC LIMIT ? OFFSET ?",
            from_clause, where_clause
        );

        // Execute count query
        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);

        // Bind parameters for count query
        if let Some(session_id) = &filters.session_id {
            count_query = count_query.bind(session_id);
        }
        if let Some(streamer_id) = &filters.streamer_id {
            count_query = count_query.bind(streamer_id);
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
        let mut data_query = sqlx::query_as::<_, MediaOutputDbModel>(&data_sql);

        // Bind parameters for data query
        if let Some(session_id) = &filters.session_id {
            data_query = data_query.bind(session_id);
        }
        if let Some(streamer_id) = &filters.streamer_id {
            data_query = data_query.bind(streamer_id);
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

        let outputs = data_query.fetch_all(&self.pool).await?;

        Ok((outputs, total_count))
    }
}
