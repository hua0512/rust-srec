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

    // Ending a recording defers the physical delete by one pass: the transition
    // just published has not reached `PipelineCoordinator`, so nothing can yet
    // tell whether a session-complete DAG is owed.
    assert!(
        harness
            .streamer_row()
            .await
            .is_some_and(|row| row.deleted_at.is_some())
    );
    assert_eq!(
        harness
            .container
            .runtime_coordinator
            .reap_marked_streamers(OBSERVE_RETIREMENT)
            .await,
        1
    );
    assert!(harness.streamer_row().await.is_none());

    // The recording history survives the delete.
    let session = harness
        .session_repository()
        .get_session(SESSION_ID)
        .await
        .expect("the session row must survive the streamer delete");
    assert_eq!(session.streamer_id, None);
    assert_eq!(session.streamer_name.as_deref(), Some(STREAMER_NAME));
    assert!(session.end_time.is_some());
    assert_eq!(
        harness.session_ended_events().await,
        1,
        "the reaping pass must not write a second audit row"
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
async fn outstanding_pipeline_work_defers_the_reap() {
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
        // Both leave the row marked for one pass, because ending a recording is
        // what defers the reap; the reaper then removes it.
        assert!(
            harness
                .streamer_row()
                .await
                .is_some_and(|row| row.deleted_at.is_some())
        );
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
