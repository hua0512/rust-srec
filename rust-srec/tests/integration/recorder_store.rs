//! Real SQLite integration tests for `RecorderStore`.

use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use async_trait::async_trait;
use jiff::Timestamp;
use rust_srec::config::ConfigEventBroadcaster;
use rust_srec::credentials::{
    CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialOwner,
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

#[tokio::test]
async fn session_lifecycle_outbox_timestamps_and_suppression_contracts() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, committed_state, _notify) = setup_store().await;
        insert_streamer(&pool, "s1", "Alice", StreamerState::NotLive).await;

        // 1. Created session: exact millisecond alignment across live_sessions,
        //    session_events, monitor_event_outbox.created_at, OutboxEvent::StreamOnline,
        //    and StreamerCache.
        let t_start = Timestamp::from_millisecond(1_735_000_123_456).unwrap();
        let outcome = store
            .commit(DbCommitOp::StartOrResumeSession {
                streamer_id: "s1".into(),
                streamer_name: "Alice".into(),
                title: "Initial Title".into(),
                category: Some("Gaming".into()),
                current_avatar: None,
                new_avatar: Some("https://cdn.example/avatar.png".into()),
                credential_binding: None,
                now: t_start,
            })
            .await
            .unwrap();
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

        // 2. ReusedActive on an already-open row reuses the session_id, appends title,
        //    and emits StreamOnline(ReusedActive) with exact created_at.
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

        // 3. Hysteresis entry + ResumeHysteresis + AddSessionTitle deduplication,
        //    asserting actual persisted session_events and live_sessions.titles.
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
        assert_eq!(outbox[1].created_at, t_reuse.as_millisecond());
        assert!(matches!(
            &outbox[1].event,
            OutboxEvent::StreamOnline {
                started_at_ms,
                ..
            } if *started_at_ms == t_reuse.as_millisecond()
        ));
        assert_eq!(outbox[2].created_at, t_resume.as_millisecond());
        assert!(matches!(
            &outbox[2].event,
            OutboxEvent::StreamOnline {
                started_at_ms,
                ..
            } if *started_at_ms == t_resume.as_millisecond()
        ));

        // 4. end_session_only (update_streamer_state: None) emits StreamOffline ONCE
        //    with exact duration_secs & ended_at_ms; subsequent monitor offline updates
        //    streamer state to NOT_LIVE, clears errors, emits ChecksRestored, and applies
        //    infra_block without double-emitting StreamOffline.
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

        // Reset backoff so subsequent session-start subtests run cleanly.
        sqlx::query(
            "UPDATE streamers SET state = 'NOT_LIVE', disabled_until = NULL WHERE id = 's1'",
        )
        .execute(&pool)
        .await
        .unwrap();

        // ResumeHysteresis on an already-ended session fails transiently (Conflict),
        // and AddSessionTitle on an ended session is a no-op.
        assert!(matches!(
            store
                .commit(DbCommitOp::ResumeHysteresis {
                    streamer_id: "s1".into(),
                    streamer_name: "Alice".into(),
                    session_id: session_id.clone(),
                    title: "Too Late".into(),
                    category: None,
                    hysteresis_duration_secs: 10,
                    now: t_second_end,
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

        let outbox = fetch_outbox_events(&pool).await;
        assert_eq!(outbox.len(), 5);
        assert_eq!(outbox[3].created_at, t_end.as_millisecond());
        assert_eq!(
            outbox[3].event,
            OutboxEvent::StreamOffline {
                streamer_id: "s1".into(),
                streamer_name: "Alice".into(),
                session_id: session_id.clone(),
                duration_secs: Some(300.0),
                ended_at_ms: t_end.as_millisecond(),
            }
        );
        assert_eq!(outbox[4].created_at, t_second_end.as_millisecond());
        assert_eq!(
            outbox[4].event,
            OutboxEvent::ChecksRestored {
                streamer_id: "s1".into(),
                streamer_name: "Alice".into(),
                occurred_at_ms: t_second_end.as_millisecond(),
            }
        );

        // 5. Stale session_id cannot close a successor session, and suppressed causes
        //    plus explicit `suppress_offline_notification: true` never enqueue StreamOffline.
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
                .commit(DbCommitOp::EndSession {
                    streamer_id: "s1".into(),
                    streamer_name: "Alice".into(),
                    session_id: Some(session_id.clone()),
                    cause: TerminalCause::StreamerOffline,
                    via_hysteresis: false,
                    update_streamer_state: Some(StreamerState::NotLive),
                    clear_errors: false,
                    checks_restored_threshold: None,
                    infra_block: None,
                    suppress_offline_notification: false,
                    now: t_s,
                })
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

        // 6. Start & ResumeHysteresis suppression guards: Disabled, soft-deleted,
        //    OutOfSpace, and active TemporalDisabled backoff.
        insert_streamer(&pool, "s-disabled", "Disabled", StreamerState::Disabled).await;
        insert_streamer(&pool, "s-deleted", "Deleted", StreamerState::NotLive).await;
        sqlx::query("UPDATE streamers SET deleted_at = 1 WHERE id = 's-deleted'")
            .execute(&pool)
            .await
            .unwrap();
        insert_streamer(&pool, "s-oos", "OOS", StreamerState::OutOfSpace).await;
        insert_streamer(&pool, "s-backoff", "Backoff", StreamerState::TemporalDisabled).await;
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

        // 7. Self-healing multiple stale open live_sessions rows keeps the most recent
        //    and closes older rows with `stale_active_replaced` audit without StreamOffline.
        let (heal_pool, heal_store, _, _) = setup_store().await;
        insert_streamer(&heal_pool, "s-heal", "Healed", StreamerState::NotLive).await;
        sqlx::query("DROP INDEX live_sessions_one_active_per_streamer")
            .execute(&heal_pool)
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
            .execute(&heal_pool)
            .await
            .unwrap();
        }
        let t_heal = Timestamp::from_millisecond(1_735_003_020_000).unwrap();
        let heal_outcome = heal_store
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
                .fetch_one(&heal_pool)
                .await
                .unwrap();
        assert_eq!(healed_end, Some(t_heal.as_millisecond()));
        let heal_outbox = fetch_outbox_events(&heal_pool).await;
        assert_eq!(heal_outbox.len(), 1);
        assert_eq!(heal_outbox[0].event_type, "StreamOnline");
    })
    .await
    .expect("session lifecycle integration test must finish");
}

#[tokio::test]
async fn rewrite_ended_cause_and_error_infra_fatal_transitions() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (pool, store, committed_state, _notify) = setup_store().await;
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

        // 1. RewriteEndedCause updates via_hysteresis Completed -> UserDisabled
        //    without altering live_sessions.end_time or emitting outbox rows.
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

        // 2. Transient errors: ChecksFailing emitted ONLY on exact threshold (3),
        //    ClearLastError keeps error count, ClearErrors emits ChecksRestored once.
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

        // 3. InfraBlockReason persists OutOfSpace without touching error count.
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

        // 4. RecordFilteredState updates state without enqueueing outbox events.
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

        // 5. RecordFatalError closes active session, writes FatalError matching
        //    the monitor_event_outbox wire contract with exact timestamp, and emits
        //    no spurious StreamOffline.
        sqlx::query(
            "UPDATE streamers SET state = 'NOT_LIVE', disabled_until = NULL WHERE id = 's1'",
        )
        .execute(&pool)
        .await
        .unwrap();
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

        // 6. monitor_may_write guard: Disabled streamers ignore monitor error/infra/filter/fatal writes.
        insert_streamer(&pool, "s-dis", "Disabled", StreamerState::Disabled).await;
        let outbox_before_dis = fetch_outbox_events(&pool).await.len();
        store
            .commit(DbCommitOp::RecordTransientError {
                streamer_id: "s-dis".into(),
                streamer_name: "Disabled".into(),
                platform: "bilibili".into(),
                error: "ignored".into(),
                offline_check_count: 1,
                now: t_fatal,
            })
            .await
            .unwrap();
        store
            .commit(infra_block_op(
                "s-dis",
                InfraBlockReason::CircuitBreaker {
                    retry_after_secs: 60,
                },
                t_fatal,
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
                now: t_fatal,
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
                now: t_fatal,
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
    .expect("rewrite/error/fatal integration test must finish");
}

trait RewriteCheckExt {
    fn is_updated_rewrite(&self) -> bool;
}

impl RewriteCheckExt for DbCommitOutcome {
    fn is_updated_rewrite(&self) -> bool {
        matches!(self, Self::EndedCauseRewritten { updated: true, .. })
    }
}

#[tokio::test]
async fn segments_credential_bindings_and_retirement_contracts() {
    tokio::time::timeout(Duration::from_secs(20), async {
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

        let t0 = Timestamp::from_millisecond(1_735_020_000_000).unwrap();
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
                now: t0,
            })
            .await
            .unwrap()
        else {
            panic!("expected session started");
        };
        let binding_v1 = credential_binding.expect("committed binding");
        assert_eq!(binding_v1.epoch, 1);

        // 1. Segment persistence, index progression, and danmu output removal adjust
        //    total_size_bytes while leaving video outputs and session_segments intact.
        assert_eq!(
            store.next_session_segment_index(&session_id).await.unwrap(),
            0
        );
        store
            .persist_video_segment(VideoSegmentRecord {
                session_id: session_id.clone(),
                streamer_id: "s1".into(),
                segment_index: 0,
                segment_path: "/rec/seg0.flv".into(),
                started_at: Some(t0),
                completed_at: Timestamp::from_millisecond(1_735_020_010_000).unwrap(),
                duration_secs: 10.0,
                size_bytes: 1_000,
                split_reason_code: None,
                split_reason_details_json: None,
            })
            .await
            .unwrap();
        assert_eq!(
            store.next_session_segment_index(&session_id).await.unwrap(),
            1
        );

        store
            .persist_danmu_segment(DanmuSegmentRecord {
                session_id: session_id.clone(),
                streamer_id: "s1".into(),
                segment_index: 0,
                output_path: "/rec/seg0.xml".into(),
                size_bytes: 250,
                message_count: 12,
            })
            .await
            .unwrap();
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

        // 2. CommitCredentialBinding rotates to profile_b (epoch 1 -> 2), and rejects stale epoch.
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

        // 3. EndForCredentialRetirement ends ONLY when the exact (session_id, binding)
        //    matches an active row in retirement_credential_profiles, with no StreamOffline.
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
    .expect("segment and credential binding test must finish");
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

#[tokio::test]
async fn per_streamer_lease_handles_pre_commit_cancellation_and_listener_runs_lock_free() {
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

        // 1. Dropping a caller BEFORE commit seals cancels cleanly without leaking
        //    the per-streamer lease or leaving a half-created session.
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

        // 2. Subsequent commits wake `outbox_notify` and invoke `PostPublicationListener`
        //    with all locks (SQLite write connection, cache publication, streamer lease) free.
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
    .expect("lease cancellation and post-publication listener test must finish");
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
