use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use tokio::sync::mpsc;

use super::ServiceContainer;
use crate::database::committed_writer::{CommitPhase, CommitTestGate};
use crate::database::models::{LiveSessionDbModel, StreamerDbModel};
use crate::database::repositories::session::SessionRepository;
use crate::database::repositories::streamer::{SqlxStreamerRepository, StreamerRepository};
use crate::domain::StreamerState;
use crate::downloader::{
    DownloadManagerEvent, DownloadProgressEvent, DownloadTerminalEvent, EngineEndSignal,
};
use crate::recorder::ActorRegistry;
use crate::recorder::fsm::FsmInput;
use crate::recorder::ports::{
    DbCommitOutcome, InitialStreamerSnapshot, LiveProbePayload, PipelineNotification, ProbeStatus,
    ResolvedActorConfig, RetirementBounds, SessionStartDisposition,
};
use crate::session::TerminalCause;

struct ActorHarness {
    container: ServiceContainer,
    pool: sqlx::SqlitePool,
    write_pool: sqlx::SqlitePool,
    registry: Arc<ActorRegistry>,
    _temp_dir: tempfile::TempDir,
}

impl ActorHarness {
    async fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let db_path = temp_dir.path().join("actor-registry.db");
        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
        let (pool, write_pool) = crate::database::init_database_pools(&db_url)
            .await
            .expect("init pools");
        crate::database::run_migrations(&pool)
            .await
            .expect("run migrations");
        let container = ServiceContainer::with_config(
            pool.clone(),
            write_pool.clone(),
            Duration::from_secs(60),
            64,
        )
        .await
        .expect("build actor container");
        // Stub extraction so actors spawned for synthetic `example.test` URLs
        // stay resident instead of exiting immediately with `FatalError`.
        container
            .extractor_runner
            .set_test_extraction(Some(Arc::new(|_, _| {
                Box::pin(async { Ok(crate::monitor::LiveStatus::Offline) })
            })));
        let registry = container.actor_registry.clone();
        Self {
            container,
            pool,
            write_pool,
            registry,
            _temp_dir: temp_dir,
        }
    }

    async fn seed_streamer(&self, id: &str, state: StreamerState) -> StreamerDbModel {
        let mut row = StreamerDbModel::new(
            format!("Streamer {id}"),
            format!("https://example.test/{id}"),
            "platform-huya",
        );
        row.id = id.to_owned();
        row.state = state.as_str().to_owned();
        SqlxStreamerRepository::new(self.pool.clone(), self.write_pool.clone())
            .create_streamer(&row)
            .await
            .expect("insert streamer");
        self.container
            .streamer_manager
            .hydrate()
            .await
            .expect("hydrate streamers");
        row
    }

    async fn shutdown(self) {
        self.registry
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(3))
            .await;
        self.container.notification_service.stop().await;
        self.container.pipeline_manager.stop().await;
        self.container.cancellation_token().cancel();
        self.write_pool.close().await;
        self.pool.close().await;
    }
}

fn dummy_live_payload(title: &str) -> Box<LiveProbePayload> {
    Box::new(LiveProbePayload {
        title: title.to_owned(),
        category: None,
        avatar: None,
        streams: vec![crate::monitor::StreamInfo {
            url: "http://127.0.0.1:9/live.flv".to_owned(),
            stream_format: platforms_parser::media::StreamFormat::Flv,
            media_format: platforms_parser::media::formats::MediaFormat::Flv,
            quality: "source".to_owned(),
            bitrate: 1_000_000,
            priority: 0,
            extras: None,
            codec: "h264".to_owned(),
            fps: 30.0,
            is_headers_needed: false,
            is_audio_only: false,
            format_opt_in: false,
        }],
        media_headers: None,
        media_extras: None,
        credential_binding: None,
        credential_snapshot: None,
        next_check_hint: None,
    })
}

/// While a `StreamerActor` has an in-flight `CommitDb` paused at `BeforeCommit`
/// or `AfterCommit`:
/// - the actor's `select!` loop remains responsive to mailbox stop commands,
/// - `retire_streamer` with zero bounds reports non-quiescent while the commit
///   lease is held,
/// - dropping/stopping the actor after seal still finishes the sealed commit
///   on `committed_write_supervisor` before `G+1` / DB observers proceed.
#[tokio::test]
async fn actor_commit_survives_caller_drop_and_stays_nonblocking_during_gated_write() {
    for (phase_label, phase) in [
        ("before", CommitPhase::BeforeCommit),
        ("after", CommitPhase::AfterCommit),
    ] {
        let harness = ActorHarness::new().await;
        let row = harness
            .seed_streamer("s-commit-gate", StreamerState::Live)
            .await;

        let mut session = LiveSessionDbModel::new(&row.id);
        session.id = format!("sess-{phase_label}");
        session.streamer_name = Some(row.name.clone());
        harness
            .container
            .session_repository
            .create_session(&session)
            .await
            .expect("seed open session");

        let snapshot = InitialStreamerSnapshot {
            streamer_id: row.id.clone(),
            state: StreamerState::Live,
            deleted: false,
            consecutive_error_count: 0,
            disabled_until: None,
            last_error: None,
            config: ResolvedActorConfig {
                streamer_name: row.name.clone(),
                streamer_url: row.url.clone(),
                platform: "huya".to_owned(),
                check_interval_ms: 600_000,
                offline_check_interval_ms: 600_000,
                offline_check_count: 1,
                ..ResolvedActorConfig::default()
            },
            config_revision: 1,
        };
        let (actor, handle) = harness.registry.build_actor_for_test(2, snapshot);
        let exit_tx =
            harness
                .registry
                .register_actor_handle_for_test(row.id.clone(), 2, handle.clone());
        let registry_ref = harness.registry.clone();
        let sid_clone = row.id.clone();
        let actor_task = tokio::spawn(async move {
            let reason = actor.run().await;
            registry_ref.remove_actor_for_test(&sid_clone, Some(2));
            let _ = exit_tx.send(true);
            reason
        });

        let writer = harness
            .container
            .streamer_manager
            .committed_state()
            .expect("committed state")
            .writer
            .clone();
        let gate = Arc::new(CommitTestGate::default());
        writer.set_commit_gate(phase, Some(gate.clone()));

        // `Disable` on a Live streamer with no running download immediately
        // emits `CommitDb(EndSession)` with `UserDisabled`.
        assert!(handle.send_input(FsmInput::Disable));

        tokio::time::timeout(Duration::from_secs(2), gate.started.notified())
            .await
            .expect("actor CommitDb must reach commit gate");
        assert!(
            harness.registry.store().has_in_flight_commit(&row.id),
            "per-streamer commit lease must be held while gated"
        );

        // Observe-only retirement while the commit is paused must report non-quiescent.
        let retirement = harness
            .registry
            .retire_streamer(
                &row.id,
                &row.name,
                RetirementBounds {
                    actor: Duration::ZERO,
                    download: Duration::ZERO,
                    pipeline: Duration::ZERO,
                },
            )
            .await;
        assert!(
            !retirement.is_quiescent(),
            "in-flight CommitDb + live actor must keep retirement non-quiescent"
        );

        // Aborting the actor task after the commit is sealed must NOT lose the commit.
        actor_task.abort();
        let _ = actor_task.await;
        harness.registry.remove_actor_for_test(&row.id, None);

        writer.set_commit_gate(phase, None);
        gate.release.notify_one();

        // Next generation / waiter must observe the sealed commit finish cleanly.
        tokio::time::timeout(
            Duration::from_secs(2),
            harness
                .registry
                .store()
                .wait_for_streamer_quiescence(&row.id),
        )
        .await
        .expect("sealed commit must finish after gate release");

        let end_time: Option<i64> =
            sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
                .bind(&session.id)
                .fetch_one(&harness.pool)
                .await
                .unwrap();
        assert!(
            end_time.is_some(),
            "sealed EndSession commit must persist even after actor task drop ({phase_label})"
        );
        assert!(
            !harness.registry.store().has_in_flight_commit(&row.id),
            "per-streamer commit lease must release after sealed commit completes"
        );
        // Because the actor was aborted before delivering `SessionEnded` to the
        // pipeline, Owner #4 (`pipeline`) must still block quiescence until settled.
        let before_pipeline_settle = harness
            .registry
            .outstanding_physical_owners_for_test(&row.id, Duration::ZERO)
            .await;
        assert!(
            before_pipeline_settle
                .iter()
                .all(|owner| !owner.contains("actor") && !owner.contains("commit lease")),
            "actor and commit lease owners must be clear, got: {before_pipeline_settle:?}"
        );
        assert!(
            before_pipeline_settle
                .iter()
                .any(|owner| owner.contains("pipeline")),
            "unsettled ended session must still hold pipeline quiescence, got: {before_pipeline_settle:?}"
        );

        harness
            .registry
            .deliver_pipeline_notification_for_test(PipelineNotification::SessionEnded {
                session_id: session.id.clone(),
                streamer_id: row.id.clone(),
                cause: TerminalCause::UserDisabled,
                should_run_session_complete: false,
            })
            .await;
        assert!(
            harness.registry.is_quiescent_for_test(&row.id).await,
            "quiescence must turn true once actor, commit lease, and pipeline all settle"
        );

        harness.shutdown().await;
    }
}

/// When committing a quiet-period expiry to SQLite fails transiently, the actor
/// retains the open session and retries once the fault clears.
#[tokio::test]
async fn quiet_period_commit_failure_retries_and_closes_session_once_fault_clears() {
    let harness = ActorHarness::new().await;
    let row = harness
        .seed_streamer("s-quiet-retry", StreamerState::Live)
        .await;

    let mut session = LiveSessionDbModel::new(&row.id);
    session.id = "sess-quiet-retry".to_owned();
    session.streamer_name = Some(row.name.clone());
    harness
        .container
        .session_repository
        .create_session(&session)
        .await
        .unwrap();

    // Inject a deferred-FK SQLite fault so the first EndSession reaches
    // `CommitPhase::BeforeCommit` deterministically and then fails on `COMMIT`.
    sqlx::query(
        "CREATE TABLE quiet_commit_fault \
         (session_id TEXT REFERENCES live_sessions(id) DEFERRABLE INITIALLY DEFERRED)",
    )
    .execute(&harness.write_pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_quiet_end AFTER UPDATE OF end_time ON live_sessions \
         BEGIN INSERT INTO quiet_commit_fault VALUES ('missing-session'); END",
    )
    .execute(&harness.write_pool)
    .await
    .unwrap();

    // Build an actor with a very short offline_check_delay_ms so the quiet period
    // backstop (`(count + 1) * delay`) expires in ~200 ms.
    let snapshot = InitialStreamerSnapshot {
        streamer_id: row.id.clone(),
        state: StreamerState::NotLive,
        deleted: false,
        consecutive_error_count: 0,
        disabled_until: None,
        last_error: None,
        config: ResolvedActorConfig {
            streamer_name: row.name.clone(),
            streamer_url: row.url.clone(),
            platform: "huya".to_owned(),
            check_interval_ms: 600_000,
            offline_check_interval_ms: 100,
            offline_check_count: 1,
            ..ResolvedActorConfig::default()
        },
        config_revision: 1,
    };
    let (mut actor, handle) = harness.registry.build_actor_for_test(1, snapshot);

    // Open the session inside the FSM by stepping through StartOrResume -> DbCommitted
    // without touching any external network, then drive an ambiguous DownloadEnded
    // that opens the QuietPeriod.
    let ctx = crate::recorder::fsm::StepContext::deterministic(
        std::time::Instant::now(),
        Timestamp::now(),
    );
    let _ = actor.fsm_mut().step(ctx, FsmInput::TimerFired);
    let probe_id = actor
        .fsm_mut()
        .in_flight_probe
        .as_ref()
        .expect("TimerFired on idle immediate check must spawn a probe")
        .probe_id;
    let start_effects = actor.fsm_mut().step(
        ctx,
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Live(dummy_live_payload("Quiet Retry Live"))),
        },
    );
    let start_op_id = start_effects
        .iter()
        .find_map(|effect| match effect {
            crate::recorder::FsmEffect::CommitDb { op_id, .. } => Some(*op_id),
            _ => None,
        })
        .expect("live probe must emit StartOrResume CommitDb");
    let started_effects = actor.fsm_mut().step(
        ctx,
        FsmInput::DbCommitted {
            op_id: start_op_id,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: session.id.clone(),
                started_at: Timestamp::now(),
                disposition: SessionStartDisposition::ReusedActive,
                credential_binding: None,
            }),
        },
    );
    let attempt_id = started_effects
        .iter()
        .find_map(|effect| match effect {
            crate::recorder::FsmEffect::StartAttempt(spec) => Some(spec.attempt_id),
            _ => None,
        })
        .expect("SessionStarted with streams must emit StartAttempt");

    // Step `AttemptStarted` + ambiguous `DownloadEnded` synchronously so the FSM
    // opens a `QuietPeriod`, then shorten its backstop to 50ms and suppress
    // background network polls so the actor exercises the real SQLite retry loop.
    let _ = actor.fsm_mut().step(
        ctx,
        FsmInput::AttemptStarted {
            attempt_id,
            session_id: session.id.clone(),
            download_id: "dl-quiet".to_owned(),
            engine_type: crate::downloader::engine::EngineType::Mesio,
            protocol: crate::downloader::DownloadProtocol::Flv,
        },
    );
    let _ = actor.fsm_mut().step(
        ctx,
        FsmInput::DownloadEnded {
            attempt_id,
            event: DownloadTerminalEvent::Completed {
                download_id: "dl-quiet".to_owned(),
                streamer_id: row.id.clone(),
                streamer_name: row.name.clone(),
                session_id: session.id.clone(),
                total_bytes: 1024,
                total_duration_secs: 5.0,
                total_segments: 1,
                file_path: None,
                engine_signal: EngineEndSignal::CleanDisconnect,
                stop_cause: None,
            },
        },
    );
    // Acknowledge RecordHysteresisEntered so pending_db is free for the backstop EndSession.
    if let Some(pending) = actor.fsm_mut().pending_db.as_ref() {
        let pending_id = pending.op_id;
        let _ = actor.fsm_mut().step(
            ctx,
            FsmInput::DbCommitted {
                op_id: pending_id,
                result: Ok(DbCommitOutcome::HysteresisEnteredRecorded),
            },
        );
    }
    if let crate::recorder::fsm::SessionSlot::Open(open) = &mut actor.fsm_mut().session {
        open.quiet_period
            .as_mut()
            .expect("ambiguous clean disconnect must arm QuietPeriod")
            .backstop_deadline = std::time::Instant::now() + Duration::from_millis(50);
    }
    actor.fsm_mut().monitor = crate::recorder::fsm::MonitorPhase::Idle {
        next_check: None,
        recurring_interval_ms: None,
        batch_routine_deadline: None,
    };

    let writer = harness
        .container
        .streamer_manager
        .committed_state()
        .expect("committed state")
        .writer
        .clone();
    let gate = Arc::new(CommitTestGate::default());
    writer.set_commit_gate(CommitPhase::BeforeCommit, Some(gate.clone()));

    let actor_task = tokio::spawn(actor.run());

    // Wait deterministically for the first quiet-period EndSession attempt to hit
    // the SQLite fault (rather than guessing with a fixed sleep).
    tokio::time::timeout(Duration::from_secs(2), gate.started.notified())
        .await
        .expect("quiet-period backstop must attempt EndSession");
    writer.set_commit_gate(CommitPhase::BeforeCommit, None);
    gate.release.notify_one();
    tokio::time::timeout(
        Duration::from_secs(2),
        harness
            .registry
            .store()
            .wait_for_streamer_quiescence(&row.id),
    )
    .await
    .expect("faulted EndSession attempt must finish and release its lease");

    let end_time_while_faulted: Option<i64> =
        sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
            .bind(&session.id)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
    assert!(
        end_time_while_faulted.is_none(),
        "session must stay open while EndSession commit is failing"
    );

    // Clear the SQLite fault; the 1s quiet-period retry must close the session.
    sqlx::query("DROP TRIGGER reject_quiet_end")
        .execute(&harness.write_pool)
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let end_time: Option<i64> =
                sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
                    .bind(&session.id)
                    .fetch_one(&harness.pool)
                    .await
                    .unwrap();
            if end_time.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("quiet-period retry must commit EndSession once the SQLite fault clears");

    assert!(handle.send_input(FsmInput::Shutdown));
    let _ = tokio::time::timeout(Duration::from_secs(2), actor_task)
        .await
        .expect("actor should stop on shutdown");

    harness.shutdown().await;
}

/// `stop_deadline == ZERO` must never clear an active physical owner (such as a
/// `StartupLeaseRegistry` lease covering the pre-active startup window).
#[tokio::test]
async fn retire_streamer_requires_all_four_physical_owners_past_stop_deadline() {
    let harness = ActorHarness::new().await;
    let row = harness
        .seed_streamer("s-retire-lease", StreamerState::NotLive)
        .await;

    // Register a startup lease (representing a download between Preflight and StartEngine).
    let (tx, _rx) = mpsc::unbounded_channel();
    let lease = harness.registry.startup_leases().register_lease(
        row.id.clone(),
        "sess-lease".to_owned(),
        7,
        tx,
    );

    let blocked = harness
        .registry
        .retire_streamer(
            &row.id,
            &row.name,
            RetirementBounds {
                actor: Duration::ZERO,
                download: Duration::ZERO,
                pipeline: Duration::ZERO,
            },
        )
        .await;
    assert!(
        !blocked.is_quiescent(),
        "active startup lease must block quiescence even past zero stop_deadline"
    );
    assert!(
        blocked
            .outstanding
            .iter()
            .any(|owner| owner.contains("startup lease")),
        "expected startup lease in outstanding owners, got: {:?}",
        blocked.outstanding
    );
    assert!(
        lease.is_cancelled(),
        "retire_streamer must cancel active startup leases"
    );

    // Dropping the lease releases the physical owner; retirement is now quiescent.
    drop(lease);
    let clear = harness
        .registry
        .retire_streamer(
            &row.id,
            &row.name,
            RetirementBounds {
                actor: Duration::ZERO,
                download: Duration::ZERO,
                pipeline: Duration::ZERO,
            },
        )
        .await;
    assert!(
        clear.is_quiescent(),
        "expected full quiescence after lease drop, got: {:?}",
        clear.outstanding
    );

    harness.shutdown().await;
}

/// A streamer persisted in `OutOfSpace` must stay dormant on startup (no resident
/// actor) until `OutputRootGate` recovery clears its error state (`OutOfSpace -> NotLive`),
/// at which point `PostPublicationListener` wakes it asynchronously.
#[tokio::test]
async fn persisted_out_of_space_stays_dormant_on_startup_and_wakes_on_recovery() {
    let harness = ActorHarness::new().await;
    let row = harness
        .seed_streamer("s-out-of-space", StreamerState::OutOfSpace)
        .await;

    harness.registry.start();
    // Allow initial AllStreamers reconciliation pass to run.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !harness.registry.has_actor(&row.id),
        "persisted OutOfSpace streamer must stay dormant on startup"
    );

    // Simulate OutputRootGate recovery hook clearing OutOfSpace -> NotLive.
    harness
        .container
        .streamer_manager
        .clear_error_state(&row.id)
        .await
        .expect("clear OutOfSpace error state");

    tokio::time::timeout(Duration::from_secs(3), async {
        while !harness.registry.has_actor(&row.id) {
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("clearing OutOfSpace must wake a resident actor via PostPublicationListener");
    assert_eq!(
        harness
            .container
            .streamer_manager
            .get_streamer(&row.id)
            .expect("woken streamer metadata must remain cached")
            .state,
        StreamerState::NotLive
    );

    harness.shutdown().await;
}

/// Even after a streamer's actor has exited, a trailing `SegmentCompleted` event
/// delivered over the download coordination channel must still persist the closed
/// segment row into `session_segments` and acknowledge completion.
#[tokio::test]
async fn trailing_closed_segment_persists_to_db_after_actor_exits() {
    let harness = ActorHarness::new().await;
    let row = harness
        .seed_streamer("s-trailing-seg", StreamerState::Disabled)
        .await;

    let mut session = LiveSessionDbModel::new(&row.id);
    session.id = "sess-trailing-seg".to_owned();
    session.streamer_name = Some(row.name.clone());
    harness
        .container
        .session_repository
        .create_session(&session)
        .await
        .unwrap();

    // Lower min_segment_size_bytes so a small synthetic file satisfies the keep threshold.
    let mut global = harness
        .container
        .config_service
        .get_global_config()
        .await
        .unwrap();
    global.min_segment_size_bytes = 1024;
    harness
        .container
        .config_service
        .update_global_config(&global)
        .await
        .unwrap();
    harness
        .container
        .config_service
        .invalidate_streamer(&row.id);

    let segment_path = harness._temp_dir.path().join("trailing_seg_0.flv");
    tokio::fs::write(&segment_path, vec![0u8; 4096])
        .await
        .unwrap();

    assert!(
        !harness.registry.has_actor(&row.id),
        "actor must be absent to exercise session-level trailing segment fallback"
    );

    let result = harness
        .registry
        .handle_required_download_event_for_test(DownloadManagerEvent::Progress(
            DownloadProgressEvent::SegmentCompleted {
                download_id: "dl-trailing".to_owned(),
                streamer_id: row.id.clone(),
                streamer_name: row.name.clone(),
                session_id: session.id.clone(),
                segment_path: segment_path.to_string_lossy().into_owned(),
                segment_index: 0,
                duration_secs: 12.5,
                size_bytes: 4096,
                started_at: Some(Timestamp::now()),
                split_reason_code: None,
                split_reason_details_json: None,
                split_by_format_change: false,
                completed_at: Timestamp::now(),
            },
        ))
        .await;
    assert!(
        result.is_ok(),
        "trailing closed segment fallback must succeed: {result:?}"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_segments WHERE session_id = ? AND segment_index = 0",
    )
    .bind(&session.id)
    .fetch_one(&harness.pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "trailing closed segment must persist into session_segments after actor exit"
    );

    harness.shutdown().await;
}

/// Poll `condition` every 20 ms for up to `limit`.
async fn eventually(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    tokio::time::timeout(limit, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn streamer_reenabled_during_its_stop_is_monitored_once_the_stop_ends() {
    use crate::streamer::manager::StreamerUpdateParams;

    let harness = ActorHarness::new().await;
    let row = harness
        .seed_streamer("s-reenable", StreamerState::Live)
        .await;
    let mut session = LiveSessionDbModel::new(&row.id);
    session.id = "sess-reenable".to_owned();
    session.streamer_name = Some(row.name.clone());
    harness
        .container
        .session_repository
        .create_session(&session)
        .await
        .unwrap();

    // Hand-step an actor into a running download of that session.
    let snapshot = InitialStreamerSnapshot {
        streamer_id: row.id.clone(),
        state: StreamerState::NotLive,
        deleted: false,
        consecutive_error_count: 0,
        disabled_until: None,
        last_error: None,
        config: ResolvedActorConfig {
            streamer_name: row.name.clone(),
            streamer_url: row.url.clone(),
            platform: "huya".to_owned(),
            check_interval_ms: 600_000,
            offline_check_interval_ms: 600_000,
            offline_check_count: 1,
            ..ResolvedActorConfig::default()
        },
        config_revision: 1,
    };
    // Outside the range the registry hands out, so the respawned generation
    // gets its own attempt ids.
    const OLD_GENERATION: u64 = 1_000;
    let (mut actor, handle) = harness
        .registry
        .build_actor_for_test(OLD_GENERATION, snapshot);
    let ctx = crate::recorder::fsm::StepContext::deterministic(
        std::time::Instant::now(),
        Timestamp::now(),
    );
    let _ = actor.fsm_mut().step(ctx, FsmInput::TimerFired);
    let probe_id = actor
        .fsm_mut()
        .in_flight_probe
        .as_ref()
        .expect("probe")
        .probe_id;
    let start_effects = actor.fsm_mut().step(
        ctx,
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Live(dummy_live_payload("Re-enable"))),
        },
    );
    let start_op_id = start_effects
        .iter()
        .find_map(|effect| match effect {
            crate::recorder::FsmEffect::CommitDb { op_id, .. } => Some(*op_id),
            _ => None,
        })
        .expect("start commit");
    let started_effects = actor.fsm_mut().step(
        ctx,
        FsmInput::DbCommitted {
            op_id: start_op_id,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: session.id.clone(),
                started_at: Timestamp::now(),
                disposition: SessionStartDisposition::ReusedActive,
                credential_binding: None,
            }),
        },
    );
    let attempt_id = started_effects
        .iter()
        .find_map(|effect| match effect {
            crate::recorder::FsmEffect::StartAttempt(spec) => Some(spec.attempt_id),
            _ => None,
        })
        .expect("start attempt");
    let _ = actor.fsm_mut().step(
        ctx,
        FsmInput::AttemptStarted {
            attempt_id,
            session_id: session.id.clone(),
            download_id: "dl-reenable".to_owned(),
            engine_type: crate::downloader::engine::EngineType::Mesio,
            protocol: crate::downloader::DownloadProtocol::Flv,
        },
    );
    harness
        .registry
        .supervise_actor_for_test(&row.id, OLD_GENERATION, actor, handle.clone());
    harness.registry.start();

    let set_state = |state| StreamerUpdateParams {
        id: row.id.clone(),
        name: None,
        url: None,
        platform_config_id: None,
        template_config_id: None,
        priority: None,
        state: Some(state),
        streamer_specific_config: None,
    };
    let monitor_state = || {
        harness
            .registry
            .runtime_view(&row.id)
            .map(|view| view.monitor_state)
    };

    // Disable: the actor waits for the engine to confirm the stop.
    harness
        .container
        .streamer_manager
        .partial_update_streamer(set_state(StreamerState::Disabled))
        .await
        .unwrap();
    assert!(
        eventually(Duration::from_secs(5), || monitor_state()
            == Some(StreamerState::Disabled))
        .await,
        "the actor enters its stop"
    );

    // Re-enable while the old generation is still stopping.
    harness
        .container
        .streamer_manager
        .partial_update_streamer(set_state(StreamerState::NotLive))
        .await
        .unwrap();
    harness
        .registry
        .reconcile_streamer_for_test(&row.id, false)
        .await;

    // The engine confirms the stop and the old generation ends its session.
    assert!(handle.send_input(FsmInput::DownloadEnded {
        attempt_id,
        event: DownloadTerminalEvent::Cancelled {
            download_id: "dl-reenable".to_owned(),
            streamer_id: row.id.clone(),
            streamer_name: row.name.clone(),
            session_id: session.id.clone(),
            cause: crate::downloader::DownloadStopCause::StreamerDisabled,
        },
    }));

    let monitored = eventually(Duration::from_secs(5), || {
        harness.registry.has_actor(&row.id)
            && monitor_state().is_some_and(|state| state != StreamerState::Disabled)
    })
    .await;
    let state = monitor_state();
    harness.shutdown().await;
    assert!(monitored, "re-enabled streamer is not monitored: {state:?}");
}

#[tokio::test]
async fn global_settings_save_reconfigures_actors_without_forcing_checks() {
    use crate::config::ConfigUpdateEvent;
    use crate::recorder::actor::{ActorHandle, ActorMessage};

    let harness = ActorHarness::new().await;
    let row = harness
        .seed_streamer("s-config", StreamerState::NotLive)
        .await;
    let (handle, mut inbox) = ActorHandle::detached_for_test();
    let _exit = harness
        .registry
        .register_actor_handle_for_test(row.id.clone(), 1, handle);
    harness.registry.start();

    harness
        .registry
        .handle_config_event(&ConfigUpdateEvent::GlobalUpdated);
    // Queued behind the global update, so its arrival means the update was handled.
    harness
        .registry
        .handle_config_event(&ConfigUpdateEvent::StreamerErrorCleared {
            streamer_id: row.id.clone(),
        });

    let mut inputs = Vec::new();
    let saw_both = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let Some(message) = inbox.recv().await else {
                return false;
            };
            if let ActorMessage::Input(input) = message {
                inputs.push(input);
            }
            let configured = inputs
                .iter()
                .any(|input| matches!(input, FsmInput::ConfigUpdated { .. }));
            let marker = inputs
                .iter()
                .any(|input| matches!(input, FsmInput::ErrorCleared));
            if configured && marker {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false);
    while let Ok(ActorMessage::Input(input)) = inbox.try_recv() {
        inputs.push(input);
    }
    harness.shutdown().await;

    assert!(saw_both, "config update and marker arrive: {inputs:?}");
    assert!(
        !inputs.iter().any(|input| matches!(
            input,
            FsmInput::CredentialMaterialChanged | FsmInput::CheckNow
        )),
        "a settings save must not force a check: {inputs:?}"
    );
}

#[tokio::test]
async fn failed_config_lookup_keeps_the_last_good_config_and_retries() {
    use crate::config::ConfigUpdateEvent;
    use crate::recorder::actor::{ActorHandle, ActorMessage};

    let harness = ActorHarness::new().await;
    let row = harness
        .seed_streamer("s-config-fail", StreamerState::NotLive)
        .await;
    let (handle, mut inbox) = ActorHandle::detached_for_test();
    let _exit = harness
        .registry
        .register_actor_handle_for_test(row.id.clone(), 1, handle);
    harness.registry.start();

    let set_url = async |url: &str| {
        sqlx::query("UPDATE streamers SET url = ? WHERE id = ?")
            .bind(url)
            .bind(&row.id)
            .execute(&harness.write_pool)
            .await
            .expect("update streamer url");
        harness
            .container
            .config_service
            .invalidate_streamer(&row.id);
    };
    // An unparsable URL makes the config lookup fail.
    set_url("not a url").await;
    harness
        .registry
        .handle_config_event(&ConfigUpdateEvent::StreamerMetadataUpdated {
            streamer_id: row.id.clone(),
        });
    // Queued behind the update, so its arrival means the update was handled.
    harness
        .registry
        .handle_config_event(&ConfigUpdateEvent::StreamerErrorCleared {
            streamer_id: row.id.clone(),
        });

    let mut inputs = Vec::new();
    let mut next_input = async || match inbox.recv().await {
        Some(ActorMessage::Input(input)) => Some(input),
        Some(_) => None,
        None => panic!("actor inbox closed"),
    };
    let marker = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(input) = next_input().await {
                let done = matches!(input, FsmInput::ErrorCleared);
                inputs.push(input);
                if done {
                    return;
                }
            }
        }
    })
    .await;
    let pushed_while_failing = inputs
        .iter()
        .any(|input| matches!(input, FsmInput::ConfigUpdated { .. }));

    // Once lookups succeed again, the scheduled retry pushes the config.
    set_url(&row.url).await;
    let retried = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(FsmInput::ConfigUpdated { .. }) = next_input().await {
                return;
            }
        }
    })
    .await;
    harness.shutdown().await;

    assert!(marker.is_ok(), "marker arrives: {inputs:?}");
    assert!(
        !pushed_while_failing,
        "a failed lookup must not replace the config: {inputs:?}"
    );
    assert!(retried.is_ok(), "the lookup is retried after it recovers");
}
