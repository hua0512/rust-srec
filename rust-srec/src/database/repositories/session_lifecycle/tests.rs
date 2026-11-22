use jiff::Timestamp;
use sqlx::{Row, SqlitePool};

use super::*;
use crate::database::models::StreamerDbModel;
use crate::database::repositories::{SqlxStreamerRepository, StreamerRepository as _};
use crate::database::{init_pool_with_size, run_migrations};
use crate::domain::StreamerState;
use crate::session::events::{SessionEventPayload, TerminalCauseDto};

const STREAMER_ID: &str = "test-streamer";

async fn setup_pool() -> SqlitePool {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    create_streamer(&pool, STREAMER_ID, "Test").await;
    pool
}

async fn create_streamer(pool: &SqlitePool, id: &str, name: &str) {
    let mut streamer =
        StreamerDbModel::new(name, format!("https://example.com/{id}"), "platform-twitch");
    streamer.id = id.to_string();
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .create_streamer(&streamer)
        .await
        .unwrap();
}

fn start_inputs(now: Timestamp) -> StartSessionInputs {
    StartSessionInputs {
        credential_binding: None,
        streamer_id: STREAMER_ID.to_string(),
        streamer_name: "Test".to_string(),
        streamer_url: format!("https://example.com/{STREAMER_ID}"),
        current_avatar: None,
        new_avatar: None,
        title: "Live!".to_string(),
        category: None,
        streams: vec![],
        media_headers: None,
        media_extras: None,
        now,
    }
}

fn end_inputs(
    session_id: Option<String>,
    state_was_live: bool,
    now: Timestamp,
) -> EndSessionInputs {
    EndSessionInputs {
        streamer_id: STREAMER_ID.to_string(),
        streamer_name: "Test".to_string(),
        session_id,
        state_was_live,
        clear_errors: false,
        checks_restored_threshold: None,
        cause: TerminalCauseDto::StreamerOffline,
        via_hysteresis: false,
        now,
    }
}

async fn streamer_state(pool: &SqlitePool) -> String {
    sqlx::query("SELECT state FROM streamers WHERE id = ?")
        .bind(STREAMER_ID)
        .fetch_one(pool)
        .await
        .unwrap()
        .get::<String, _>(0)
}

async fn set_streamer_state(pool: &SqlitePool, state: &str) {
    sqlx::query("UPDATE streamers SET state = ? WHERE id = ?")
        .bind(state)
        .bind(STREAMER_ID)
        .execute(pool)
        .await
        .unwrap();
}

async fn outbox_event_types(pool: &SqlitePool) -> Vec<String> {
    sqlx::query("SELECT event_type FROM monitor_event_outbox ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.get::<String, _>(0))
        .collect()
}

#[tokio::test]
async fn repeated_end_preserves_timestamp_audit_and_offline_observation() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();
    let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
    let sid = started.session_id();
    assert_eq!(
        repo.end_session_only(
            STREAMER_ID,
            Some(sid),
            TerminalCauseDto::Completed,
            false,
            now
        )
        .await
        .unwrap()
        .as_deref(),
        Some(sid)
    );
    let later = now + jiff::SignedDuration::from_secs(10);
    assert!(
        repo.end_session_only(
            STREAMER_ID,
            Some(sid),
            TerminalCauseDto::Completed,
            false,
            later
        )
        .await
        .unwrap()
        .is_none()
    );
    let offline = repo
        .end(end_inputs(Some(sid.to_string()), true, later))
        .await
        .unwrap();
    assert!(offline.resolved_session_id.is_none());
    assert!(offline.offline_event_emitted);
    assert_eq!(streamer_state(&pool).await, "NOT_LIVE");
    let duplicate = repo
        .end(end_inputs(Some(sid.to_string()), true, later))
        .await
        .unwrap();
    assert!(duplicate.resolved_session_id.is_none());
    assert!(!duplicate.offline_event_emitted);
    let end_time: i64 = sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
        .bind(sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(end_time, now.as_millisecond());
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_events WHERE session_id = ? AND kind = 'session_ended'",
    )
    .bind(sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn offline_rejects_session_owned_by_another_streamer() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    create_streamer(&pool, "other-streamer", "Other").await;
    let now = Timestamp::now();
    let mut inputs = start_inputs(now);
    inputs.streamer_id = "other-streamer".to_string();
    let started = repo.start_or_resume(inputs).await.unwrap();
    set_streamer_state(&pool, "LIVE").await;
    let events_before = outbox_event_types(&pool).await;
    let outcome = repo
        .end(end_inputs(
            Some(started.session_id().to_string()),
            true,
            now,
        ))
        .await
        .unwrap();
    assert!(outcome.resolved_session_id.is_none());
    assert!(!outcome.offline_event_emitted);
    assert!(repo.is_session_active(started.session_id()).await.unwrap());
    assert_eq!(streamer_state(&pool).await, "LIVE");
    assert_eq!(outbox_event_types(&pool).await, events_before);
}

#[tokio::test]
async fn offline_preserves_inactive_streamer_state() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();
    let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
    set_streamer_state(&pool, "DISABLED").await;
    let outcome = repo
        .end(end_inputs(
            Some(started.session_id().to_string()),
            true,
            now,
        ))
        .await
        .unwrap();
    assert_eq!(
        outcome.resolved_session_id.as_deref(),
        Some(started.session_id())
    );
    assert!(!outcome.offline_event_emitted);
    assert_eq!(streamer_state(&pool).await, "DISABLED");
}

#[tokio::test]
async fn cause_rewrite_preserves_authoritative_and_direct_ends() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    for (cause, via_hysteresis) in [
        (TerminalCauseDto::StreamerOffline, true),
        (TerminalCauseDto::Completed, false),
    ] {
        let now = Timestamp::now();
        let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
        repo.end_session_only(
            STREAMER_ID,
            Some(started.session_id()),
            cause.clone(),
            via_hysteresis,
            now,
        )
        .await
        .unwrap();
        assert!(
            !repo
                .rewrite_session_ended_cause(started.session_id(), TerminalCauseDto::UserDisabled)
                .await
                .unwrap()
        );
        let payload: String = sqlx::query_scalar(
            "SELECT payload FROM session_events WHERE session_id = ? AND kind = 'session_ended'",
        )
        .bind(started.session_id())
        .fetch_one(&pool)
        .await
        .unwrap();
        let parsed: SessionEventPayload = serde_json::from_str(&payload).unwrap();
        assert!(
            matches!(parsed, SessionEventPayload::SessionEnded { cause: actual, .. } if actual == cause)
        );
    }
}

#[tokio::test]
async fn end_with_no_id_falls_back_to_active_session() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();

    let started = repo.start_or_resume(start_inputs(now)).await.unwrap();

    let outcome = repo
        .end(end_inputs(
            None,
            true,
            now + jiff::SignedDuration::from_secs(10),
        ))
        .await
        .unwrap();

    assert_eq!(
        outcome.resolved_session_id.as_deref(),
        Some(started.session_id())
    );
    assert!(outcome.offline_event_emitted);
}

#[tokio::test]
async fn end_session_only_closes_row_without_touching_streamer_or_outbox() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();

    let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
    let id = started.session_id().to_string();

    let resolved = repo
        .end_session_only(
            STREAMER_ID,
            Some(&id),
            TerminalCauseDto::Completed,
            false,
            now + jiff::SignedDuration::from_secs(5),
        )
        .await
        .unwrap();

    assert_eq!(resolved.as_deref(), Some(id.as_str()));
    assert_eq!(streamer_state(&pool).await, "LIVE");
    assert_eq!(outbox_event_types(&pool).await, vec!["StreamerLive"]);
}

#[tokio::test]
async fn end_for_out_of_schedule_updates_state_and_state_changed_outbox() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();

    let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
    let sid = started.session_id().to_string();

    let resolved = repo
        .end_for_out_of_schedule(EndForOutOfScheduleInputs {
            streamer_id: STREAMER_ID.to_string(),
            streamer_name: "Test".to_string(),
            session_id: Some(sid.clone()),
            old_state: StreamerState::Live,
            via_hysteresis: false,
            now: now + jiff::SignedDuration::from_secs(5),
        })
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(sid.as_str()));

    let end_time: Option<i64> =
        sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(end_time.is_some());

    assert_eq!(streamer_state(&pool).await, "OUT_OF_SCHEDULE");
    assert_eq!(
        outbox_event_types(&pool).await,
        vec!["StreamerLive", "StateChanged"]
    );

    let payload: String = sqlx::query_scalar(
        "SELECT payload FROM session_events
         WHERE session_id = ? AND kind = 'session_ended'
         ORDER BY id DESC LIMIT 1",
    )
    .bind(&sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(payload.contains("\"out_of_schedule\""), "got: {payload}");
    assert!(payload.contains("\"via_hysteresis\":false"));
}

#[tokio::test]
async fn end_for_out_of_schedule_already_out_of_schedule_without_session_is_noop_event() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());

    sqlx::query("UPDATE streamers SET state = 'OUT_OF_SCHEDULE' WHERE id = ?")
        .bind(STREAMER_ID)
        .execute(&pool)
        .await
        .unwrap();

    let resolved = repo
        .end_for_out_of_schedule(EndForOutOfScheduleInputs {
            streamer_id: STREAMER_ID.to_string(),
            streamer_name: "Test".to_string(),
            session_id: None,
            old_state: StreamerState::OutOfSchedule,
            via_hysteresis: false,
            now: Timestamp::now(),
        })
        .await
        .unwrap();
    assert!(resolved.is_none());
    assert!(outbox_event_types(&pool).await.is_empty());
}

#[tokio::test]
async fn end_for_out_of_schedule_already_out_of_schedule_with_session_still_emits_event() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();

    let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
    let sid = started.session_id().to_string();
    set_streamer_state(&pool, "OUT_OF_SCHEDULE").await;

    let resolved = repo
        .end_for_out_of_schedule(EndForOutOfScheduleInputs {
            streamer_id: STREAMER_ID.to_string(),
            streamer_name: "Test".to_string(),
            session_id: Some(sid.clone()),
            old_state: StreamerState::OutOfSchedule,
            via_hysteresis: false,
            now: now + jiff::SignedDuration::from_secs(5),
        })
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(sid.as_str()));
    assert_eq!(
        outbox_event_types(&pool).await,
        vec!["StreamerLive", "StateChanged"]
    );
}

#[tokio::test]
async fn start_or_resume_cleans_all_stale_active_when_most_recent_is_ended() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();

    sqlx::query("DROP INDEX live_sessions_one_active_per_streamer")
        .execute(&pool)
        .await
        .unwrap();
    for (id, offset, ended) in [
        ("stale-A", -180, false),
        ("stale-B", -120, false),
        ("stale-C", -60, true),
    ] {
        let end_time =
            ended.then(|| (now + jiff::SignedDuration::from_secs(offset + 30)).as_millisecond());
        sqlx::query(
            "INSERT INTO live_sessions (id, streamer_id, start_time, end_time, titles, total_size_bytes)
             VALUES (?, ?, ?, ?, '[]', 0)",
        )
        .bind(id)
        .bind(STREAMER_ID)
        .bind((now + jiff::SignedDuration::from_secs(offset)).as_millisecond())
        .bind(end_time)
        .execute(&pool)
        .await
        .unwrap();
    }

    let outcome = repo.start_or_resume(start_inputs(now)).await.unwrap();
    assert!(matches!(outcome, StartSessionOutcome::Created { .. }));

    let active: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM live_sessions WHERE streamer_id = ? AND end_time IS NULL",
    )
    .bind(STREAMER_ID)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(active, vec![outcome.session_id().to_string()]);

    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM session_events
         WHERE streamer_id = ? ORDER BY occurred_at ASC, id ASC",
    )
    .bind(STREAMER_ID)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        kinds,
        vec!["session_ended", "session_ended", "session_started"]
    );
}

#[tokio::test]
async fn start_or_resume_cleans_stale_active_keeping_most_recent() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();

    sqlx::query("DROP INDEX live_sessions_one_active_per_streamer")
        .execute(&pool)
        .await
        .unwrap();
    for (id, offset) in [("stale-A", -120), ("stale-B", -60)] {
        sqlx::query(
            "INSERT INTO live_sessions (id, streamer_id, start_time, end_time, titles, total_size_bytes)
             VALUES (?, ?, ?, NULL, '[]', 0)",
        )
        .bind(id)
        .bind(STREAMER_ID)
        .bind((now + jiff::SignedDuration::from_secs(offset)).as_millisecond())
        .execute(&pool)
        .await
        .unwrap();
    }

    let outcome = repo.start_or_resume(start_inputs(now)).await.unwrap();
    match outcome {
        StartSessionOutcome::ReusedActive { ref session_id } => {
            assert_eq!(
                session_id, "stale-B",
                "the most-recent active row is reused"
            );
        }
        other => panic!("expected ReusedActive(stale-B), got {other:?}"),
    }

    let active: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM live_sessions
         WHERE streamer_id = ? AND end_time IS NULL",
    )
    .bind(STREAMER_ID)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(active, vec!["stale-B".to_string()]);

    let events: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT kind, payload FROM session_events
         WHERE streamer_id = ? ORDER BY occurred_at ASC, id ASC",
    )
    .bind(STREAMER_ID)
    .fetch_all(&pool)
    .await
    .unwrap();
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, vec!["session_ended"]);

    let (_, payload) = &events[0];
    let parsed: serde_json::Value = serde_json::from_str(payload.as_deref().unwrap()).unwrap();
    assert_eq!(parsed["cause"]["type"], "rejected");
    assert_eq!(parsed["cause"]["reason"], "stale_active_replaced");
    assert_eq!(parsed["via_hysteresis"], false);
}

#[tokio::test]
async fn end_for_disable_atomic_writes() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();

    let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
    let sid = started.session_id().to_string();

    let resolved = repo
        .end_for_disable(
            STREAMER_ID,
            Some(&sid),
            false,
            now + jiff::SignedDuration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(sid.as_str()));

    let end_time: Option<i64> =
        sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(end_time.is_some());

    assert_eq!(streamer_state(&pool).await, "LIVE");
    assert_eq!(outbox_event_types(&pool).await, vec!["StreamerLive"]);

    let payload: String = sqlx::query_scalar(
        "SELECT payload FROM session_events
         WHERE session_id = ? AND kind = 'session_ended'
         ORDER BY id DESC LIMIT 1",
    )
    .bind(&sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(payload.contains("\"user_disabled\""), "got: {payload}");
}

#[tokio::test]
async fn end_for_disable_idempotent_on_already_ended_id() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();

    let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
    let sid = started.session_id().to_string();

    let _first = repo
        .end_for_disable(STREAMER_ID, Some(&sid), false, now)
        .await
        .unwrap();
    let second = repo
        .end_for_disable(STREAMER_ID, Some(&sid), false, now)
        .await
        .unwrap();
    assert!(
        second.is_none(),
        "second call against already-ended session must return Ok(None)"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_events
         WHERE session_id = ? AND kind = 'session_ended'",
    )
    .bind(&sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn or4_rewrite_session_ended_cause_updates_only_latest() {
    let pool = setup_pool().await;
    let repo = SessionLifecycleRepository::new(pool.clone());
    let now = Timestamp::now();

    let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
    let sid = started.session_id().to_string();

    repo.end_session_only(
        STREAMER_ID,
        Some(&sid),
        TerminalCauseDto::Completed,
        true,
        now,
    )
    .await
    .unwrap();

    let updated = repo
        .rewrite_session_ended_cause(&sid, TerminalCauseDto::UserDisabled)
        .await
        .unwrap();
    assert!(updated);

    let payload: String = sqlx::query_scalar(
        "SELECT payload FROM session_events
         WHERE session_id = ? AND kind = 'session_ended'
         ORDER BY id DESC LIMIT 1",
    )
    .bind(&sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(payload.contains("\"user_disabled\""), "got: {payload}");
    assert!(
        payload.contains("\"via_hysteresis\":true"),
        "via_hysteresis must be preserved across rewrite, got: {payload}"
    );
}
