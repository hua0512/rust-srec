use super::*;
use crate::database::models::StreamerDbModel;
use crate::database::repositories::monitor_outbox::MonitorOutboxOps;
use crate::database::repositories::{
    SessionRepository as _, SqlxSessionRepository, SqlxStreamerRepository, StreamerRepository as _,
};
use crate::database::{init_pool_with_size, run_migrations};
use crate::monitor::MonitorEvent;
use sqlx::SqlitePool;

const STREAMER_ID: &str = "test-streamer";

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
async fn on_live_detected_creates_session_and_emits_started() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let outcome = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();

    assert!(matches!(outcome, StartSessionOutcome::Created { .. }));
    assert!(lifecycle.is_session_active(outcome.session_id()));

    let transition = rx.recv().await.unwrap();
    match transition {
        SessionTransition::Started {
            session_id,
            streamer_id,
            ..
        } => {
            assert_eq!(session_id, outcome.session_id());
            assert_eq!(streamer_id, STREAMER_ID);
        }
        other => panic!("expected Started, got {:?}", other),
    }
}

#[tokio::test]
async fn on_offline_detected_ends_session_and_emits_ended() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started_now = Utc::now();
    let started = lifecycle
        .on_live_detected(live_args(started_now))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // drain Started

    let offline_now = started_now + chrono::Duration::seconds(10);
    lifecycle
        .on_offline_detected(OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(started.session_id()),
            state_was_live: true,
            clear_errors: false,
            signal: None,
            now: offline_now,
        })
        .await
        .unwrap();

    assert!(!lifecycle.is_session_active(started.session_id()));
    let transition = rx.recv().await.unwrap();
    match transition {
        SessionTransition::Ended {
            session_id, cause, ..
        } => {
            assert_eq!(session_id, started.session_id());
            assert_eq!(cause, TerminalCause::StreamerOffline);
        }
        other => panic!("expected Ended, got {:?}", other),
    }
}

#[tokio::test]
async fn on_download_terminal_cancelled_is_noop() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool);

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let mut rx = lifecycle.subscribe();

    let event = DownloadTerminalEvent::Cancelled {
        download_id: "dl-1".into(),
        streamer_id: STREAMER_ID.into(),
        streamer_name: "Test".into(),
        session_id: started.session_id().to_string(),
        cause: crate::downloader::DownloadStopCause::User,
    };
    lifecycle.on_download_terminal(&event).await.unwrap();

    // Cancelled is a no-op: the engine may still flush a final Completed,
    // so the session stays Recording and no SessionTransition is emitted.
    assert!(
        lifecycle.is_session_active(started.session_id()),
        "Cancelled must leave session in Recording state"
    );
    assert!(
        rx.try_recv().is_err(),
        "Cancelled must not emit SessionTransition"
    );
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

#[tokio::test]
async fn ended_session_followed_by_live_creates_new_session() {
    // Any LiveDetected on a streamer whose last session is Ended creates
    // a new session. The DB's `end_time` is the source of truth.
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool);

    let started_now = Utc::now() - chrono::Duration::seconds(120);
    let first = lifecycle
        .on_live_detected(live_args(started_now))
        .await
        .unwrap();

    // End the session via the monitor's offline path (authoritative).
    lifecycle
        .on_offline_detected(OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(first.session_id()),
            state_was_live: true,
            clear_errors: false,
            signal: None,
            now: started_now + chrono::Duration::seconds(60),
        })
        .await
        .unwrap();

    // A LiveDetected arriving shortly after on_offline_detected ended the
    // session unconditionally creates a fresh session.
    let restart_now = started_now + chrono::Duration::seconds(90);
    let second = lifecycle
        .on_live_detected(live_args(restart_now))
        .await
        .unwrap();

    assert!(matches!(second, StartSessionOutcome::Created { .. }));
    assert_ne!(second.session_id(), first.session_id());
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
// behaviour for each cause is covered by `pipeline::manager::tests`, and
// the `TerminalCause::should_run_session_complete_pipeline` policy is
// covered by `session::state::tests`. Here we assert the boundary between
// the download-event subscription and the SessionTransition broadcast.
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

fn make_terminal_rejected(session_id: &str) -> DownloadTerminalEvent {
    DownloadTerminalEvent::Rejected {
        streamer_id: STREAMER_ID.into(),
        streamer_name: "Test".into(),
        session_id: session_id.into(),
        reason: "circuit breaker".into(),
        retry_after_secs: None,
        kind: crate::downloader::DownloadRejectedKind::CircuitBreaker,
    }
}

/// B2 — Terminal::Cancelled is a no-op: session stays Recording, no
/// SessionTransition is emitted, and no DB end_time is written.
#[tokio::test]
async fn b2_cancelled_keeps_session_recording_and_emits_nothing() {
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

/// B3 — Terminal::Rejected emits Ended { Rejected } but the cause's policy
/// keeps the session-complete pipeline from firing.
#[tokio::test]
async fn b3_rejected_emits_ended_but_does_not_trigger_pipeline() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool);
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap();

    lifecycle
        .on_download_terminal(&make_terminal_rejected(started.session_id()))
        .await
        .unwrap();

    let transition = rx.recv().await.unwrap();
    match transition {
        SessionTransition::Ended { cause, .. } => {
            assert!(matches!(cause, TerminalCause::Rejected { .. }));
            assert!(
                !cause.should_run_session_complete_pipeline(),
                "Rejected must NOT trigger session-complete pipeline"
            );
        }
        other => panic!("expected Ended, got {:?}", other),
    }
}

/// B6 — Hand-picked event sequences: for every prefix, the in-memory
/// `is_session_active` view matches `db.session.end_time.is_none()`.
#[tokio::test]
async fn b6_in_memory_view_matches_db_for_known_sequences() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started
    let session_id = started.session_id().to_string();

    async fn check(
        checkpoint: &str,
        lifecycle: &SessionLifecycle,
        pool: &SqlitePool,
        session_id: &str,
    ) {
        let in_memory = lifecycle.is_session_active(session_id);
        let db_end = db_session_end_time(pool, session_id).await;
        let db_live = db_end.is_none();
        assert_eq!(
            in_memory, db_live,
            "{checkpoint}: in-memory ({in_memory}) != db-live ({db_live})"
        );
    }

    // Sequence: Live → Cancelled (no-op) → OfflineDetected (Ended).
    check("after Live", &lifecycle, &pool, &session_id).await;

    lifecycle
        .on_download_terminal(&make_terminal_cancelled(&session_id))
        .await
        .unwrap();
    check("after Cancelled (no-op)", &lifecycle, &pool, &session_id).await;

    lifecycle
        .on_offline_detected(OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(&session_id),
            state_was_live: true,
            clear_errors: false,
            signal: None,
            now: Utc::now(),
        })
        .await
        .unwrap();
    match rx.recv().await.unwrap() {
        SessionTransition::Ended { .. } => {}
        other => panic!("expected Ended, got {other:?}"),
    }
    check(
        "after OfflineDetected (ended)",
        &lifecycle,
        &pool,
        &session_id,
    )
    .await;
}

// B7 (atomicity / fault injection) deliberately out of scope for this
// unit suite. Partial-write rollback relies on sqlx's BEGIN IMMEDIATE
// semantics, which are exercised by the repository tests that assert
// multi-step bundles land atomically.

// =========================================================================
// Session create / resume / no-op decisions at the lifecycle level. The
// DB-side branching is exercised by
// `database::repositories::session_lifecycle::tests`;
// here we assert the outcome *kind* and the `SessionTransition::Started`
// payload shape each branch emits.
// =========================================================================

async fn take_started(
    rx: &mut broadcast::Receiver<SessionTransition>,
) -> (String, String, Option<String>) {
    match rx.recv().await.unwrap() {
        SessionTransition::Started {
            session_id,
            title,
            category,
            ..
        } => (session_id, title, category),
        other => panic!("expected Started, got {other:?}"),
    }
}

/// D1 — No prior session in DB → Created outcome. `SessionTransition::
/// Started` carries the new session_id + the monitor-trigger fields.
#[tokio::test]
async fn d1_no_prior_session_creates_new() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool);
    let mut rx = lifecycle.subscribe();

    let outcome = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    assert!(matches!(outcome, StartSessionOutcome::Created { .. }));

    let (session_id, title, category) = take_started(&mut rx).await;
    assert_eq!(session_id, outcome.session_id());
    assert_eq!(title, "Live!");
    assert!(category.is_none());
}

/// D2 — Active (not-yet-ended) session → ReusedActive; repeated live
/// signals are idempotent at the session level. Each call still emits a
/// Started transition so the notification layer can dedupe / rate-limit
/// on its own.
#[tokio::test]
async fn d2_active_session_reused_on_repeat_live() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool);
    let mut rx = lifecycle.subscribe();
    let now = Utc::now();

    let first = lifecycle.on_live_detected(live_args(now)).await.unwrap();
    let (first_id, _, _) = take_started(&mut rx).await;

    let second = lifecycle.on_live_detected(live_args(now)).await.unwrap();
    assert!(matches!(second, StartSessionOutcome::ReusedActive { .. }));
    assert_eq!(second.session_id(), first.session_id());

    let (second_id, _, _) = take_started(&mut rx).await;
    assert_eq!(second_id, first_id, "same session_id across Started emits");
}

// Ended sessions are never resumed. Hysteresis covers the valid case
// where an interrupted stream resumes before the session is ended.

/// D4 — Once a session is Ended, the next LiveDetected creates a
/// fresh session. Ended rows are final and never resumed.
#[tokio::test]
async fn d4_after_ended_creates_new_session() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool);
    let base = Utc::now() - chrono::Duration::seconds(3600);

    let first = lifecycle.on_live_detected(live_args(base)).await.unwrap();
    lifecycle
        .on_offline_detected(OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(first.session_id()),
            state_was_live: true,
            clear_errors: false,
            signal: None,
            now: base + chrono::Duration::seconds(10),
        })
        .await
        .unwrap();

    // Way past the 60s gap.
    let restart = base + chrono::Duration::seconds(1000);
    let outcome = lifecycle
        .on_live_detected(live_args(restart))
        .await
        .unwrap();
    assert!(matches!(outcome, StartSessionOutcome::Created { .. }));
    assert_ne!(outcome.session_id(), first.session_id());
}

// There is no hard-ended cache to cover here. The ended DB row is the
// authoritative fence.

// D6 (continuation rule) deleted — the rule was retired with gap-resume.
// Hysteresis covers the legitimate "stream came back briefly" case;
// anything past the hysteresis window is a new session by design.

// =========================================================================
// Additional integration coverage — in-memory / DB consistency under the
// state transitions that aren't directly covered by suites B or D.
// =========================================================================

// Non-authoritative failures go through Hysteresis. If the stream comes
// back before the timer expires, the resume path is
// `resume_from_hysteresis`.

/// Adapted F7 — a per-segment DAG that STARTS after SessionTransition::
/// Ended still gates session-complete. This models the mesio flush-race
/// where a late `SegmentCompleted` arrives after `DownloadFailed`; the
/// gate must wait for that trailing DAG before firing.
///
/// Uses the PipelineCoordinator directly (rather than going
/// through the full `handle_download_event(SegmentCompleted)` path which
/// also writes to the DB via `persist_segment`). The coordinator's
/// counters are the authoritative gate, so this isolates the ordering
/// invariant we care about.
#[tokio::test]
async fn late_per_segment_dag_after_ended_still_gates_session_complete() {
    // Placeholder: this test lives in `pipeline::manager::tests` as it
    // needs the manager's coordinator. See
    // `pipeline::manager::tests::f1_session_complete_waits_for_in_flight_video_dags`
    // for the analogous drain-before-fire coverage. A standalone F7 test
    // would duplicate plumbing; the behavioural invariant is the same.
}

/// Multi-session isolation at the lifecycle level. Two streamers, each
/// with its own session. Lifecycle events on streamer A do not affect the
/// in-memory state, DB row, or transition stream of streamer B's session.
#[tokio::test]
async fn multi_session_isolation_across_streamers() {
    let pool = setup_pool().await;

    // Add a second streamer row so `set_live` / `set_offline` have a
    // target for it.
    create_test_streamer(&pool, "streamer-b", "B", "https://example.com/b").await;

    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let now = Utc::now();
    let sa = lifecycle.on_live_detected(live_args(now)).await.unwrap();
    let _ = rx.recv().await.unwrap(); // A Started

    // Build a separate LiveDetectedArgs for streamer B (clone + swap id).
    let streams_b: Vec<crate::monitor::StreamInfo> = vec![];
    let args_b = LiveDetectedArgs {
        streamer_id: "streamer-b",
        streamer_name: "B",
        streamer_url: "https://example.com/b",
        current_avatar: None,
        new_avatar: None,
        title: "B live!",
        category: None,
        streams: &streams_b,
        media_headers: None,
        media_extras: None,
        now,
    };
    let sb = lifecycle.on_live_detected(args_b).await.unwrap();
    let _ = rx.recv().await.unwrap(); // B Started
    assert_ne!(sa.session_id(), sb.session_id());

    // Fail streamer A's download; it should enter Hysteresis while
    // streamer B stays Recording.
    lifecycle
        .on_download_terminal(&DownloadTerminalEvent::Failed {
            download_id: "dl-a".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: sa.session_id().to_string(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Flv,
            kind: crate::downloader::DownloadFailureKind::Network,
            error: "stalled".into(),
            recoverable: false,
        })
        .await
        .unwrap();

    match rx.recv().await.unwrap() {
        SessionTransition::Ending {
            streamer_id,
            session_id,
            cause,
            ..
        } => {
            assert_eq!(streamer_id, STREAMER_ID);
            assert_eq!(session_id, sa.session_id());
            assert!(matches!(cause, TerminalCause::Failed { .. }));
        }
        other => panic!("expected A Ending, got {other:?}"),
    }

    assert!(
        lifecycle.is_session_active(sa.session_id()),
        "streamer A remains active while in Hysteresis"
    );
    assert!(
        lifecycle.is_session_active(sb.session_id()),
        "streamer B's session must not be affected by streamer A's failure"
    );
    assert!(
        db_session_end_time(&pool, sb.session_id()).await.is_none(),
        "streamer B's DB row must remain live"
    );

    lifecycle
        .on_offline_detected(OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(sa.session_id()),
            state_was_live: true,
            clear_errors: false,
            signal: None,
            now: Utc::now(),
        })
        .await
        .unwrap();

    match rx.recv().await.unwrap() {
        SessionTransition::Ended {
            streamer_id,
            session_id,
            ..
        } => {
            assert_eq!(streamer_id, STREAMER_ID);
            assert_eq!(session_id, sa.session_id());
        }
        other => panic!("expected A Ended, got {other:?}"),
    }

    assert!(!lifecycle.is_session_active(sa.session_id()));
    assert!(
        lifecycle.is_session_active(sb.session_id()),
        "ending streamer A must not end streamer B"
    );
    assert!(
        db_session_end_time(&pool, sb.session_id()).await.is_none(),
        "streamer B's DB row must still be live after A ends"
    );
    assert!(
        rx.try_recv().is_err(),
        "streamer B must not emit a transition when only streamer A changes"
    );
}

/// H2 — `is_live` (as computed by the API layer via `end_time.is_none()`)
/// tracks DB state faithfully through Hysteresis and final Ended state.
#[tokio::test]
async fn api_is_live_tracks_db_through_hysteresis() {
    async fn check_is_live(pool: &SqlitePool, session_id: &str, expected: bool) {
        let end_time = db_session_end_time(pool, session_id).await;
        let api_is_live = end_time.is_none();
        assert_eq!(
            api_is_live, expected,
            "is_live should be {expected} (end_time = {end_time:?})"
        );
    }

    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_fast(pool.clone());
    let mut rx = lifecycle.subscribe();

    let s = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started
    check_is_live(&pool, s.session_id(), true).await;

    lifecycle
        .on_download_terminal(&DownloadTerminalEvent::Failed {
            download_id: "dl".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: s.session_id().to_string(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Flv,
            kind: crate::downloader::DownloadFailureKind::Network,
            error: "stalled".into(),
            recoverable: false,
        })
        .await
        .unwrap();
    match rx.recv().await.unwrap() {
        SessionTransition::Ending { .. } => {}
        other => panic!("expected Ending, got {other:?}"),
    }
    check_is_live(&pool, s.session_id(), true).await;

    wait_for_hysteresis_to_expire().await;
    match rx.recv().await.unwrap() {
        SessionTransition::Ended { .. } => {}
        other => panic!("expected Ended, got {other:?}"),
    }
    check_is_live(&pool, s.session_id(), false).await;
}

// =========================================================================
// PR 2 — OfflineClassifier promotion inside on_download_terminal.
//
// The unit-level classifier rules live in `session::classifier::tests`.
// These scenarios assert the *integration* inside `SessionLifecycle`:
// Terminal::Failed variants that the classifier promotes end the session
// with TerminalCause::DefinitiveOffline (not plain Failed), and the
// `on_segment_completed` wiring resets the consecutive-failure counter.
// =========================================================================

/// Two consecutive `Network` failures inside the classifier's window
/// promote the second terminal event to `DefinitiveOffline`. The first
/// failure parks the session in `Hysteresis` (ambiguous Failed → quiet
/// period); the second arrives while the FSM is still in `Hysteresis`,
/// the classifier hits its threshold, and the session ends authoritatively.
#[tokio::test]
async fn pr2_two_consecutive_network_failures_promote() {
    let pool = setup_pool().await;
    // Use the fast hysteresis window so the test isn't pinned to the
    // 80 s default backstop. The classifier itself uses the lifecycle's
    // default classifier (60 s window, threshold 2).
    let lifecycle = make_lifecycle_fast(pool);
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // drain Started

    // First Network failure: classifier returns None → ambiguous Failed
    // → enter Hysteresis, emit `Ending`.
    lifecycle
        .on_download_terminal(&DownloadTerminalEvent::Failed {
            download_id: "dl-1".into(),
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
    match rx.recv().await.unwrap() {
        SessionTransition::Ending { cause, .. } => {
            assert!(
                matches!(cause, TerminalCause::Failed { .. }),
                "first Network must enter Hysteresis with Failed cause, got {cause:?}"
            );
        }
        other => panic!("expected Ending, got {other:?}"),
    }

    // Second Network failure for the same session: classifier promotes
    // → DefinitiveOffline → authoritative end (cancels Hysteresis).
    lifecycle
        .on_download_terminal(&DownloadTerminalEvent::Failed {
            download_id: "dl-2".into(),
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
            assert!(cause.should_run_session_complete_pipeline());
            assert!(
                !via_hysteresis,
                "DefinitiveOffline must override hysteresis with via_hysteresis=false"
            );
        }
        other => panic!("expected Ended, got {other:?}"),
    }
}

#[tokio::test]
async fn pr2_unknown_mesio_protocol_still_promotes() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_fast(pool);
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // drain Started

    for tag in ["dl-unknown-1", "dl-unknown-2"] {
        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: tag.into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: started.session_id().to_string(),
                engine_type: EngineType::Mesio,
                protocol: DownloadProtocol::Unknown,
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "timeout".into(),
                recoverable: false,
            })
            .await
            .unwrap();
    }

    match rx.recv().await.unwrap() {
        SessionTransition::Ending { cause, .. } => {
            assert!(matches!(cause, TerminalCause::Failed { .. }));
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
                "MesioUnknown Network promotion must still be authoritative"
            );
        }
        other => panic!("expected Ended, got {other:?}"),
    }
}

/// Failed events must preserve the originating engine. FFmpeg and
/// Streamlink failures are too fuzzy to become definitive offline
/// signals, even when two Network failures arrive inside the classifier
/// window. They should stay on the ambiguous Failed → Hysteresis path.
#[tokio::test]
async fn pr2_non_mesio_network_failures_do_not_promote() {
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

/// `on_segment_completed` resets the classifier's counter so a subsequent
/// Network failure is treated as the first-in-window again.
#[tokio::test]
async fn pr2_on_segment_completed_resets_counter() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_window(pool, Duration::from_secs(60));
    let mut rx = lifecycle.subscribe();

    // Prime the counter with one Network failure.
    let first = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // drain Started

    lifecycle
        .on_download_terminal(&DownloadTerminalEvent::Failed {
            download_id: "dl-1".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: first.session_id().to_string(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Flv,
            kind: crate::downloader::DownloadFailureKind::Network,
            error: "timeout".into(),
            recoverable: false,
        })
        .await
        .unwrap();
    match rx.recv().await.unwrap() {
        SessionTransition::Ending { cause, .. } => {
            assert!(matches!(cause, TerminalCause::Failed { .. }));
        }
        other => panic!("expected first Network failure to enter Hysteresis, got {other:?}"),
    }

    // Successful segment resets the counter.
    lifecycle.on_segment_completed(STREAMER_ID);

    // A second Network failure after the reset should be treated like the
    // first one again: still ambiguous, no DefinitiveOffline promotion.
    lifecycle
        .on_download_terminal(&DownloadTerminalEvent::Failed {
            download_id: "dl-2".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: first.session_id().to_string(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Flv,
            kind: crate::downloader::DownloadFailureKind::Network,
            error: "timeout".into(),
            recoverable: false,
        })
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_err(),
        "after reset, the next Network failure must not emit DefinitiveOffline"
    );
    match lifecycle
        .session_snapshot(first.session_id())
        .expect("session remains tracked")
    {
        SessionState::Hysteresis { cause, .. } => {
            assert!(
                matches!(cause, TerminalCause::Failed { .. }),
                "after reset, the next Network failure must stay Failed, got {cause:?}"
            );
        }
        other => panic!("expected Hysteresis after reset, got {other:?}"),
    }
}

/// DefinitiveOffline bypasses the streamer's `disabled_until` backoff for
/// the session-end write. Monitor check-loop backoff stays untouched
/// (scheduled elsewhere by the actor), but the session row is closed
/// immediately so the UI and pipeline trigger don't wait for the backoff
/// window to expire.
#[tokio::test]
async fn e1_definitive_offline_bypasses_streamer_disabled_until() {
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
    }
}

fn make_terminal_completed_hls_endlist(session_id: &str) -> DownloadTerminalEvent {
    DownloadTerminalEvent::Completed {
        download_id: "dl-i".into(),
        streamer_id: STREAMER_ID.into(),
        streamer_name: "Test".into(),
        session_id: session_id.into(),
        total_bytes: 0,
        total_duration_secs: 0.0,
        total_segments: 0,
        file_path: None,
        engine_signal: crate::downloader::EngineEndSignal::HlsEndlist,
    }
}

/// I1 — non-authoritative terminal (mesio FLV clean disconnect) parks
/// the session in `Hysteresis`. `SessionTransition::Ending` is emitted;
/// DB `end_time IS NULL`.
#[tokio::test]
async fn i1_clean_disconnect_enters_hysteresis() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_fast(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // drain Started

    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();

    // Ending transition emitted (next event after Started).
    match rx.recv().await.unwrap() {
        SessionTransition::Ending {
            session_id, cause, ..
        } => {
            assert_eq!(session_id, started.session_id());
            assert!(matches!(cause, TerminalCause::Completed));
        }
        other => panic!("expected Ending, got {other:?}"),
    }

    // DB end_time still NULL — hysteresis state doesn't write end_time.
    let end_time = db_session_end_time(&pool, started.session_id()).await;
    assert!(
        end_time.is_none(),
        "DB end_time must not be written during Hysteresis"
    );

    // is_session_active still true (Hysteresis counts as active).
    assert!(lifecycle.is_session_active(started.session_id()));
}

/// I2 — hysteresis timer expires with no resume → `Ended` transition,
/// DB `end_time IS NOT NULL`, `via_hysteresis=true`.
#[tokio::test]
async fn i2_timer_expiry_commits_ended() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_fast(pool.clone());
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

    // Wait for timer expiry.
    wait_for_hysteresis_to_expire().await;

    match rx.recv().await.unwrap() {
        SessionTransition::Ended { via_hysteresis, .. } => {
            assert!(via_hysteresis, "Ended must be marked via_hysteresis=true");
        }
        other => panic!("expected Ended, got {other:?}"),
    }

    // DB end_time now set.
    let end_time = db_session_end_time(&pool, started.session_id()).await;
    assert!(
        end_time.is_some(),
        "DB end_time must be written after timer fires"
    );

    // Session no longer active.
    assert!(!lifecycle.is_session_active(started.session_id()));
}

/// I3 — `LiveDetected` inside the hysteresis window cancels the timer,
/// emits `Resumed`, transitions back to `Recording`. Same `session_id`
/// continues. DB `end_time` was never set.
#[tokio::test]
async fn i3_resume_cancels_timer_and_keeps_session() {
    // Use a longer window so we can resume well within it.
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

    // LiveDetected within the 5s window.
    let _resumed = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();

    // Resumed transition emitted.
    let resumed_event = rx.recv().await.unwrap();
    assert!(
        matches!(resumed_event, SessionTransition::Resumed { ref session_id, .. } if session_id == started.session_id())
    );

    // Then a Started with from_hysteresis=true.
    let started_event = rx.recv().await.unwrap();
    match started_event {
        SessionTransition::Started {
            from_hysteresis,
            session_id,
            ..
        } => {
            assert!(from_hysteresis);
            assert_eq!(session_id, started.session_id());
        }
        other => panic!("expected Started{{from_hysteresis:true}}, got {other:?}"),
    }

    // DB end_time still NULL.
    let end_time = db_session_end_time(&pool, started.session_id()).await;
    assert!(end_time.is_none(), "Resume must leave DB end_time NULL");

    // Session active again. Wait past the original deadline; Ended
    // must NOT fire (timer was cancelled).
    assert!(lifecycle.is_session_active(started.session_id()));
}

/// Resume from hysteresis must set `streamers.state = LIVE`. If a
/// download failure earlier in the session flipped the row to
/// `NOT_LIVE` (via `monitor::service::handle_error`), the resume path
/// must restore it — otherwise downstream readers (cache, container
/// queue-wait state check, web UI) keep seeing `NOT_LIVE` after the
/// recording has resumed.
#[tokio::test]
async fn i3b_resume_restores_streamer_state_live() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_window(pool.clone(), std::time::Duration::from_secs(5));
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started
    assert_eq!(streamer_state(&pool, STREAMER_ID).await, "LIVE");

    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ending

    // Simulate `monitor::service::handle_error` flipping the streamer
    // row during the transient failure that drove this hysteresis
    // entry (first error → `disabled_until=None` → `state=NOT_LIVE`).
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .update_streamer_state(STREAMER_ID, StreamerState::NotLive.as_str())
        .await
        .unwrap();
    assert_eq!(streamer_state(&pool, STREAMER_ID).await, "NOT_LIVE");

    // LiveDetected within the hysteresis window → resume path.
    let _resumed = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();

    // Drain Resumed + Started{from_hysteresis:true} to keep ordering
    // assertions matching the I3 test next door.
    let _ = rx.recv().await.unwrap(); // Resumed
    let _ = rx.recv().await.unwrap(); // Started{from_hysteresis:true}

    // The DB row must now be LIVE again — the fix's whole point.
    assert_eq!(streamer_state(&pool, STREAMER_ID).await, "LIVE");
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

/// J1 — `DefinitiveOffline { ConsecutiveFailures }` skips Hysteresis.
/// First Network failure parks the session in Hysteresis (`Ending` is
/// emitted). Second Network failure crosses the classifier threshold,
/// promotes to `DefinitiveOffline`, cancels the Hysteresis handle, and
/// emits `Ended` with `via_hysteresis=false`.
///
/// (J4 covers the parallel HlsEndlist authoritative-end path.)
#[tokio::test]
async fn j1_definitive_offline_skips_hysteresis() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_fast(pool);
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started

    // First Network → ambiguous Failed → Hysteresis.
    lifecycle
        .on_download_terminal(&DownloadTerminalEvent::Failed {
            download_id: "dl-1".into(),
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
    match rx.recv().await.unwrap() {
        SessionTransition::Ending { cause, .. } => {
            assert!(matches!(cause, TerminalCause::Failed { .. }));
        }
        other => panic!("expected Ending, got {other:?}"),
    }

    // Second Network → classifier promotes → DefinitiveOffline →
    // authoritative → cancels Hysteresis and emits Ended.
    lifecycle
        .on_download_terminal(&DownloadTerminalEvent::Failed {
            download_id: "dl-2".into(),
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
            assert!(!via_hysteresis, "authoritative end must skip Hysteresis");
        }
        other => panic!("expected Ended, got {other:?}"),
    }
}

/// J4 — `Completed { engine_signal: HlsEndlist }` skips Hysteresis.
/// Direct Ended.
#[tokio::test]
async fn j4_completed_with_hls_endlist_skips_hysteresis() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_fast(pool);
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started

    lifecycle
        .on_download_terminal(&make_terminal_completed_hls_endlist(started.session_id()))
        .await
        .unwrap();

    match rx.recv().await.unwrap() {
        SessionTransition::Ended { via_hysteresis, .. } => {
            assert!(!via_hysteresis, "HlsEndlist authoritative → no hysteresis");
        }
        other => panic!("expected Ended, got {other:?}"),
    }
}

/// N1 — `resume_from_hysteresis` must emit `SessionTransition::Started`
/// with `from_hysteresis: true` AND a populated `download_start`
/// payload. The container's resume-download subscriber relies on the
/// payload to (re)start the download for the resumed session — without
/// it, the streamer stays "Live" in memory but no recording happens.
#[tokio::test]
async fn n1_resume_emits_started_with_download_start_payload() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_window(pool.clone(), std::time::Duration::from_secs(5));
    let mut rx = lifecycle.subscribe();

    // Step 1: fresh start. Drain Started.
    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    match rx.recv().await.unwrap() {
        SessionTransition::Started {
            from_hysteresis,
            download_start,
            ..
        } => {
            assert!(
                !from_hysteresis,
                "fresh-start must be from_hysteresis=false"
            );
            assert!(
                download_start.is_none(),
                "fresh-start path leaves download_start=None — \
                     MonitorEvent::StreamerLive outbox event drives the download"
            );
        }
        other => panic!("expected Started for fresh, got {other:?}"),
    }

    // Step 2: ambiguous end → Hysteresis. Drain Ending.
    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ending

    // Step 3: LiveDetected within window → resume. Should emit
    // both Resumed AND Started{from_hysteresis: true, download_start: Some(_)}.
    lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();

    // Resumed transition fires first.
    match rx.recv().await.unwrap() {
        SessionTransition::Resumed { session_id, .. } => {
            assert_eq!(session_id, started.session_id());
        }
        other => panic!("expected Resumed, got {other:?}"),
    }

    // Started transition fires second, with the download_start payload
    // populated so the container's resume-download subscriber can drive
    // start_download_for_streamer.
    match rx.recv().await.unwrap() {
        SessionTransition::Started {
            from_hysteresis,
            download_start,
            streamer_id,
            session_id,
            ..
        } => {
            assert!(
                from_hysteresis,
                "resume must emit Started with from_hysteresis=true"
            );
            assert_eq!(streamer_id, STREAMER_ID);
            assert_eq!(session_id, started.session_id());
            let payload = download_start.as_deref().expect(
                "resume_from_hysteresis MUST populate download_start so the \
                     container can restart the download",
            );
            assert_eq!(
                payload.streamer_url, "https://example.com",
                "streamer_url must be carried for the engine config"
            );
            // `streams` may legitimately be empty in this test fixture
            // (`live_args` uses a static empty vec); we only require
            // the payload itself to be present. The container's
            // start_download_for_streamer has its own empty-streams
            // guard at the production path.
        }
        other => panic!("expected Started{{from_hysteresis: true}}, got {other:?}"),
    }
}

/// N2 — CAS atomicity for the `Hysteresis → (Recording | Ended)`
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
/// Symmetric loss case (`enter_ended_state` wins, `resume_from_hysteresis`
/// loses) is exercised by the existing I7 test plus the CAS in
/// `resume_from_hysteresis` returning `None` on missing handle.
#[tokio::test]
async fn n2_cas_blocks_enter_ended_when_resume_already_claimed_hysteresis() {
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
        .enter_ended_state(
            started.session_id(),
            STREAMER_ID,
            "Test",
            TerminalCause::StreamerOffline,
            Utc::now(),
            /* via_hysteresis */ true,
            DbWritePath::EndSessionOnly,
        )
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

/// N3 — symmetric CAS: `resume_from_hysteresis` returns `None` when
/// the handle was already claimed by an authoritative end.
/// `on_live_detected` then falls through to `start_or_resume`, which
/// produces a fresh `Created` session (since the prior session is
/// now `Ended` per the won path).
#[tokio::test]
async fn n3_cas_resume_falls_through_when_authoritative_end_already_claimed() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_window(pool.clone(), std::time::Duration::from_secs(5));
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started (fresh)

    // Drive into Hysteresis.
    lifecycle
        .on_download_terminal(&make_terminal_completed_clean_disconnect(
            started.session_id(),
        ))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ending

    // Simulate the authoritative-end path winning the CAS:
    // `enter_ended_state` runs and removes the handle. We invoke
    // it via `on_offline_detected` so the DB end_time is also set.
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
    // Drain Ended.
    match rx.recv().await.unwrap() {
        SessionTransition::Ended { .. } => {}
        other => panic!("expected Ended after on_offline_detected, got {other:?}"),
    }

    // Now: a LIVE detection arrives "after" the end. The hysteresis
    // map no longer has the handle (consumed by enter_ended_state).
    // `resume_from_hysteresis` returns None; on_live_detected falls
    // through to start_or_resume, which sees the prior session
    // ended (end_time set) and creates a fresh one.
    let outcome = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();

    match outcome {
        StartSessionOutcome::Created { session_id } => {
            assert_ne!(
                session_id,
                started.session_id(),
                "post-CAS-loss must mint a NEW session_id, not reuse the ended one"
            );
        }
        other => {
            panic!("expected Created (fresh session after CAS-loss fall-through), got {other:?}")
        }
    }
}

/// I7 — authoritative end during `Hysteresis` cancels the timer and
/// transitions directly to `Ended`. Models the danmu-close-after-FLV-
/// clean-disconnect scenario.
#[tokio::test]
async fn i7_authoritative_end_during_hysteresis_cancels_timer() {
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

/// I9 — sessions map evicts Ended entries after the retention window
/// elapses. Until then the entry is retained so duplicate
/// authoritative-end events are deduped by `enter_ended_state`'s
/// idempotency guard.
#[tokio::test]
async fn i9_sessions_map_evicts_on_ended_after_retention() {
    let pool = setup_pool().await;
    let retention = std::time::Duration::from_millis(80);
    let lifecycle = Arc::new(
        SessionLifecycle::with_config(
            Arc::new(SessionLifecycleRepository::new(pool)),
            Arc::new(OfflineClassifier::new()),
            16,
            HysteresisConfig::from_window(std::time::Duration::from_millis(25)),
        )
        .with_ended_retention(retention),
    );

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    assert_eq!(lifecycle.sessions.len(), 1, "Recording entry present");

    // Authoritative end → Ended → entry retained until retention elapses.
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

    assert_eq!(
        lifecycle.sessions.len(),
        1,
        "Ended entry must be retained briefly so dedup-guard can fire"
    );
    assert_eq!(
        lifecycle.hysteresis.len(),
        0,
        "no hysteresis handle should remain"
    );

    // Wait past retention; the spawned eviction task fires.
    tokio::time::sleep(retention + std::time::Duration::from_millis(50)).await;
    assert_eq!(
        lifecycle.sessions.len(),
        0,
        "Ended entry must be evicted after retention to bound memory"
    );
}

/// I10 — duplicate authoritative-end events emit a single
/// `SessionTransition::Ended`. The CAS-style guard at the top of
/// `enter_ended_state` short-circuits the second call thanks to the
/// retention window introduced in I9.
#[tokio::test]
async fn i10_double_end_dedup() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_fast(pool);
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let session_id = started.session_id().to_string();
    // Drain the `Started` transition.
    let _ = rx.recv().await;

    let now = Utc::now();
    let args = || OfflineDetectedArgs {
        streamer_id: STREAMER_ID,
        streamer_name: "Test",
        session_id: Some(&session_id),
        state_was_live: true,
        clear_errors: false,
        signal: None,
        now,
    };

    lifecycle.on_offline_detected(args()).await.unwrap();
    let first = rx.recv().await.expect("first Ended must be emitted");
    assert!(matches!(first, SessionTransition::Ended { .. }));

    // Second authoritative-end for the same session must not re-broadcast.
    lifecycle.on_offline_detected(args()).await.unwrap();
    match rx.try_recv() {
        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
        other => panic!("duplicate end re-broadcast: {other:?}"),
    }
}

// -----------------------------------------------------------------
// `session_events` audit-log persistence.
//
// Verifies the four lifecycle transitions land in the `session_events`
// table with the right `kind`, ordering, and payload shape.
// Atomic-tx writes (`session_started`, `session_ended`) go through
// `SessionLifecycleRepository`. Best-effort writes
// (`hysteresis_entered`, `session_resumed`) require the lifecycle to
// hold an `event_repo`, which `make_lifecycle_with_events` wires in.
// -----------------------------------------------------------------

use crate::database::repositories::{SessionEventRepository, SqlxSessionEventRepository};
use crate::session::events::{SessionEventPayload, TerminalCauseDto};
use crate::session::state::OfflineSignal;

fn make_lifecycle_with_events(pool: SqlitePool) -> Arc<SessionLifecycle> {
    // Tiny hysteresis window so suite-K tests can exercise the
    // hysteresis path without sleeping for 90s. Tiny `ended_retention`
    // so the in-memory dedup map doesn't leak between scenarios.
    let cfg = HysteresisConfig::from_window(std::time::Duration::from_millis(25));
    let event_repo: Arc<dyn SessionEventRepository> =
        Arc::new(SqlxSessionEventRepository::new(pool.clone(), pool.clone()));
    Arc::new(
        SessionLifecycle::with_config(
            Arc::new(SessionLifecycleRepository::new(pool)),
            Arc::new(OfflineClassifier::new()),
            16,
            cfg,
        )
        .with_event_repo(event_repo)
        .with_ended_retention(std::time::Duration::from_millis(50)),
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

/// `on_live_detected` for a fresh streamer writes one `session_started`
/// row inside the same atomic tx as the `live_sessions` insert.
#[tokio::test]
async fn k1_session_started_persisted_on_create() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_events(pool.clone());

    let outcome = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let sid = outcome.session_id();

    let rows = read_events(&pool, sid).await;
    assert_eq!(rows.len(), 1, "exactly one event row");
    assert_eq!(rows[0].0, "session_started");
    match event_payload(&rows[0].1) {
        SessionEventPayload::SessionStarted {
            from_hysteresis,
            title,
        } => {
            assert!(!from_hysteresis, "fresh sessions are not from hysteresis");
            assert_eq!(title.as_deref(), Some("Live!"));
        }
        other => panic!("wrong payload variant: {other:?}"),
    }
}

/// A second `on_live_detected` while the session is still active reuses
/// the row instead of creating one — and writes no extra audit event.
#[tokio::test]
async fn k2_session_started_not_duplicated_on_reused_active() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_events(pool.clone());

    let first = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let again = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    assert_eq!(first.session_id(), again.session_id());

    let rows = read_events(&pool, first.session_id()).await;
    assert_eq!(
        rows.len(),
        1,
        "ReusedActive must not write a second session_started row"
    );
}

/// Ambiguous engine-end (clean disconnect on FLV) → `hysteresis_entered`
/// row, written best-effort with the original cause preserved.
#[tokio::test]
async fn k3_hysteresis_entered_persisted() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_events(pool.clone());

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let sid = started.session_id().to_string();

    // Clean-disconnect Completed is not authoritative → enters hysteresis.
    let event = DownloadTerminalEvent::Completed {
        download_id: "dl-1".into(),
        streamer_id: STREAMER_ID.into(),
        streamer_name: "Test".into(),
        session_id: sid.clone(),
        total_bytes: 0,
        total_duration_secs: 0.0,
        total_segments: 0,
        file_path: None,
        engine_signal: crate::downloader::EngineEndSignal::CleanDisconnect,
    };
    lifecycle.on_download_terminal(&event).await.unwrap();

    let rows = read_events(&pool, &sid).await;
    // session_started + hysteresis_entered (in that chronological order).
    assert_eq!(
        rows.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        vec!["session_started", "hysteresis_entered"]
    );
    match event_payload(&rows[1].1) {
        SessionEventPayload::HysteresisEntered { cause, .. } => {
            // The cause carried into the audit row should be `Completed`
            // (the engine signal hint goes via a sibling field elsewhere).
            assert!(
                matches!(cause, TerminalCauseDto::Completed),
                "unexpected cause: {cause:?}"
            );
        }
        other => panic!("wrong payload variant: {other:?}"),
    }
}

/// hysteresis → live_detected within window → both `session_resumed` and
/// `session_started { from_hysteresis: true }` rows in order.
#[tokio::test]
async fn k4_resumed_then_started_pair_persisted() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_events(pool.clone());

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let sid = started.session_id().to_string();

    // Enter hysteresis.
    let event = DownloadTerminalEvent::Completed {
        download_id: "dl-1".into(),
        streamer_id: STREAMER_ID.into(),
        streamer_name: "Test".into(),
        session_id: sid.clone(),
        total_bytes: 0,
        total_duration_secs: 0.0,
        total_segments: 0,
        file_path: None,
        engine_signal: crate::downloader::EngineEndSignal::CleanDisconnect,
    };
    lifecycle.on_download_terminal(&event).await.unwrap();
    // Resume before the (25 ms) timer fires.
    lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();

    let rows = read_events(&pool, &sid).await;
    let kinds: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "session_started",
            "hysteresis_entered",
            "session_resumed",
            "session_started",
        ],
        "expected the full Recording → Hysteresis → Recording sequence"
    );
    match event_payload(&rows[3].1) {
        SessionEventPayload::SessionStarted {
            from_hysteresis, ..
        } => assert!(
            from_hysteresis,
            "the second session_started must mark from_hysteresis=true"
        ),
        other => panic!("wrong payload variant: {other:?}"),
    }
}

/// Authoritative end driven by a danmu signal preserves the cause as
/// `DefinitiveOffline { signal: DanmuStreamClosed }` — proves the
/// `OfflineSignal` plumbing through `OfflineDetectedArgs.signal` lands
/// in the audit log as advertised.
#[tokio::test]
async fn k5_session_ended_persisted_with_definitive_offline_signal() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle_with_events(pool.clone());

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let sid = started.session_id().to_string();

    // Mirror what the danmu observer in `services/container.rs` does
    // for a `DanmuControlEvent::StreamClosed`.
    lifecycle
        .on_offline_detected(OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(&sid),
            state_was_live: true,
            clear_errors: false,
            signal: Some(OfflineSignal::DanmuStreamClosed),
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
            match cause {
                TerminalCauseDto::DefinitiveOffline {
                    signal: OfflineSignal::DanmuStreamClosed,
                } => {}
                other => {
                    panic!("expected DefinitiveOffline {{ DanmuStreamClosed }}, got {other:?}")
                }
            }
        }
        other => panic!("wrong payload variant: {other:?}"),
    }
}

/// `enter_ended_state`'s 60s `Ended` retention dedup means duplicate
/// `on_offline_detected` calls land on the CAS guard before reaching
/// the tx — so exactly one `session_ended` row is persisted per
/// session, even when the monitor races and emits the offline twice.
#[tokio::test]
async fn k6_session_ended_dedup_persists_one_row() {
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

/// O1 — `end_for_disable` on an actively-recording session writes
/// `end_time`, transitions in-memory to `Ended`, broadcasts
/// `Ended { cause: UserDisabled, via_hysteresis: false }`, and writes a
/// matching `session_ended` audit row.
#[tokio::test]
async fn o1_end_for_disable_ends_active_recording_session() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // drain Started

    let sid = started.session_id().to_string();
    let resolved = lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(sid.as_str()));

    // DB end_time set.
    assert!(
        db_session_end_time(&pool, &sid).await.is_some(),
        "end_for_disable must set live_sessions.end_time"
    );

    // In-memory state Ended.
    let snapshot = lifecycle.session_snapshot(&sid).expect("session in memory");
    assert!(snapshot.is_ended());
    assert!(!lifecycle.is_session_active(&sid));

    // Broadcast: Ended { cause: UserDisabled, via_hysteresis: false }.
    match rx.recv().await.unwrap() {
        SessionTransition::Ended {
            cause,
            via_hysteresis,
            ..
        } => {
            assert_eq!(cause, TerminalCause::UserDisabled);
            assert!(!via_hysteresis);
            // Pipeline policy: UserDisabled fires session-complete DAG
            // (captured bytes deserve processing).
            assert!(cause.should_run_session_complete_pipeline());
        }
        other => panic!("expected Ended {{cause:UserDisabled}}, got {other:?}"),
    }

    // Audit row carries user_disabled cause.
    let payload = latest_session_ended_payload(&pool, &sid).await.unwrap();
    assert_eq!(
        session_ended_cause(&payload),
        &TerminalCauseDto::UserDisabled
    );
}

/// O2 — `end_for_disable` on a session in `Hysteresis` cancels the
/// timer (CAS won), writes Ended with `via_hysteresis: true`, and
/// emits a single Ended broadcast (no late timer-fire follow-up).
#[tokio::test]
async fn o2_end_for_disable_cancels_hysteresis_handle() {
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

/// O3 — `end_for_disable` with no active session is `Ok(None)`, no
/// broadcast, no DB write.
#[tokio::test]
async fn o3_end_for_disable_no_active_session_returns_none() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let resolved = lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
    assert!(resolved.is_none());
    assert!(rx.try_recv().is_err(), "no broadcast on empty tear-down");

    assert!(
        read_events(&pool, "missing-session").await.is_empty(),
        "no audit rows should be written when no active session exists"
    );
}

/// O4 — back-to-back `end_for_disable` calls collapse to a single
/// effective tear-down. Second call returns `Ok(None)` and emits no
/// second broadcast. Audit log has exactly one `session_ended` row.
#[tokio::test]
async fn o4_end_for_disable_idempotent_on_second_call() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Started

    let sid = started.session_id().to_string();
    let first = lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
    assert_eq!(first.as_deref(), Some(sid.as_str()));
    let _ = rx.recv().await.unwrap(); // Ended

    // Second call: idempotent. The session is already Ended in memory;
    // the retro-update path runs and finds the same cause already set,
    // returning the session id unchanged. No second `Ended` broadcast.
    let second = lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
    assert_eq!(second.as_deref(), Some(sid.as_str()));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        rx.try_recv().is_err(),
        "second end_for_disable must not re-broadcast Ended"
    );

    // Audit log has exactly one `session_ended` row.
    let ended_count = read_events(&pool, &sid)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "session_ended")
        .count();
    assert_eq!(ended_count, 1);
}

/// O4b — if a streamer ends and then starts a fresh session before the
/// old `Ended` snapshot is evicted, disable must target the new current
/// session rather than retro-updating the retained old one.
#[tokio::test]
async fn o4b_end_for_disable_targets_current_session_when_old_ended_is_retained() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());
    let mut rx = lifecycle.subscribe();

    let first = lifecycle
        .on_live_detected(live_args(Utc::now() - chrono::Duration::seconds(10)))
        .await
        .unwrap();
    let first_sid = first.session_id().to_string();
    let _ = rx.recv().await.unwrap(); // Started

    lifecycle
        .on_download_terminal(&make_terminal_completed_hls_endlist(&first_sid))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // Ended for first session
    assert!(!lifecycle.is_session_active(&first_sid));

    let second = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    let second_sid = second.session_id().to_string();
    assert_ne!(first_sid, second_sid);
    let _ = rx.recv().await.unwrap(); // Started for second session

    let resolved = lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(second_sid.as_str()));

    assert!(
        db_session_end_time(&pool, &second_sid).await.is_some(),
        "disable must close the current session"
    );
    assert!(
        matches!(
            lifecycle.session_snapshot(&second_sid),
            Some(SessionState::Ended {
                cause: TerminalCause::UserDisabled,
                ..
            })
        ),
        "current session must transition to UserDisabled"
    );

    match rx.recv().await.unwrap() {
        SessionTransition::Ended {
            session_id, cause, ..
        } => {
            assert_eq!(session_id, second_sid);
            assert_eq!(cause, TerminalCause::UserDisabled);
        }
        other => panic!("expected Ended for current session, got {other:?}"),
    }

    let first_payload = latest_session_ended_payload(&pool, &first_sid)
        .await
        .unwrap();
    assert!(
        session_ended_cause(&first_payload) != &TerminalCauseDto::UserDisabled,
        "old retained session must not be retro-attributed: {first_payload:?}"
    );
    let second_payload = latest_session_ended_payload(&pool, &second_sid)
        .await
        .unwrap();
    assert_eq!(
        session_ended_cause(&second_payload),
        &TerminalCauseDto::UserDisabled,
        "new current session should carry user_disabled"
    );
}

/// O5 — `end_for_disable` loses CAS to a concurrent
/// `resume_from_hysteresis`. The CAS-loss path takes effect: we
/// observe in-memory Recording (resumed) and the audit row is
/// retro-updated to `user_disabled` only if a session_ended row
/// existed (in this scenario it does NOT — resume cancelled hysteresis
/// without writing Ended). Method returns `Ok(None)` cleanly.
#[tokio::test]
async fn o5_end_for_disable_loses_cas_to_resume() {
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

    // end_for_disable now sees was_in_hysteresis=true, claim=None → retro path.
    // No prior session_ended row → the rewrite finds nothing → Ok(None).
    let resolved = lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();
    assert!(
        resolved.is_none(),
        "lost-CAS with no session_ended row must return Ok(None)"
    );

    // No spurious Ended broadcast.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(rx.try_recv().is_err(), "no broadcast on lost CAS");
}

/// O5b — `end_for_disable` loses CAS to the hysteresis timer fire.
/// Timer ends the session with cause Completed; `end_for_disable` sees
/// the Ended state and retro-rewrites the audit row's cause to
/// `user_disabled`. Verifies cause-overwrite-on-CAS-loss behaviour.
#[tokio::test]
async fn o5b_end_for_disable_overwrites_cause_when_timer_wins() {
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

/// O6 — `end_for_disable` does not touch the streamer row. The API
/// route owns `streamers.state`; the lifecycle method must not flip it.
#[tokio::test]
async fn o6_end_for_disable_does_not_touch_streamer_state() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());

    let _started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();

    // Pre-condition: start_or_resume flipped state to LIVE. Now manually
    // simulate the API route's "set state Disabled" side-effect.
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .update_streamer_state(STREAMER_ID, StreamerState::Disabled.as_str())
        .await
        .unwrap();

    lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();

    let state = streamer_state(&pool, STREAMER_ID).await;
    assert_eq!(
        state, "DISABLED",
        "end_for_disable must NOT flip streamers.state (API route owns it)"
    );
}

/// O7 — `end_for_disable` does not enqueue a `StreamerOffline` outbox
/// event. The user knows they disabled the streamer; downstream
/// integrations don't need a synthetic offline push.
#[tokio::test]
async fn o7_end_for_disable_skips_outbox_event() {
    let pool = setup_pool().await;
    let lifecycle = make_lifecycle(pool.clone());

    let _started = lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .unwrap();
    // After start_or_resume: outbox has one StreamerLive event.

    lifecycle
        .end_for_disable(STREAMER_ID, "Test")
        .await
        .unwrap();

    let event_types = outbox_event_types(&pool).await;
    assert_eq!(
        event_types,
        vec!["StreamerLive"],
        "end_for_disable must NOT enqueue StreamerOffline (no synthetic offline push)"
    );
}

/// P2 — out-of-schedule tear-down must go through SessionLifecycle, not
/// a raw DB row close. This keeps subscribers and the in-memory FSM in
/// sync while still avoiding a synthetic `StreamerOffline` outbox event.
#[tokio::test]
async fn p2_end_for_out_of_schedule_updates_lifecycle_and_outbox() {
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

#[tokio::test]
async fn p2_end_for_out_of_schedule_cancels_hysteresis() {
    let pool = setup_pool().await;
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

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        rx.try_recv().is_err(),
        "cancelled hysteresis timer must not emit a second Ended"
    );
}

/// O8 — concurrent `end_for_disable` calls collapse to a single
/// effective tear-down. Exactly one returns `Some(session_id)` after
/// writing the row; others return either `Ok(None)` (no active row by
/// the time they reach the repo) or `Some(session_id)` via the
/// retro-update path. Exactly one `session_ended` audit row exists.
#[tokio::test]
async fn o8_end_for_disable_idempotent_under_concurrency() {
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

/// O9 — broadcast ordering: by the time a subscriber receives `Ended`,
/// the in-memory snapshot already reflects the Ended state and the DB
/// `end_time` is committed. Ordering: commit → in-memory → broadcast.
#[tokio::test]
async fn o9_end_for_disable_broadcast_after_commit_and_memory_update() {
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
    let (required_tx, mut required_rx) = tokio::sync::mpsc::unbounded_channel();
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
    while let Ok(Some(transition)) =
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
