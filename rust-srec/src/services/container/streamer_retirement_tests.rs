//! Deleting a streamer as mark -> retire -> reap.
//!
//! Every test here drives a real `ServiceContainer`, because the property under
//! test is the interaction between `StreamerManager`, `RuntimeCoordinator`,
//! `SessionLifecycle`, `Scheduler` and `PipelineManager` rather than any one of
//! them.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;

use crate::api::models::{BatchStreamerAction, BatchStreamerRequest};
use crate::api::routes::streamers::{self, StreamerRouteState};
use crate::database::models::{
    JobDbModel, LiveSessionDbModel, StreamerDbModel, StreamerState as DbStreamerState,
};
use crate::database::repositories::job::JobRepository;
use crate::database::repositories::session::SessionRepository;
use crate::database::repositories::streamer::StreamerRepository;
use crate::services::runtime_coordinator::{INTERACTIVE_RETIREMENT, OBSERVE_RETIREMENT};
use crate::session::{SessionTransition, TerminalCause};

const STREAMER_ID: &str = "streamer-retire";
const STREAMER_NAME: &str = "Retiring streamer";
const SESSION_ID: &str = "session-retire";
const STREAMER_URL: &str = "https://example.com/live/retire";

struct Harness {
    container: super::ServiceContainer,
    pool: SqlitePool,
    write_pool: SqlitePool,
    _temp_dir: tempfile::TempDir,
}

impl Harness {
    /// A container on a migrated file-backed database with no streamers yet.
    ///
    /// Seeding is a separate step because ordering against
    /// [`Self::start_scheduler`] matters: `Scheduler::spawn_initial_actors` runs
    /// once at the top of `Scheduler::run` over `get_all_active`, and an actor
    /// spawned there polls the platform for real and writes streamer state.
    /// Tests that only need the scheduler's command loop start it first and seed
    /// afterwards.
    async fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("test directory should initialize");
        let database_url = format!(
            "sqlite:{}?mode=rwc",
            temp_dir.path().join("retirement.sqlite").to_string_lossy()
        );
        let (pool, write_pool) = crate::database::init_database_pools(&database_url)
            .await
            .expect("test database pools should initialize");
        crate::database::run_migrations(&pool)
            .await
            .expect("test migrations should succeed");

        let container = super::ServiceContainer::new(pool.clone(), write_pool.clone())
            .await
            .expect("service container should initialize");

        // The config-event handler is what reacts to the
        // `StreamerStateSyncedFromDb { is_active: false }` that
        // `StreamerManager::mark_deleting` publishes, so a retirement is only
        // exercised against the real concurrency when it is running.
        container.setup_config_event_subscriptions();

        Self {
            container,
            pool,
            write_pool,
            _temp_dir: temp_dir,
        }
    }

    /// Insert the streamer row and hydrate it into `StreamerManager`.
    async fn seed_streamer(&self) {
        let mut streamer = StreamerDbModel::new(STREAMER_NAME, STREAMER_URL, "platform-huya");
        streamer.id = STREAMER_ID.to_string();
        streamer.state = DbStreamerState::Live.as_str().to_string();
        self.streamer_repository()
            .create_streamer(&streamer)
            .await
            .expect("streamer row should persist");
        self.hydrate().await;
    }

    async fn hydrate(&self) {
        self.container
            .streamer_manager
            .hydrate()
            .await
            .expect("streamer hydration should succeed");
    }

    /// A `live_sessions` row that is still recording, as a streamer deleted
    /// mid-broadcast has.
    async fn seed_active_session(&self) {
        let mut session = LiveSessionDbModel::new(STREAMER_ID);
        session.id = SESSION_ID.to_string();
        session.streamer_name = Some(STREAMER_NAME.to_string());
        self.session_repository()
            .create_session(&session)
            .await
            .expect("session row should persist");
    }

    fn streamer_repository(
        &self,
    ) -> crate::database::repositories::streamer::SqlxStreamerRepository {
        crate::database::repositories::streamer::SqlxStreamerRepository::new(
            self.pool.clone(),
            self.write_pool.clone(),
        )
    }

    fn session_repository(&self) -> crate::database::repositories::session::SqlxSessionRepository {
        crate::database::repositories::session::SqlxSessionRepository::new(
            self.pool.clone(),
            self.write_pool.clone(),
        )
    }

    fn job_repository(&self) -> crate::database::repositories::job::SqlxJobRepository {
        crate::database::repositories::job::SqlxJobRepository::new(
            self.pool.clone(),
            self.write_pool.clone(),
        )
    }

    /// Starts the scheduler and waits for its event loop to be the one holding
    /// the command receiver: `SchedulerHandle::remove_streamer_awaitable`
    /// answers `None` until then, which a retirement reads as "the actor stop
    /// was not observed".
    async fn start_scheduler(&self) {
        self.container
            .start_scheduler()
            .expect("scheduler should start");

        for _ in 0..200 {
            if self
                .container
                .scheduler_handle()
                .remove_streamer_awaitable("scheduler-loop-probe")
                .await
                .is_some()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the scheduler event loop should start answering commands");
    }

    fn route_state(&self) -> StreamerRouteState {
        StreamerRouteState::for_test(
            self.container.config_service.clone(),
            self.container.streamer_manager.clone(),
            Arc::new(
                crate::database::repositories::SqlxStreamerCheckHistoryRepository::new(
                    self.pool.clone(),
                    self.write_pool.clone(),
                ),
            ),
            self.container.runtime_coordinator.clone(),
        )
    }

    async fn streamer_row(&self) -> Option<StreamerDbModel> {
        sqlx::query_as::<_, StreamerDbModel>("SELECT * FROM streamers WHERE id = ?")
            .bind(STREAMER_ID)
            .fetch_optional(&self.pool)
            .await
            .expect("streamer lookup should succeed")
    }

    async fn session_ended_events(&self) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM session_events WHERE session_id = ? AND kind = 'session_ended'",
        )
        .bind(SESSION_ID)
        .fetch_one(&self.pool)
        .await
        .expect("session event count should succeed")
    }
}

/// Deleting a streamer mid-recording must end its session through
/// `SessionLifecycle` rather than leaving `trg_live_session_orphan_ends` to
/// stamp `end_time` inside the `DELETE FROM streamers`. Only the lifecycle path
/// writes the `session_ended` audit row and broadcasts
/// `SessionTransition::Ended`, which is what the pipeline and notification
/// layers act on.
#[tokio::test]
async fn deleting_a_recording_streamer_ends_its_session_through_the_lifecycle() {
    let harness = Harness::new().await;
    harness.start_scheduler().await;
    harness.seed_streamer().await;
    harness.seed_active_session().await;
    let mut transitions = harness.container.session_lifecycle.subscribe();

    assert!(
        harness
            .container
            .runtime_coordinator
            .delete_streamer(STREAMER_ID, INTERACTIVE_RETIREMENT)
            .await
            .expect("delete should succeed")
    );

    let transition = tokio::time::timeout(Duration::from_secs(5), transitions.recv())
        .await
        .expect("an Ended transition should be broadcast")
        .expect("the transition channel should stay open");
    let pipeline_transition = transition.clone();
    match transition {
        SessionTransition::Ended {
            session_id,
            streamer_name,
            cause,
            ..
        } => {
            assert_eq!(session_id, SESSION_ID);
            assert_eq!(cause, TerminalCause::UserDisabled);
            // Resolved from the marked `streamers` row, which still exists while
            // the retirement runs.
            assert_eq!(streamer_name, STREAMER_NAME);
        }
        other => panic!("expected an Ended transition, got {other:?}"),
    }

    assert_eq!(
        harness.session_ended_events().await,
        1,
        "the lifecycle path must write the session_ended audit row"
    );

    // The end consumer has not acknowledged this session yet.
    assert!(
        harness
            .streamer_row()
            .await
            .is_some_and(|row| row.deleted_at.is_some())
    );

    // The recording history is already detached from the streamer.
    let session = harness
        .session_repository()
        .get_session(SESSION_ID)
        .await
        .expect("the session row must survive the streamer delete");
    assert_eq!(session.streamer_name.as_deref(), Some(STREAMER_NAME));
    assert!(session.end_time.is_some());

    harness
        .container
        .pipeline_manager
        .handle_session_transition(pipeline_transition)
        .await;
    assert!(
        harness
            .container
            .session_lifecycle
            .session_snapshot(SESSION_ID)
            .is_some()
    );
    assert_eq!(
        harness
            .container
            .runtime_coordinator
            .reap_marked_streamers(OBSERVE_RETIREMENT)
            .await,
        1
    );
    assert!(
        harness.streamer_row().await.is_none(),
        "a durable receipt permits reaping without waiting for cache eviction"
    );
}

/// The `StreamerStateSyncedFromDb` that `StreamerManager::mark_deleting`
/// publishes must not make `ConfigEventHandler` run its own
/// `handle_streamer_disabled`. That stop does not wait on `AttemptCompletion`,
/// so it would end the session before `RuntimeCoordinator::retire_streamer`
/// gets there, and every delete would run two concurrent stops.
#[tokio::test]
async fn the_config_event_handler_leaves_a_marked_streamers_stop_to_the_retirement() {
    let harness = Harness::new().await;
    harness.seed_streamer().await;
    harness.seed_active_session().await;

    harness
        .container
        .streamer_manager
        .mark_deleting(STREAMER_ID)
        .await
        .expect("marking should succeed")
        .expect("the streamer should be markable");

    harness
        .container
        .apply_config_event_for_test(
            crate::config::ConfigUpdateEvent::StreamerStateSyncedFromDb {
                streamer_id: STREAMER_ID.to_string(),
                is_active: false,
            },
        )
        .await;

    assert_eq!(
        harness.session_ended_events().await,
        0,
        "the retirement owns the session end for a marked streamer"
    );
    let session = harness
        .session_repository()
        .get_session(SESSION_ID)
        .await
        .expect("the session row should exist");
    assert_eq!(session.end_time, None);

    // A merely disabled streamer still gets the stop, with a real open session.
    let disabled = Harness::new().await;
    disabled.seed_streamer().await;
    disabled.seed_active_session().await;
    sqlx::query("UPDATE streamers SET state = 'DISABLED' WHERE id = ?")
        .bind(STREAMER_ID)
        .execute(&disabled.write_pool)
        .await
        .unwrap();
    disabled.hydrate().await;
    disabled
        .container
        .apply_config_event_for_test(
            crate::config::ConfigUpdateEvent::StreamerStateSyncedFromDb {
                streamer_id: STREAMER_ID.to_string(),
                is_active: false,
            },
        )
        .await;
    assert_eq!(disabled.session_ended_events().await, 1);
}

/// The deferral has to hold whoever ended the session, not just when the
/// retirement ended it: a download attempt's terminal outcome reaching
/// `SessionLifecycle` during `stop_download_with_reason` ends it just as well,
/// and `PipelineCoordinator` is no more likely to have seen that end.
#[tokio::test]
async fn a_session_ended_by_another_task_still_defers_the_reap() {
    let harness = Harness::new().await;
    harness.start_scheduler().await;
    harness.seed_streamer().await;
    harness.seed_active_session().await;

    // Stands in for whatever ends the session first; what matters to the
    // retirement is that `SessionLifecycle` still holds the ended session.
    harness
        .container
        .session_lifecycle
        .end_for_disable(STREAMER_ID, STREAMER_NAME)
        .await
        .expect("the session should end")
        .expect("a session should have been ended");

    assert!(
        harness
            .container
            .runtime_coordinator
            .delete_streamer(STREAMER_ID, INTERACTIVE_RETIREMENT)
            .await
            .expect("delete should succeed")
    );
    assert!(
        harness
            .streamer_row()
            .await
            .is_some_and(|row| row.deleted_at.is_some()),
        "a session end this pass could not have observed must defer the reap"
    );
}

/// The reap is gated on the retirement acknowledging. With the scheduler's event
/// loop not running, `SchedulerHandle::remove_streamer_awaitable` answers
/// `None` — nothing observed the actor stop — so the row keeps its marker and
/// the streamer stands down without being removed.
#[tokio::test]
async fn an_unobserved_actor_removal_leaves_the_row_marked() {
    let harness = Harness::new().await;
    harness.seed_streamer().await;

    assert!(
        harness
            .container
            .runtime_coordinator
            .delete_streamer(STREAMER_ID, INTERACTIVE_RETIREMENT)
            .await
            .expect("delete should succeed")
    );

    let row = harness
        .streamer_row()
        .await
        .expect("an unacknowledged retirement must keep the row");
    assert!(row.deleted_at.is_some());

    // The user no longer has this streamer, and no owner may start work for it.
    let metadata = harness
        .container
        .streamer_manager
        .get_streamer(STREAMER_ID)
        .expect("the runtime keeps the marked metadata");
    assert!(metadata.is_deleted());
    assert!(!metadata.is_active());
    assert!(harness.container.streamer_manager.get_all().is_empty());

    // Once the loop runs, the same retirement acknowledges and the sweep reaps.
    harness.start_scheduler().await;
    assert_eq!(
        harness
            .container
            .runtime_coordinator
            .reap_marked_streamers(OBSERVE_RETIREMENT)
            .await,
        1
    );
    assert!(harness.streamer_row().await.is_none());
}

/// A crash between the mark and the reap leaves a row nobody is retiring. The
/// reaper's first pass is the startup recovery for exactly that row.
#[tokio::test]
async fn a_row_left_marked_by_a_crash_is_reaped_at_startup() {
    let harness = Harness::new().await;
    harness.seed_streamer().await;
    sqlx::query("UPDATE streamers SET deleted_at = ? WHERE id = ?")
        .bind(crate::database::time::now_ms())
        .bind(STREAMER_ID)
        .execute(&harness.write_pool)
        .await
        .expect("marker should persist");

    // Re-hydrating is what a restart does; the marked row must come back as
    // inactive metadata rather than as a streamer the scheduler picks up.
    harness.hydrate().await;
    assert!(
        harness
            .container
            .streamer_manager
            .get_all_active()
            .is_empty()
    );

    harness.start_scheduler().await;
    assert_eq!(
        harness
            .container
            .runtime_coordinator
            .reap_marked_streamers(OBSERVE_RETIREMENT)
            .await,
        1
    );
    assert!(harness.streamer_row().await.is_none());
}

/// A `job` row that has not reached a terminal status keeps the session's
/// pipeline work outstanding, so the reap must not run. Once the job completes,
/// the next sweep removes the row.
#[tokio::test]
async fn an_older_sessions_pipeline_work_defers_the_reap() {
    let harness = Harness::new().await;
    harness.start_scheduler().await;
    harness.seed_streamer().await;
    harness.seed_active_session().await;
    // Ended before the delete, so `SessionLifecycle::end_for_disable` has
    // nothing to move and the only reason left to defer is the job below.
    harness
        .session_repository()
        .end_session(SESSION_ID, crate::database::time::now_ms())
        .await
        .expect("session should end");
    harness
        .session_repository()
        .mark_session_complete_dispatched(SESSION_ID)
        .await
        .unwrap();
    sqlx::query("INSERT INTO live_sessions(id, streamer_id, start_time, end_time, session_complete_dispatched) VALUES ('newer-session', ?, ?, ?, 1)")
        .bind(STREAMER_ID)
        .bind(crate::database::time::now_ms() + 1)
        .bind(crate::database::time::now_ms() + 2)
        .execute(&harness.write_pool).await.unwrap();

    let mut job = JobDbModel::new("remux", "{}");
    job.session_id = Some(SESSION_ID.to_string());
    job.streamer_id = Some(STREAMER_ID.to_string());
    harness
        .job_repository()
        .create_job(&job)
        .await
        .expect("job row should persist");

    assert!(
        harness
            .container
            .runtime_coordinator
            .delete_streamer(STREAMER_ID, INTERACTIVE_RETIREMENT)
            .await
            .expect("delete should succeed")
    );
    assert!(
        harness
            .streamer_row()
            .await
            .is_some_and(|row| row.deleted_at.is_some()),
        "post-processing in flight must defer the physical delete"
    );

    sqlx::query("UPDATE job SET status = 'COMPLETED' WHERE id = ?")
        .bind(&job.id)
        .execute(&harness.write_pool)
        .await
        .expect("job status update should succeed");

    assert_eq!(
        harness
            .container
            .runtime_coordinator
            .reap_marked_streamers(OBSERVE_RETIREMENT)
            .await,
        1
    );
    assert!(harness.streamer_row().await.is_none());
}

/// `DELETE /api/streamers/{id}` — which the MCP `streamer_delete` tool calls
/// directly — and `BatchStreamerAction::Delete` both go through
/// `RuntimeCoordinator::delete_streamer`, so both end the recording through
/// `SessionLifecycle` and neither can remove a row the runtime still holds.
#[tokio::test]
async fn failed_final_dispatch_stays_recoverable_until_its_preset_is_restored() {
    use crate::database::models::{
        DagPipelineDefinition, DagStep, JobPreset, PipelineStep, SessionSegmentDbModel,
    };

    let harness = Harness::new().await;
    harness.start_scheduler().await;
    harness.seed_streamer().await;
    harness.seed_active_session().await;
    harness
        .session_repository()
        .end_session(SESSION_ID, crate::database::time::now_ms())
        .await
        .unwrap();
    let input = harness._temp_dir.path().join("input.mp4");
    tokio::fs::write(&input, b"recorded segment").await.unwrap();
    harness
        .session_repository()
        .create_session_segment(&SessionSegmentDbModel::new(
            SESSION_ID,
            0,
            input.to_str().unwrap(),
            1.0,
            16,
            Default::default(),
            Default::default(),
        ))
        .await
        .unwrap();
    let definition = DagPipelineDefinition::new(
        "finish",
        vec![DagStep::new(
            "finish",
            PipelineStep::Preset {
                name: "retirement-finish".to_string(),
            },
        )],
    );
    sqlx::query("UPDATE global_config SET session_complete_pipeline = ?, pipeline = NULL, paired_segment_pipeline = NULL, record_danmu = 0, auto_thumbnail = 0")
        .bind(serde_json::to_string(&definition).unwrap()).execute(&harness.write_pool).await.unwrap();
    harness
        .container
        .config_service
        .invalidate_streamer(STREAMER_ID);
    harness
        .container
        .streamer_manager
        .mark_deleting(STREAMER_ID)
        .await
        .unwrap()
        .unwrap();
    harness
        .container
        .pipeline_manager
        .recover_jobs()
        .await
        .unwrap();
    assert_eq!(
        harness
            .container
            .runtime_coordinator
            .reap_marked_streamers(OBSERVE_RETIREMENT)
            .await,
        0
    );
    assert!(harness.streamer_row().await.is_some());
    assert!(
        !harness
            .session_repository()
            .session_pipeline_settled(SESSION_ID)
            .await
            .unwrap()
    );

    harness
        .container
        .pipeline_manager
        .create_preset(&JobPreset {
            id: "retirement-finish-preset".to_string(),
            name: "retirement-finish".to_string(),
            description: None,
            category: None,
            processor: "remux".to_string(),
            config: "{}".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
        .await
        .unwrap();
    assert_eq!(
        harness
            .container
            .runtime_coordinator
            .reap_marked_streamers(OBSERVE_RETIREMENT)
            .await,
        0
    );
    let published: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dag_execution WHERE session_id = ? AND segment_source = 'session_complete'")
        .bind(SESSION_ID).fetch_one(&harness.pool).await.unwrap();
    assert_eq!(published, 1);
    assert!(
        harness
            .session_repository()
            .session_pipeline_settled(SESSION_ID)
            .await
            .unwrap()
    );
    assert!(
        harness.streamer_row().await.is_some(),
        "the published job still needs its streamer metadata"
    );
}

#[tokio::test]
async fn the_api_and_batch_delete_routes_share_the_retirement_path() {
    for use_batch in [false, true] {
        let harness = Harness::new().await;
        harness.start_scheduler().await;
        harness.seed_streamer().await;
        harness.seed_active_session().await;
        let state = harness.route_state();

        if use_batch {
            let response = streamers::batch_streamers(
                axum::extract::State(state),
                axum::Json(BatchStreamerRequest {
                    ids: vec![STREAMER_ID.to_string()],
                    action: BatchStreamerAction::Delete,
                }),
            )
            .await
            .expect("batch delete should succeed");
            assert_eq!(response.succeeded, 1, "{:?}", response.results);
        } else {
            streamers::delete_streamer(
                axum::extract::State(state),
                axum::extract::Path(STREAMER_ID.to_string()),
            )
            .await
            .map(|_| ())
            .expect("delete should succeed");
        }

        assert_eq!(
            harness.session_ended_events().await,
            1,
            "both routes must end the session through the lifecycle"
        );
        // Both leave the row marked, because ending a recording is what defers
        // the reap.
        assert!(
            harness
                .streamer_row()
                .await
                .is_some_and(|row| row.deleted_at.is_some())
        );
    }
}

/// The route rejects a streamer whose retirement is already under way, so a
/// second delete cannot start a competing one.
#[tokio::test]
async fn the_api_delete_route_reports_an_already_deleted_streamer_as_missing() {
    let harness = Harness::new().await;
    harness.seed_streamer().await;
    harness
        .container
        .streamer_manager
        .mark_deleting(STREAMER_ID)
        .await
        .expect("marking should succeed")
        .expect("the streamer should be markable");

    let error = streamers::delete_streamer(
        axum::extract::State(harness.route_state()),
        axum::extract::Path(STREAMER_ID.to_string()),
    )
    .await
    .map(|_| ())
    .expect_err("a marked streamer is gone as far as the API is concerned");
    assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
}

/// A configuration import writes its markers inside its own transaction, so an
/// import that fails after `apply_streamers` stops nothing: the rollback takes
/// the markers with it and every streamer keeps recording.
#[tokio::test]
async fn a_rolled_back_import_marks_nothing() {
    let harness = Harness::new().await;
    harness.seed_streamer().await;

    let mut tx = crate::database::begin_immediate(&harness.write_pool)
        .await
        .expect("transaction should begin");
    assert!(
        crate::database::repositories::streamer::mark_streamer_deleted(
            &mut *tx,
            STREAMER_ID,
            crate::database::time::now_ms(),
        )
        .await
        .expect("marker should be written inside the transaction")
    );
    drop(tx);

    let row = harness
        .streamer_row()
        .await
        .expect("the streamer row should survive");
    assert!(
        row.deleted_at.is_none(),
        "a rejected import must leave no marker behind"
    );
    assert!(
        harness
            .container
            .streamer_manager
            .get_streamer(STREAMER_ID)
            .is_some_and(|metadata| metadata.is_active()),
        "the streamer must still be monitored"
    );
}
