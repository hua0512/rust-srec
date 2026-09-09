//! Row binding ownership for session writes. Callers retain commit and retry ownership.

use sqlx::SqliteConnection;

use crate::Result;
use crate::database::models::{LiveSessionDbModel, MediaOutputDbModel, SessionSegmentDbModel};

const INSERT_SESSION: &str = r#"
    INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time, titles, total_size_bytes)
    VALUES (?, ?, ?, ?, ?, ?, ?)
"#;

const END_SESSION: &str = r#"
    UPDATE live_sessions
    SET end_time = ?,
        total_size_bytes = (SELECT COALESCE(SUM(size_bytes), 0) FROM media_outputs WHERE session_id = ?)
    WHERE id = ? AND (? OR end_time IS NULL)
"#;

const INSERT_MEDIA: &str = r#"
    INSERT INTO media_outputs (id, session_id, parent_media_output_id, file_path, file_type, size_bytes, created_at)
    VALUES (?, ?, ?, ?, ?, ?, ?)
"#;

const INSERT_SEGMENT: &str = r#"
    INSERT INTO session_segments (
        id, session_id, segment_index, file_path, duration_secs, size_bytes,
        split_reason_code, split_reason_details_json, created_at, completed_at, persisted_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

pub(crate) enum EndCondition {
    Any,
    ActiveOnly,
}

pub(crate) async fn insert_session(
    connection: &mut SqliteConnection,
    session: &LiveSessionDbModel,
) -> Result<()> {
    sqlx::query(INSERT_SESSION)
        .bind(&session.id)
        .bind(&session.streamer_id)
        .bind(&session.streamer_name)
        .bind(session.start_time)
        .bind(session.end_time)
        .bind(&session.titles)
        .bind(session.total_size_bytes)
        .execute(connection)
        .await?;
    Ok(())
}

pub(crate) async fn end_session(
    connection: &mut SqliteConnection,
    id: &str,
    end_time_ms: i64,
    condition: EndCondition,
) -> Result<u64> {
    let result = sqlx::query(END_SESSION)
        .bind(end_time_ms)
        .bind(id)
        .bind(id)
        .bind(matches!(condition, EndCondition::Any))
        .execute(connection)
        .await?;
    Ok(result.rows_affected())
}

/// The caller must hold a transaction so row insertion and total-size accounting
/// commit together, including when a subsequent segment insertion fails.
pub(crate) async fn insert_media_output(
    connection: &mut SqliteConnection,
    output: &MediaOutputDbModel,
) -> Result<()> {
    sqlx::query(INSERT_MEDIA)
        .bind(&output.id)
        .bind(&output.session_id)
        .bind(&output.parent_media_output_id)
        .bind(&output.file_path)
        .bind(&output.file_type)
        .bind(output.size_bytes)
        .bind(output.created_at)
        .execute(&mut *connection)
        .await?;
    sqlx::query("UPDATE live_sessions SET total_size_bytes = total_size_bytes + ? WHERE id = ?")
        .bind(output.size_bytes)
        .bind(&output.session_id)
        .execute(connection)
        .await?;
    Ok(())
}

pub(crate) async fn insert_segment(
    connection: &mut SqliteConnection,
    segment: &SessionSegmentDbModel,
) -> Result<()> {
    sqlx::query(INSERT_SEGMENT)
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
        .execute(connection)
        .await?;
    Ok(())
}
