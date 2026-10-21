use super::*;
use crate::database::models::StreamerDbModel;
use crate::database::repositories::monitor_outbox::MonitorOutboxOps;
use crate::database::repositories::{
    SessionRepository as _, SqlxSessionRepository, SqlxStreamerRepository, StreamerRepository as _,
};
use crate::database::{init_pool_with_size, run_migrations};
use crate::monitor::MonitorEvent;
use sqlx::SqlitePool;

mod committed_ownership;

const STREAMER_ID: &str = "test-streamer";

#[tokio::test]
async fn lifecycle_entrypoints_wait_for_streamer_serialization() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let sid = started.session_id();
    let guard = lifecycle.lock_streamer(STREAMER_ID).await;
    let event = make_terminal_completed_hls_endlist(sid);
    let mut operations: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>> = vec![
        Box::pin(async {
            lifecycle
                .on_live_detected(live_args(Utc::now()))
                .await
                .unwrap();
        }),
        Box::pin(async {
            lifecycle
                .on_offline_detected(OfflineDetectedArgs {
                    streamer_id: STREAMER_ID,
                    streamer_name: "Test",
                    session_id: Some(sid),
                    state_was_live: true,
                    clear_errors: false,
                    signal: None,
                    now: Utc::now(),
                })
                .await
                .unwrap();
        }),
        Box::pin(async {
            lifecycle.on_download_terminal(&event).await.unwrap();
        }),
        Box::pin(async {
            lifecycle
                .end_for_disable(STREAMER_ID, "Test")
                .await
                .unwrap();
        }),
        Box::pin(async {
            lifecycle
                .end_for_out_of_schedule(STREAMER_ID, "Test", StreamerState::Live)
                .await
                .unwrap();
        }),
    ];
    // Poll every entrypoint while its stripe is held. None may even acquire
    // the database connection until the preceding operation publishes.
    for operation in &mut operations {
        std::future::poll_fn(|cx| {
            assert!(operation.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
    }
    let connection = tokio::time::timeout(Duration::from_secs(1), pool.acquire())
        .await
        .expect("blocked operations must not enter the DB")
        .unwrap();
    drop(connection);
    assert!(db_session_end_time(&pool, sid).await.is_none());
    drop(operations);
    drop(guard);
    lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
    assert!(db_session_end_time(&pool, sid).await.is_some());
}

#[tokio::test]
async fn expired_timer_cannot_claim_replacement_after_resume() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_window(pool.clone(), Duration::from_secs(1));
    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let sid = started.session_id();
    let guard = lifecycle.lock_streamer(STREAMER_ID).await;
    lifecycle
        .enter_hysteresis_state(
            sid,
            STREAMER_ID,
            "Test",
            TerminalCause::Completed,
            Utc::now(),
        )
        .await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    // SQLite runs on a worker thread. Paused time can auto-advance pool
    // deadlines before that thread replies, so resume before any DB awaits.
    tokio::time::resume();
    assert!(lifecycle.hysteresis.contains_key(sid));
    assert!(
        lifecycle
            .resume_from_hysteresis(sid, &live_args(Utc::now()))
            .await
            .is_some()
    );

    // A new ambiguous terminal owns a distinct cancellation token. Seed
    // its handle without another task so joining below isolates the old timer.
    let replacement = HysteresisHandle::new(Duration::from_secs(60));
    lifecycle.sessions.insert(
        sid.to_string(),
        SessionState::hysteresis(
            STREAMER_ID,
            sid,
            Utc::now(),
            Utc::now(),
            TerminalCause::Completed,
            replacement.deadline,
        ),
    );
    lifecycle.hysteresis.insert(sid.to_string(), replacement);
    drop(guard);
    let mut tasks = DrainedTasks::take_from(&lifecycle.hysteresis_tasks);
    tokio::time::timeout(Duration::from_secs(1), tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(lifecycle.hysteresis.contains_key(sid));
    assert!(lifecycle.session_snapshot(sid).unwrap().is_hysteresis());
    assert!(db_session_end_time(&pool, sid).await.is_none());
    lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
}

#[tokio::test]
async fn duplicate_terminal_after_eviction_preserves_end_and_audit() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let sid = started.session_id();
    lifecycle
        .on_download_terminal(&make_terminal_completed_hls_endlist(sid))
        .await
        .unwrap();
    let end_time = db_session_end_time(&pool, sid).await;
    lifecycle.sessions.remove(sid);
    let mut rx = lifecycle.subscribe();
    lifecycle
        .on_download_terminal(&make_terminal_completed_hls_endlist(sid))
        .await
        .unwrap();
    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(sid))
        .await
        .unwrap();
    assert_eq!(db_session_end_time(&pool, sid).await, end_time);
    assert!(lifecycle.session_snapshot(sid).is_none());
    assert!(rx.try_recv().is_err());
    assert_eq!(
        read_events(&pool, sid)
            .await
            .into_iter()
            .filter(|(kind, _)| kind == "session_ended")
            .count(),
        1
    );
}

#[tokio::test]
async fn delayed_offline_for_old_session_preserves_live_successor() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let first = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    lifecycle
        .on_download_terminal(&make_terminal_completed_hls_endlist(first.session_id()))
        .await
        .unwrap();
    let first_end = db_session_end_time(&pool, first.session_id()).await;
    let second = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    assert_ne!(first.session_id(), second.session_id());
    let events_before = outbox_event_types(&pool).await;
    let mut rx = lifecycle.subscribe();
    // The persisted guard must survive expiry of the old in-memory snapshot.
    lifecycle.sessions.remove(first.session_id());
    let outcome = lifecycle
        .on_offline_detected(OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(first.session_id()),
            state_was_live: true,
            clear_errors: true,
            signal: None,
            now: Utc::now(),
        })
        .await
        .unwrap();
    assert!(outcome.resolved_session_id.is_none());
    assert!(!outcome.offline_event_emitted);
    assert_eq!(
        db_session_end_time(&pool, first.session_id()).await,
        first_end
    );
    assert!(
        db_session_end_time(&pool, second.session_id())
            .await
            .is_none()
    );
    assert_eq!(streamer_state(&pool, STREAMER_ID).await, "LIVE");
    assert!(
        lifecycle
            .session_snapshot(second.session_id())
            .unwrap()
            .is_recording()
    );
    assert_eq!(outbox_event_types(&pool).await, events_before);
    assert!(rx.try_recv().is_err());
}

async fn setup_pool() -> SqlitePool {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    create_test_streamer(&pool, STREAMER_ID, "Test", "https://example.com").await;
    pool
}

fn test_streamer_model(id: &str, name: &str, url: &str) -> StreamerDbModel {
    let mut streamer = StreamerDbModel::new(name, url, "platform-twitch");
    streamer.id = id.to_string();
    streamer
}

async fn create_test_streamer(pool: &SqlitePool, id: &str, name: &str, url: &str) {
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .create_streamer(&test_streamer_model(id, name, url))
        .await
        .unwrap();
}

async fn streamer_state(pool: &SqlitePool, streamer_id: &str) -> String {
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .get_streamer(streamer_id)
        .await
        .unwrap()
        .state
}

async fn outbox_event_types(pool: &SqlitePool) -> Vec<String> {
    MonitorOutboxOps::fetch_undelivered(pool, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| {
            let event: MonitorEvent =
                serde_json::from_str(&entry.payload).expect("outbox payload deserialises");
            match event {
                MonitorEvent::StreamerLive { .. } => "StreamerLive",
                MonitorEvent::StreamerOffline { .. } => "StreamerOffline",
                MonitorEvent::FatalError { .. } => "FatalError",
                MonitorEvent::TransientError { .. } => "TransientError",
                MonitorEvent::StateChanged { .. } => "StateChanged",
            }
            .to_string()
        })
        .collect()
}

fn make_lifecycle(pool: SqlitePool) -> Arc<SessionLifecycle> {
    Arc::new(SessionLifecycle::new(
        Arc::new(SessionLifecycleRepository::new(pool)),
        Arc::new(OfflineClassifier::new()),
        16,
    ))
}

/// Same as `make_lifecycle` but with a tunable hysteresis window — useful
/// for tests that need to drive timer expiry without sleeping for 90s.
fn make_lifecycle_with_window(
    pool: SqlitePool,
    window: std::time::Duration,
) -> Arc<SessionLifecycle> {
    let cfg = HysteresisConfig::from_window(window);
    Arc::new(SessionLifecycle::with_config(
        Arc::new(SessionLifecycleRepository::new(pool)),
        Arc::new(OfflineClassifier::new()),
        16,
        cfg,
    ))
}

/// Fast-path test helper: 25ms window + a 100ms sleep after firing
/// `on_download_terminal` lets ambiguous-Failed scenarios reach `Ended`
/// without a real 90s wait. Tests that don't care about the
/// Recording→Hysteresis intermediate state use this to assert on the
/// final Ended state directly.
fn make_lifecycle_fast(pool: SqlitePool) -> Arc<SessionLifecycle> {
    make_lifecycle_with_window(pool, std::time::Duration::from_millis(25))
}

async fn wait_for_hysteresis_to_expire() {
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}

fn live_args<'a>(now: DateTime<Utc>) -> LiveDetectedArgs<'a> {
    // Static empty collections keep the test signatures simple.
    static EMPTY_STREAMS: std::sync::OnceLock<Vec<crate::monitor::StreamInfo>> =
        std::sync::OnceLock::new();
    let streams = EMPTY_STREAMS.get_or_init(Vec::new);
    LiveDetectedArgs {
        credential_binding: None,
        streamer_id: STREAMER_ID,
        streamer_name: "Test",
        streamer_url: "https://example.com",
        current_avatar: None,
        new_avatar: None,
        title: "Live!",
        category: None,
        streams,
        media_headers: None,
        media_extras: None,
        now,
    }
}

#[tokio::test]
async fn on_download_terminal_is_idempotent() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool);
    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();

    let event = DownloadTerminalEvent::Completed {
        download_id: "dl-1".into(),
        streamer_id: STREAMER_ID.into(),
        streamer_name: "Test".into(),
        session_id: started.session_id().to_string(),
        total_bytes: 0,
        total_duration_secs: 0.0,
        total_segments: 0,
        file_path: None,
        engine_signal: crate::downloader::EngineEndSignal::Unknown,
        stop_cause: None,
    };
    lifecycle.on_download_terminal(&event).await.unwrap();

    // Second call must be a no-op.
    let mut rx = lifecycle.subscribe();
    lifecycle.on_download_terminal(&event).await.unwrap();
    assert!(
        rx.try_recv().is_err(),
        "second terminal event should not re-emit SessionTransition::Ended"
    );
}

// =========================================================================
// Download-terminal scenarios.
//
// These scenarios pin two invariants of `on_download_terminal`: every
// terminal cause maps to the right SessionTransition (in particular,
// DownloadFailed must end the session and carry a positive
// `should_run_session_complete_pipeline` decision), and the in-memory
// `is_session_active` view stays consistent with `live_sessions.end_time`.
//
// These tests are intentionally SessionLifecycle-scoped: pipeline-side
// behaviour for each cause is covered by `pipeline::manager::tests`. Here
// we assert the boundary between the download-event subscription and the
// SessionTransition broadcast.
// =========================================================================

async fn db_session_end_time(pool: &SqlitePool, session_id: &str) -> Option<i64> {
    SqlxSessionRepository::new(pool.clone(), pool.clone())
        .get_session(session_id)
        .await
        .unwrap()
        .end_time
}

fn make_terminal_cancelled(session_id: &str) -> DownloadTerminalEvent {
    DownloadTerminalEvent::Cancelled {
        download_id: "dl-b".into(),
        streamer_id: STREAMER_ID.into(),
        streamer_name: "Test".into(),
        session_id: session_id.into(),
        cause: crate::downloader::DownloadStopCause::User,
    }
}

/// Terminal::Cancelled is a no-op: session stays Recording, no
/// SessionTransition is emitted, and no DB end_time is written.
#[tokio::test]
async fn cancelled_keeps_session_recording_and_emits_nothing() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // drain Started

    lifecycle
        .on_download_terminal(&make_terminal_cancelled(started.session_id()))
        .await
        .unwrap();

    assert!(
        lifecycle.is_session_active(started.session_id()),
        "Cancelled must leave session in Recording"
    );
    assert!(
        rx.try_recv().is_err(),
        "Cancelled must not emit SessionTransition"
    );
    assert!(
        db_session_end_time(&pool, started.session_id())
            .await
            .is_none(),
        "Cancelled must not write end_time to DB"
    );
}

/// A download that never started recorded nothing for a hysteresis window to
/// protect: the session ends at once and the post-recording pipeline is skipped.
#[tokio::test]
async fn rejected_start_ends_the_session_at_once_without_the_pipeline() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // drain Started

    lifecycle
        .on_download_terminal(&DownloadTerminalEvent::Rejected {
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: started.session_id().into(),
            reason: "circuit breaker".into(),
            retry_after_secs: None,
            kind: crate::downloader::DownloadRejectedKind::CircuitBreaker,
        })
        .await
        .unwrap();

    match rx.try_recv().expect("the end is published synchronously") {
        SessionTransition::Ended {
            cause,
            via_hysteresis,
            ..
        } => {
            assert!(matches!(cause, TerminalCause::Rejected { .. }), "{cause:?}");
            assert!(!via_hysteresis);
            assert!(!cause.should_run_session_complete_pipeline());
        }
        other => panic!("expected Ended, got {other:?}"),
    }
    assert!(!lifecycle.is_session_active(started.session_id()));
    assert!(
        db_session_end_time(&pool, started.session_id())
            .await
            .is_some()
    );
}

// Atomicity / fault injection is deliberately out of scope here.
// Partial-write rollback relies on sqlx's BEGIN IMMEDIATE semantics,
// which the `SessionLifecycleRepository` tests cover by asserting that
// multi-step bundles land atomically.

// =========================================================================
// OfflineClassifier promotion inside on_download_terminal.
//
// The unit-level classifier rules live in `session::classifier::tests`.
// These scenarios assert the *integration* inside `SessionLifecycle`:
// Terminal::Failed variants that the classifier promotes end the session
// with TerminalCause::DefinitiveOffline (not plain Failed), and non-mesio
// failures are never promoted.
// =========================================================================

/// Failed events must preserve the originating engine. FFmpeg and
/// Streamlink failures are too fuzzy to become definitive offline
/// signals, even when two Network failures arrive inside the classifier
/// window. They should stay on the ambiguous Failed → Hysteresis path.
#[tokio::test]
async fn non_mesio_network_failures_do_not_promote() {
    for engine_type in [EngineType::Ffmpeg, EngineType::Streamlink] {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_with_window(pool, Duration::from_secs(60));
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // drain Started

        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl-1".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: started.session_id().to_string(),
                engine_type,
                protocol: DownloadProtocol::Flv,
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "timeout".into(),
                recoverable: false,
            })
            .await
            .unwrap();
        match rx.recv().await.unwrap() {
            SessionTransition::Ending { cause, .. } => {
                assert!(
                    matches!(cause, TerminalCause::Failed { .. }),
                    "{engine_type} first Network must enter Hysteresis with Failed cause, got {cause:?}"
                );
            }
            other => panic!("expected Ending for {engine_type}, got {other:?}"),
        }

        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl-2".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: started.session_id().to_string(),
                engine_type,
                protocol: DownloadProtocol::Flv,
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "timeout".into(),
                recoverable: false,
            })
            .await
            .unwrap();

        assert!(
            rx.try_recv().is_err(),
            "{engine_type} second Network failure must not emit DefinitiveOffline"
        );
        match lifecycle
            .session_snapshot(started.session_id())
            .expect("session still tracked")
        {
            SessionState::Hysteresis { cause, .. } => {
                assert!(
                    matches!(cause, TerminalCause::Failed { .. }),
                    "{engine_type} must remain hysteresis with Failed cause, got {cause:?}"
                );
            }
            other => panic!("expected Hysteresis for {engine_type}, got {other:?}"),
        }
    }
}

/// DefinitiveOffline bypasses the streamer's `disabled_until` backoff for
/// the session-end write. Monitor check-loop backoff stays untouched
/// (scheduled elsewhere by the actor), but the session row is closed
/// immediately so the UI and pipeline trigger don't wait for the backoff
/// window to expire.
#[tokio::test]
async fn definitive_offline_bypasses_streamer_disabled_until() {
    let pool = setup_pool().await;

    // Place the streamer in a long backoff window.
    let backoff_until_ms = (Utc::now() + chrono::Duration::seconds(240)).timestamp_millis();
    let streamer_repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
    let mut streamer = streamer_repo.get_streamer(STREAMER_ID).await.unwrap();
    streamer.disabled_until = Some(backoff_until_ms);
    streamer.consecutive_error_count = Some(3);
    streamer_repo.update_streamer(&streamer).await.unwrap();

    // `make_lifecycle_fast` shrinks the hysteresis backstop to 25 ms so
    // the test doesn't have to wait for the default 80 s window in case
    // the classifier never promotes. The backoff-bypass invariant being
    // tested is independent of the hysteresis window length.
    let lifecycle = make_lifecycle_fast(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // drain Started

    // Drive DefinitiveOffline via the consecutive-Network rule (the 404
    // fast-path was deleted because transient 404s overfired in
    // production — see `session::classifier` docs). The first Failed
    // event parks the session in Hysteresis; the second crosses the
    // classifier threshold and forces an authoritative end.
    for tag in ["dl-1", "dl-2"] {
        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: tag.into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: started.session_id().to_string(),
                engine_type: EngineType::Mesio,
                protocol: DownloadProtocol::Flv,
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "timeout".into(),
                recoverable: false,
            })
            .await
            .unwrap();
    }

    // Session ended within the two synchronous awaits above — no backoff
    // wait, no hysteresis-timer wait. The classifier promoted the second
    // Failed straight to authoritative DefinitiveOffline, which cancels
    // the in-flight Hysteresis handle and writes end_time in one tx.
    assert!(!lifecycle.is_session_active(started.session_id()));
    assert!(
        db_session_end_time(&pool, started.session_id())
            .await
            .is_some(),
        "session end_time must be written without waiting on backoff"
    );

    // Receiver should see Ending (first Failed) followed by Ended
    // (second Failed → DefinitiveOffline).
    match rx.recv().await.unwrap() {
        SessionTransition::Ending { cause, .. } => {
            assert!(
                matches!(cause, TerminalCause::Failed { .. }),
                "first Network must enter Hysteresis with Failed cause, got {cause:?}"
            );
        }
        other => panic!("expected Ending, got {other:?}"),
    }
    match rx.recv().await.unwrap() {
        SessionTransition::Ended {
            cause,
            via_hysteresis,
            ..
        } => {
            assert!(matches!(
                cause,
                TerminalCause::DefinitiveOffline {
                    signal: crate::session::OfflineSignal::ConsecutiveFailures(2)
                }
            ));
            assert!(
                !via_hysteresis,
                "DefinitiveOffline must skip the hysteresis-timer path"
            );
        }
        other => panic!("expected Ended, got {other:?}"),
    }

    // Streamer-side backoff is unchanged by the session-end write —
    // disabled_until and consecutive_error_count remain as seeded so
    // the monitor's next tick is still throttled as before.
    let streamer = streamer_repo.get_streamer(STREAMER_ID).await.unwrap();
    assert_eq!(
        streamer.disabled_until,
        Some(backoff_until_ms),
        "disabled_until must remain set (only session-end bypasses backoff)"
    );
    assert_eq!(
        streamer.consecutive_error_count,
        Some(3),
        "consecutive_error_count must remain set"
    );
}

// =========================================================================
// Hysteresis correctness scenarios.
//
// These tests drive the FSM directly. They use a 25 ms hysteresis window
// so timer expiry is observable without sleeping for the production 90 s
// default.
// =========================================================================

fn make_terminal_completed_clean_disconnect(session_id: &str) -> DownloadTerminalEvent {
    DownloadTerminalEvent::Completed {
        download_id: "dl-i".into(),
        streamer_id: STREAMER_ID.into(),
        streamer_name: "Test".into(),
        session_id: session_id.into(),
        total_bytes: 0,
        total_duration_secs: 0.0,
        total_segments: 0,
        file_path: None,
        engine_signal: crate::downloader::EngineEndSignal::CleanDisconnect,
        stop_cause: None,
    }
}

fn make_terminal_completed_hls_endlist(session_id: &str) -> DownloadTerminalEvent {
    make_terminal_completed_with_signal(session_id, crate::downloader::EngineEndSignal::HlsEndlist)
}

fn make_terminal_completed_with_signal(
    session_id: &str,
    engine_signal: crate::downloader::EngineEndSignal,
) -> DownloadTerminalEvent {
    DownloadTerminalEvent::Completed {
        download_id: "dl-i".into(),
        streamer_id: STREAMER_ID.into(),
        streamer_name: "Test".into(),
        session_id: session_id.into(),
        total_bytes: 0,
        total_duration_secs: 0.0,
        total_segments: 0,
        file_path: None,
        engine_signal,
        stop_cause: None,
    }
}

#[tokio::test]
async fn shutdown_cancels_and_joins_hysteresis_without_ending_session() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_window(pool.clone(), std::time::Duration::from_secs(5));
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap();
    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap();

    lifecycle
        .shutdown_until(tokio::time::Instant::now() + std::time::Duration::from_secs(1))
        .await;

    assert!(
        db_session_end_time(&pool, started.session_id())
            .await
            .is_none()
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), rx.recv())
            .await
            .is_err(),
        "a cancelled timer must not publish Ended after the shutdown fence"
    );
}

#[tokio::test]
async fn shutdown_drains_admitted_operations_and_rejects_late_mutations() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool);
    let admitted_operation = lifecycle.operation_gate.read().await;

    let shutdown_lifecycle = lifecycle.clone();
    let mut shutdown = tokio::spawn(async move {
        shutdown_lifecycle
            .shutdown_until(tokio::time::Instant::now() + std::time::Duration::from_secs(1))
            .await
    });

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), &mut shutdown)
            .await
            .is_err(),
        "shutdown must wait for an operation admitted before the fence"
    );

    drop(admitted_operation);
    shutdown.await.expect("shutdown task should join");

    let error = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .expect_err("a lifecycle mutation after shutdown must be rejected");
    assert!(error.to_string().contains("shutting down"));
}

#[tokio::test]
async fn shutdown_drains_cancelled_timer_after_graceful_deadline() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool);
    let release_timer = Arc::new(tokio::sync::Notify::new());
    let task_release = release_timer.clone();
    lifecycle
        .hysteresis_tasks
        .lock()
        .spawn(async move { task_release.notified().await });

    let shutdown_lifecycle = lifecycle.clone();
    let mut shutdown = tokio::spawn(async move {
        shutdown_lifecycle
            .shutdown_until(tokio::time::Instant::now() + std::time::Duration::from_millis(10))
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), &mut shutdown)
            .await
            .is_err(),
        "deadline expiry must not abort an owned timer future"
    );

    release_timer.notify_one();
    let report = tokio::time::timeout(std::time::Duration::from_secs(1), shutdown)
        .await
        .expect("shutdown should finish after timer settlement is released")
        .expect("shutdown task should join");
    assert_eq!(report.forced_timer_count, 1);
    assert!(
        report
            .overruns
            .iter()
            .any(|overrun| overrun.contains("graceful hysteresis timer deadline exceeded"))
    );
    assert!(
        report.failures.is_empty(),
        "a timer that stayed contained must not be reported as a failure: {:?}",
        report.failures
    );
}

/// A live detection that lost the race against a user disable must be
/// suppressed by the row-level guard inside `start_or_resume`: no
/// session in memory, no transition broadcast, and `state = DISABLED`
/// stays untouched instead of being overwritten with LIVE.
#[tokio::test]
async fn on_live_detected_suppressed_for_disabled_streamer() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .update_streamer_state(STREAMER_ID, StreamerState::Disabled.as_str())
        .await
        .unwrap();

    let outcome = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();

    assert_eq!(
        outcome,
        StartSessionOutcome::SuppressedInactive {
            state: StreamerState::Disabled
        }
    );
    assert_eq!(streamer_state(&pool, STREAMER_ID).await, "DISABLED");
    assert!(
        rx.try_recv().is_err(),
        "suppressed live detection must not broadcast a transition"
    );
    assert!(outbox_event_types(&pool).await.is_empty());
}

/// The same race on the hysteresis-resume path: the resume must abort
/// before claiming the hysteresis exit, so the handle stays armed (the
/// disable teardown or the timer ends the session through the normal
/// path), no Resumed/Started is broadcast, and the DISABLED row is not
/// flipped to LIVE.
#[tokio::test]
async fn resume_suppressed_for_disabled_streamer_leaves_hysteresis_armed() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_window(pool.clone(), std::time::Duration::from_secs(5));
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started

    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ending

    // User disable commits while the live re-check is in flight.
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .update_streamer_state(STREAMER_ID, StreamerState::Disabled.as_str())
        .await
        .unwrap();

    let outcome = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();

    assert_eq!(
        outcome,
        StartSessionOutcome::SuppressedInactive {
            state: StreamerState::Disabled
        }
    );
    assert_eq!(
        streamer_state(&pool, STREAMER_ID).await,
        "DISABLED",
        "resume must not overwrite the user's DISABLED state with LIVE"
    );
    assert!(
        lifecycle.hysteresis.get(started.session_id()).is_some(),
        "hysteresis handle must stay armed so the normal end path owns the session"
    );
    assert!(
        rx.try_recv().is_err(),
        "suppressed resume must not broadcast Resumed/Started"
    );
    assert!(
        db_session_end_time(&pool, started.session_id())
            .await
            .is_none(),
        "the session end stays owned by the disable teardown / hysteresis timer"
    );
}

/// CAS atomicity for the `Hysteresis → (Recording | Ended)`
/// transition. We model the race by:
///
/// 1. Driving the session into `Hysteresis`.
/// 2. Manually consuming the hysteresis handle out of the lifecycle's
///    map (simulating a winning resume that already claimed the CAS).
/// 3. Calling `enter_ended_state` directly (simulating a losing
///    timer-fire / authoritative end that arrived after the resume).
///
/// Expected: `enter_ended_state` detects `was_in_hysteresis=true`
/// AND `claim=None` and bails — no `SessionTransition::Ended` is
/// broadcast, no DB end_time write, no in-memory state change to
/// `Ended`. The session stays `Hysteresis` (which the simulated
/// resume would then move to `Recording` if it completed).
///
/// The symmetric loss case (`enter_ended_state` wins,
/// `resume_from_hysteresis` loses) is covered by
/// `authoritative_end_during_hysteresis_cancels_timer` plus the CAS in
/// `resume_from_hysteresis` returning `None` on a missing handle.
#[tokio::test]
async fn cas_blocks_enter_ended_when_resume_already_claimed_hysteresis() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_window(pool.clone(), std::time::Duration::from_secs(5));
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started (fresh)

    // Step 1: drive into Hysteresis.
    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ending

    // Step 2: simulate a winning resume by removing the hysteresis
    // handle out from under the lifecycle. (In a real race, this
    // would happen inside `resume_from_hysteresis`'s CAS line.)
    // We don't proceed to the rest of resume — we just want to test
    // the symmetric path: `enter_ended_state` finds the handle gone.
    let claimed = lifecycle.hysteresis.remove(started.session_id());
    assert!(
        claimed.is_some(),
        "test pre-condition: hysteresis handle should exist after Hysteresis entry"
    );

    // Step 3: call `enter_ended_state` directly (the path the
    // hysteresis timer would take on fire, or `on_offline_detected`
    // would take on authoritative end).
    lifecycle
        .enter_ended_state(EndedStateTransition {
            session_id: started.session_id(),
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            cause: TerminalCause::StreamerOffline,
            ended_at: Utc::now(),
            via_hysteresis: true,
            db_write: DbWritePath::EndSessionOnly,
        })
        .await
        .unwrap();

    // Expected: enter_ended_state bailed via the CAS-lost path.
    // No `Ended` broadcast.
    let timeout = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
    assert!(
        timeout.is_err(),
        "enter_ended_state must NOT emit Ended when hysteresis CAS was lost; \
             got transition = {:?}",
        timeout
    );

    // In-memory session state must still be Hysteresis (a real
    // resume would then move it to Recording; we're testing the
    // intermediate consistency).
    assert!(
        lifecycle.is_session_active(started.session_id()),
        "session must remain active after CAS-lost enter_ended_state"
    );
    let state_kind = lifecycle
        .sessions
        .get(started.session_id())
        .map(|e| e.value().kind_str())
        .unwrap_or("(missing)");
    assert_eq!(
        state_kind, "hysteresis",
        "in-memory state must remain Hysteresis after CAS-lost enter_ended_state"
    );

    // DB end_time must NOT be set.
    let end_time = db_session_end_time(&pool, started.session_id()).await;
    assert!(
        end_time.is_none(),
        "DB end_time must NOT be written when CAS was lost"
    );
}

/// authoritative end during `Hysteresis` cancels the timer and
/// transitions directly to `Ended`. Models the danmu-close-after-FLV-
/// clean-disconnect scenario.
#[tokio::test]
async fn authoritative_end_during_hysteresis_cancels_timer() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_window(pool.clone(), std::time::Duration::from_secs(5));
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started

    // Step 1: ambiguous end → Hysteresis.
    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ending

    // Step 2: authoritative offline (monitor StreamerOffline) arrives
    // mid-window. Should cancel the timer and commit Ended immediately.
    lifecycle
        .on_offline_detected(OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(started.session_id()),
            state_was_live: true,
            clear_errors: false,
            signal: None,
            now: Utc::now(),
        })
        .await
        .unwrap();

    match rx.recv().await.unwrap() {
        SessionTransition::Ended {
            cause,
            via_hysteresis,
            ..
        } => {
            assert!(matches!(cause, TerminalCause::StreamerOffline));
            assert!(
                via_hysteresis,
                "session was in Hysteresis when authoritatively ended"
            );
        }
        other => panic!("expected Ended, got {other:?}"),
    }

    // No further events should arrive (the original timer was cancelled).
    wait_for_hysteresis_to_expire().await;
    assert!(
        rx.try_recv().is_err(),
        "timer must be cancelled, no late Ended"
    );
}

// -----------------------------------------------------------------
// `session_events` audit-log persistence.
//
// Atomic-tx writes (`session_started`, `session_ended`) go through
// `SessionLifecycleRepository`. Best-effort writes
// (`hysteresis_entered`, `session_resumed`) require the lifecycle to
// hold an `event_repo`, which `make_lifecycle_with_events` wires in.
// -----------------------------------------------------------------

use crate::database::repositories::{SessionEventRepository, SqlxSessionEventRepository};
use crate::session::events::{SessionEventPayload, TerminalCauseDto};

fn make_lifecycle_with_events(pool: SqlitePool) -> Arc<SessionLifecycle> {
    // Audit tests drive transitions explicitly and never wait for expiry.
    // Keep the default hysteresis and dedup retention windows so SQLite
    // worker latency and CI scheduling cannot expire the state under test.
    let event_repo: Arc<dyn SessionEventRepository> =
        Arc::new(SqlxSessionEventRepository::new(pool.clone(), pool.clone()));
    Arc::new(
        SessionLifecycle::new(
            Arc::new(SessionLifecycleRepository::new(pool)),
            Arc::new(OfflineClassifier::new()),
            16,
        )
        .with_event_repo(event_repo),
    )
}

async fn read_events(
    pool: &SqlitePool,
    session_id: &str,
) -> Vec<(String, Option<SessionEventPayload>)> {
    SqlxSessionEventRepository::new(pool.clone(), pool.clone())
        .list_for_session(session_id)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.kind, row.payload))
        .collect()
}

fn event_payload(raw: &Option<SessionEventPayload>) -> SessionEventPayload {
    raw.clone().expect("payload present")
}

fn session_ended_cause(payload: &SessionEventPayload) -> &TerminalCauseDto {
    let SessionEventPayload::SessionEnded { cause, .. } = payload else {
        panic!("expected session_ended payload, got {payload:?}");
    };
    cause
}

/// `enter_ended_state`'s 60s `Ended` retention dedup means duplicate
/// `on_offline_detected` calls land on the CAS guard before reaching
/// the tx — so exactly one `session_ended` row is persisted per
/// session, even when the monitor races and emits the offline twice.
#[tokio::test]
async fn session_ended_dedup_persists_one_row() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_events(pool.clone());

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let sid = started.session_id().to_string();
    let now = Utc::now();
    let mk_args = || OfflineDetectedArgs {
        streamer_id: STREAMER_ID,
        streamer_name: "Test",
        session_id: Some(&sid),
        state_was_live: true,
        clear_errors: false,
        signal: None,
        now,
    };

    lifecycle.on_offline_detected(mk_args()).await.unwrap();
    lifecycle.on_offline_detected(mk_args()).await.unwrap();

    let rows = read_events(&pool, &sid).await;
    let ended_count = rows.iter().filter(|(k, _)| k == "session_ended").count();
    assert_eq!(
        ended_count, 1,
        "duplicate authoritative-end must not double-write the audit row"
    );
}

// =========================================================================
// `end_for_disable` user-disabled teardown.
//
// Verifies that user-initiated tear-down keeps in-memory FSM and DB in
// lockstep, runs the same CAS protocol as enter_ended_state, and
// produces a single authoritative `Ended` broadcast with cause
// user_disabled.
// =========================================================================

/// Helper: read latest session_ended audit row payload for a session.
async fn latest_session_ended_payload(pool: &SqlitePool, sid: &str) -> Option<SessionEventPayload> {
    SqlxSessionEventRepository::new(pool.clone(), pool.clone())
        .list_for_session(sid)
        .await
        .unwrap()
        .into_iter()
        .rev()
        .find(|row| row.kind == "session_ended")
        .and_then(|row| row.payload)
}

/// `end_for_disable` on a session in `Hysteresis` cancels the
/// timer (CAS won), writes Ended with `via_hysteresis: true`, and
/// emits a single Ended broadcast (no late timer-fire follow-up).
#[tokio::test]
async fn end_for_disable_cancels_hysteresis_handle() {
    let pool = setup_pool().await;
    // Long window so the timer cannot fire during the test.
    let lifecycle = make_lifecycle_with_window(pool.clone(), Duration::from_secs(60));
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started

    // Park in Hysteresis.
    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ending

    let sid = started.session_id().to_string();
    let resolved = lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(sid.as_str()));

    // Hysteresis handle removed, timer cancelled.
    assert!(
        !lifecycle.hysteresis.contains_key(&sid),
        "hysteresis handle must be claimed and removed"
    );

    // Broadcast: Ended with via_hysteresis=true.
    match rx.recv().await.unwrap() {
        SessionTransition::Ended {
            cause,
            via_hysteresis,
            ..
        } => {
            assert_eq!(cause, TerminalCause::UserDisabled);
            assert!(
                via_hysteresis,
                "via_hysteresis must reflect that we tore down a Hysteresis state"
            );
        }
        other => panic!("expected Ended, got {other:?}"),
    }

    // No follow-up Ended from the timer (its cancel token was tripped).
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        rx.try_recv().is_err(),
        "no second Ended must arrive from the cancelled timer"
    );
}

/// Missing timer ownership does not prove the DB session has ended.
#[tokio::test]
async fn end_for_disable_loses_cas_to_resume() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_window(pool.clone(), Duration::from_secs(60));
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started

    // Park in Hysteresis.
    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ending

    // Manually consume the hysteresis handle to simulate "resume already won
    // CAS but in-memory state is still Hysteresis."
    let sid = started.session_id().to_string();
    let claimed = lifecycle.hysteresis.remove(&sid);
    assert!(claimed.is_some(), "test seed: handle must exist");

    // Disable must still close the active row.
    let resolved = lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(sid.as_str()));
    assert!(db_session_end_time(&pool, &sid).await.is_some());
    assert!(matches!(rx.try_recv(), Ok(SessionTransition::Ended { .. })));
    claimed.unwrap().1.cancel();
}

/// O5b — `end_for_disable` loses CAS to the hysteresis timer fire.
/// Timer ends the session with cause Completed; `end_for_disable` sees
/// the Ended state and retro-rewrites the audit row's cause to
/// `user_disabled`. Verifies cause-overwrite-on-CAS-loss behaviour.
#[tokio::test]
async fn end_for_disable_overwrites_cause_when_timer_wins() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_fast(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started

    // Park in Hysteresis (25 ms window).
    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ending

    // Wait for the timer to fire and write Ended with cause=Completed.
    wait_for_hysteresis_to_expire().await;
    match rx.recv().await.unwrap() {
        SessionTransition::Ended { cause, .. } => {
            assert_eq!(
                cause,
                TerminalCause::Completed,
                "timer-fire path uses the cause that put us into hysteresis"
            );
        }
        other => panic!("expected Ended {{cause:Completed}}, got {other:?}"),
    }

    // Now disable cleanup arrives late. end_for_disable sees Ended in
    // memory → retro-update path: rewrites session_ended row's cause
    // and patches in-memory snapshot's cause to UserDisabled.
    let sid = started.session_id().to_string();
    let resolved = lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(sid.as_str()));

    // Audit row's cause is now user_disabled.
    let payload = latest_session_ended_payload(&pool, &sid).await.unwrap();
    assert_eq!(
        session_ended_cause(&payload),
        &TerminalCauseDto::UserDisabled
    );

    // In-memory snapshot's cause was patched.
    let snap = lifecycle.session_snapshot(&sid).expect("session in memory");
    match snap {
        SessionState::Ended { cause, .. } => assert_eq!(cause, TerminalCause::UserDisabled),
        other => panic!("expected Ended state, got {other:?}"),
    }

    // No fresh Ended broadcast on retro-update (would double-fire
    // notifications). Receiver is empty.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        rx.try_recv().is_err(),
        "retro-update must NOT re-broadcast Ended"
    );
}

/// out-of-schedule tear-down must go through SessionLifecycle, not
/// a raw DB row close. This keeps subscribers and the in-memory FSM in
/// sync while still avoiding a synthetic `StreamerOffline` outbox event.
#[tokio::test]
async fn end_for_out_of_schedule_updates_lifecycle_and_outbox() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started

    let sid = started.session_id().to_string();
    let resolved = lifecycle
        .end_for_out_of_schedule(STREAMER_ID, "Test", StreamerState::Live)
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(sid.as_str()));

    assert!(
        db_session_end_time(&pool, &sid).await.is_some(),
        "out-of-schedule must set live_sessions.end_time"
    );

    let snapshot = lifecycle.session_snapshot(&sid).expect("session in memory");
    match snapshot {
        SessionState::Ended { cause, .. } => {
            assert_eq!(cause, TerminalCause::OutOfSchedule);
            assert!(cause.should_run_session_complete_pipeline());
        }
        other => panic!("expected Ended state, got {other:?}"),
    }

    match rx.recv().await.unwrap() {
        SessionTransition::Ended {
            session_id,
            cause,
            via_hysteresis,
            ..
        } => {
            assert_eq!(session_id, sid);
            assert_eq!(cause, TerminalCause::OutOfSchedule);
            assert!(!via_hysteresis);
        }
        other => panic!("expected Ended {{cause:OutOfSchedule}}, got {other:?}"),
    }

    let state = streamer_state(&pool, STREAMER_ID).await;
    assert_eq!(state, "OUT_OF_SCHEDULE");

    let event_types = outbox_event_types(&pool).await;
    assert_eq!(
        event_types,
        vec!["StreamerLive", "StateChanged"],
        "schedule stop must enqueue StateChanged, not StreamerOffline"
    );

    let payload = latest_session_ended_payload(&pool, &sid).await.unwrap();
    assert_eq!(
        session_ended_cause(&payload),
        &TerminalCauseDto::OutOfSchedule
    );
}

/// A schedule stop during hysteresis claims the hysteresis handle (so its
/// timer cannot end the session a second time) and reports the end as
/// reached through hysteresis. `end_for_out_of_schedule_inner` duplicates
/// rather than shares the disable path's claim.
#[tokio::test]
async fn end_for_out_of_schedule_cancels_hysteresis() {
    let pool = setup_pool().await;
    // Long window so the timer cannot fire during the test.
    let lifecycle = make_lifecycle_with_window(pool.clone(), Duration::from_secs(60));
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started
    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ending

    let sid = started.session_id().to_string();
    lifecycle
        .end_for_out_of_schedule(STREAMER_ID, "Test", StreamerState::Live)
        .await
        .unwrap();

    assert!(
        !lifecycle.hysteresis.contains_key(&sid),
        "out-of-schedule must claim and remove the hysteresis handle"
    );
    match rx.recv().await.unwrap() {
        SessionTransition::Ended {
            cause,
            via_hysteresis,
            ..
        } => {
            assert_eq!(cause, TerminalCause::OutOfSchedule);
            assert!(via_hysteresis);
        }
        other => panic!("expected Ended {{cause:OutOfSchedule}}, got {other:?}"),
    }
}

/// An explicit offline signal (the danmu stream closing) is recorded as a
/// definitive offline carrying that signal, not as a plain status-check
/// offline.
#[tokio::test]
async fn session_ended_persisted_with_definitive_offline_signal() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_events(pool.clone());

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let sid = started.session_id().to_string();

    lifecycle
        .on_offline_detected(OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(&sid),
            state_was_live: true,
            clear_errors: false,
            signal: Some(crate::session::OfflineSignal::DanmuStreamClosed),
            now: Utc::now(),
        })
        .await
        .unwrap();

    let rows = read_events(&pool, &sid).await;
    let last = rows.last().expect("at least one event");
    assert_eq!(last.0, "session_ended");
    match event_payload(&last.1) {
        SessionEventPayload::SessionEnded {
            cause,
            via_hysteresis,
        } => {
            assert!(!via_hysteresis, "direct authoritative end, not via timer");
            assert_eq!(
                cause,
                TerminalCauseDto::DefinitiveOffline {
                    signal: crate::session::OfflineSignal::DanmuStreamClosed,
                }
            );
        }
        other => panic!("wrong payload variant: {other:?}"),
    }
}

/// concurrent `end_for_disable` calls collapse to a single
/// effective tear-down. Exactly one returns `Some(session_id)` after
/// writing the row; others return either `Ok(None)` (no active row by
/// the time they reach the repo) or `Some(session_id)` via the
/// retro-update path. Exactly one `session_ended` audit row exists.
#[tokio::test]
async fn end_for_disable_idempotent_under_concurrency() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let sid = started.session_id().to_string();

    // Spawn 8 concurrent end_for_disable calls.
    let mut handles = Vec::new();
    for _ in 0..8 {
        let lc = lifecycle.clone();
        handles.push(tokio::spawn(async move {
            lc.end_for_disable(STREAMER_ID, "Test").await
        }));
    }
    for h in handles {
        // All must succeed (no errors), regardless of which path won.
        h.await.unwrap().unwrap();
    }

    // Audit log must contain exactly one session_ended row.
    let ended_count = read_events(&pool, &sid)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "session_ended")
        .count();
    assert_eq!(ended_count, 1, "concurrent calls must collapse to one row");

    // The single row's cause is user_disabled.
    let payload = latest_session_ended_payload(&pool, &sid).await.unwrap();
    assert_eq!(
        session_ended_cause(&payload),
        &TerminalCauseDto::UserDisabled
    );
}

/// broadcast ordering: by the time a subscriber receives `Ended`,
/// the in-memory snapshot already reflects the Ended state and the DB
/// `end_time` is committed. Ordering: commit → in-memory → broadcast.
#[tokio::test]
async fn end_for_disable_broadcast_after_commit_and_memory_update() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started

    let sid = started.session_id().to_string();
    let lc = lifecycle.clone();
    let pool_clone = pool.clone();
    let sid_clone = sid.clone();
    let observer = tokio::spawn(async move {
        // Wait for the Ended broadcast.
        loop {
            match rx.recv().await.unwrap() {
                SessionTransition::Ended { .. } => break,
                _ => continue,
            }
        }
        // At this point both invariants must hold:
        // 1. In-memory snapshot is Ended.
        let snap = lc.session_snapshot(&sid_clone).expect("session in memory");
        assert!(
            snap.is_ended(),
            "in-memory state must be Ended before subscriber sees broadcast"
        );
        // 2. DB end_time is committed.
        let end_time = db_session_end_time(&pool_clone, &sid_clone).await;
        assert!(
            end_time.is_some(),
            "DB end_time must be committed before subscriber sees broadcast"
        );
    });

    lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();

    observer.await.unwrap();
}

#[tokio::test]
async fn required_transition_survives_lagged_observer() {
    let pool = setup_pool().await;
    let (required_tx, mut required_rx) = crate::session::session_transition_channel();
    let lifecycle = SessionLifecycle::new(
        Arc::new(SessionLifecycleRepository::new(pool)),
        Arc::new(OfflineClassifier::new()),
        4,
    )
    .with_required_transition_sender(required_tx);
    let mut observer = lifecycle.subscribe();

    for index in 0..8 {
        lifecycle.publish_transition(SessionTransition::Resumed {
            session_id: format!("session-{index}"),
            streamer_id: STREAMER_ID.to_string(),
            resumed_at: Utc::now(),
            hysteresis_duration: chrono::Duration::seconds(1),
        });
    }

    let expected_session_id = "required-session";
    lifecycle.publish_transition(SessionTransition::Ended {
        session_id: expected_session_id.to_string(),
        streamer_id: STREAMER_ID.to_string(),
        streamer_name: "Test".to_string(),
        ended_at: Utc::now(),
        cause: TerminalCause::Completed,
        via_hysteresis: false,
    });

    let mut received_terminal = false;
    while let Ok(Ok(Some(transition))) =
        tokio::time::timeout(std::time::Duration::from_secs(1), required_rx.recv()).await
    {
        if matches!(
            transition,
            SessionTransition::Ended { ref session_id, .. } if session_id == expected_session_id
        ) {
            received_terminal = true;
            break;
        }
    }

    assert!(received_terminal, "required transition was not delivered");
    assert!(matches!(
        observer.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
    ));
}

async fn ended_lifecycle_with_long_retention() -> (Arc<SessionLifecycle>, String) {
    let pool = setup_pool().await;
    let lifecycle = Arc::new(
        SessionLifecycle::new(
            Arc::new(SessionLifecycleRepository::new(pool)),
            Arc::new(OfflineClassifier::new()),
            16,
        )
        .with_ended_retention(Duration::from_secs(3600)),
    );
    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let session_id = started.session_id().to_string();
    lifecycle
        .on_download_terminal(&make_terminal_completed_hls_endlist(&session_id))
        .await
        .unwrap();
    (lifecycle, session_id)
}

#[tokio::test]
async fn shutdown_joins_evictions_without_waiting_for_retention() {
    let (lifecycle, session_id) = ended_lifecycle_with_long_retention().await;
    assert!(Arc::strong_count(&lifecycle.sessions) > 1);
    let mut events = lifecycle.subscribe();
    let report = tokio::time::timeout(
        Duration::from_secs(2),
        lifecycle.shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1)),
    )
    .await
    .expect("eviction shutdown must not wait an hour");
    assert!(report.failures.is_empty());
    assert!(report.overruns.is_empty());
    assert_eq!(report.forced_timer_count, 0);
    assert_eq!(
        Arc::strong_count(&lifecycle.sessions),
        1,
        "all eviction futures released the map"
    );
    assert!(lifecycle.session_snapshot(&session_id).unwrap().is_ended());
    assert!(events.try_recv().is_err());
    lifecycle.schedule_ended_eviction(STREAMER_ID, &session_id);
    assert_eq!(
        Arc::strong_count(&lifecycle.sessions),
        1,
        "shutdown fences late eviction admission"
    );
}

#[tokio::test]
async fn forced_shutdown_joins_eviction_futures() {
    let (lifecycle, session_id) = ended_lifecycle_with_long_retention().await;
    assert!(Arc::strong_count(&lifecycle.sessions) > 1);
    let aborted = tokio::time::timeout(
        Duration::from_secs(2),
        lifecycle.abort_timers(tokio::time::Instant::now() + Duration::from_secs(1)),
    )
    .await
    .expect("aborted evictions must settle promptly");
    assert_eq!(aborted, 0, "evictions are not hysteresis timers");
    assert_eq!(Arc::strong_count(&lifecycle.sessions), 1);
    assert!(lifecycle.session_snapshot(&session_id).unwrap().is_ended());
}

#[tokio::test]
async fn owned_eviction_preserves_the_live_successor() {
    let (lifecycle, old_id) = ended_lifecycle_with_long_retention().await;
    let next = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let next_id = next.session_id().to_string();
    assert_ne!(old_id, next_id);
    // Pause only after SQLite replies; worker-thread I/O must not race virtual deadlines.
    tokio::time::pause();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(3601)).await;
    tokio::time::resume();
    tokio::time::timeout(Duration::from_secs(1), async {
        while lifecycle.session_snapshot(&old_id).is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("retention must evict the old session");
    assert!(lifecycle.is_session_active(&next_id));
    assert_eq!(
        lifecycle.current_session_id_for_streamer(STREAMER_ID),
        Some(next_id)
    );
    lifecycle
        .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
        .await;
}
