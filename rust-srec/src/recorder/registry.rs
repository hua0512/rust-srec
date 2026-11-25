//! Actor registry, crash supervision, download/danmu coordination bridges,
//! post-publication reconciliation, and 4-owner physical retirement quiescence.

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use futures::{FutureExt, StreamExt};
use jiff::Timestamp;
use parking_lot::Mutex;
use tokio::sync::{Semaphore, broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::{ConfigService, ConfigUpdateEvent};
use crate::credentials::CredentialOwner;
use crate::danmu::events::DanmuCoordinationReceiver;
use crate::danmu::{CollectionStopReason, DanmuControlEvent, DanmuEvent, DanmuService};
use crate::database::repositories::config::SqlxConfigRepository;
use crate::database::repositories::streamer::SqlxStreamerRepository;
use crate::domain::StreamerState;
use crate::downloader::{
    DownloadCoordinationReceiver, DownloadManager, DownloadManagerEvent, DownloadProgressEvent,
    DownloadStopCause, RecorderDownloadFeedback, RecorderFeedbackPermit,
};
use crate::monitor::BigoCheckTrigger;
use crate::pipeline::PipelineManager;
use crate::recorder::actor::effects::{
    close_danmu_segment, delete_discarded_files, deliver_pipeline_notification, start_danmu_segment,
};
use crate::recorder::actor::startup_task::StartupLeaseRegistry;
use crate::recorder::actor::{ActorHandle, ActorMessage, ActorRuntimeDeps, StreamerActor};
use crate::recorder::fsm::FsmInput;
use crate::recorder::fsm::segment::{
    paired_danmu_path, should_discard_segment, should_record_recovery_from_progress,
};
use crate::recorder::ports::{
    ActorStopReason, DanmuCloseSegmentSpec, DanmuSegmentRecord, DanmuStopResult, DbCommitOp,
    DbCommitOutcome, InitialStreamerSnapshot, PipelineNotification, RecorderStorePort,
    ResolvedActorConfig, RetirementBounds, StreamerRetirement, StreamerRuntimeView,
    VideoSegmentRecord, should_spawn_actor,
};
use crate::recorder::prober::RecorderProber;
use crate::recorder::store::{PostPublicationChangeSet, PostPublicationListener, RecorderStore};
use crate::session::TerminalCause;
use crate::streamer::{StreamerManager, StreamerMetadata};
use crate::utils::failure_samples::FailureSamples;
use crate::utils::required_channel::Delivery;
use crate::utils::task_supervisor::TaskSupervisor;

/// Minimum interval between routine `DownloadHeartbeat` deliveries per download.
const HEARTBEAT_MIN_INTERVAL: Duration = Duration::from_secs(5);

/// Bounded capacity for reserved lifecycle feedback permits (`DownloadStarted`
/// and `DownloadTerminalEvent`).
const MAX_GLOBAL_FEEDBACK_PERMITS: usize = 1024;
const MAX_STREAMER_FEEDBACK_PERMITS: usize = 32;

/// Maximum automatic crash-restart attempts within the backoff window.
const MAX_CRASH_RESTARTS: u32 = 5;
const CRASH_RESTART_BASE_BACKOFF: Duration = Duration::from_secs(2);
const CRASH_RESTART_MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Grace period for credential retirement physical stop confirmation.
const CREDENTIAL_RETIREMENT_STOP_TIMEOUT: Duration = Duration::from_secs(20);

/// Bound on individual actor stop confirmation during graceful process shutdown.
const SHUTDOWN_ACTOR_WAIT: Duration = Duration::from_secs(10);

struct ActorEntry {
    generation: u64,
    handle: ActorHandle,
    exit_rx: watch::Receiver<bool>,
}

enum ReconcileCommand {
    Batch(PostPublicationChangeSet),
    Streamer {
        streamer_id: String,
        reconfigured: bool,
    },
    AllStreamers {
        reconfigured: bool,
    },
    Platform(String),
    Template(String),
    CredentialMaterial(CredentialOwner),
    ErrorCleared(String),
    FiltersUpdated(String),
    CheckNow(String),
    RetryAdmission(String),
}

struct RecorderFeedbackState {
    capacity_limit: usize,
    per_streamer: HashMap<String, Arc<Semaphore>>,
}

/// Reserved-capacity download lifecycle sink implementing
/// [`RecorderDownloadFeedback`] for `DownloadManager`.
pub(crate) struct RecorderFeedbackSink {
    monitoring_stopped: AtomicBool,
    global_capacity: Arc<Semaphore>,
    state: Mutex<RecorderFeedbackState>,
    registry: parking_lot::RwLock<Option<Weak<ActorRegistry>>>,
}

impl RecorderFeedbackSink {
    fn new() -> Self {
        Self {
            monitoring_stopped: AtomicBool::new(false),
            global_capacity: Arc::new(Semaphore::new(MAX_GLOBAL_FEEDBACK_PERMITS)),
            state: Mutex::new(RecorderFeedbackState {
                capacity_limit: MAX_GLOBAL_FEEDBACK_PERMITS,
                per_streamer: HashMap::new(),
            }),
            registry: parking_lot::RwLock::new(None),
        }
    }

    fn bind_registry(&self, registry: &Arc<ActorRegistry>) {
        *self.registry.write() = Some(Arc::downgrade(registry));
    }

    fn mark_stopped(&self) {
        self.monitoring_stopped.store(true, Ordering::Release);
    }
}

impl RecorderDownloadFeedback for RecorderFeedbackSink {
    fn ensure_attempt_capacity(&self, attempts: usize) {
        let needed = attempts
            .saturating_mul(2)
            .saturating_add(MAX_GLOBAL_FEEDBACK_PERMITS)
            .min(Semaphore::MAX_PERMITS);
        let mut state = self.state.lock();
        if needed > state.capacity_limit {
            self.global_capacity
                .add_permits(needed - state.capacity_limit);
            state.capacity_limit = needed;
        }
    }

    fn monitoring_stopped(&self) -> bool {
        self.monitoring_stopped.load(Ordering::Acquire)
    }

    fn reserve(&self, streamer_id: &str) -> std::result::Result<RecorderFeedbackPermit, String> {
        let global = self
            .global_capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| "recorder lifecycle capacity is full".to_owned())?;
        let streamer_sem = self
            .state
            .lock()
            .per_streamer
            .entry(streamer_id.to_owned())
            .or_insert_with(|| Arc::new(Semaphore::new(MAX_STREAMER_FEEDBACK_PERMITS)))
            .clone();
        let streamer = streamer_sem
            .try_acquire_owned()
            .map_err(|_| "streamer lifecycle capacity is full".to_owned())?;
        Ok(RecorderFeedbackPermit::new(
            streamer_id.to_owned(),
            global,
            streamer,
        ))
    }

    fn reserve_attempt(
        &self,
        streamer_id: &str,
    ) -> std::result::Result<(RecorderFeedbackPermit, RecorderFeedbackPermit), String> {
        Ok((self.reserve(streamer_id)?, self.reserve(streamer_id)?))
    }

    fn publish_reserved(&self, event: &DownloadManagerEvent, permit: RecorderFeedbackPermit) {
        let Some(registry) = self.registry.read().as_ref().and_then(Weak::upgrade) else {
            return;
        };
        registry.deliver_reserved_download_event(event, permit);
    }

    fn request_recheck(&self, streamer_id: &str) {
        if let Some(registry) = self.registry.read().as_ref().and_then(Weak::upgrade) {
            registry.request_retry_admission(streamer_id);
        }
    }

    fn stop_monitoring(&self) {
        self.mark_stopped();
    }
}

/// Dependencies required to construct [`ActorRegistry`].
pub struct ActorRegistryDependencies {
    pub store: Arc<RecorderStore>,
    pub prober: Arc<RecorderProber>,
    pub download_manager: Arc<DownloadManager>,
    pub danmu_service: Arc<DanmuService>,
    pub pipeline_manager: Arc<PipelineManager>,
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    pub config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    pub check_interval_ms: u64,
    pub root_cancel: CancellationToken,
}

/// Registry and crash supervisor for all resident `StreamerActor` tasks.
pub struct ActorRegistry {
    actors: DashMap<String, ActorEntry>,
    generation_counter: AtomicU64,
    config_revisions: DashMap<String, Arc<AtomicU64>>,
    crash_counts: DashMap<String, u32>,
    runtime_views: Arc<DashMap<String, StreamerRuntimeView>>,
    /// Tracks discarded `(session_id, segment_index)` keys for trailing danmu
    /// completions that arrive after the streamer's actor has already exited.
    discarded_segments_fallback: DashMap<(String, u32), Instant>,
    /// Tracks `session_id -> streamer_id` for danmu events (like `Reconnecting` /
    /// `CollectionStopped`) whose payload does not carry `streamer_id`.
    danmu_session_streamers: DashMap<String, String>,
    deps: ActorRuntimeDeps,
    check_interval_ms: AtomicU64,
    feedback: Arc<RecorderFeedbackSink>,
    actor_tasks: TaskSupervisor,
    reconcile_tasks: TaskSupervisor,
    reconcile_tx: mpsc::UnboundedSender<ReconcileCommand>,
    reconcile_rx: Mutex<Option<mpsc::UnboundedReceiver<ReconcileCommand>>>,
    root_cancel: CancellationToken,
    started: AtomicBool,
    shutting_down: AtomicBool,
}

impl ActorRegistry {
    /// Construct a new [`ActorRegistry`] and bind its reserved download feedback sink.
    pub fn new(deps: ActorRegistryDependencies) -> Arc<Self> {
        let runtime_views = Arc::new(DashMap::new());
        let leases = Arc::new(StartupLeaseRegistry::new(deps.root_cancel.child_token()));
        let actor_deps = ActorRuntimeDeps {
            store: deps.store,
            prober: deps.prober,
            download_manager: deps.download_manager,
            danmu_service: deps.danmu_service,
            pipeline_manager: deps.pipeline_manager,
            streamer_manager: deps.streamer_manager,
            config_service: deps.config_service,
            leases,
            runtime_views: runtime_views.clone(),
        };
        let (reconcile_tx, reconcile_rx) = mpsc::unbounded_channel();
        let feedback = Arc::new(RecorderFeedbackSink::new());
        let registry = Arc::new(Self {
            actors: DashMap::new(),
            generation_counter: AtomicU64::new(1),
            config_revisions: DashMap::new(),
            crash_counts: DashMap::new(),
            runtime_views,
            discarded_segments_fallback: DashMap::new(),
            danmu_session_streamers: DashMap::new(),
            deps: actor_deps,
            check_interval_ms: AtomicU64::new(deps.check_interval_ms.max(100)),
            feedback: feedback.clone(),
            actor_tasks: TaskSupervisor::with_cancellation(deps.root_cancel.child_token()),
            reconcile_tasks: TaskSupervisor::with_cancellation(deps.root_cancel.child_token()),
            reconcile_tx,
            reconcile_rx: Mutex::new(Some(reconcile_rx)),
            root_cancel: deps.root_cancel,
            started: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
        });
        feedback.bind_registry(&registry);
        registry
    }

    /// Reserved-capacity download feedback sink to register on `DownloadManager`.
    pub(crate) fn download_feedback(&self) -> Arc<dyn RecorderDownloadFeedback> {
        self.feedback.clone()
    }

    /// Access the unidirectional read-only runtime views map (for API/MCP/health probes).
    pub fn runtime_views(&self) -> &Arc<DashMap<String, StreamerRuntimeView>> {
        &self.runtime_views
    }

    /// Snapshot the current read-only [`StreamerRuntimeView`] for `streamer_id`.
    pub fn runtime_view(&self, streamer_id: &str) -> Option<StreamerRuntimeView> {
        self.runtime_views
            .get(streamer_id)
            .map(|entry| entry.value().clone())
    }

    /// Access the shared [`RecorderStore`].
    pub fn store(&self) -> &Arc<RecorderStore> {
        &self.deps.store
    }

    /// Access the shared [`RecorderProber`].
    pub fn prober(&self) -> &Arc<RecorderProber> {
        &self.deps.prober
    }

    /// Access the independent [`StartupLeaseRegistry`].
    pub fn startup_leases(&self) -> &Arc<StartupLeaseRegistry> {
        &self.deps.leases
    }

    /// Update the global default check interval applied to subsequent config resolutions.
    pub fn set_check_interval_ms(&self, check_interval_ms: u64) {
        self.check_interval_ms
            .store(check_interval_ms.max(100), Ordering::Release);
    }

    /// Start the asynchronous reconciliation loop and hydrate resident actors for
    /// all currently active streamers.
    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(rx) = self.reconcile_rx.lock().take() else {
            return;
        };
        let registry = self.clone();
        self.reconcile_tasks
            .spawn("recorder reconciler", async move {
                registry.run_reconciler(rx).await;
            });

        let _ = self.reconcile_tx.send(ReconcileCommand::AllStreamers {
            reconfigured: false,
        });
    }

    /// Spawn the required download coordination consumer and edge-throttled
    /// progress observer on `supervisor`.
    pub(crate) fn spawn_download_bridges(
        self: &Arc<Self>,
        coordination_rx: DownloadCoordinationReceiver,
        supervisor: &TaskSupervisor,
    ) {
        let observer_rx = self.deps.download_manager.subscribe();
        let registry_obs = self.clone();
        let cancel_obs = self.root_cancel.child_token();
        supervisor.spawn("recorder download progress observer", async move {
            registry_obs
                .run_download_observer(observer_rx, cancel_obs)
                .await;
        });

        let registry_coord = self.clone();
        supervisor.spawn_critical("recorder download coordination", async move {
            registry_coord
                .run_download_coordination(coordination_rx)
                .await
        });
    }

    /// Spawn the required FIFO danmu coordination consumer on `supervisor`.
    pub(crate) fn spawn_danmu_bridge(
        self: &Arc<Self>,
        danmu_rx: DanmuCoordinationReceiver,
        supervisor: &TaskSupervisor,
    ) {
        let registry = self.clone();
        supervisor.spawn_critical("recorder danmu coordination", async move {
            registry.run_danmu_coordination(danmu_rx).await
        });
    }

    /// Request an immediate status check for `streamer_id`.
    pub fn check_now(&self, streamer_id: &str) {
        let _ = self
            .reconcile_tx
            .send(ReconcileCommand::CheckNow(streamer_id.to_owned()));
    }

    /// Notify `streamer_id` that lifecycle admission capacity recovered.
    pub fn request_retry_admission(&self, streamer_id: &str) {
        let _ = self
            .reconcile_tx
            .send(ReconcileCommand::RetryAdmission(streamer_id.to_owned()));
    }

    /// Route a [`ConfigUpdateEvent`] into the asynchronous reconciler.
    pub fn handle_config_event(&self, event: &ConfigUpdateEvent) {
        match event {
            ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id } => {
                let _ = self.reconcile_tx.send(ReconcileCommand::Streamer {
                    streamer_id: streamer_id.clone(),
                    reconfigured: true,
                });
            }
            ConfigUpdateEvent::StreamerStateSyncedFromDb { streamer_id, .. } => {
                let _ = self.reconcile_tx.send(ReconcileCommand::Streamer {
                    streamer_id: streamer_id.clone(),
                    reconfigured: false,
                });
            }
            ConfigUpdateEvent::StreamerDeleted { streamer_id } => {
                let _ = self
                    .reconcile_tx
                    .send(ReconcileCommand::Batch(PostPublicationChangeSet {
                        updated_streamers: Vec::new(),
                        removed_streamers: vec![streamer_id.clone()],
                        reconfigured_streamers: Vec::new(),
                        outbox_enqueued: false,
                    }));
            }
            ConfigUpdateEvent::StreamerErrorCleared { streamer_id } => {
                let _ = self
                    .reconcile_tx
                    .send(ReconcileCommand::ErrorCleared(streamer_id.clone()));
            }
            ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } => {
                let _ = self
                    .reconcile_tx
                    .send(ReconcileCommand::FiltersUpdated(streamer_id.clone()));
            }
            ConfigUpdateEvent::PlatformUpdated { platform_id } => {
                let _ = self
                    .reconcile_tx
                    .send(ReconcileCommand::Platform(platform_id.clone()));
            }
            ConfigUpdateEvent::TemplateUpdated { template_id } => {
                let _ = self
                    .reconcile_tx
                    .send(ReconcileCommand::Template(template_id.clone()));
            }
            ConfigUpdateEvent::CredentialMaterialChanged { owner } => {
                let _ = self
                    .reconcile_tx
                    .send(ReconcileCommand::CredentialMaterial(owner.clone()));
            }
            ConfigUpdateEvent::GlobalUpdated => {
                let _ = self
                    .reconcile_tx
                    .send(ReconcileCommand::AllStreamers { reconfigured: true });
            }
            ConfigUpdateEvent::EngineUpdated { .. } => {}
        }
    }

    /// Number of currently resident actors.
    pub fn actor_count(&self) -> usize {
        self.actors.len()
    }

    /// Whether a streamer currently has a resident actor.
    pub fn has_actor(&self, streamer_id: &str) -> bool {
        self.actors.contains_key(streamer_id)
    }

    /// Strictly verify all 4 physical resource owners for `streamer_id`:
    /// 1. Actor exited & zero in-flight `RecorderStore` commit leases,
    /// 2. Zero startup leases, zero pending queue entries, zero active downloads,
    /// 3. Zero active danmu collections,
    /// 4. Zero unsettled sessions or jobs in `PipelineManager`.
    pub async fn is_quiescent(&self, streamer_id: &str) -> bool {
        self.outstanding_physical_owners(streamer_id, Duration::ZERO)
            .await
            .is_empty()
    }

    /// Stand down every runtime owner for a streamer marked with `deleted_at`
    /// and report which physical owners remain active, if any.
    ///
    /// Stop deadlines never clear an outstanding physical owner, so callers
    /// (`reap_deleted`) stay strictly gated on real quiescence.
    pub(crate) async fn retire_streamer(
        &self,
        streamer_id: &str,
        streamer_name: &str,
        bounds: RetirementBounds,
    ) -> StreamerRetirement {
        let mut outstanding = Vec::new();

        // 1. Tell the resident actor to retire and wait up to `bounds.actor`.
        if let Some(mut exit_rx) = self.actors.get(streamer_id).map(|entry| {
            let _ = entry.handle.send_input(FsmInput::Retire { bounds });
            entry.exit_rx.clone()
        }) && !*exit_rx.borrow()
            && !bounds.actor.is_zero()
        {
            let _ = tokio::time::timeout(bounds.actor, async {
                while !*exit_rx.borrow() {
                    if exit_rx.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
        }
        if self.actors.contains_key(streamer_id) {
            outstanding.push("recorder actor".to_string());
        }

        // 2. Cancel startup leases and stop any active downloads independently of actor state.
        self.deps.leases.cancel_streamer(streamer_id);
        let active_downloads = self
            .deps
            .download_manager
            .active_download_ids_for_streamer(streamer_id);
        for (download_id, _) in &active_downloads {
            if bounds.download.is_zero() {
                if let Err(error) = self
                    .deps
                    .download_manager
                    .request_stop_download(download_id, DownloadStopCause::StreamerDisabled)
                    && !matches!(error, crate::Error::NotFound { .. })
                {
                    warn!(%streamer_id, %download_id, %error, "Failed to request download stop during retirement");
                }
            } else {
                match tokio::time::timeout(
                    bounds.download,
                    self.deps.download_manager.stop_download_with_reason(
                        download_id,
                        DownloadStopCause::StreamerDisabled,
                    ),
                )
                .await
                {
                    Ok(Ok(())) | Ok(Err(crate::Error::NotFound { .. })) => {}
                    Ok(Err(error)) => {
                        warn!(%streamer_id, %download_id, %error, "Download stop failed during retirement");
                    }
                    Err(_) => {
                        warn!(%streamer_id, %download_id, "Download stop timed out during retirement");
                    }
                }
            }
        }

        // 3. Stop any remaining danmu collection for this streamer.
        if let Some(danmu_session_id) = self.deps.danmu_service.get_session_by_streamer(streamer_id)
        {
            if bounds.download.is_zero() {
                let danmu_service = self.deps.danmu_service.clone();
                let sid = danmu_session_id.clone();
                self.reconcile_tasks
                    .spawn("recorder observe-retirement danmu stop", async move {
                        let _ = danmu_service
                            .stop_collection_with_outcome(&sid, CollectionStopReason::SessionEnded)
                            .await;
                    });
            } else {
                match tokio::time::timeout(
                    bounds.download,
                    self.deps.danmu_service.stop_collection_with_outcome(
                        &danmu_session_id,
                        CollectionStopReason::SessionEnded,
                    ),
                )
                .await
                {
                    Ok(DanmuStopResult::Terminated | DanmuStopResult::NotRegistered) => {}
                    Ok(DanmuStopResult::Unconfirmed { error }) => {
                        warn!(%streamer_id, session_id = %danmu_session_id, %error, "Danmu stop unconfirmed during retirement");
                    }
                    Err(_) => {
                        warn!(%streamer_id, session_id = %danmu_session_id, "Danmu stop timed out during retirement");
                    }
                }
            }
        }

        // 4. Wait for any in-flight RecorderStore commit lease for this streamer.
        if self.deps.store.has_in_flight_commit(streamer_id) && !bounds.download.is_zero() {
            let _ = tokio::time::timeout(
                bounds.download,
                self.deps.store.wait_for_streamer_quiescence(streamer_id),
            )
            .await;
        }

        // 5. If physical download + danmu are clear, ensure any still-open SQLite session is closed
        //    (covers crash recovery or dormant streamers that had no resident actor).
        let download_busy = self.deps.leases.has_in_flight_for_streamer(streamer_id)
            || self
                .deps
                .download_manager
                .has_pending_for_streamer(streamer_id)
            || self
                .deps
                .download_manager
                .has_any_active_download_for_streamer(streamer_id);
        let danmu_busy = self
            .deps
            .danmu_service
            .has_active_collection_for_streamer(streamer_id);

        if !download_busy
            && !danmu_busy
            && !self.actors.contains_key(streamer_id)
            && !self.deps.store.has_in_flight_commit(streamer_id)
            && let Ok(Some(open_session_id)) = self.deps.store.active_session_id(streamer_id).await
        {
            match self
                .deps
                .store
                .commit(DbCommitOp::EndSession {
                    streamer_id: streamer_id.to_owned(),
                    streamer_name: streamer_name.to_owned(),
                    session_id: Some(open_session_id.clone()),
                    cause: TerminalCause::UserDisabled,
                    via_hysteresis: false,
                    update_streamer_state: None,
                    clear_errors: false,
                    checks_restored_threshold: None,
                    infra_block: None,
                    suppress_offline_notification: true,
                    now: Timestamp::now(),
                })
                .await
            {
                Ok(DbCommitOutcome::SessionEnded {
                    session_id: Some(sid),
                    ..
                }) => {
                    deliver_pipeline_notification(
                        &self.deps.pipeline_manager,
                        &self.deps.download_manager,
                        &self.deps.leases,
                        PipelineNotification::SessionEnded {
                            session_id: sid,
                            streamer_id: streamer_id.to_owned(),
                            cause: TerminalCause::UserDisabled,
                            should_run_session_complete: false,
                        },
                    )
                    .await;
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(%streamer_id, ?error, "Failed to close residual session during retirement");
                    outstanding.push(format!("session close: {error:?}"));
                }
            }
        }

        // 6. Final strict 4-owner verification (never bypassed by stop deadlines).
        let final_outstanding = self
            .outstanding_physical_owners(streamer_id, bounds.pipeline)
            .await;
        for owner in final_outstanding {
            if !outstanding.contains(&owner) {
                outstanding.push(owner);
            }
        }

        if outstanding.is_empty() {
            self.deps.prober.credential_blocks().clear(streamer_id);
            self.runtime_views.remove(streamer_id);
        }

        StreamerRetirement { outstanding }
    }

    /// Mark, retire, and reap `streamer_id` in one call.
    ///
    /// Commits `streamers.deleted_at` before waiting on any runtime owner. If
    /// retirement does not settle within `bounds`, the marker remains for the
    /// periodic streamer reaper sweep. Returns `Ok(false)` when no row was
    /// marked (unknown streamer or concurrent delete).
    pub(crate) async fn delete_streamer(
        &self,
        streamer_id: &str,
        bounds: RetirementBounds,
    ) -> crate::Result<bool> {
        let Some(marked) = self
            .deps
            .streamer_manager
            .mark_deleting(streamer_id)
            .await?
        else {
            return Ok(false);
        };

        self.retire_and_reap(streamer_id, &marked.name, bounds)
            .await?;
        Ok(true)
    }

    /// Retire a streamer already carrying `deleted_at` and reap its row once
    /// every physical owner is quiescent.
    pub(crate) async fn retire_and_reap(
        &self,
        streamer_id: &str,
        streamer_name: &str,
        bounds: RetirementBounds,
    ) -> crate::Result<bool> {
        let retirement = self
            .retire_streamer(streamer_id, streamer_name, bounds)
            .await;

        if !retirement.is_quiescent() {
            info!(
                streamer_id,
                outstanding = %retirement.outstanding.join("; "),
                "Streamer stays marked deleted until its remaining work finishes"
            );
            return Ok(false);
        }

        self.deps.streamer_manager.reap_deleted(streamer_id).await
    }

    /// Retire and reap every streamer row carrying a `deleted_at` marker.
    ///
    /// Reports how many rows were physically removed.
    pub(crate) async fn reap_marked_streamers(&self, bounds: RetirementBounds) -> usize {
        let mut reaped = 0;
        for metadata in self.deps.streamer_manager.get_pending_deletion() {
            match self
                .retire_and_reap(&metadata.id, &metadata.name, bounds)
                .await
            {
                Ok(true) => reaped += 1,
                Ok(false) => {}
                Err(error) => warn!(
                    streamer_id = %metadata.id,
                    error = %error,
                    "Failed to reap a deleted streamer"
                ),
            }
        }
        reaped
    }

    /// Refresh resolved `offline_check_*` fields on cached streamer metadata
    /// with bounded concurrency.
    pub(crate) async fn refresh_metadata_offline_checks(
        &self,
        streamer_ids: impl IntoIterator<Item = String>,
    ) {
        const MAX_CONCURRENT_CONFIG_REFRESHES: usize = 16;
        futures::stream::iter(streamer_ids)
            .map(|id| async move {
                self.refresh_metadata_offline_check(&id).await;
            })
            .buffer_unordered(MAX_CONCURRENT_CONFIG_REFRESHES)
            .for_each(|()| async {})
            .await;
    }

    /// Refresh resolved `offline_check_*` fields for a single streamer's cached metadata.
    pub(crate) async fn refresh_metadata_offline_check(&self, streamer_id: &str) {
        match self
            .deps
            .config_service
            .get_config_for_streamer(streamer_id)
            .await
        {
            Ok(merged) => self
                .deps
                .streamer_manager
                .apply_resolved_config(streamer_id, &merged),
            Err(error) => debug!(
                streamer_id,
                error = %error,
                "Skipping resolved configuration refresh"
            ),
        }
    }

    /// Settle an active session bound to a retiring credential profile before
    /// `reap_retired_profiles` may physically delete the profile row.
    ///
    /// Never ends the session in SQLite while any download or danmu resource for
    /// the captured session remains active or unconfirmed.
    pub(crate) async fn settle_credential_retirement(&self, streamer_id: &str) {
        let (session_id, binding) = match self
            .deps
            .store
            .retiring_credential_session(streamer_id)
            .await
        {
            Ok(Some(session)) => session,
            Ok(None) => return,
            Err(error) => {
                warn!(%streamer_id, %error, "Could not inspect credential retirement");
                return;
            }
        };

        let streamer_name = self
            .deps
            .streamer_manager
            .get_streamer(streamer_id)
            .map(|metadata| metadata.name)
            .unwrap_or_default();

        if let Some(entry) = self.actors.get(streamer_id) {
            let _ = entry.handle.send_input(FsmInput::CredentialRetire {
                session_id: session_id.clone(),
                binding: binding.clone(),
            });
        }

        // Cancel any in-flight startup lease for this session.
        self.deps.leases.cancel_session(&session_id);

        // Wait for every active download belonging to the captured session to stop.
        let downloads: Vec<_> = self
            .deps
            .download_manager
            .active_download_ids_for_streamer(streamer_id)
            .into_iter()
            .filter(|(_, sid)| sid == &session_id)
            .collect();
        for (download_id, _) in downloads {
            match tokio::time::timeout(
                CREDENTIAL_RETIREMENT_STOP_TIMEOUT,
                self.deps
                    .download_manager
                    .stop_download_with_reason(&download_id, DownloadStopCause::StreamerDisabled),
            )
            .await
            {
                Ok(Ok(())) | Ok(Err(crate::Error::NotFound { .. })) => {}
                _ => {
                    warn!(
                        %streamer_id,
                        %session_id,
                        "Credential retirement awaits captured engine attempt termination"
                    );
                    return;
                }
            }
        }

        // Confirm danmu stopped before touching the DB session row.
        if self.deps.danmu_service.is_collecting(&session_id) {
            let outcome = self
                .deps
                .danmu_service
                .stop_collection_with_outcome(&session_id, CollectionStopReason::SessionEnded)
                .await;
            if matches!(outcome, DanmuStopResult::Unconfirmed { .. }) {
                warn!(
                    %streamer_id,
                    %session_id,
                    ?outcome,
                    "Credential retirement awaits danmu completion"
                );
                return;
            }
        }

        // Ensure any in-flight commit (including one initiated by the actor) settles first.
        self.deps
            .store
            .wait_for_streamer_quiescence(streamer_id)
            .await;

        // Idempotently finalize the captured session in SQLite if still open.
        match self
            .deps
            .store
            .commit(DbCommitOp::EndForCredentialRetirement {
                streamer_id: streamer_id.to_owned(),
                streamer_name,
                session_id: session_id.clone(),
                binding,
                via_hysteresis: false,
                now: Timestamp::now(),
            })
            .await
        {
            Ok(DbCommitOutcome::CredentialRetirementEnded { ended: true, .. }) => {
                deliver_pipeline_notification(
                    &self.deps.pipeline_manager,
                    &self.deps.download_manager,
                    &self.deps.leases,
                    PipelineNotification::SessionEnded {
                        session_id,
                        streamer_id: streamer_id.to_owned(),
                        cause: TerminalCause::UserDisabled,
                        should_run_session_complete: false,
                    },
                )
                .await;
            }
            Ok(_) => {}
            Err(error) => {
                warn!(
                    %streamer_id,
                    %session_id,
                    ?error,
                    "Credential retirement could not finalize captured session"
                );
            }
        }
    }

    /// Gracefully shut down all resident actors, startup leases, and the reconciler.
    pub(crate) async fn shutdown_until(&self, deadline: tokio::time::Instant) -> bool {
        self.shutting_down.store(true, Ordering::Release);
        self.feedback.mark_stopped();
        self.root_cancel.cancel();

        // Broadcast `Shutdown` to every resident actor (does NOT close open DB sessions).
        let handles: Vec<ActorHandle> = self
            .actors
            .iter()
            .map(|entry| entry.value().handle.clone())
            .collect();
        for handle in handles {
            let _ = handle.send_input(FsmInput::Shutdown);
        }

        let remaining = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .min(SHUTDOWN_ACTOR_WAIT);
        let leases_ok = self.deps.leases.shutdown(remaining).await;

        let remaining = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .max(Duration::from_millis(500));
        let actors_ok = self.actor_tasks.shutdown(remaining).await;
        let reconciler_ok = self
            .reconcile_tasks
            .shutdown(
                deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .max(Duration::from_millis(200)),
            )
            .await;

        leases_ok && actors_ok && reconciler_ok
    }

    /// Abort remaining actors and startup tasks after the shutdown force deadline.
    pub(crate) async fn abort_all(&self, hard_deadline: tokio::time::Instant) -> usize {
        self.shutting_down.store(true, Ordering::Release);
        self.feedback.mark_stopped();
        self.root_cancel.cancel();
        let (leases_aborted, actors_aborted, reconciler_aborted) = tokio::join!(
            self.deps.leases.abort_all(hard_deadline),
            self.actor_tasks.abort_all(hard_deadline),
            self.reconcile_tasks.abort_all(hard_deadline),
        );
        leases_aborted + actors_aborted + reconciler_aborted
    }

    async fn outstanding_physical_owners(
        &self,
        streamer_id: &str,
        pipeline_bound: Duration,
    ) -> Vec<String> {
        let mut outstanding = Vec::new();
        if self.actors.contains_key(streamer_id) {
            outstanding.push("recorder actor".to_string());
        }
        if self.deps.store.has_in_flight_commit(streamer_id) {
            outstanding.push("database commit lease".to_string());
        }
        if self.deps.leases.has_in_flight_for_streamer(streamer_id) {
            outstanding.push("download startup lease".to_string());
        }
        if self
            .deps
            .download_manager
            .has_pending_for_streamer(streamer_id)
        {
            outstanding.push("queued download".to_string());
        }
        if self
            .deps
            .download_manager
            .has_any_active_download_for_streamer(streamer_id)
        {
            outstanding.push("active download".to_string());
        }
        if self
            .deps
            .danmu_service
            .has_active_collection_for_streamer(streamer_id)
        {
            outstanding.push("danmu collection".to_string());
        }
        match self
            .deps
            .pipeline_manager
            .drain_for_streamer(streamer_id, pipeline_bound)
            .await
        {
            Ok(drain) if drain.is_drained() => {}
            Ok(drain) => {
                outstanding.push(format!(
                    "pipeline work: {} unsettled sessions, {} jobs",
                    drain.sessions.len(),
                    drain.jobs
                ));
            }
            Err(error) => {
                outstanding.push(format!("pipeline settlement: {error}"));
            }
        }
        outstanding
    }

    // ========== Async Reconciliation Loop ==========

    async fn run_reconciler(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<ReconcileCommand>) {
        loop {
            tokio::select! {
                _ = self.root_cancel.cancelled() => {
                    debug!("Recorder reconciler shutting down");
                    break;
                }
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else {
                        break;
                    };
                    self.handle_reconcile_command(cmd).await;
                }
            }
        }
    }

    async fn handle_reconcile_command(self: &Arc<Self>, cmd: ReconcileCommand) {
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        match cmd {
            ReconcileCommand::Batch(changes) => {
                let reconfigured: HashSet<&str> = changes
                    .reconfigured_streamers
                    .iter()
                    .map(String::as_str)
                    .collect();
                for streamer_id in &changes.removed_streamers {
                    self.deps.prober.credential_blocks().clear(streamer_id);
                    self.runtime_views.remove(streamer_id);
                    if let Some(entry) = self.actors.get(streamer_id) {
                        let _ = entry.handle.send_input(FsmInput::Disable);
                    }
                }
                let mut seen = HashSet::new();
                for streamer_id in &changes.updated_streamers {
                    if seen.insert(streamer_id.as_str()) {
                        let is_reconf = reconfigured.contains(streamer_id.as_str());
                        self.reconcile_streamer(streamer_id, is_reconf).await;
                    }
                }
                for streamer_id in &changes.reconfigured_streamers {
                    if seen.insert(streamer_id.as_str()) {
                        self.reconcile_streamer(streamer_id, true).await;
                    }
                }
            }
            ReconcileCommand::Streamer {
                streamer_id,
                reconfigured,
            } => {
                self.reconcile_streamer(&streamer_id, reconfigured).await;
            }
            ReconcileCommand::AllStreamers { reconfigured } => {
                let check_delay = self
                    .deps
                    .config_service
                    .get_cached_global_config()
                    .await
                    .ok()
                    .and_then(|global| u64::try_from(global.streamer_check_delay_ms).ok());
                if let Some(ms) = check_delay {
                    self.set_check_interval_ms(ms);
                }
                let all_ids: Vec<String> = self
                    .deps
                    .streamer_manager
                    .get_all()
                    .into_iter()
                    .map(|metadata| metadata.id)
                    .collect();
                for streamer_id in all_ids {
                    self.reconcile_streamer(&streamer_id, reconfigured).await;
                }
                // Stop actors whose streamer vanished from active set.
                let resident_ids: Vec<String> = self
                    .actors
                    .iter()
                    .map(|entry| entry.key().clone())
                    .collect();
                for streamer_id in resident_ids {
                    if self
                        .deps
                        .streamer_manager
                        .get_streamer_snapshot(&streamer_id)
                        .is_none()
                    {
                        self.reconcile_streamer(&streamer_id, false).await;
                    }
                }
            }
            ReconcileCommand::Platform(platform_id) => {
                let affected: Vec<String> = self
                    .deps
                    .streamer_manager
                    .get_all()
                    .into_iter()
                    .filter(|metadata| metadata.platform_config_id == platform_id)
                    .map(|metadata| metadata.id)
                    .collect();
                for streamer_id in affected {
                    self.reconcile_streamer(&streamer_id, true).await;
                    self.notify_credential_material_changed(&streamer_id);
                }
            }
            ReconcileCommand::Template(template_id) => {
                let affected: Vec<String> = self
                    .deps
                    .streamer_manager
                    .get_all()
                    .into_iter()
                    .filter(|metadata| {
                        metadata.template_config_id.as_deref() == Some(template_id.as_str())
                    })
                    .map(|metadata| metadata.id)
                    .collect();
                for streamer_id in affected {
                    self.reconcile_streamer(&streamer_id, true).await;
                    self.notify_credential_material_changed(&streamer_id);
                }
            }
            ReconcileCommand::CredentialMaterial(owner) => {
                let affected = self.streamers_for_credential_owner(&owner);
                for streamer_id in affected {
                    self.reconcile_streamer(&streamer_id, true).await;
                    self.notify_credential_material_changed(&streamer_id);
                }
            }
            ReconcileCommand::ErrorCleared(streamer_id) => {
                self.reconcile_streamer(&streamer_id, false).await;
                if let Some(entry) = self.actors.get(&streamer_id) {
                    let _ = entry.handle.send_input(FsmInput::ErrorCleared);
                }
            }
            ReconcileCommand::FiltersUpdated(streamer_id) => {
                self.reconcile_streamer(&streamer_id, true).await;
                if let Some(entry) = self.actors.get(&streamer_id) {
                    let _ = entry.handle.send_input(FsmInput::CheckNow);
                }
            }
            ReconcileCommand::CheckNow(streamer_id) => {
                if let Some(entry) = self.actors.get(&streamer_id) {
                    let _ = entry.handle.send_input(FsmInput::CheckNow);
                }
            }
            ReconcileCommand::RetryAdmission(streamer_id) => {
                if let Some(entry) = self.actors.get(&streamer_id) {
                    let _ = entry.handle.send_input(FsmInput::RetryAdmission);
                }
            }
        }
    }

    async fn reconcile_streamer(self: &Arc<Self>, streamer_id: &str, reconfigured: bool) {
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let Some(metadata) = self
            .deps
            .streamer_manager
            .get_streamer_snapshot(streamer_id)
        else {
            // Streamer removed from cache: clean up any lingering actor.
            self.deps.prober.credential_blocks().clear(streamer_id);
            if let Some(entry) = self.actors.get(streamer_id) {
                let _ = entry.handle.send_input(FsmInput::Disable);
            }
            return;
        };

        if metadata.is_deleted() {
            // Deletion retirement owns stopping this streamer via `retire_streamer`.
            self.deps.prober.credential_blocks().clear(streamer_id);
            return;
        }

        if should_spawn_actor(metadata.state, metadata.is_deleted()) {
            if let Some(entry) = self.actors.get(streamer_id) {
                if reconfigured {
                    let handle = entry.handle.clone();
                    drop(entry);
                    self.push_resolved_config(streamer_id, &metadata, &handle)
                        .await;
                    let _ = handle.send_input(FsmInput::CredentialMaterialChanged);
                }
                return;
            }
            // Wait for any previous generation's committed DB write before spawning generation G+1.
            self.deps
                .store
                .wait_for_streamer_quiescence(streamer_id)
                .await;
            // Re-read authoritative snapshot after quiescence wait.
            let Some(fresh) = self
                .deps
                .streamer_manager
                .get_streamer_snapshot(streamer_id)
            else {
                return;
            };
            if !should_spawn_actor(fresh.state, fresh.is_deleted())
                || self.actors.contains_key(streamer_id)
                || self.shutting_down.load(Ordering::Acquire)
            {
                return;
            }
            self.spawn_actor(fresh).await;
            return;
        }

        // Inactive states (`Disabled`, `OutOfSpace`, `NotFound`, `FatalError`, `Cancelled`).
        // A live actor that itself transitioned into `OutOfSpace` stays alive in
        // `OutputRootCooldown` until cooldown expires or the output-root gate recovers;
        // persisted `OutOfSpace` at startup has no actor and stays dormant until recovery.
        if metadata.state == StreamerState::OutOfSpace {
            return;
        }

        self.deps.prober.credential_blocks().clear(streamer_id);
        if let Some(entry) = self.actors.get(streamer_id) {
            let _ = entry.handle.send_input(FsmInput::Disable);
        }
    }

    async fn spawn_actor(self: &Arc<Self>, metadata: Arc<StreamerMetadata>) {
        let streamer_id = metadata.id.clone();
        let revision = self.next_config_revision(&streamer_id);
        let config = self.resolve_actor_config(&streamer_id, &metadata).await;
        let snapshot = InitialStreamerSnapshot {
            streamer_id: streamer_id.clone(),
            state: metadata.state,
            deleted: metadata.is_deleted(),
            consecutive_error_count: metadata.consecutive_error_count,
            disabled_until: metadata.disabled_until,
            last_error: metadata.last_error.clone(),
            config,
            config_revision: revision,
        };
        let generation = self.generation_counter.fetch_add(1, Ordering::AcqRel);
        let (actor, handle) = StreamerActor::new(generation, snapshot, self.deps.clone());
        let (exit_tx, exit_rx) = watch::channel(false);

        self.actors.insert(
            streamer_id.clone(),
            ActorEntry {
                generation,
                handle,
                exit_rx,
            },
        );

        let registry = self.clone();
        let task_streamer_id = streamer_id.clone();
        let task_exit_tx = exit_tx.clone();
        let spawned = self
            .actor_tasks
            .spawn("recorder streamer actor", async move {
                let outcome = AssertUnwindSafe(actor.run()).catch_unwind().await;
                registry.on_actor_exit(&task_streamer_id, generation, outcome, task_exit_tx);
            });
        if !spawned {
            self.actors
                .remove_if(&streamer_id, |_, entry| entry.generation == generation);
            let _ = exit_tx.send(true);
        }
    }

    fn on_actor_exit(
        self: &Arc<Self>,
        streamer_id: &str,
        generation: u64,
        outcome: std::result::Result<ActorStopReason, Box<dyn std::any::Any + Send>>,
        exit_tx: watch::Sender<bool>,
    ) {
        // Anti-ghost fence: only remove the registry entry and runtime view if
        // this exiting generation is still the installed one.
        let removed = self
            .actors
            .remove_if(streamer_id, |_, entry| entry.generation == generation)
            .is_some();
        if removed {
            self.runtime_views.remove(streamer_id);
        }
        let _ = exit_tx.send(true);

        match outcome {
            Ok(reason) => {
                self.crash_counts.remove(streamer_id);
                if matches!(
                    reason,
                    ActorStopReason::Disabled
                        | ActorStopReason::FatalError { .. }
                        | ActorStopReason::Retired { .. }
                ) {
                    self.deps.prober.credential_blocks().clear(streamer_id);
                }
            }
            Err(_) => {
                warn!(
                    %streamer_id,
                    generation,
                    "StreamerActor panicked; scheduling supervised restart if still active"
                );
                if self.shutting_down.load(Ordering::Acquire) {
                    return;
                }
                let attempts = {
                    let mut count = self.crash_counts.entry(streamer_id.to_owned()).or_insert(0);
                    *count = count.saturating_add(1);
                    *count
                };
                if attempts > MAX_CRASH_RESTARTS {
                    warn!(
                        %streamer_id,
                        attempts,
                        "StreamerActor exceeded crash restart budget"
                    );
                    return;
                }
                let backoff = CRASH_RESTART_BASE_BACKOFF
                    .saturating_mul(
                        1u32.checked_shl(attempts.saturating_sub(1))
                            .unwrap_or(u32::MAX),
                    )
                    .min(CRASH_RESTART_MAX_BACKOFF);
                let tx = self.reconcile_tx.clone();
                let cancel = self.root_cancel.child_token();
                let id = streamer_id.to_owned();
                self.reconcile_tasks
                    .spawn("recorder crash restart", async move {
                        tokio::select! {
                            _ = cancel.cancelled() => {}
                            _ = tokio::time::sleep(backoff) => {
                                let _ = tx.send(ReconcileCommand::Streamer {
                                    streamer_id: id,
                                    reconfigured: false,
                                });
                            }
                        }
                    });
            }
        }
    }

    fn next_config_revision(&self, streamer_id: &str) -> u64 {
        let counter = self
            .config_revisions
            .entry(streamer_id.to_owned())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone();
        counter.fetch_add(1, Ordering::AcqRel).saturating_add(1)
    }

    async fn resolve_actor_config(
        &self,
        streamer_id: &str,
        metadata: &StreamerMetadata,
    ) -> ResolvedActorConfig {
        let check_interval_ms = self.check_interval_ms.load(Ordering::Acquire);
        match self
            .deps
            .config_service
            .get_config_for_streamer(streamer_id)
            .await
        {
            Ok(merged) => {
                self.deps
                    .streamer_manager
                    .apply_resolved_config(streamer_id, &merged);
                ResolvedActorConfig::from_metadata_and_merged(metadata, &merged, check_interval_ms)
            }
            Err(error) => {
                warn!(
                    %streamer_id,
                    %error,
                    "Failed to resolve merged config for actor; using metadata defaults"
                );
                ResolvedActorConfig {
                    streamer_name: metadata.name.clone(),
                    streamer_url: metadata.url.clone(),
                    platform: metadata.platform().to_owned(),
                    avatar_url: metadata.avatar_url.clone(),
                    check_interval_ms,
                    offline_check_interval_ms: metadata.offline_check_delay_ms,
                    offline_check_count: metadata.offline_check_count,
                    priority: metadata.priority,
                    ..ResolvedActorConfig::default()
                }
            }
        }
    }

    async fn push_resolved_config(
        &self,
        streamer_id: &str,
        metadata: &StreamerMetadata,
        handle: &ActorHandle,
    ) {
        // Allocate revision strictly BEFORE awaiting async config resolution so
        // an older slow resolution cannot overwrite a newer revision in the FSM.
        let revision = self.next_config_revision(streamer_id);
        let config = self.resolve_actor_config(streamer_id, metadata).await;
        let _ = handle.send_input(FsmInput::ConfigUpdated {
            revision,
            config: Box::new(config),
        });
    }

    fn notify_credential_material_changed(&self, streamer_id: &str) {
        if let Some(entry) = self.actors.get(streamer_id) {
            let _ = entry.handle.send_input(FsmInput::CredentialMaterialChanged);
        }
    }

    fn streamers_for_credential_owner(&self, owner: &CredentialOwner) -> Vec<String> {
        let all = self.deps.streamer_manager.get_all();
        match owner {
            CredentialOwner::Platform { platform_id } => all
                .into_iter()
                .filter(|metadata| metadata.platform_config_id == *platform_id)
                .map(|metadata| metadata.id)
                .collect(),
            CredentialOwner::Template { template_id } => all
                .into_iter()
                .filter(|metadata| {
                    metadata.template_config_id.as_deref() == Some(template_id.as_str())
                })
                .map(|metadata| metadata.id)
                .collect(),
            CredentialOwner::Streamer { streamer_id } => vec![streamer_id.clone()],
        }
    }

    // ========== Download & Danmu Coordination Bridges ==========

    fn deliver_reserved_download_event(
        &self,
        event: &DownloadManagerEvent,
        permit: RecorderFeedbackPermit,
    ) {
        let streamer_id = event.streamer_id();
        if permit.streamer_id() != streamer_id {
            warn!(
                permit_streamer = permit.streamer_id(),
                event_streamer = streamer_id,
                "Reserved download permit belonged to another streamer"
            );
            return;
        }
        match event {
            DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
                download_id,
                session_id,
                engine_type,
                ..
            }) => {
                let (attempt_id, protocol) = self
                    .deps
                    .leases
                    .attempt_and_protocol_for_download(download_id)
                    .unwrap_or_else(|| {
                        let id = self
                            .deps
                            .leases
                            .attempt_for_session(streamer_id, session_id)
                            .unwrap_or(0);
                        (id, crate::downloader::DownloadProtocol::Unknown)
                    });
                let input = FsmInput::AttemptStarted {
                    attempt_id,
                    session_id: session_id.clone(),
                    download_id: download_id.clone(),
                    engine_type: *engine_type,
                    protocol,
                };
                if let Some(entry) = self.actors.get(streamer_id) {
                    let _ = entry.handle.send_message(ActorMessage::ReservedDownload {
                        input,
                        _permit: permit,
                    });
                }
            }
            DownloadManagerEvent::Terminal(terminal) => {
                let attempt_id = terminal
                    .download_id()
                    .and_then(|download_id| self.deps.leases.take_attempt_for_download(download_id))
                    .or_else(|| {
                        self.deps
                            .leases
                            .attempt_for_session(streamer_id, terminal.session_id())
                    })
                    .unwrap_or(0);
                let input = FsmInput::DownloadEnded {
                    attempt_id,
                    event: terminal.clone(),
                };
                if let Some(entry) = self.actors.get(streamer_id) {
                    let _ = entry.handle.send_message(ActorMessage::ReservedDownload {
                        input,
                        _permit: permit,
                    });
                    if !self.has_physical_media_lease(streamer_id) {
                        let _ = entry.handle.send_input(FsmInput::PhysicalLeaseReleased);
                    }
                }
            }
            _ => {}
        }
    }

    fn has_physical_media_lease(&self, streamer_id: &str) -> bool {
        self.deps.leases.has_in_flight_for_streamer(streamer_id)
            || self
                .deps
                .download_manager
                .has_pending_for_streamer(streamer_id)
            || self
                .deps
                .download_manager
                .has_any_active_download_for_streamer(streamer_id)
            || self
                .deps
                .danmu_service
                .has_active_collection_for_streamer(streamer_id)
    }

    async fn run_download_observer(
        self: Arc<Self>,
        mut receiver: broadcast::Receiver<DownloadManagerEvent>,
        cancel: CancellationToken,
    ) {
        // Per-download heartbeat throttle + edge detector for sustained recovery.
        let mut heartbeat_state: HashMap<String, (Instant, bool)> = HashMap::new();
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    debug!("Recorder download progress observer shutting down");
                    break;
                }
                result = receiver.recv() => {
                    match result {
                        Ok(DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadQueued {
                            streamer_id,
                            session_id,
                            ..
                        })) => {
                            let attempt_id = self
                                .deps
                                .leases
                                .mark_queued(&streamer_id, &session_id)
                                .unwrap_or(0);
                            if let Some(entry) = self.actors.get(&streamer_id) {
                                let _ = entry.handle.send_input(FsmInput::AttemptQueued {
                                    attempt_id,
                                    session_id,
                                });
                            }
                        }
                        Ok(DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadDequeued {
                            streamer_id,
                            session_id,
                            ..
                        })) => {
                            let attempt_id = self
                                .deps
                                .leases
                                .mark_dequeued(&streamer_id, &session_id)
                                .unwrap_or(0);
                            if let Some(entry) = self.actors.get(&streamer_id) {
                                let _ = entry.handle.send_input(FsmInput::AttemptDequeued {
                                    attempt_id,
                                    session_id,
                                });
                            }
                        }
                        Ok(DownloadManagerEvent::Progress(DownloadProgressEvent::Progress {
                            download_id,
                            streamer_id,
                            session_id,
                            progress,
                            ..
                        })) => {
                            let now = Instant::now();
                            let recovered = should_record_recovery_from_progress(&progress);
                            let should_emit = match heartbeat_state.get(&download_id) {
                                None => true,
                                Some((last_sent, prev_recovered)) => {
                                    (recovered && !*prev_recovered)
                                        || now.saturating_duration_since(*last_sent) >= HEARTBEAT_MIN_INTERVAL
                                }
                            };
                            if should_emit {
                                heartbeat_state.insert(download_id.clone(), (now, recovered));
                                if let Some(entry) = self.actors.get(&streamer_id) {
                                    let _ = entry.handle.send_input(FsmInput::DownloadHeartbeat {
                                        session_id,
                                        download_id,
                                        progress: Some(progress),
                                    });
                                }
                            }
                        }
                        Ok(DownloadManagerEvent::Terminal(terminal)) => {
                            if let Some(download_id) = terminal.download_id() {
                                heartbeat_state.remove(download_id);
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!(skipped, "Recorder download progress observer lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    }

    async fn run_download_coordination(
        self: Arc<Self>,
        mut receiver: DownloadCoordinationReceiver,
    ) -> std::result::Result<(), String> {
        while let Some(delivery) = receiver
            .recv_event()
            .await
            .map_err(|error| error.to_string())?
        {
            let (event, acknowledgement) = delivery.into_parts();
            let result = self.handle_required_download_event(event).await;
            let ack_result = result.as_ref().map(|_| ()).map_err(ToString::to_string);
            if acknowledgement.send(ack_result).is_err() {
                debug!("Recorder download coordination ack receiver was dropped");
            }
            if let Err(error) = result {
                warn!(
                    %error,
                    "Recorder download coordination event failed; publishing attempt was notified"
                );
            }
        }
        debug!("Recorder download coordination drained and shut down");
        Ok(())
    }

    async fn handle_required_download_event(
        &self,
        event: DownloadManagerEvent,
    ) -> std::result::Result<(), String> {
        match event {
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
                download_id,
                streamer_id,
                session_id,
                segment_path,
                segment_index,
                started_at,
                ..
            }) => {
                let attempt_id = self
                    .deps
                    .leases
                    .attempt_for_download(&download_id)
                    .or_else(|| {
                        self.deps
                            .leases
                            .attempt_for_session(&streamer_id, &session_id)
                    })
                    .unwrap_or(0);
                if let Some(handle) = self.actors.get(&streamer_id).map(|e| e.handle.clone()) {
                    let (ack_tx, ack_rx) = oneshot::channel();
                    let input = FsmInput::SegmentStarted {
                        attempt_id,
                        session_id: session_id.clone(),
                        download_id,
                        segment_index,
                        segment_path: segment_path.clone(),
                        started_at,
                    };
                    if handle.send_message(ActorMessage::SegmentStarted { input, ack: ack_tx })
                        && let Ok(res) = ack_rx.await
                    {
                        return res;
                    }
                }
                // Actor already stopped/retired; still keep danmu segment aligned if open.
                start_danmu_segment(
                    &self.deps.danmu_service,
                    &session_id,
                    segment_index,
                    &segment_path,
                    started_at,
                )
                .await;
                Ok(())
            }
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
                download_id,
                streamer_id,
                session_id,
                segment_path,
                segment_index,
                started_at,
                completed_at,
                duration_secs,
                size_bytes,
                split_reason_code,
                split_reason_details_json,
                split_by_format_change,
                ..
            }) => {
                let effective_size_bytes = tokio::fs::metadata(&segment_path)
                    .await
                    .map_or(size_bytes, |meta| meta.len());
                let attempt_id = self
                    .deps
                    .leases
                    .attempt_for_download(&download_id)
                    .or_else(|| {
                        self.deps
                            .leases
                            .attempt_for_session(&streamer_id, &session_id)
                    })
                    .unwrap_or(0);

                if let Some(handle) = self.actors.get(&streamer_id).map(|e| e.handle.clone()) {
                    let (ack_tx, ack_rx) = oneshot::channel();
                    let input = FsmInput::SegmentCompleted {
                        attempt_id,
                        session_id: session_id.clone(),
                        download_id: download_id.clone(),
                        segment_index,
                        segment_path: segment_path.clone(),
                        started_at,
                        completed_at,
                        duration_secs,
                        size_bytes,
                        effective_size_bytes,
                        split_reason_code: split_reason_code.clone(),
                        split_reason_details_json: split_reason_details_json.clone(),
                        split_by_format_change,
                    };
                    if handle.send_message(ActorMessage::SegmentCompleted { input, ack: ack_tx })
                        && let Ok(res) = ack_rx.await
                    {
                        return res;
                    }
                }

                // Session-level segment persistence decoupled from Actor lifetime:
                // trailing segments from stale attempts after actor exit still
                // respect min-size discard, close danmu, persist to SQLite, and
                // notify the pipeline.
                self.persist_trailing_video_segment(
                    &streamer_id,
                    &session_id,
                    segment_index,
                    segment_path,
                    started_at,
                    completed_at,
                    duration_secs,
                    size_bytes,
                    effective_size_bytes,
                    split_reason_code,
                    split_reason_details_json,
                    split_by_format_change,
                )
                .await
            }
            DownloadManagerEvent::Terminal(_) => {
                // All preceding `SegmentStarted` and `SegmentCompleted` receipts
                // have already settled before `AttemptTask` reaches this barrier.
                // `DownloadEnded` is delivered strictly after `finalizer.release_active()`
                // via `RecorderFeedbackSink::publish_reserved`.
                Ok(())
            }
            DownloadManagerEvent::Progress(_) => Ok(()),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "trailing segment persistence mirrors every explicit SegmentCompleted field"
    )]
    async fn persist_trailing_video_segment(
        &self,
        streamer_id: &str,
        session_id: &str,
        segment_index: u32,
        segment_path: String,
        started_at: Option<Timestamp>,
        completed_at: Timestamp,
        duration_secs: f64,
        size_bytes: u64,
        effective_size_bytes: u64,
        split_reason_code: Option<String>,
        split_reason_details_json: Option<String>,
        split_by_format_change: bool,
    ) -> std::result::Result<(), String> {
        let min_bytes = self
            .deps
            .config_service
            .get_config_for_streamer(streamer_id)
            .await
            .ok()
            .and_then(|config| u64::try_from(config.min_segment_size_bytes).ok())
            .unwrap_or(0);
        let discard =
            should_discard_segment(effective_size_bytes, min_bytes, split_by_format_change);

        if discard {
            self.discarded_segments_fallback
                .insert((session_id.to_owned(), segment_index), Instant::now());
            close_danmu_segment(
                &self.deps.danmu_service,
                &DanmuCloseSegmentSpec {
                    session_id: session_id.to_owned(),
                    segment_index,
                    discard: true,
                },
            )
            .await;
            let danmu_path = paired_danmu_path(&segment_path);
            delete_discarded_files(Some(&segment_path), Some(&danmu_path)).await;
            if let Err(error) = self
                .deps
                .store
                .remove_danmu_segment_output(session_id, &danmu_path)
                .await
            {
                warn!(%session_id, %danmu_path, %error, "Failed to remove discarded trailing danmu output");
            }
            return Ok(());
        }

        close_danmu_segment(
            &self.deps.danmu_service,
            &DanmuCloseSegmentSpec {
                session_id: session_id.to_owned(),
                segment_index,
                discard: false,
            },
        )
        .await;

        self.deps
            .store
            .persist_video_segment(VideoSegmentRecord {
                session_id: session_id.to_owned(),
                streamer_id: streamer_id.to_owned(),
                segment_index,
                segment_path: segment_path.clone(),
                started_at,
                completed_at,
                duration_secs,
                size_bytes,
                split_reason_code,
                split_reason_details_json,
            })
            .await
            .map_err(|error| format!("failed to persist trailing video segment: {error}"))?;

        self.deps
            .pipeline_manager
            .handle_recorder_notification(
                PipelineNotification::VideoSegmentCompleted {
                    session_id: session_id.to_owned(),
                    streamer_id: streamer_id.to_owned(),
                    segment_index,
                    path: Path::new(&segment_path).to_path_buf(),
                    auto_thumbnail_candidate: segment_index == 0,
                }
                .into(),
            )
            .await
            .map_err(|error| format!("failed to notify pipeline for trailing segment: {error}"))?;
        Ok(())
    }

    async fn run_danmu_coordination(
        self: Arc<Self>,
        mut receiver: DanmuCoordinationReceiver,
    ) -> std::result::Result<(), String> {
        let mut failures = FailureSamples::default();
        let acknowledgement = loop {
            match receiver.recv().await.map_err(|error| error.to_string())? {
                Delivery::Event(event) => {
                    if let Err(error) = self.handle_required_danmu_event(event).await {
                        warn!(%error, "Recorder danmu coordination event failed");
                        failures.push(error);
                    }
                }
                Delivery::Shutdown(acknowledgement) => break acknowledgement,
            }
        };
        if acknowledgement.send(failures.into_report()).is_err() {
            debug!("Recorder danmu coordination shutdown ack receiver was dropped");
        }
        debug!("Recorder danmu coordination drained and shut down");
        Ok(())
    }

    async fn handle_required_danmu_event(
        &self,
        event: DanmuEvent,
    ) -> std::result::Result<(), String> {
        match event {
            DanmuEvent::CollectionStarted {
                session_id,
                streamer_id,
            } => {
                self.danmu_session_streamers
                    .insert(session_id.clone(), streamer_id.clone());
                if let Some(res) = self
                    .send_danmu_fifo(
                        &streamer_id,
                        FsmInput::DanmuCollectionStarted {
                            session_id: session_id.clone(),
                        },
                    )
                    .await
                {
                    return res;
                }
                self.deps
                    .pipeline_manager
                    .handle_recorder_notification(
                        PipelineNotification::DanmuCollectionStarted {
                            session_id,
                            streamer_id,
                        }
                        .into(),
                    )
                    .await
                    .map_err(|error| format!("failed to apply danmu collection start: {error}"))
            }
            DanmuEvent::SegmentCompleted {
                session_id,
                streamer_id,
                segment_id,
                output_path,
                message_count,
            } => {
                let metadata = match tokio::fs::metadata(&output_path).await {
                    Ok(meta) => meta,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        debug!(
                            path = %output_path.display(),
                            "Danmu segment file no longer exists; skipping"
                        );
                        return Ok(());
                    }
                    Err(error) => {
                        return Err(format!(
                            "failed reading danmu segment metadata {}: {error}",
                            output_path.display()
                        ));
                    }
                };
                let Some(segment_index) = parse_segment_index_from_danmu(&segment_id, &output_path)
                else {
                    warn!(
                        %session_id,
                        %segment_id,
                        path = %output_path.display(),
                        "Failed to parse danmu segment index; skipping"
                    );
                    return Ok(());
                };
                let output_path_str = output_path.to_string_lossy().into_owned();
                if let Some(res) = self
                    .send_danmu_fifo(
                        &streamer_id,
                        FsmInput::DanmuSegmentCompleted {
                            session_id: session_id.clone(),
                            segment_index,
                            output_path: output_path_str.clone(),
                            message_count,
                            size_bytes: metadata.len(),
                        },
                    )
                    .await
                {
                    return res;
                }

                // Fallback after actor exit: honor fallback discard guard.
                if self
                    .discarded_segments_fallback
                    .remove(&(session_id.clone(), segment_index))
                    .is_some()
                {
                    delete_discarded_files(None, Some(&output_path_str)).await;
                    return Ok(());
                }

                self.deps
                    .store
                    .persist_danmu_segment(DanmuSegmentRecord {
                        session_id: session_id.clone(),
                        streamer_id: streamer_id.clone(),
                        segment_index,
                        output_path: output_path_str,
                        message_count,
                        size_bytes: metadata.len(),
                    })
                    .await
                    .map_err(|error| {
                        format!("failed to persist trailing danmu segment: {error}")
                    })?;

                self.deps
                    .pipeline_manager
                    .handle_recorder_notification(
                        PipelineNotification::DanmuSegmentCompleted {
                            session_id,
                            streamer_id,
                            segment_index,
                            path: output_path,
                            message_count,
                        }
                        .into(),
                    )
                    .await
                    .map_err(|error| {
                        format!("failed to notify pipeline for trailing danmu segment: {error}")
                    })
            }
            DanmuEvent::Control {
                session_id,
                streamer_id,
                control,
                ..
            } => {
                if let Some(res) = self
                    .send_danmu_fifo(
                        &streamer_id,
                        FsmInput::DanmuControl {
                            session_id: session_id.clone(),
                            control: control.clone(),
                        },
                    )
                    .await
                {
                    return res;
                }
                if let DanmuControlEvent::RoomInfoChanged {
                    title: Some(title), ..
                } = control
                    && let Err(error) = self
                        .deps
                        .store
                        .commit(DbCommitOp::AddSessionTitle {
                            session_id,
                            title,
                            now: Timestamp::now(),
                        })
                        .await
                {
                    warn!(%streamer_id, ?error, "Failed to persist trailing danmu title update");
                }
                Ok(())
            }
            DanmuEvent::Reconnecting {
                session_id,
                attempt,
            } => {
                if let Some(streamer_id) = self
                    .danmu_session_streamers
                    .get(&session_id)
                    .map(|entry| entry.value().clone())
                    && let Some(entry) = self.actors.get(&streamer_id)
                {
                    let _ = entry.handle.send_input(FsmInput::DanmuReconnecting {
                        session_id,
                        attempt,
                    });
                }
                Ok(())
            }
            DanmuEvent::Reconnected {
                session_id,
                attempts,
                downtime_secs,
            } => {
                if let Some(streamer_id) = self
                    .danmu_session_streamers
                    .get(&session_id)
                    .map(|entry| entry.value().clone())
                    && let Some(entry) = self.actors.get(&streamer_id)
                {
                    let _ = entry.handle.send_input(FsmInput::DanmuReconnected {
                        session_id,
                        attempts,
                        downtime_secs,
                    });
                }
                Ok(())
            }
            DanmuEvent::CollectionStopped {
                session_id,
                total_count,
            } => {
                self.discarded_segments_fallback
                    .retain(|(sid, _), _| sid != &session_id);
                let streamer_id = self
                    .danmu_session_streamers
                    .remove(&session_id)
                    .map(|(_, id)| id);
                if let Some(streamer_id) = streamer_id
                    && let Some(res) = self
                        .send_danmu_fifo(
                            &streamer_id,
                            FsmInput::DanmuCollectionStopped {
                                session_id: session_id.clone(),
                                total_count,
                                result: DanmuStopResult::Terminated,
                            },
                        )
                        .await
                {
                    if let Some(entry) = self.actors.get(&streamer_id)
                        && !self.has_physical_media_lease(&streamer_id)
                    {
                        let _ = entry.handle.send_input(FsmInput::PhysicalLeaseReleased);
                    }
                    return res;
                }
                self.deps
                    .pipeline_manager
                    .handle_recorder_notification(
                        PipelineNotification::DanmuCollectionStopped {
                            session_id,
                            total_count,
                        }
                        .into(),
                    )
                    .await
                    .map_err(|error| format!("failed to apply danmu collection stop: {error}"))
            }
            DanmuEvent::SegmentStarted { .. }
            | DanmuEvent::ReconnectFailed { .. }
            | DanmuEvent::Error { .. } => Ok(()),
        }
    }

    async fn send_danmu_fifo(
        &self,
        streamer_id: &str,
        input: FsmInput,
    ) -> Option<std::result::Result<(), String>> {
        let handle = self
            .actors
            .get(streamer_id)
            .map(|entry| entry.handle.clone())?;
        let (done_tx, done_rx) = oneshot::channel();
        if !handle.send_message(ActorMessage::DanmuFifo {
            input,
            done: done_tx,
        }) {
            return None;
        }
        done_rx.await.ok()
    }

    #[cfg(test)]
    pub(crate) fn spawn_dormant_actor_for_test(
        self: &Arc<Self>,
        streamer_id: &str,
        streamer_name: &str,
        streamer_url: &str,
    ) {
        let generation = self.generation_counter.fetch_add(1, Ordering::AcqRel);
        let snapshot = InitialStreamerSnapshot {
            streamer_id: streamer_id.to_owned(),
            state: StreamerState::TemporalDisabled,
            deleted: false,
            consecutive_error_count: 1,
            disabled_until: Timestamp::now()
                .checked_add(jiff::SignedDuration::from_hours(1))
                .ok(),
            last_error: None,
            config: ResolvedActorConfig {
                streamer_name: streamer_name.to_owned(),
                streamer_url: streamer_url.to_owned(),
                platform: "huya".to_owned(),
                check_interval_ms: 600_000,
                offline_check_interval_ms: 600_000,
                offline_check_count: 1,
                ..ResolvedActorConfig::default()
            },
            config_revision: 1,
        };
        let (actor, handle) = StreamerActor::new(generation, snapshot, self.deps.clone());
        let (exit_tx, exit_rx) = watch::channel(false);
        self.actors.insert(
            streamer_id.to_owned(),
            ActorEntry {
                generation,
                handle,
                exit_rx,
            },
        );
        let registry = self.clone();
        let task_streamer_id = streamer_id.to_owned();
        let task_exit_tx = exit_tx.clone();
        let spawned = self
            .actor_tasks
            .spawn("recorder test streamer actor", async move {
                let outcome = AssertUnwindSafe(actor.run()).catch_unwind().await;
                registry.on_actor_exit(&task_streamer_id, generation, outcome, task_exit_tx);
            });
        if !spawned {
            self.actors
                .remove_if(streamer_id, |_, entry| entry.generation == generation);
            let _ = exit_tx.send(true);
        }
    }

    #[cfg(test)]
    pub(crate) async fn reconcile_streamer_for_test(
        self: &Arc<Self>,
        streamer_id: &str,
        reconfigured: bool,
    ) {
        self.reconcile_streamer(streamer_id, reconfigured).await;
    }

    #[cfg(test)]
    pub(crate) fn startup_task_deps_for_test(&self) -> super::actor::startup_task::StartupTaskDeps {
        super::actor::startup_task::StartupTaskDeps {
            download_manager: self.deps.download_manager.clone(),
            danmu_service: self.deps.danmu_service.clone(),
            streamer_manager: self.deps.streamer_manager.clone(),
            config_service: self.deps.config_service.clone(),
            store: self.deps.store.clone(),
            prober: self.deps.prober.clone(),
            leases: self.deps.leases.clone(),
        }
    }
}

#[async_trait]
impl PostPublicationListener for ActorRegistry {
    async fn on_published(&self, changes: PostPublicationChangeSet) {
        if changes.is_empty() || self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let _ = self.reconcile_tx.send(ReconcileCommand::Batch(changes));
    }
}

impl BigoCheckTrigger for ActorRegistry {
    fn request_check(&self, streamer_id: &str) {
        self.check_now(streamer_id);
    }
}

fn parse_segment_index_from_danmu(segment_id: &str, output_path: &Path) -> Option<u32> {
    if let Ok(index) = segment_id.parse::<u32>() {
        return Some(index);
    }
    let stem = output_path.file_stem()?.to_str()?;
    let suffix = stem.rsplit('_').next()?;
    suffix.parse::<u32>().ok()
}

#[cfg(test)]
mod tests;
