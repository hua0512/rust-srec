//! Deleting a streamer as mark -> retire -> reap.
//!
//! Every test here drives a real `ServiceContainer`, because the property under
//! test is the interaction between `StreamerManager`, `ActorRegistry`,
//! `RecorderStore`, and `PipelineManager` rather than any one of them.

use std::time::Duration;

use sqlx::SqlitePool;

use crate::database::models::{JobDbModel, LiveSessionDbModel, StreamerDbModel};
use crate::database::repositories::job::JobRepository;
use crate::database::repositories::session::SessionRepository;
use crate::database::repositories::streamer::StreamerRepository;
use crate::domain::StreamerState;
use crate::recorder::{
    INTERACTIVE_RETIREMENT, OBSERVE_RETIREMENT, PipelineNotification, RetirementBounds,
};

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
    /// [`Self::start_registry`] matters: `ActorRegistry::start` reconciles
    /// `get_all` on boot, and an actor spawned for a live streamer polls its
    /// platform. Tests that only need the reconciler loop running start it
    /// first and seed afterwards.
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
        streamer.state = StreamerState::Live.as_str().to_string();
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

    fn start_registry(&self) {
        self.container.actor_registry.start();
    }

    async fn streamer_row(&self) -> Option<StreamerDbModel> {
        sqlx::query_as::<_, StreamerDbModel>("SELECT * FROM streamers WHERE id = ?")
            .bind(STREAMER_ID)
            .fetch_optional(&self.pool)
            .await
            .expect("streamer lookup should succeed")
    }

    /// Give every session a one-step session-complete pipeline.
    async fn configure_session_complete_pipeline(&self) {
        use crate::database::models::{DagPipelineDefinition, DagStep, PipelineStep};

        let definition = DagPipelineDefinition::new(
            "finish",
            vec![DagStep::new(
                "finish",
                PipelineStep::Inline {
                    processor: "execute".to_string(),
                    config: serde_json::json!({
                        "command": if cfg!(windows) { "exit /B 0" } else { "true" }
                    }),
                },
            )],
        );
        sqlx::query("UPDATE global_config SET session_complete_pipeline = ?, pipeline = NULL, paired_segment_pipeline = NULL, record_danmu = 0, auto_thumbnail = 0")
            .bind(serde_json::to_string(&definition).unwrap())
            .execute(&self.write_pool)
            .await
            .expect("pipeline config should update");
        self.container
            .config_service
            .invalidate_streamer(STREAMER_ID);
    }

    /// Report a recorded segment of the seeded session to the pipeline, so the
    /// session has output for its session-complete pipeline.
    async fn record_segment(&self) {
        let path = self._temp_dir.path().join("segment-0.flv");
        tokio::fs::write(&path, b"recorded segment")
            .await
            .expect("segment file should write");
        self.container
            .actor_registry
            .deliver_pipeline_notification_for_test(PipelineNotification::VideoSegmentCompleted {
                session_id: SESSION_ID.to_string(),
                streamer_id: STREAMER_ID.to_string(),
                segment_index: 0,
                path,
                auto_thumbnail_candidate: false,
            })
            .await;
    }

    /// Session-complete pipeline runs created for the seeded session.
    async fn session_complete_runs(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM dag_execution WHERE session_id = ? AND segment_source = 'session_complete'")
            .bind(SESSION_ID)
            .fetch_one(&self.pool)
            .await
            .expect("run count should load")
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

/// Reconciliation triggered by `StreamerManager::mark_deleting` must leave
/// stopping a marked streamer to `ActorRegistry::retire_streamer`, whereas a
/// merely disabled streamer still receives `FsmInput::Disable` and closes its
/// active session.
#[tokio::test]
async fn reconciliation_leaves_a_marked_streamers_stop_to_the_retirement() {
    let harness = Harness::new().await;
    harness.seed_streamer().await;
    harness.seed_active_session().await;
    harness
        .container
        .actor_registry
        .spawn_dormant_actor_for_test(STREAMER_ID, STREAMER_NAME, STREAMER_URL);

    harness
        .container
        .streamer_manager
        .mark_deleting(STREAMER_ID)
        .await
        .expect("marking should succeed")
        .expect("the streamer should be markable");

    harness
        .container
        .actor_registry
        .reconcile_streamer_for_test(STREAMER_ID, false)
        .await;

    assert!(
        harness.container.actor_registry.has_actor(STREAMER_ID),
        "marked streamer actor must stay owned by retirement"
    );
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
    disabled
        .container
        .actor_registry
        .spawn_dormant_actor_for_test(STREAMER_ID, STREAMER_NAME, STREAMER_URL);
    sqlx::query("UPDATE streamers SET state = 'DISABLED' WHERE id = ?")
        .bind(STREAMER_ID)
        .execute(&disabled.write_pool)
        .await
        .unwrap();
    disabled.hydrate().await;
    disabled
        .container
        .actor_registry
        .reconcile_streamer_for_test(STREAMER_ID, false)
        .await;

    tokio::time::timeout(Duration::from_secs(3), async {
        while disabled.container.actor_registry.has_actor(STREAMER_ID) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("disabled streamer actor must close its session and exit");
    assert_eq!(disabled.session_ended_events().await, 1);
}

/// When a physical owner has not settled within the retirement bound, the row
/// keeps its `deleted_at` marker while disappearing from active views; once
/// the owner releases, the next reaper sweep removes the row.
#[tokio::test]
async fn an_unsettled_owner_leaves_the_row_marked_until_reaped() {
    let harness = Harness::new().await;
    harness.seed_streamer().await;

    // Hold a startup lease so `retire_streamer` deterministically observes a
    // live physical owner regardless of thread scheduling.
    let (actor_tx, _actor_rx) = tokio::sync::mpsc::unbounded_channel();
    let lease = harness
        .container
        .actor_registry
        .startup_leases()
        .register_lease(STREAMER_ID.to_string(), SESSION_ID.to_string(), 1, actor_tx);

    assert!(
        harness
            .container
            .actor_registry
            .delete_streamer(STREAMER_ID, OBSERVE_RETIREMENT)
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

    // Once the remaining physical owner releases, the sweep reaps the row.
    drop(lease);
    assert_eq!(
        harness
            .container
            .actor_registry
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
    // inactive metadata rather than as a streamer the registry spawns an actor for.
    harness.hydrate().await;
    assert!(
        harness
            .container
            .streamer_manager
            .get_all_active()
            .is_empty()
    );

    harness.start_registry();
    assert_eq!(
        harness
            .container
            .actor_registry
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
    harness.start_registry();
    harness.seed_streamer().await;
    harness.seed_active_session().await;
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

    // The job is still pending, so the delete waits out the whole pipeline
    // bound before it leaves the row marked.
    let bounds = RetirementBounds {
        pipeline: Duration::from_millis(100),
        ..INTERACTIVE_RETIREMENT
    };
    assert!(
        harness
            .container
            .actor_registry
            .delete_streamer(STREAMER_ID, bounds)
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
            .actor_registry
            .reap_marked_streamers(OBSERVE_RETIREMENT)
            .await,
        1
    );
    assert!(harness.streamer_row().await.is_none());
}

#[tokio::test]
async fn failed_final_dispatch_stays_recoverable_until_its_preset_is_restored() {
    use crate::database::models::{
        DagPipelineDefinition, DagStep, JobPreset, PipelineStep, SessionSegmentDbModel,
    };

    let harness = Harness::new().await;
    harness.start_registry();
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
            .actor_registry
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
            created_at: jiff::Timestamp::now(),
            updated_at: jiff::Timestamp::now(),
        })
        .await
        .unwrap();
    assert_eq!(
        harness
            .container
            .actor_registry
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

/// Deleting a streamer that has no actor ends its open session from the
/// registry, which must still run the session-complete pipeline.
#[tokio::test]
async fn a_session_ended_by_deletion_runs_the_session_complete_pipeline() {
    let harness = Harness::new().await;
    harness.seed_streamer().await;
    harness.seed_active_session().await;
    harness.configure_session_complete_pipeline().await;
    harness.record_segment().await;

    harness
        .container
        .actor_registry
        .delete_streamer(STREAMER_ID, OBSERVE_RETIREMENT)
        .await
        .expect("delete should succeed");

    assert_eq!(harness.session_complete_runs().await, 1);
}

/// Retiring the account a recording uses ends its session from the registry,
/// which must still run the session-complete pipeline.
#[tokio::test]
async fn a_session_ended_by_account_retirement_runs_the_session_complete_pipeline() {
    use crate::credentials::{
        CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialOwner,
        CredentialSelection, ResolvedCredentialPolicy,
    };

    let harness = Harness::new().await;
    harness.seed_streamer().await;
    harness.seed_active_session().await;
    harness.configure_session_complete_pipeline().await;
    harness.record_segment().await;
    let profile = harness
        .container
        .credential_profiles
        .create(
            "platform-huya",
            "Retiring account",
            true,
            &CredentialMaterial {
                cookies: "session=retiring".into(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            },
            &crate::proxies::ProxyRoute::Inherit,
        )
        .await
        .expect("profile should persist");
    let binding = CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: profile.id.clone(),
        },
        revision: profile.revision as u64,
        policy: ResolvedCredentialPolicy::new(
            "platform-huya".into(),
            CredentialOwner::Platform {
                platform_id: "platform-huya".into(),
            },
            CredentialSelection::Fixed {
                credential_id: profile.id.clone(),
            },
        )
        .unwrap(),
        epoch: 0,
    };
    sqlx::query("UPDATE live_sessions SET credential_binding = ? WHERE id = ?")
        .bind(serde_json::to_string(&binding).unwrap())
        .bind(SESSION_ID)
        .execute(&harness.write_pool)
        .await
        .expect("binding should persist");
    sqlx::query("INSERT INTO retirement_credential_profiles (profile_id) VALUES (?)")
        .bind(&profile.id)
        .execute(&harness.write_pool)
        .await
        .expect("retirement should persist");

    harness
        .container
        .actor_registry
        .settle_credential_retirement(STREAMER_ID)
        .await;

    assert!(
        harness
            .session_repository()
            .get_session(SESSION_ID)
            .await
            .unwrap()
            .end_time
            .is_some(),
        "the retirement ends the session"
    );
    assert_eq!(harness.session_complete_runs().await, 1);
}
