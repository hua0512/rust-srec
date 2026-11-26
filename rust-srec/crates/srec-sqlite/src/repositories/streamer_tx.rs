//! Transactional operations for streamers.
//!
//! This module provides transaction-aware operations for streamer state management.
//! Use these when you need to update streamer state as part of a larger transaction
//! (e.g., session management, outbox events).

use jiff::Timestamp;
use sqlx::SqliteConnection;

use crate::DbResult as Result;
use crate::models::StreamerDbModel;

/// Transactional operations for streamers.
///
/// These methods operate within an existing transaction and do NOT commit.
/// The caller is responsible for committing or rolling back the transaction.
pub struct StreamerTxOps;

impl StreamerTxOps {
    /// Read administrative authority under the same writer reservation as the
    /// monitor mutation. A stale check may not revive a disabled or retired row.
    pub async fn monitor_may_write(tx: &mut SqliteConnection, streamer_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM streamers WHERE id = ? AND deleted_at IS NULL AND state != 'DISABLED')").bind(streamer_id).fetch_one(tx).await?)
    }
    /// Update streamer state within a transaction.
    pub async fn update_state(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        state: &str,
    ) -> Result<u64> {
        Ok(u64::from(
            Self::update_state_row(tx, streamer_id, state)
                .await?
                .is_some(),
        ))
    }

    pub async fn update_state_row(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        state: &str,
    ) -> Result<Option<StreamerDbModel>> {
        let result = sqlx::query_as::<_, StreamerDbModel>(
            "UPDATE streamers SET state = ? WHERE id = ? RETURNING *",
        )
        .bind(state)
        .bind(streamer_id)
        .fetch_optional(tx)
        .await?;

        Ok(result)
    }

    /// Read the streamer's current `state` column.
    ///
    /// Returns `None` when the row is missing or holds an unknown state
    /// string. Session-start guards call this inside the same
    /// `BEGIN IMMEDIATE` transaction that would write `state = 'LIVE'`, so
    /// the answer is serialized against concurrent state writes (notably
    /// the user-disable commit in `StreamerManager::partial_update_streamer`)
    /// — unlike the in-memory metadata cache, which lags those commits.
    pub async fn get_state(
        tx: &mut SqliteConnection,
        streamer_id: &str,
    ) -> Result<Option<srec_core::domain::StreamerState>> {
        let state: Option<String> = sqlx::query_scalar("SELECT state FROM streamers WHERE id = ?")
            .bind(streamer_id)
            .fetch_optional(tx)
            .await?;

        Ok(state
            .as_deref()
            .and_then(srec_core::domain::StreamerState::parse))
    }

    /// Update the `last_error` column within a transaction.
    ///
    /// Pass `None` to clear the field. Used by infrastructure-level blocks
    /// (like the output-root gate) that need to stamp a distinctive marker
    /// on `last_error` so a later recovery pass can filter affected streamers.
    pub async fn update_last_error_row(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        last_error: Option<&str>,
    ) -> Result<Option<StreamerDbModel>> {
        let result = sqlx::query_as::<_, StreamerDbModel>(
            "UPDATE streamers SET last_error = ? WHERE id = ? RETURNING *",
        )
        .bind(last_error)
        .bind(streamer_id)
        .fetch_optional(tx)
        .await?;
        Ok(result)
    }

    /// Set streamer to LIVE state and record `last_live_time`.
    pub async fn set_live(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        last_live_time: Timestamp,
    ) -> Result<u64> {
        Ok(u64::from(
            Self::set_live_row(tx, streamer_id, last_live_time)
                .await?
                .is_some(),
        ))
    }

    pub async fn set_live_row(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        last_live_time: Timestamp,
    ) -> Result<Option<StreamerDbModel>> {
        let result = sqlx::query_as::<_, StreamerDbModel>(
            r#"
            UPDATE streamers
            SET state = 'LIVE',
                last_live_time = ?
            WHERE id = ? RETURNING *
            "#,
        )
        .bind(last_live_time.as_millisecond())
        .bind(streamer_id)
        .fetch_optional(tx)
        .await?;

        Ok(result)
    }

    /// Set streamer to NOT_LIVE state.
    pub async fn set_offline(tx: &mut SqliteConnection, streamer_id: &str) -> Result<u64> {
        Ok(u64::from(
            Self::set_offline_row(tx, streamer_id).await?.is_some(),
        ))
    }

    pub async fn set_offline_row(
        tx: &mut SqliteConnection,
        streamer_id: &str,
    ) -> Result<Option<StreamerDbModel>> {
        let result = sqlx::query_as::<_, StreamerDbModel>(
            "UPDATE streamers SET state = 'NOT_LIVE' WHERE id = ? RETURNING *",
        )
        .bind(streamer_id)
        .fetch_optional(tx)
        .await?;

        Ok(result)
    }

    /// Update streamer avatar within a transaction.
    pub async fn update_avatar(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        avatar_url: &str,
    ) -> Result<u64> {
        Ok(u64::from(
            Self::update_avatar_row(tx, streamer_id, avatar_url)
                .await?
                .is_some(),
        ))
    }

    pub async fn update_avatar_row(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        avatar_url: &str,
    ) -> Result<Option<StreamerDbModel>> {
        let result = sqlx::query_as::<_, StreamerDbModel>(
            "UPDATE streamers SET avatar = ? WHERE id = ? RETURNING *",
        )
        .bind(avatar_url)
        .bind(streamer_id)
        .fetch_optional(tx)
        .await?;

        Ok(result)
    }

    pub async fn increment_error_row(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        error_message: &str,
    ) -> Result<StreamerDbModel> {
        sqlx::query_as("UPDATE streamers SET consecutive_error_count = COALESCE(consecutive_error_count, 0) + 1, last_error = ? WHERE id = ? RETURNING *")
            .bind(error_message).bind(streamer_id).fetch_optional(tx).await?.ok_or_else(|| crate::DbError::not_found("streamer", streamer_id))
    }

    /// Set disabled_until for temporary backoff.
    ///
    /// When a timestamp is provided, also sets state to TEMPORAL_DISABLED.
    /// When clearing (None), sets state back to NOT_LIVE.
    pub async fn set_disabled_until_row(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        disabled_until: Option<Timestamp>,
    ) -> Result<Option<StreamerDbModel>> {
        let disabled_until_ms = disabled_until.map(|dt| dt.as_millisecond());
        let state = if disabled_until.is_some() {
            "TEMPORAL_DISABLED"
        } else {
            "NOT_LIVE"
        };

        let result = sqlx::query_as::<_, StreamerDbModel>(
            "UPDATE streamers SET disabled_until = ?, state = ? WHERE id = ? RETURNING *",
        )
        .bind(disabled_until_ms)
        .bind(state)
        .bind(streamer_id)
        .fetch_optional(tx)
        .await?;

        Ok(result)
    }

    /// Clear error state (reset consecutive_error_count, disabled_until, last_error).
    pub async fn clear_error_state(tx: &mut SqliteConnection, streamer_id: &str) -> Result<u64> {
        Ok(u64::from(
            Self::clear_error_state_row(tx, streamer_id)
                .await?
                .is_some(),
        ))
    }

    pub async fn clear_error_state_row(
        tx: &mut SqliteConnection,
        streamer_id: &str,
    ) -> Result<Option<StreamerDbModel>> {
        let result = sqlx::query_as::<_, StreamerDbModel>(
            r#"
            UPDATE streamers
            SET consecutive_error_count = 0,
                disabled_until = NULL,
                last_error = NULL
            WHERE id = ? RETURNING *
            "#,
        )
        .bind(streamer_id)
        .fetch_optional(tx)
        .await?;

        Ok(result)
    }

    /// Set a fatal error state (NOT_FOUND, FATAL_ERROR, etc.).
    pub async fn set_fatal_error_row(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        state: &str,
        reason: &str,
    ) -> Result<Option<StreamerDbModel>> {
        let result = sqlx::query_as::<_, StreamerDbModel>(
            "UPDATE streamers SET state = ?, last_error = ? WHERE id = ? RETURNING *",
        )
        .bind(state)
        .bind(reason)
        .bind(streamer_id)
        .fetch_optional(tx)
        .await?;

        Ok(result)
    }
}
