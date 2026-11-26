//! Real SQLite integration tests for `RecorderStore`.

use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use async_trait::async_trait;
use jiff::Timestamp;
use rust_srec::config::ConfigEventBroadcaster;
use rust_srec::credentials::{
    CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialOwner, CredentialProfile,
    CredentialSelection, PoolStrategy, ResolvedCredentialPolicy,
};
use rust_srec::database::models::{StreamerDbModel, TitleEntry};
use rust_srec::database::repositories::{
    ConfigRepository as _, CredentialProfileRepository, MonitorOutboxOps, SqlxConfigRepository,
    SqlxStreamerRepository, StreamerRepository as _,
};
use rust_srec::database::time::ms_to_datetime;
use rust_srec::domain::StreamerState;
use rust_srec::domain::streamer::FatalErrorType;
use rust_srec::downloader::IoErrorKindSer;
use rust_srec::monitor::InfraBlockReason;
use rust_srec::notification::OutboxRecord;
use rust_srec::proxies::ProxyRoute;
use rust_srec::recorder::{
    DanmuSegmentRecord, DbCommitFailure, DbCommitOp, DbCommitOutcome, OutboxEvent,
    PostPublicationChangeSet, PostPublicationListener, RecorderStore, SessionStartDisposition,
    VideoSegmentRecord,
};
use rust_srec::session::{SessionEventPayload, TerminalCause, TerminalCauseDto};
use rust_srec::streamer::CommittedStreamerState;
use sqlx::SqlitePool;
use tokio::sync::{Mutex, Notify};

async fn setup_store() -> (
    SqlitePool,
    RecorderStore,
    Arc<CommittedStreamerState>,
    Arc<Notify>,
) {
    let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    rust_srec::database::run_migrations(&pool).await.unwrap();
    let notify = Arc::new(Notify::new());
    let (store, committed_state) = RecorderStore::with_new_committed_state(
        pool.clone(),
        pool.clone(),
        ConfigEventBroadcaster::new(),
    )
    .unwrap();
    let store = store.with_outbox_notify(notify.clone());
    (pool, store, committed_state, notify)
}

async fn insert_streamer(pool: &SqlitePool, id: &str, name: &str, state: StreamerState) {
    let mut model = StreamerDbModel::new(
        name.to_owned(),
        format!("https://example.test/{id}"),
        "platform-bilibili",
    );
    model.id = id.to_owned();
    model.state = state.as_str().to_owned();
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .create_streamer(&model)
        .await
        .unwrap();
}

struct StoredOutboxEvent {
    streamer_id: Option<String>,
    event_type: String,
    created_at: i64,
    event: OutboxEvent,
}

async fn fetch_outbox_events(pool: &SqlitePool) -> Vec<StoredOutboxEvent> {
    MonitorOutboxOps::fetch_undelivered(pool, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| {
            let OutboxRecord::V2(event) = OutboxRecord::decode(&entry.payload).unwrap() else {
                panic!(
                    "expected V2 outbox event, got legacy payload: {}",
                    entry.payload
                );
            };
            StoredOutboxEvent {
                streamer_id: entry.streamer_id,
                event_type: entry.event_type,
                created_at: entry.created_at,
                event,
            }
        })
        .collect()
}

/// The rejected-download end that persists an infrastructure block, as the
/// actor commits it when preflight refuses a start, for a streamer with no
/// open session.
fn infra_block_op(streamer_id: &str, reason: InfraBlockReason, now: Timestamp) -> DbCommitOp {
    DbCommitOp::EndSession {
        streamer_id: streamer_id.into(),
        streamer_name: streamer_id.into(),
        session_id: None,
        cause: TerminalCause::Rejected {
            reason: "output root unavailable".into(),
        },
        via_hysteresis: false,
        update_streamer_state: None,
        clear_errors: false,
        checks_restored_threshold: None,
        infra_block: Some(reason),
        suppress_offline_notification: false,
        now,
    }
}

/// Alice goes live titled "Initial Title" in Gaming with a new avatar: the
/// start that opens the session the lifecycle tests reuse, resume and end.
fn alice_goes_live(now: Timestamp) -> DbCommitOp {
    DbCommitOp::StartOrResumeSession {
        streamer_id: "s1".into(),
        streamer_name: "Alice".into(),
        title: "Initial Title".into(),
        category: Some("Gaming".into()),
        current_avatar: None,
        new_avatar: Some("https://cdn.example/avatar.png".into()),
        credential_binding: None,
        now,
    }
}

/// The monitor's offline end of Alice's `session_id`, which also sets her
/// `NotLive`.
fn alice_goes_offline(session_id: &str, now: Timestamp) -> DbCommitOp {
    DbCommitOp::EndSession {
        streamer_id: "s1".into(),
        streamer_name: "Alice".into(),
        session_id: Some(session_id.into()),
        cause: TerminalCause::StreamerOffline,
        via_hysteresis: false,
        update_streamer_state: Some(StreamerState::NotLive),
        clear_errors: false,
        checks_restored_threshold: None,
        infra_block: None,
        suppress_offline_notification: false,
        now,
    }
}

/// The commit's millisecond is the start in `live_sessions`, the
/// `session_started` event, the outbox row and its StreamOnline payload, and
/// the cache shows the streamer live with its new avatar.
#[tokio::test]
async fn created_session_shares_its_start_millisecond_across_rows_outbox_and_cache() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Alice", StreamerState::NotLive).await;

        let t_start = Timestamp::from_millisecond(1_735_000_123_456).unwrap();
        let outcome = store.commit(alice_goes_live(t_start)).await.unwrap();
        let (session_id, started_at) = match outcome {
            DbCommitOutcome::SessionStarted {
                session_id,
                started_at,
                disposition,
                credential_binding,
            } => {
                assert_eq!(disposition, SessionStartDisposition::Created);
                assert_eq!(credential_binding, None);
                (session_id, started_at)
            }
            other => panic!("expected SessionStarted, got {other:?}"),
        };
        assert_eq!(started_at.as_millisecond(), t_start.as_millisecond());

        let db_start: i64 =
            sqlx::query_scalar("SELECT start_time FROM live_sessions WHERE id = ?")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let event_occurred: i64 = sqlx::query_scalar(
            "SELECT occurred_at FROM session_events WHERE session_id = ? AND kind = 'session_started'",
        )
        .bind(&session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(db_start, t_start.as_millisecond());
        assert_eq!(event_occurred, t_start.as_millisecond());

        let cached = committed_state.get_streamer("s1").unwrap();
        assert_eq!(cached.state, StreamerState::Live);
        assert_eq!(
            cached.avatar_url.as_deref(),
            Some("https://cdn.example/avatar.png")
        );

        let outbox = fetch_outbox_events(&pool).await;
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].streamer_id.as_deref(), Some("s1"));
        assert_eq!(outbox[0].event_type, "StreamOnline");
        assert_eq!(outbox[0].created_at, t_start.as_millisecond());
        assert_eq!(
            outbox[0].event,
            OutboxEvent::StreamOnline {
                streamer_id: "s1".into(),
                streamer_name: "Alice".into(),
                session_id: session_id.clone(),
                title: "Initial Title".into(),
                category: Some("Gaming".into()),
                started_at_ms: t_start.as_millisecond(),
            }
        );
    })
    .await
    .expect("created session test must finish");
}

/// The reused session keeps its id and original start time.
#[tokio::test]
async fn start_on_an_open_session_reuses_it_and_enqueues_stream_online_at_the_reuse_time() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, _committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Alice", StreamerState::NotLive).await;
        let t_start = Timestamp::from_millisecond(1_735_000_123_456).unwrap();
        let DbCommitOutcome::SessionStarted { session_id, .. } =
            store.commit(alice_goes_live(t_start)).await.unwrap()
        else {
            panic!("expected SessionStarted");
        };

        let t_reuse = Timestamp::from_millisecond(1_735_000_200_000).unwrap();
        let reused = store
            .commit(DbCommitOp::StartOrResumeSession {
                streamer_id: "s1".into(),
                streamer_name: "Alice".into(),
                title: "Updated Title".into(),
                category: None,
                current_avatar: Some("https://cdn.example/avatar.png".into()),
                new_avatar: Some("https://cdn.example/avatar.png".into()),
                credential_binding: None,
                now: t_reuse,
            })
            .await
            .unwrap();
        assert_eq!(
            reused,
            DbCommitOutcome::SessionStarted {
                session_id: session_id.clone(),
                started_at: ms_to_datetime(t_start.as_millisecond()),
                disposition: SessionStartDisposition::ReusedActive,
                credential_binding: None,
            }
        );

        // Row 0 is the go-live's StreamOnline.
        let outbox = fetch_outbox_events(&pool).await;
        assert_eq!(outbox[1].created_at, t_reuse.as_millisecond());
        assert!(matches!(
            &outbox[1].event,
            OutboxEvent::StreamOnline {
                started_at_ms,
                ..
            } if *started_at_ms == t_reuse.as_millisecond()
        ));
    })
    .await
    .expect("reused session test must finish");
}

/// Entering and resuming hysteresis record `hysteresis_entered`,
/// `session_resumed` and `session_started` events at their commit times, the
/// resume enqueues a StreamOnline, and the session's titles keep each title
/// once, in the order first seen.
#[tokio::test]
async fn hysteresis_resume_records_its_events_and_titles_are_kept_once_in_order() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, _committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Alice", StreamerState::NotLive).await;
        let t_start = Timestamp::from_millisecond(1_735_000_123_456).unwrap();
        let DbCommitOutcome::SessionStarted { session_id, .. } =
            store.commit(alice_goes_live(t_start)).await.unwrap()
        else {
            panic!("expected SessionStarted");
        };
        store
            .commit(DbCommitOp::StartOrResumeSession {
                streamer_id: "s1".into(),
                streamer_name: "Alice".into(),
                title: "Updated Title".into(),
                category: None,
                current_avatar: Some("https://cdn.example/avatar.png".into()),
                new_avatar: Some("https://cdn.example/avatar.png".into()),
                credential_binding: None,
                now: Timestamp::from_millisecond(1_735_000_200_000).unwrap(),
            })
            .await
            .unwrap();

        let t_hyst = Timestamp::from_millisecond(1_735_000_250_000).unwrap();
        let t_deadline = Timestamp::from_millisecond(1_735_000_310_000).unwrap();
        assert_eq!(
            store
                .commit(DbCommitOp::RecordHysteresisEntered {
                    streamer_id: "s1".into(),
                    session_id: session_id.clone(),
                    cause: TerminalCauseDto::Completed,
                    resume_deadline: t_deadline,
                    occurred_at: t_hyst,
                })
                .await
                .unwrap(),
            DbCommitOutcome::HysteresisEnteredRecorded
        );

        let t_resume = Timestamp::from_millisecond(1_735_000_280_000).unwrap();
        assert_eq!(
            store
                .commit(DbCommitOp::ResumeHysteresis {
                    streamer_id: "s1".into(),
                    streamer_name: "Alice".into(),
                    session_id: session_id.clone(),
                    title: "Resumed Title".into(),
                    category: Some("Chatting".into()),
                    hysteresis_duration_secs: 30,
                    now: t_resume,
                })
                .await
                .unwrap(),
            DbCommitOutcome::HysteresisResumed {
                session_id: session_id.clone(),
                started_at: ms_to_datetime(t_start.as_millisecond()),
            }
        );

        assert_eq!(
            store
                .commit(DbCommitOp::AddSessionTitle {
                    session_id: session_id.clone(),
                    title: "Resumed Title".into(),
                    now: t_resume,
                })
                .await
                .unwrap(),
            DbCommitOutcome::SessionTitleAdded { changed: false }
        );
        let t_mid_title = Timestamp::from_millisecond(1_735_000_290_000).unwrap();
        assert_eq!(
            store
                .commit(DbCommitOp::AddSessionTitle {
                    session_id: session_id.clone(),
                    title: "Midstream Title".into(),
                    now: t_mid_title,
                })
                .await
                .unwrap(),
            DbCommitOutcome::SessionTitleAdded { changed: true }
        );

        let audit_rows: Vec<(String, i64, String)> = sqlx::query_as(
            "SELECT kind, occurred_at, payload FROM session_events \
             WHERE session_id = ? ORDER BY id",
        )
        .bind(&session_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(audit_rows.len(), 4);
        assert_eq!(audit_rows[1].0, "hysteresis_entered");
        assert_eq!(audit_rows[1].1, t_hyst.as_millisecond());
        assert_eq!(audit_rows[2].0, "session_resumed");
        assert_eq!(audit_rows[2].1, t_resume.as_millisecond());
        assert_eq!(
            serde_json::from_str::<SessionEventPayload>(&audit_rows[2].2).unwrap(),
            SessionEventPayload::SessionResumed {
                hysteresis_duration_secs: 30,
            }
        );
        assert_eq!(audit_rows[3].0, "session_started");
        assert_eq!(audit_rows[3].1, t_resume.as_millisecond());
        assert_eq!(
            serde_json::from_str::<SessionEventPayload>(&audit_rows[3].2).unwrap(),
            SessionEventPayload::SessionStarted {
                from_hysteresis: true,
                title: Some("Resumed Title".into()),
            }
        );

        let raw_titles: String =
            sqlx::query_scalar("SELECT titles FROM live_sessions WHERE id = ?")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let titles: Vec<TitleEntry> = serde_json::from_str(&raw_titles).unwrap();
        let title_names: Vec<&str> = titles.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            title_names,
            vec![
                "Initial Title",
                "Updated Title",
                "Resumed Title",
                "Midstream Title",
            ]
        );

        let outbox = fetch_outbox_events(&pool).await;
        assert_eq!(outbox.len(), 3);
        assert_eq!(outbox[2].created_at, t_resume.as_millisecond());
        assert!(matches!(
            &outbox[2].event,
            OutboxEvent::StreamOnline {
                started_at_ms,
                ..
            } if *started_at_ms == t_resume.as_millisecond()
        ));
    })
    .await
    .expect("hysteresis resume test must finish");
}

/// Ending the session without a state change enqueues one StreamOffline with
/// its exact duration and end time and keeps the streamer live. The monitor's
/// offline commit that follows finds the session closed, so it clears the
/// errors, enqueues ChecksRestored and applies the circuit-breaker block
/// without a second StreamOffline.
#[tokio::test]
async fn ending_a_session_enqueues_stream_offline_once_and_the_offline_check_restores_checks() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Alice", StreamerState::NotLive).await;
        let t_start = Timestamp::from_millisecond(1_735_000_123_456).unwrap();
        let DbCommitOutcome::SessionStarted { session_id, .. } =
            store.commit(alice_goes_live(t_start)).await.unwrap()
        else {
            panic!("expected SessionStarted");
        };

        sqlx::query("UPDATE streamers SET consecutive_error_count = 3 WHERE id = 's1'")
            .execute(&pool)
            .await
            .unwrap();
        let t_end = Timestamp::from_millisecond(1_735_000_423_456).unwrap(); // exactly +300.0s
        let first_end = store
            .commit(DbCommitOp::EndSession {
                streamer_id: "s1".into(),
                streamer_name: "Alice".into(),
                session_id: Some(session_id.clone()),
                cause: TerminalCause::Completed,
                via_hysteresis: false,
                update_streamer_state: None,
                clear_errors: false,
                checks_restored_threshold: None,
                infra_block: None,
                suppress_offline_notification: false,
                now: t_end,
            })
            .await
            .unwrap();
        assert_eq!(
            first_end,
            DbCommitOutcome::SessionEnded {
                session_id: Some(session_id.clone()),
                started_at: Some(ms_to_datetime(t_start.as_millisecond())),
                ended_at: t_end,
                transitioned_from_open: true,
            }
        );
        assert_eq!(
            committed_state.get_streamer("s1").unwrap().state,
            StreamerState::Live
        );

        let t_second_end = Timestamp::from_millisecond(1_735_000_430_000).unwrap();
        let second_end = store
            .commit(DbCommitOp::EndSession {
                streamer_id: "s1".into(),
                streamer_name: "Alice".into(),
                session_id: Some(session_id.clone()),
                cause: TerminalCause::StreamerOffline,
                via_hysteresis: false,
                update_streamer_state: Some(StreamerState::NotLive),
                clear_errors: true,
                checks_restored_threshold: Some(3),
                infra_block: Some(InfraBlockReason::CircuitBreaker {
                    retry_after_secs: 60,
                }),
                suppress_offline_notification: false,
                now: t_second_end,
            })
            .await
            .unwrap();
        assert_eq!(
            second_end,
            DbCommitOutcome::SessionEnded {
                session_id: None,
                started_at: None,
                ended_at: t_second_end,
                transitioned_from_open: false,
            }
        );
        let cached_after_second = committed_state.get_streamer("s1").unwrap();
        assert_eq!(cached_after_second.consecutive_error_count, 0);
        assert!(cached_after_second.disabled_until.is_some());

        // Row 0 is the go-live's StreamOnline.
        let outbox = fetch_outbox_events(&pool).await;
        assert_eq!(outbox.len(), 3);
        assert_eq!(outbox[1].created_at, t_end.as_millisecond());
        assert_eq!(
            outbox[1].event,
            OutboxEvent::StreamOffline {
                streamer_id: "s1".into(),
                streamer_name: "Alice".into(),
                session_id: session_id.clone(),
                duration_secs: Some(300.0),
                ended_at_ms: t_end.as_millisecond(),
            }
        );
        assert_eq!(outbox[2].created_at, t_second_end.as_millisecond());
        assert_eq!(
            outbox[2].event,
            OutboxEvent::ChecksRestored {
                streamer_id: "s1".into(),
                streamer_name: "Alice".into(),
                occurred_at_ms: t_second_end.as_millisecond(),
            }
        );
    })
    .await
    .expect("session end test must finish");
}

/// Resuming hysteresis on an ended session fails as a transient conflict, and
/// adding a title to it changes nothing.
#[tokio::test]
async fn an_ended_session_cannot_resume_hysteresis_or_gain_a_title() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, _committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Alice", StreamerState::NotLive).await;
        let t_start = Timestamp::from_millisecond(1_735_000_123_456).unwrap();
        let DbCommitOutcome::SessionStarted { session_id, .. } =
            store.commit(alice_goes_live(t_start)).await.unwrap()
        else {
            panic!("expected SessionStarted");
        };
        let t_end = Timestamp::from_millisecond(1_735_000_423_456).unwrap();
        store
            .commit(alice_goes_offline(&session_id, t_end))
            .await
            .unwrap();
        let outbox_before = fetch_outbox_events(&pool).await.len();

        let t_resume = Timestamp::from_millisecond(1_735_000_430_000).unwrap();
        assert!(matches!(
            store
                .commit(DbCommitOp::ResumeHysteresis {
                    streamer_id: "s1".into(),
                    streamer_name: "Alice".into(),
                    session_id: session_id.clone(),
                    title: "Too Late".into(),
                    category: None,
                    hysteresis_duration_secs: 10,
                    now: t_resume,
                })
                .await,
            Err(DbCommitFailure::Transient { .. })
        ));
        assert_eq!(
            store
                .commit(DbCommitOp::AddSessionTitle {
                    session_id: session_id.clone(),
                    title: "Too Late".into(),
                    now: t_end,
                })
                .await
                .unwrap(),
            DbCommitOutcome::SessionTitleAdded { changed: false }
        );
        assert_eq!(fetch_outbox_events(&pool).await.len(), outbox_before);
    })
    .await
    .expect("ended session test must finish");
}

/// An end that names the previous session leaves its successor open and live.
/// Ends for a user disable, the schedule or a stale-row replacement, and ends
/// that set `suppress_offline_notification`, enqueue no StreamOffline.
#[tokio::test]
async fn stale_session_ids_cannot_end_a_successor_and_suppressed_ends_skip_stream_offline() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Alice", StreamerState::NotLive).await;
        let t_start = Timestamp::from_millisecond(1_735_000_123_456).unwrap();
        let DbCommitOutcome::SessionStarted { session_id, .. } =
            store.commit(alice_goes_live(t_start)).await.unwrap()
        else {
            panic!("expected SessionStarted");
        };
        // This end enqueues the only StreamOffline the outbox should hold.
        let t_end = Timestamp::from_millisecond(1_735_000_423_456).unwrap();
        store
            .commit(alice_goes_offline(&session_id, t_end))
            .await
            .unwrap();

        for (suppressed_cause, explicit_suppress) in [
            (TerminalCause::UserDisabled, false),
            (TerminalCause::OutOfSchedule, false),
            (
                TerminalCause::Rejected {
                    reason: "stale_active_replaced".into(),
                },
                false,
            ),
            (TerminalCause::Completed, true),
        ] {
            let t_s = Timestamp::from_millisecond(1_735_001_000_000).unwrap();
            let DbCommitOutcome::SessionStarted {
                session_id: sid, ..
            } = store
                .commit(DbCommitOp::StartOrResumeSession {
                    streamer_id: "s1".into(),
                    streamer_name: "Alice".into(),
                    title: "Next".into(),
                    category: None,
                    current_avatar: None,
                    new_avatar: None,
                    credential_binding: None,
                    now: t_s,
                })
                .await
                .unwrap()
            else {
                panic!("expected new session");
            };

            // Stale previous session_id must NOT touch the new active session.
            let stale_res = store
                .commit(alice_goes_offline(&session_id, t_s))
                .await
                .unwrap();
            assert!(matches!(
                stale_res,
                DbCommitOutcome::SessionEnded {
                    transitioned_from_open: false,
                    ..
                }
            ));
            assert_eq!(
                committed_state.get_streamer("s1").unwrap().state,
                StreamerState::Live
            );

            let ended = store
                .commit(DbCommitOp::EndSession {
                    streamer_id: "s1".into(),
                    streamer_name: "Alice".into(),
                    session_id: Some(sid),
                    cause: suppressed_cause,
                    via_hysteresis: false,
                    update_streamer_state: Some(StreamerState::NotLive),
                    clear_errors: false,
                    checks_restored_threshold: None,
                    infra_block: None,
                    suppress_offline_notification: explicit_suppress,
                    now: Timestamp::from_millisecond(1_735_001_060_000).unwrap(),
                })
                .await
                .unwrap();
            assert!(matches!(
                ended,
                DbCommitOutcome::SessionEnded {
                    transitioned_from_open: true,
                    ..
                }
            ));
        }
        let offline_count = fetch_outbox_events(&pool)
            .await
            .into_iter()
            .filter(|e| e.event_type == "StreamOffline")
            .count();
        assert_eq!(
            offline_count, 1,
            "suppressed causes must not emit StreamOffline"
        );
    })
    .await
    .expect("stale end and suppression test must finish");
}

/// Disabled and soft-deleted streamers report Disabled with no retry time;
/// OutOfSpace and an unexpired backoff report the time left until
/// `disabled_until`.
#[tokio::test]
async fn start_and_resume_are_suppressed_while_disabled_deleted_out_of_space_or_backed_off() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, _committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s-disabled", "Disabled", StreamerState::Disabled).await;
        insert_streamer(&pool, "s-deleted", "Deleted", StreamerState::NotLive).await;
        sqlx::query("UPDATE streamers SET deleted_at = 1 WHERE id = 's-deleted'")
            .execute(&pool)
            .await
            .unwrap();
        insert_streamer(&pool, "s-oos", "OOS", StreamerState::OutOfSpace).await;
        insert_streamer(
            &pool,
            "s-backoff",
            "Backoff",
            StreamerState::TemporalDisabled,
        )
        .await;
        let t_now = Timestamp::from_millisecond(1_735_002_000_000).unwrap();
        sqlx::query("UPDATE streamers SET disabled_until = ? WHERE id IN ('s-oos', 's-backoff')")
            .bind(t_now.as_millisecond() + 45_000)
            .execute(&pool)
            .await
            .unwrap();

        for (sid, expected_state, expect_retry) in [
            ("s-disabled", StreamerState::Disabled, false),
            ("s-deleted", StreamerState::Disabled, false),
            ("s-oos", StreamerState::OutOfSpace, true),
            ("s-backoff", StreamerState::TemporalDisabled, true),
        ] {
            let res = store
                .commit(DbCommitOp::StartOrResumeSession {
                    streamer_id: sid.into(),
                    streamer_name: sid.into(),
                    title: "Suppressed".into(),
                    category: None,
                    current_avatar: None,
                    new_avatar: None,
                    credential_binding: None,
                    now: t_now,
                })
                .await
                .unwrap();
            match res {
                DbCommitOutcome::SessionStartSuppressed { state, retry_after } => {
                    assert_eq!(state, expected_state, "streamer {sid}");
                    assert_eq!(retry_after.is_some(), expect_retry, "streamer {sid}");
                    if expect_retry {
                        assert_eq!(retry_after, Some(Duration::from_secs(45)));
                    }
                }
                other => panic!("expected suppression for {sid}, got {other:?}"),
            }
        }

        assert_eq!(
            store
                .commit(DbCommitOp::ResumeHysteresis {
                    streamer_id: "s-disabled".into(),
                    streamer_name: "Disabled".into(),
                    session_id: "any".into(),
                    title: "Suppressed".into(),
                    category: None,
                    hysteresis_duration_secs: 5,
                    now: t_now,
                })
                .await
                .unwrap(),
            DbCommitOutcome::HysteresisResumeSuppressed {
                state: StreamerState::Disabled,
                retry_after: None,
            }
        );
    })
    .await
    .expect("start suppression test must finish");
}

/// A start that finds several open rows for the streamer reuses the newest and
/// closes the older ones at the commit time.
#[tokio::test]
async fn start_reuses_the_newest_open_session_and_closes_older_ones_without_stream_offline() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, _committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s-heal", "Healed", StreamerState::NotLive).await;
        sqlx::query("DROP INDEX live_sessions_one_active_per_streamer")
            .execute(&pool)
            .await
            .unwrap();
        for (id, start_ms) in [("stale-old", 1_735_003_000_000i64), ("stale-new", 1_735_003_010_000)]
        {
            sqlx::query(
                "INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, total_size_bytes) \
                 VALUES (?, 's-heal', 'Healed', ?, 0)",
            )
            .bind(id)
            .bind(start_ms)
            .execute(&pool)
            .await
            .unwrap();
        }
        let t_heal = Timestamp::from_millisecond(1_735_003_020_000).unwrap();
        let heal_outcome = store
            .commit(DbCommitOp::StartOrResumeSession {
                streamer_id: "s-heal".into(),
                streamer_name: "Healed".into(),
                title: "Healed Title".into(),
                category: None,
                current_avatar: None,
                new_avatar: None,
                credential_binding: None,
                now: t_heal,
            })
            .await
            .unwrap();
        assert!(matches!(
            heal_outcome,
            DbCommitOutcome::SessionStarted {
                ref session_id,
                disposition: SessionStartDisposition::ReusedActive,
                ..
            } if session_id == "stale-new"
        ));
        let healed_end: Option<i64> =
            sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = 'stale-old'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(healed_end, Some(t_heal.as_millisecond()));
        let heal_outbox = fetch_outbox_events(&pool).await;
        assert_eq!(heal_outbox.len(), 1);
        assert_eq!(heal_outbox[0].event_type, "StreamOnline");
    })
    .await
    .expect("open session self-heal test must finish");
}

trait RewriteCheckExt {
    fn is_updated_rewrite(&self) -> bool;
}

impl RewriteCheckExt for DbCommitOutcome {
    fn is_updated_rewrite(&self) -> bool {
        matches!(self, Self::EndedCauseRewritten { updated: true, .. })
    }
}

/// RewriteEndedCause turns a hysteresis end recorded as Completed into
/// UserDisabled without moving the end time or enqueueing alerts, and a second
/// rewrite of the now authoritative cause changes nothing.
#[tokio::test]
async fn rewriting_a_hysteresis_completed_end_changes_only_its_cause_and_only_once() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, _committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Bob", StreamerState::NotLive).await;

        let t0 = Timestamp::from_millisecond(1_735_010_000_000).unwrap();
        let DbCommitOutcome::SessionStarted {
            session_id: hyst_sid,
            ..
        } = store
            .commit(DbCommitOp::StartOrResumeSession {
                streamer_id: "s1".into(),
                streamer_name: "Bob".into(),
                title: "Live".into(),
                category: None,
                current_avatar: None,
                new_avatar: None,
                credential_binding: None,
                now: t0,
            })
            .await
            .unwrap()
        else {
            panic!("expected session start");
        };

        let t_end = Timestamp::from_millisecond(1_735_010_060_000).unwrap();
        store
            .commit(DbCommitOp::EndSession {
                streamer_id: "s1".into(),
                streamer_name: "Bob".into(),
                session_id: Some(hyst_sid.clone()),
                cause: TerminalCause::Completed,
                via_hysteresis: true,
                update_streamer_state: Some(StreamerState::NotLive),
                clear_errors: false,
                checks_restored_threshold: None,
                infra_block: None,
                suppress_offline_notification: false,
                now: t_end,
            })
            .await
            .unwrap();
        let outbox_before = fetch_outbox_events(&pool).await.len();

        assert_eq!(
            store
                .commit(DbCommitOp::RewriteEndedCause {
                    session_id: hyst_sid.clone(),
                    new_cause: TerminalCauseDto::UserDisabled,
                })
                .await
                .unwrap(),
            DbCommitOutcome::EndedCauseRewritten {
                session_id: hyst_sid.clone(),
                updated: true,
            }
        );
        let stored_end: i64 = sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
            .bind(&hyst_sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored_end, t_end.as_millisecond());
        assert_eq!(fetch_outbox_events(&pool).await.len(), outbox_before);

        let raw_payload: String = sqlx::query_scalar(
            "SELECT payload FROM session_events WHERE session_id = ? AND kind = 'session_ended'",
        )
        .bind(&hyst_sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        let parsed: SessionEventPayload = serde_json::from_str(&raw_payload).unwrap();
        assert_eq!(
            parsed,
            SessionEventPayload::SessionEnded {
                cause: TerminalCauseDto::UserDisabled,
                via_hysteresis: true,
            }
        );

        // Rewriting an already-authoritative cause is a no-op.
        assert!(
            !store
                .commit(DbCommitOp::RewriteEndedCause {
                    session_id: hyst_sid,
                    new_cause: TerminalCauseDto::UserDisabled,
                })
                .await
                .unwrap()
                .is_updated_rewrite()
        );
    })
    .await
    .expect("ended cause rewrite test must finish");
}

/// ChecksFailing is enqueued only when the error count reaches the offline
/// check count, where backoff also starts. ClearLastError keeps the count, and
/// only the first of two ClearErrors enqueues ChecksRestored.
#[tokio::test]
async fn transient_errors_alert_only_at_the_threshold_and_clearing_them_restores_once() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Bob", StreamerState::NotLive).await;

        for step in 1..=4 {
            let t = Timestamp::from_millisecond(1_735_010_100_000 + step * 1_000).unwrap();
            let outcome = store
                .commit(DbCommitOp::RecordTransientError {
                    streamer_id: "s1".into(),
                    streamer_name: "Bob".into(),
                    platform: "bilibili".into(),
                    error: format!("timeout #{step}"),
                    offline_check_count: 3,
                    now: t,
                })
                .await
                .unwrap();
            match outcome {
                DbCommitOutcome::TransientErrorRecorded {
                    consecutive_error_count,
                    disabled_until,
                } => {
                    assert_eq!(consecutive_error_count, step as i32);
                    assert_eq!(disabled_until.is_some(), step >= 3);
                }
                other => panic!("unexpected outcome: {other:?}"),
            }
        }

        store
            .commit(DbCommitOp::ClearLastError {
                streamer_id: "s1".into(),
            })
            .await
            .unwrap();
        let after_clear_last = committed_state.get_streamer("s1").unwrap();
        assert_eq!(after_clear_last.last_error, None);
        assert_eq!(after_clear_last.consecutive_error_count, 4);

        let t_restored = Timestamp::from_millisecond(1_735_010_200_000).unwrap();
        assert_eq!(
            store
                .commit(DbCommitOp::ClearErrors {
                    streamer_id: "s1".into(),
                    streamer_name: "Bob".into(),
                    checks_restored_threshold: 3,
                    set_state: Some(StreamerState::NotLive),
                    now: t_restored,
                })
                .await
                .unwrap(),
            DbCommitOutcome::ErrorsCleared
        );
        // Second ClearErrors must not re-emit ChecksRestored.
        assert_eq!(
            store
                .commit(DbCommitOp::ClearErrors {
                    streamer_id: "s1".into(),
                    streamer_name: "Bob".into(),
                    checks_restored_threshold: 3,
                    set_state: None,
                    now: Timestamp::from_millisecond(1_735_010_201_000).unwrap(),
                })
                .await
                .unwrap(),
            DbCommitOutcome::ErrorsCleared
        );

        let outbox_after_errors = fetch_outbox_events(&pool).await;
        let failing_events: Vec<&StoredOutboxEvent> = outbox_after_errors
            .iter()
            .filter(|e| e.event_type == "ChecksFailing")
            .collect();
        assert_eq!(failing_events.len(), 1);
        assert_eq!(failing_events[0].created_at, 1_735_010_103_000);
        assert_eq!(
            failing_events[0].event,
            OutboxEvent::ChecksFailing {
                streamer_id: "s1".into(),
                streamer_name: "Bob".into(),
                platform: "bilibili".into(),
                last_error: "timeout #3".into(),
                consecutive_errors: 3,
                occurred_at_ms: 1_735_010_103_000,
            }
        );
        let restored_events: Vec<&StoredOutboxEvent> = outbox_after_errors
            .iter()
            .filter(|e| e.event_type == "ChecksRestored")
            .collect();
        assert_eq!(restored_events.len(), 1);
        assert_eq!(restored_events[0].created_at, t_restored.as_millisecond());
        assert_eq!(
            restored_events[0].event,
            OutboxEvent::ChecksRestored {
                streamer_id: "s1".into(),
                streamer_name: "Bob".into(),
                occurred_at_ms: t_restored.as_millisecond(),
            }
        );
    })
    .await
    .expect("transient error test must finish");
}

/// An unavailable output root blocks the streamer as OutOfSpace with the path
/// in its last error.
#[tokio::test]
async fn output_root_block_sets_out_of_space_without_counting_an_error() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Bob", StreamerState::NotLive).await;

        let t_infra = Timestamp::from_millisecond(1_735_010_300_000).unwrap();
        store
            .commit(infra_block_op(
                "s1",
                InfraBlockReason::OutputRootUnavailable {
                    path: "/mnt/missing".into(),
                    io_kind: IoErrorKindSer::NotFound,
                    retry_after_secs: 120,
                },
                t_infra,
            ))
            .await
            .unwrap();
        let cached_oos = committed_state.get_streamer("s1").unwrap();
        assert_eq!(cached_oos.state, StreamerState::OutOfSpace);
        assert_eq!(cached_oos.consecutive_error_count, 0);
        assert!(
            cached_oos
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("/mnt/missing"))
        );
    })
    .await
    .expect("infra block test must finish");
}

#[tokio::test]
async fn filtered_state_changes_the_state_without_enqueueing_alerts() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Bob", StreamerState::NotLive).await;
        let t_infra = Timestamp::from_millisecond(1_735_010_300_000).unwrap();
        store
            .commit(infra_block_op(
                "s1",
                InfraBlockReason::OutputRootUnavailable {
                    path: "/mnt/missing".into(),
                    io_kind: IoErrorKindSer::NotFound,
                    retry_after_secs: 120,
                },
                t_infra,
            ))
            .await
            .unwrap();

        let outbox_count_before_filter = fetch_outbox_events(&pool).await.len();
        store
            .commit(DbCommitOp::RecordFilteredState {
                streamer_id: "s1".into(),
                streamer_name: "Bob".into(),
                old_state: StreamerState::OutOfSpace,
                new_state: StreamerState::OutOfSchedule,
                reason: "title_mismatch".into(),
                now: t_infra,
            })
            .await
            .unwrap();
        assert_eq!(
            committed_state.get_streamer("s1").unwrap().state,
            StreamerState::OutOfSchedule
        );
        assert_eq!(
            fetch_outbox_events(&pool).await.len(),
            outbox_count_before_filter
        );
    })
    .await
    .expect("filtered state test must finish");
}

/// RecordFatalError ends the open session at the commit time and enqueues a
/// FatalError whose row and payload carry that time, but no StreamOffline.
#[tokio::test]
async fn fatal_error_ends_the_open_session_and_enqueues_fatal_error_without_stream_offline() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Bob", StreamerState::NotLive).await;

        let DbCommitOutcome::SessionStarted {
            session_id: fatal_sid,
            ..
        } = store
            .commit(DbCommitOp::StartOrResumeSession {
                streamer_id: "s1".into(),
                streamer_name: "Bob".into(),
                title: "Before Ban".into(),
                category: None,
                current_avatar: None,
                new_avatar: None,
                credential_binding: None,
                now: Timestamp::from_millisecond(1_735_010_400_000).unwrap(),
            })
            .await
            .unwrap()
        else {
            panic!("expected session");
        };
        let offline_before_fatal = fetch_outbox_events(&pool)
            .await
            .into_iter()
            .filter(|e| e.event_type == "StreamOffline")
            .count();
        let t_fatal = Timestamp::from_millisecond(1_735_010_500_000).unwrap();
        let fatal_outcome = store
            .commit(DbCommitOp::RecordFatalError {
                streamer_id: "s1".into(),
                streamer_name: "Bob".into(),
                session_id: Some(fatal_sid.clone()),
                error_type: FatalErrorType::NotFound,
                message: "room deleted".into(),
                new_state: StreamerState::NotFound,
                now: t_fatal,
            })
            .await
            .unwrap();
        assert_eq!(
            fatal_outcome,
            DbCommitOutcome::FatalErrorRecorded {
                session_ended: Some(fatal_sid.clone()),
                new_state: StreamerState::NotFound,
            }
        );
        assert_eq!(
            committed_state.get_streamer("s1").unwrap().state,
            StreamerState::NotFound
        );
        let fatal_end_time: Option<i64> =
            sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
                .bind(&fatal_sid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(fatal_end_time, Some(t_fatal.as_millisecond()));

        let offline_after_fatal = fetch_outbox_events(&pool)
            .await
            .into_iter()
            .filter(|e| e.event_type == "StreamOffline")
            .count();
        assert_eq!(
            offline_after_fatal, offline_before_fatal,
            "RecordFatalError must not enqueue a spurious StreamOffline"
        );

        let (raw_fatal, fatal_created_at): (String, i64) = sqlx::query_as(
            "SELECT payload, created_at FROM monitor_event_outbox \
             WHERE streamer_id = 's1' AND event_type = 'FatalError' ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(fatal_created_at, t_fatal.as_millisecond());
        let fatal_json: serde_json::Value = serde_json::from_str(&raw_fatal).unwrap();
        assert_eq!(fatal_json["FatalError"]["message"], "room deleted");
        assert_eq!(
            fatal_json["FatalError"]["timestamp"]
                .as_str()
                .unwrap()
                .parse::<Timestamp>()
                .unwrap()
                .as_millisecond(),
            t_fatal.as_millisecond()
        );
    })
    .await
    .expect("fatal error test must finish");
}

#[tokio::test]
async fn disabled_streamers_ignore_monitor_error_block_filter_and_fatal_writes() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, _committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s-dis", "Disabled", StreamerState::Disabled).await;
        let now = Timestamp::from_millisecond(1_735_010_500_000).unwrap();

        let outbox_before_dis = fetch_outbox_events(&pool).await.len();
        store
            .commit(DbCommitOp::RecordTransientError {
                streamer_id: "s-dis".into(),
                streamer_name: "Disabled".into(),
                platform: "bilibili".into(),
                error: "ignored".into(),
                offline_check_count: 1,
                now,
            })
            .await
            .unwrap();
        store
            .commit(infra_block_op(
                "s-dis",
                InfraBlockReason::CircuitBreaker {
                    retry_after_secs: 60,
                },
                now,
            ))
            .await
            .unwrap();
        store
            .commit(DbCommitOp::RecordFilteredState {
                streamer_id: "s-dis".into(),
                streamer_name: "Disabled".into(),
                old_state: StreamerState::Disabled,
                new_state: StreamerState::OutOfSchedule,
                reason: "ignored".into(),
                now,
            })
            .await
            .unwrap();
        store
            .commit(DbCommitOp::RecordFatalError {
                streamer_id: "s-dis".into(),
                streamer_name: "Disabled".into(),
                session_id: None,
                error_type: FatalErrorType::NotFound,
                message: "ignored".into(),
                new_state: StreamerState::NotFound,
                now,
            })
            .await
            .unwrap();
        let dis_row: (String, i32, Option<i64>) = sqlx::query_as(
            "SELECT state, consecutive_error_count, disabled_until FROM streamers WHERE id = 's-dis'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(dis_row, ("DISABLED".to_owned(), 0, None));
        assert_eq!(fetch_outbox_events(&pool).await.len(), outbox_before_dis);
    })
    .await
    .expect("disabled streamer guard test must finish");
}

/// Carol's open session, bound to account A of a two-account platform pool
/// that prefers account A over account B.
struct BoundSession {
    pool: SqlitePool,
    store: RecorderStore,
    session_id: String,
    started_at: Timestamp,
    profile_b: CredentialProfile,
    policy: ResolvedCredentialPolicy,
    /// The binding the session start committed, at epoch 1.
    binding_v1: CredentialBinding,
}

async fn start_bound_session() -> BoundSession {
    let (pool, store, _committed_state, _notify) = setup_store().await;
    insert_streamer(&pool, "s1", "Carol", StreamerState::NotLive).await;

    let profile_repo = CredentialProfileRepository::new(pool.clone(), pool.clone());
    let material_a = CredentialMaterial {
        cookies: "SESSDATA=a".into(),
        refresh_token: None,
        access_token: None,
        reauth_config: None,
    };
    let material_b = CredentialMaterial {
        cookies: "SESSDATA=b".into(),
        refresh_token: None,
        access_token: None,
        reauth_config: None,
    };
    let profile_a = profile_repo
        .create(
            "platform-bilibili",
            "Acct A",
            true,
            &material_a,
            &ProxyRoute::Inherit,
        )
        .await
        .unwrap();
    let profile_b = profile_repo
        .create(
            "platform-bilibili",
            "Acct B",
            true,
            &material_b,
            &ProxyRoute::Inherit,
        )
        .await
        .unwrap();

    let owner = CredentialOwner::Platform {
        platform_id: "platform-bilibili".into(),
    };
    let selection = CredentialSelection::Pool {
        credential_ids: vec![profile_a.id.clone(), profile_b.id.clone()],
        strategy: PoolStrategy::Priority,
        failover: true,
        anonymous_fallback: false,
        max_attempts: 2,
    };
    let config_repo = SqlxConfigRepository::new(pool.clone(), pool.clone());
    let platform = config_repo
        .get_platform_config("platform-bilibili")
        .await
        .unwrap();
    config_repo
        .update_platform_config_with_selection(&platform, Some(&selection))
        .await
        .unwrap();
    let policy =
        ResolvedCredentialPolicy::new("platform-bilibili".into(), owner, selection).unwrap();

    let initial_binding = CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: profile_a.id.clone(),
        },
        revision: profile_a.revision as u64,
        policy: policy.clone(),
        epoch: 0,
    };

    let started_at = Timestamp::from_millisecond(1_735_020_000_000).unwrap();
    let DbCommitOutcome::SessionStarted {
        session_id,
        credential_binding,
        ..
    } = store
        .commit(DbCommitOp::StartOrResumeSession {
            streamer_id: "s1".into(),
            streamer_name: "Carol".into(),
            title: "Managed".into(),
            category: None,
            current_avatar: None,
            new_avatar: None,
            credential_binding: Some(initial_binding),
            now: started_at,
        })
        .await
        .unwrap()
    else {
        panic!("expected session started");
    };
    BoundSession {
        pool,
        store,
        session_id,
        started_at,
        profile_b,
        policy,
        binding_v1: credential_binding.expect("committed binding"),
    }
}

/// Persisting a segment advances the next segment index and adds its size to
/// the session total; removing the danmu output subtracts its size and keeps
/// the video output and the session segment.
#[tokio::test]
async fn persisted_segments_advance_the_index_and_removing_danmu_keeps_the_video() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let BoundSession {
            pool,
            store,
            session_id,
            started_at,
            ..
        } = start_bound_session().await;

        assert_eq!(
            store.next_session_segment_index(&session_id).await.unwrap(),
            0
        );
        let video_segment = || VideoSegmentRecord {
            session_id: session_id.clone(),
            streamer_id: "s1".into(),
            segment_index: 0,
            segment_path: "/rec/seg0.flv".into(),
            started_at: Some(started_at),
            completed_at: Timestamp::from_millisecond(1_735_020_010_000).unwrap(),
            duration_secs: 10.0,
            size_bytes: 1_000,
            split_reason_code: None,
            split_reason_details_json: None,
        };
        let danmu_segment = || DanmuSegmentRecord {
            session_id: session_id.clone(),
            streamer_id: "s1".into(),
            segment_index: 0,
            output_path: "/rec/seg0.xml".into(),
            size_bytes: 250,
            message_count: 12,
        };
        // Each segment is persisted twice, as the registry does when an actor
        // panicked after its own insert; the second write changes nothing.
        for _ in 0..2 {
            store.persist_video_segment(video_segment()).await.unwrap();
        }
        assert_eq!(
            store.next_session_segment_index(&session_id).await.unwrap(),
            1
        );

        for _ in 0..2 {
            store.persist_danmu_segment(danmu_segment()).await.unwrap();
        }
        let total_with_danmu: i64 =
            sqlx::query_scalar("SELECT total_size_bytes FROM live_sessions WHERE id = ?")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(total_with_danmu, 1_250);

        store
            .remove_danmu_segment_output(&session_id, "/rec/seg0.xml")
            .await
            .unwrap();
        let total_after_remove: i64 =
            sqlx::query_scalar("SELECT total_size_bytes FROM live_sessions WHERE id = ?")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(total_after_remove, 1_000);
        let remaining_outputs: Vec<(String, String)> = sqlx::query_as(
            "SELECT file_type, file_path FROM media_outputs WHERE session_id = ? ORDER BY file_type",
        )
        .bind(&session_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            remaining_outputs,
            vec![("VIDEO".to_owned(), "/rec/seg0.flv".to_owned())]
        );
        let segment_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM session_segments WHERE session_id = ?")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(segment_count, 1);
    })
    .await
    .expect("segment persistence test must finish");
}

/// The session starts bound at epoch 1. Rotating it to account B commits
/// epoch 2, and repeating the epoch-1 request is a binding conflict.
#[tokio::test]
async fn committing_a_credential_binding_bumps_its_epoch_and_rejects_a_stale_one() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let BoundSession {
            store,
            session_id,
            profile_b,
            policy,
            binding_v1,
            ..
        } = start_bound_session().await;
        assert_eq!(binding_v1.epoch, 1);

        let rotate_req = CredentialBinding {
            identity: CredentialIdentity::Profile {
                profile_id: profile_b.id.clone(),
            },
            revision: profile_b.revision as u64,
            policy: policy.clone(),
            epoch: binding_v1.epoch,
        };
        let DbCommitOutcome::CredentialBindingCommitted {
            binding: binding_v2,
            ..
        } = store
            .commit(DbCommitOp::CommitCredentialBinding {
                streamer_id: "s1".into(),
                session_id: session_id.clone(),
                binding: rotate_req.clone(),
            })
            .await
            .unwrap()
        else {
            panic!("expected CredentialBindingCommitted");
        };
        assert_eq!(binding_v2.epoch, 2);

        let stale_err = store
            .commit(DbCommitOp::CommitCredentialBinding {
                streamer_id: "s1".into(),
                session_id: session_id.clone(),
                binding: rotate_req,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(stale_err, DbCommitFailure::CredentialBindingConflict { .. }),
            "stale epoch must return CredentialBindingConflict, got {stale_err:?}"
        );
    })
    .await
    .expect("credential binding test must finish");
}

/// EndForCredentialRetirement ends the session only when its account is
/// retiring and the request names the session's current binding; the end
/// enqueues no StreamOffline.
#[tokio::test]
async fn credential_retirement_ends_the_session_only_for_its_current_retiring_binding() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let BoundSession {
            pool,
            store,
            session_id,
            profile_b,
            policy,
            binding_v1,
            ..
        } = start_bound_session().await;
        let DbCommitOutcome::CredentialBindingCommitted {
            binding: binding_v2,
            ..
        } = store
            .commit(DbCommitOp::CommitCredentialBinding {
                streamer_id: "s1".into(),
                session_id: session_id.clone(),
                binding: CredentialBinding {
                    identity: CredentialIdentity::Profile {
                        profile_id: profile_b.id.clone(),
                    },
                    revision: profile_b.revision as u64,
                    policy,
                    epoch: binding_v1.epoch,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected CredentialBindingCommitted");
        };

        let not_retiring = store
            .commit(DbCommitOp::EndForCredentialRetirement {
                streamer_id: "s1".into(),
                streamer_name: "Carol".into(),
                session_id: session_id.clone(),
                binding: binding_v2.clone(),
                via_hysteresis: false,
                now: Timestamp::from_millisecond(1_735_020_020_000).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(
            not_retiring,
            DbCommitOutcome::CredentialRetirementEnded {
                session_id: session_id.clone(),
                ended: false,
            }
        );

        sqlx::query("INSERT INTO retirement_credential_profiles (profile_id) VALUES (?)")
            .bind(&profile_b.id)
            .execute(&pool)
            .await
            .unwrap();

        // Wrong epoch (v1) must not end the session.
        let wrong_epoch = store
            .commit(DbCommitOp::EndForCredentialRetirement {
                streamer_id: "s1".into(),
                streamer_name: "Carol".into(),
                session_id: session_id.clone(),
                binding: binding_v1,
                via_hysteresis: false,
                now: Timestamp::from_millisecond(1_735_020_030_000).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(
            wrong_epoch,
            DbCommitOutcome::CredentialRetirementEnded {
                session_id: session_id.clone(),
                ended: false,
            }
        );

        // Exact matching retiring binding ends the session cleanly without StreamOffline.
        let outbox_before = fetch_outbox_events(&pool).await.len();
        let t_retire = Timestamp::from_millisecond(1_735_020_040_000).unwrap();
        let retired = store
            .commit(DbCommitOp::EndForCredentialRetirement {
                streamer_id: "s1".into(),
                streamer_name: "Carol".into(),
                session_id: session_id.clone(),
                binding: binding_v2,
                via_hysteresis: false,
                now: t_retire,
            })
            .await
            .unwrap();
        assert_eq!(
            retired,
            DbCommitOutcome::CredentialRetirementEnded {
                session_id: session_id.clone(),
                ended: true,
            }
        );
        assert_eq!(fetch_outbox_events(&pool).await.len(), outbox_before);

        let retired_end: Option<i64> =
            sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(retired_end, Some(t_retire.as_millisecond()));
        let retired_audit: String = sqlx::query_scalar(
            "SELECT payload FROM session_events WHERE session_id = ? AND kind = 'session_ended'",
        )
        .bind(&session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<SessionEventPayload>(&retired_audit).unwrap(),
            SessionEventPayload::SessionEnded {
                cause: TerminalCauseDto::UserDisabled,
                via_hysteresis: false,
            }
        );
    })
    .await
    .expect("credential retirement test must finish");
}

#[tokio::test]
async fn dropping_a_commit_before_it_seals_releases_the_lease_and_leaves_no_session() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, base_store, committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Dave", StreamerState::NotLive).await;
        let observed = Arc::new(Mutex::new(Vec::new()));
        let store =
            base_store
                .clone()
                .with_post_publication_listener(Arc::new(LockFreeProbeListener {
                    store: base_store,
                    write_pool: pool.clone(),
                    committed_state,
                    observed: observed.clone(),
                }));

        let held_write_conn = pool.acquire().await.unwrap();
        let t_aborted = Timestamp::from_millisecond(1_735_029_000_000).unwrap();
        let mut aborted_future = Box::pin(store.commit(DbCommitOp::StartOrResumeSession {
            streamer_id: "s1".into(),
            streamer_name: "Dave".into(),
            title: "Aborted Before Commit".into(),
            category: None,
            current_avatar: None,
            new_avatar: None,
            credential_binding: None,
            now: t_aborted,
        }));
        while !store.has_in_flight_commit("s1") {
            assert!(
                matches!(futures::poll!(&mut aborted_future), Poll::Pending),
                "commit must remain pending while write_pool is held"
            );
            tokio::task::yield_now().await;
        }
        drop(aborted_future);
        drop(held_write_conn);
        store.wait_for_streamer_quiescence("s1").await;

        let open_after_abort: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM live_sessions WHERE streamer_id = 's1' AND end_time IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open_after_abort, 0);
        assert!(observed.lock().await.is_empty());
    })
    .await
    .expect("pre-commit cancellation test must finish");
}

struct LockFreeProbeListener {
    store: RecorderStore,
    write_pool: SqlitePool,
    committed_state: Arc<CommittedStreamerState>,
    observed: Arc<Mutex<Vec<PostPublicationChangeSet>>>,
}

#[async_trait]
impl PostPublicationListener for LockFreeProbeListener {
    async fn on_published(&self, changes: PostPublicationChangeSet) {
        // Must acquire the single-connection write pool, the cache publication
        // read lock, and the per-streamer serialization lease without deadlocking.
        let _conn = tokio::time::timeout(Duration::from_secs(2), self.write_pool.acquire())
            .await
            .expect("write_pool must already be released before PostPublicationListener runs")
            .unwrap();
        let _snapshot = self.committed_state.get_streamer("s1");
        tokio::time::timeout(
            Duration::from_secs(2),
            self.store.wait_for_streamer_quiescence("s1"),
        )
        .await
        .expect("per-streamer lease must already be released before PostPublicationListener runs");
        self.observed.lock().await.push(changes);
    }
}

/// A commit that enqueues an alert wakes `outbox_notify`, then runs the
/// post-publication listener with the SQLite write connection, the cache
/// publication lock and the streamer lease all free.
#[tokio::test]
async fn commits_wake_the_outbox_and_run_the_listener_with_every_lock_released() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, base_store, committed_state, outbox_notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Dave", StreamerState::NotLive).await;

        let observed = Arc::new(Mutex::new(Vec::new()));
        let store =
            base_store
                .clone()
                .with_post_publication_listener(Arc::new(LockFreeProbeListener {
                    store: base_store,
                    write_pool: pool.clone(),
                    committed_state: committed_state.clone(),
                    observed: observed.clone(),
                }));

        let notified = outbox_notify.notified();
        let t_start = Timestamp::from_millisecond(1_735_030_000_000).unwrap();
        let DbCommitOutcome::SessionStarted { session_id, .. } = store
            .commit(DbCommitOp::StartOrResumeSession {
                streamer_id: "s1".into(),
                streamer_name: "Dave".into(),
                title: "Committed".into(),
                category: None,
                current_avatar: None,
                new_avatar: None,
                credential_binding: None,
                now: t_start,
            })
            .await
            .unwrap()
        else {
            panic!("expected session start");
        };
        notified.await;

        let t_end = Timestamp::from_millisecond(1_735_030_060_000).unwrap();
        let end_outcome = store
            .commit(DbCommitOp::EndSession {
                streamer_id: "s1".into(),
                streamer_name: "Dave".into(),
                session_id: Some(session_id),
                cause: TerminalCause::StreamerOffline,
                via_hysteresis: false,
                update_streamer_state: Some(StreamerState::NotLive),
                clear_errors: false,
                checks_restored_threshold: None,
                infra_block: None,
                suppress_offline_notification: false,
                now: t_end,
            })
            .await
            .unwrap();
        assert!(matches!(
            end_outcome,
            DbCommitOutcome::SessionEnded {
                transitioned_from_open: true,
                ..
            }
        ));

        let changes = observed.lock().await;
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].updated_streamers, vec!["s1".to_owned()]);
        assert!(changes[0].outbox_enqueued);
        assert_eq!(changes[1].updated_streamers, vec!["s1".to_owned()]);
        assert!(changes[1].outbox_enqueued);
    })
    .await
    .expect("post-publication listener test must finish");
}

#[tokio::test]
async fn out_of_space_cooldown_allows_recovery_after_expiry_and_offline_preserves_state() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (pool, store, committed_state, _outbox_notify) = setup_store().await;
        insert_streamer(&pool, "s1", "OosStreamer", StreamerState::NotLive).await;

        let t0 = Timestamp::from_millisecond(1_735_040_000_000).unwrap();
        store
            .commit(infra_block_op(
                "s1",
                InfraBlockReason::OutputRootUnavailable {
                    path: std::path::PathBuf::from("/rec"),
                    io_kind: IoErrorKindSer::StorageFull,
                    retry_after_secs: 30,
                },
                t0,
            ))
            .await
            .unwrap();
        assert_eq!(
            committed_state.get_streamer("s1").unwrap().state,
            StreamerState::OutOfSpace
        );

        // 1. Inside cooldown (t0 + 10s): StartOrResumeSession is suppressed with OutOfSpace.
        let t_during = Timestamp::from_millisecond(1_735_040_010_000).unwrap();
        let suppressed = store
            .commit(DbCommitOp::StartOrResumeSession {
                streamer_id: "s1".into(),
                streamer_name: "OosStreamer".into(),
                title: "During Cooldown".into(),
                category: None,
                current_avatar: None,
                new_avatar: None,
                credential_binding: None,
                now: t_during,
            })
            .await
            .unwrap();
        assert!(matches!(
            suppressed,
            DbCommitOutcome::SessionStartSuppressed {
                state: StreamerState::OutOfSpace,
                retry_after: Some(d),
            } if d == Duration::from_secs(20)
        ));

        // 2. An offline EndSession must NOT overwrite OutOfSpace back to NotLive.
        store
            .commit(DbCommitOp::EndSession {
                streamer_id: "s1".into(),
                streamer_name: "OosStreamer".into(),
                session_id: None,
                cause: TerminalCause::StreamerOffline,
                via_hysteresis: false,
                update_streamer_state: Some(StreamerState::NotLive),
                clear_errors: false,
                checks_restored_threshold: None,
                infra_block: None,
                suppress_offline_notification: false,
                now: t_during,
            })
            .await
            .unwrap();
        assert_eq!(
            committed_state.get_streamer("s1").unwrap().state,
            StreamerState::OutOfSpace
        );

        // 3. After cooldown expires (t0 + 35s): StartOrResumeSession is permitted so
        //    the startup path can probe `ensure_output_dir` and trigger gate recovery.
        let t_after = Timestamp::from_millisecond(1_735_040_035_000).unwrap();
        let recovered = store
            .commit(DbCommitOp::StartOrResumeSession {
                streamer_id: "s1".into(),
                streamer_name: "OosStreamer".into(),
                title: "After Cooldown".into(),
                category: None,
                current_avatar: None,
                new_avatar: None,
                credential_binding: None,
                now: t_after,
            })
            .await
            .unwrap();
        assert!(matches!(
            recovered,
            DbCommitOutcome::SessionStarted {
                disposition: SessionStartDisposition::Created,
                ..
            }
        ));
    })
    .await
    .expect("out-of-space cooldown recovery integration test must finish");
}
