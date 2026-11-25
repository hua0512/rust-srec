//! Supervised download startup pipeline and actor-independent resource leases.
//!
//! Each startup attempt registers a `StreamerResourceLease` in
//! [`StartupLeaseRegistry`] **before** running `Preflight`, eliminating any
//! visibility gap between `FsmEffect::StartAttempt` and `DownloadManager`'s
//! pending/active tracking. Lease cancellation and physical-quiescence queries
//! live on [`StartupLeaseRegistry`] independently of `StreamerActor`
//! lifetime.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use pipeline_common::expand_path_template;
use platforms_parser::extractor::platforms::bigo::native::stream::NativeRoom;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::ConfigService;
use crate::credentials::{CredentialBinding, CredentialSnapshot, ProfileError};
use crate::danmu::{CollectionSpec, DanmuService};
use crate::downloader::engine::EngineType;
use crate::downloader::{
    AcquireRequest, DownloadConfig, DownloadManager, DownloadProtocol, DownloadRejectedKind,
    PreflightFailure, PreflightRequest, Priority as QueuePriority,
};
use crate::recorder::actor::ActorMessage;
use crate::recorder::fsm::FsmInput;
use crate::recorder::ports::{
    AttemptStartFailure, AttemptStartSpec, DanmuStopResult, DbCommitFailure, DbCommitOp,
    DbCommitOutcome, ProbeFailure, ProbePurpose, ProbeRequest, ProbeStatus, RecorderStorePort,
};
use crate::recorder::prober::RecorderProber;
use crate::recorder::store::RecorderStore;
use crate::streamer::StreamerManager;
use srec_core::domain::{Priority, StreamerState};
use srec_core::utils::filename::{sanitize_filename_for_template, sanitize_title_for_template};
use srec_core::utils::task_supervisor::TaskSupervisor;
use srec_sqlite::repositories::config::SqlxConfigRepository;
use srec_sqlite::repositories::streamer::SqlxStreamerRepository;

struct ActiveStartupLease {
    attempt_id: u64,
    session_id: String,
    cancel: CancellationToken,
    queued_emitted: AtomicBool,
}

/// Actor-independent supervisor tracking every in-flight download startup task,
/// its cancellation token, and the `download_id -> attempt_id` mapping.
pub struct StartupLeaseRegistry {
    leases: DashMap<String, Vec<Arc<ActiveStartupLease>>>,
    attempt_by_download: DashMap<String, (u64, DownloadProtocol)>,
    session_attempts: DashMap<(String, String), u64>,
    lease_cleared: Notify,
    tasks: TaskSupervisor,
    root_cancel: CancellationToken,
}

impl StartupLeaseRegistry {
    /// Construct a new registry bound to `root_cancel`.
    pub fn new(root_cancel: CancellationToken) -> Self {
        Self {
            leases: DashMap::new(),
            attempt_by_download: DashMap::new(),
            session_attempts: DashMap::new(),
            lease_cleared: Notify::new(),
            tasks: TaskSupervisor::with_cancellation(root_cancel.clone()),
            root_cancel,
        }
    }

    /// Synchronously register a startup lease **before** spawning or running `Preflight`.
    pub fn register_lease(
        self: &Arc<Self>,
        streamer_id: String,
        session_id: String,
        attempt_id: u64,
        actor_tx: mpsc::UnboundedSender<ActorMessage>,
    ) -> StreamerResourceLease {
        let cancel = self.root_cancel.child_token();
        let entry = Arc::new(ActiveStartupLease {
            attempt_id,
            session_id: session_id.clone(),
            cancel: cancel.clone(),
            queued_emitted: AtomicBool::new(false),
        });
        self.leases
            .entry(streamer_id.clone())
            .or_default()
            .push(entry.clone());
        self.session_attempts
            .insert((streamer_id.clone(), session_id.clone()), attempt_id);
        StreamerResourceLease {
            streamer_id,
            session_id,
            attempt_id,
            cancel,
            entry,
            actor_tx,
            disarmed: false,
            registry: self.clone(),
        }
    }

    /// Mark that `DownloadQueued` was observed for `(streamer_id, session_id)` so
    /// abandoning the startup before engine start guarantees an `AttemptDequeued`
    /// signal on lease drop.
    pub(crate) fn mark_queued(&self, streamer_id: &str, session_id: &str) -> Option<u64> {
        self.leases
            .get(streamer_id)
            .and_then(|entries| {
                entries
                    .iter()
                    .rev()
                    .find(|entry| entry.session_id == session_id)
                    .map(|entry| {
                        entry.queued_emitted.store(true, Ordering::Release);
                        entry.attempt_id
                    })
            })
            .or_else(|| {
                self.session_attempts
                    .get(&(streamer_id.to_owned(), session_id.to_owned()))
                    .map(|entry| *entry)
            })
    }

    /// Clear the queued flag once `DownloadDequeued` has already been delivered.
    pub(crate) fn mark_dequeued(&self, streamer_id: &str, session_id: &str) -> Option<u64> {
        self.leases
            .get(streamer_id)
            .and_then(|entries| {
                entries
                    .iter()
                    .rev()
                    .find(|entry| entry.session_id == session_id)
                    .map(|entry| {
                        entry.queued_emitted.store(false, Ordering::Release);
                        entry.attempt_id
                    })
            })
            .or_else(|| {
                self.session_attempts
                    .get(&(streamer_id.to_owned(), session_id.to_owned()))
                    .map(|entry| *entry)
            })
    }

    /// Bind `download_id -> (attempt_id, protocol)` synchronously during `DownloadManager` admission.
    pub(crate) fn register_download_attempt(
        &self,
        download_id: &str,
        attempt_id: u64,
        protocol: DownloadProtocol,
    ) {
        self.attempt_by_download
            .insert(download_id.to_owned(), (attempt_id, protocol));
    }

    /// Look up the FSM `attempt_id` for a running or completing `download_id`.
    pub(crate) fn attempt_for_download(&self, download_id: &str) -> Option<u64> {
        self.attempt_by_download
            .get(download_id)
            .map(|entry| entry.0)
    }

    /// Look up `(attempt_id, protocol)` for a newly started `download_id`.
    pub(crate) fn attempt_and_protocol_for_download(
        &self,
        download_id: &str,
    ) -> Option<(u64, DownloadProtocol)> {
        self.attempt_by_download
            .get(download_id)
            .map(|entry| *entry)
    }

    /// Remove and return the `attempt_id` once a download reaches its terminal event.
    pub(crate) fn take_attempt_for_download(&self, download_id: &str) -> Option<u64> {
        self.attempt_by_download
            .remove(download_id)
            .map(|(_, (attempt_id, _))| attempt_id)
    }

    /// Look up the latest `attempt_id` registered for `(streamer_id, session_id)`.
    pub(crate) fn attempt_for_session(&self, streamer_id: &str, session_id: &str) -> Option<u64> {
        self.session_attempts
            .get(&(streamer_id.to_owned(), session_id.to_owned()))
            .map(|entry| *entry)
    }

    /// Clear `(streamer_id, session_id)` once a session ends.
    pub(crate) fn clear_session(&self, streamer_id: &str, session_id: &str) {
        self.session_attempts
            .remove(&(streamer_id.to_owned(), session_id.to_owned()));
    }

    /// Cancel every startup lease belonging to `streamer_id`.
    pub(crate) fn cancel_streamer(&self, streamer_id: &str) {
        if let Some(entries) = self.leases.get(streamer_id) {
            for entry in entries.iter() {
                entry.cancel.cancel();
            }
        }
    }

    /// Cancel a specific startup attempt by `attempt_id`.
    pub(crate) fn cancel_attempt(&self, streamer_id: &str, attempt_id: u64) {
        if let Some(entries) = self.leases.get(streamer_id) {
            for entry in entries.iter() {
                if entry.attempt_id == attempt_id {
                    entry.cancel.cancel();
                }
            }
        }
    }

    /// Cancel every startup lease belonging to `session_id`.
    pub(crate) fn cancel_session(&self, session_id: &str) {
        for entry in self.leases.iter() {
            for lease in entry.value().iter() {
                if lease.session_id == session_id {
                    lease.cancel.cancel();
                }
            }
        }
    }

    /// Returns `true` while any startup lease is registered for `session_id`.
    pub(crate) fn has_in_flight_for_session(&self, session_id: &str) -> bool {
        self.leases
            .iter()
            .any(|entry| entry.value().iter().any(|l| l.session_id == session_id))
    }

    /// Wait until all startup leases for `session_id` have unregistered or `timeout` elapses.
    pub(crate) async fn wait_for_session_cleared(
        &self,
        session_id: &str,
        timeout: Duration,
    ) -> bool {
        if !self.has_in_flight_for_session(session_id) {
            return true;
        }
        tokio::time::timeout(timeout, async {
            loop {
                let notified = self.lease_cleared.notified();
                if !self.has_in_flight_for_session(session_id) {
                    return;
                }
                notified.await;
            }
        })
        .await
        .is_ok()
    }

    /// Returns `true` while any startup lease is registered for `streamer_id`.
    pub(crate) fn has_in_flight_for_streamer(&self, streamer_id: &str) -> bool {
        self.leases
            .get(streamer_id)
            .is_some_and(|entries| !entries.is_empty())
    }

    /// Spawn a supervised startup future owned by this registry.
    pub(crate) fn spawn<F>(&self, task: F) -> bool
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.tasks.spawn("recorder startup task", task)
    }

    /// Cooperatively drain in-flight startup tasks up to `grace`.
    pub(crate) async fn shutdown(&self, grace: Duration) -> bool {
        self.root_cancel.cancel();
        self.tasks.shutdown(grace).await
    }

    /// Abort any remaining startup tasks after the shutdown force deadline.
    pub(crate) async fn abort_all(&self, hard_deadline: tokio::time::Instant) -> usize {
        self.root_cancel.cancel();
        self.tasks.abort_all(hard_deadline).await
    }

    fn unregister(&self, streamer_id: &str, entry: &Arc<ActiveStartupLease>) {
        self.leases.remove_if_mut(streamer_id, |_, entries| {
            entries.retain(|existing| !Arc::ptr_eq(existing, entry));
            entries.is_empty()
        });
        self.lease_cleared.notify_waiters();
    }
}

/// RAII lease bound to one supervised startup attempt.
///
/// Dropping this lease unregisters it from [`StartupLeaseRegistry`] and emits
/// [`FsmInput::AttemptDequeued`] if the attempt had queued and never started
/// an engine.
pub struct StreamerResourceLease {
    streamer_id: String,
    session_id: String,
    attempt_id: u64,
    cancel: CancellationToken,
    entry: Arc<ActiveStartupLease>,
    actor_tx: mpsc::UnboundedSender<ActorMessage>,
    disarmed: bool,
    registry: Arc<StartupLeaseRegistry>,
}

impl StreamerResourceLease {
    pub(crate) fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Disarm the `AttemptDequeued` fallback once the engine has started.
    pub(crate) fn disarm_dequeued_guard(&mut self) {
        self.disarmed = true;
        self.entry.queued_emitted.store(false, Ordering::Release);
    }
}

impl Drop for StreamerResourceLease {
    fn drop(&mut self) {
        if !self.disarmed && self.entry.queued_emitted.swap(false, Ordering::AcqRel) {
            let _ = self
                .actor_tx
                .send(ActorMessage::Input(FsmInput::AttemptDequeued {
                    attempt_id: self.attempt_id,
                    session_id: self.session_id.clone(),
                }));
        }
        self.registry.unregister(&self.streamer_id, &self.entry);
    }
}

/// Dependencies captured by each supervised startup task.
#[derive(Clone)]
pub struct StartupTaskDeps {
    pub download_manager: Arc<DownloadManager>,
    pub danmu_service: Arc<DanmuService>,
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    pub config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    pub store: Arc<RecorderStore>,
    pub prober: Arc<RecorderProber>,
    pub leases: Arc<StartupLeaseRegistry>,
}

/// Register a [`StreamerResourceLease`] synchronously and spawn the supervised
/// startup task on [`StartupLeaseRegistry`].
pub fn spawn_startup_task(
    deps: StartupTaskDeps,
    spec: AttemptStartSpec,
    actor_tx: mpsc::UnboundedSender<ActorMessage>,
) {
    let lease = deps.leases.register_lease(
        spec.streamer_id.clone(),
        spec.session_id.clone(),
        spec.attempt_id,
        actor_tx.clone(),
    );
    let leases = deps.leases.clone();
    let fallback_tx = actor_tx.clone();
    let fallback_attempt_id = spec.attempt_id;
    let fallback_session_id = spec.session_id.clone();
    let spawned = leases.spawn(async move {
        run_startup_task(deps, spec, lease, actor_tx).await;
    });
    if !spawned {
        let _ = fallback_tx.send(ActorMessage::Input(FsmInput::AttemptStartFailed {
            attempt_id: fallback_attempt_id,
            session_id: fallback_session_id,
            reason: AttemptStartFailure::Cancelled,
        }));
    }
}

fn send_start_failed(
    actor_tx: &mpsc::UnboundedSender<ActorMessage>,
    attempt_id: u64,
    session_id: String,
    reason: AttemptStartFailure,
) {
    let _ = actor_tx.send(ActorMessage::Input(FsmInput::AttemptStartFailed {
        attempt_id,
        session_id,
        reason,
    }));
}

async fn run_startup_task(
    deps: StartupTaskDeps,
    spec: AttemptStartSpec,
    mut lease: StreamerResourceLease,
    actor_tx: mpsc::UnboundedSender<ActorMessage>,
) {
    let cancel = lease.cancel_token();
    let AttemptStartSpec {
        attempt_id,
        session_id,
        streamer_id,
        streamer_name,
        title,
        payload,
        mut credential_binding,
        from_hysteresis_resume,
        priority,
        using_recovered_media,
    } = spec;

    if lease.is_cancelled() {
        send_start_failed(
            &actor_tx,
            attempt_id,
            session_id,
            AttemptStartFailure::Cancelled,
        );
        return;
    }

    if deps.download_manager.has_active_download(&streamer_id) {
        debug!(%streamer_id, %session_id, "Download already active; skipping duplicate startup");
        send_start_failed(
            &actor_tx,
            attempt_id,
            session_id,
            AttemptStartFailure::Cancelled,
        );
        return;
    }

    let streamer_metadata = deps.streamer_manager.get_streamer(&streamer_id);
    if let Some(metadata) = &streamer_metadata {
        if !metadata.is_active() {
            info!(
                streamer_id = %streamer_id,
                state = %metadata.state,
                "Ignoring startup for inactive streamer"
            );
            send_start_failed(
                &actor_tx,
                attempt_id,
                session_id,
                AttemptStartFailure::Cancelled,
            );
            return;
        }
        if metadata.is_disabled() {
            let retry_after_secs = metadata
                .remaining_backoff_std()
                .map_or(0, |duration| duration.as_secs())
                .saturating_add(2);
            info!(
                streamer_id = %streamer_id,
                streamer_name = %streamer_name,
                disabled_until = ?metadata.disabled_until,
                retry_after_secs,
                "Rejecting startup while streamer is temporarily disabled"
            );
            match deps
                .download_manager
                .emit_rejected(
                    streamer_id.clone(),
                    streamer_name.clone(),
                    session_id.clone(),
                    "streamer temporarily disabled (error backoff)".to_string(),
                    Some(retry_after_secs),
                    DownloadRejectedKind::StreamerBackoff,
                )
                .await
            {
                Ok(()) => return,
                Err(crate::Error::RecorderFeedbackBusy { .. }) => {
                    send_start_failed(
                        &actor_tx,
                        attempt_id,
                        session_id,
                        AttemptStartFailure::RecorderFeedbackBusy,
                    );
                    return;
                }
                Err(error) => {
                    warn!(%error, "Failed to publish streamer-backoff rejection");
                    send_start_failed(
                        &actor_tx,
                        attempt_id,
                        session_id,
                        AttemptStartFailure::StreamerBackoff {
                            reason: "streamer temporarily disabled (error backoff)".to_string(),
                            retry_after_secs,
                        },
                    );
                    return;
                }
            }
        }
    }

    let is_high_priority = priority == Priority::High
        || streamer_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.priority == Priority::High);

    let resolved_config = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            send_start_failed(&actor_tx, attempt_id, session_id, AttemptStartFailure::Cancelled);
            return;
        }
        config = deps.config_service.get_config_for_streamer(&streamer_id) => config,
    };
    let merged_config = match resolved_config {
        Ok(config) => config,
        Err(error) => {
            warn!(
                streamer_id = %streamer_id,
                %error,
                "Failed to load merged config during startup; using defaults"
            );
            Arc::new(crate::config::MergedConfig::builder().build())
        }
    };

    let (
        mut streams,
        mut media_headers,
        mut media_extras,
        mut credential_snapshot,
        has_initial_payload,
    ) = match payload {
        Some(payload) => {
            if credential_binding.is_none() {
                credential_binding = payload.credential_binding;
            }
            (
                payload.streams,
                payload.media_headers,
                payload.media_extras,
                payload.credential_snapshot,
                true,
            )
        }
        None => (Vec::new(), None, None, None, false),
    };

    let managed = credential_binding.is_some() || merged_config.credential_policy.is_some();
    if streams.is_empty() && !managed && has_initial_payload {
        warn!(%streamer_id, "No streams available for download startup");
        send_start_failed(
            &actor_tx,
            attempt_id,
            session_id,
            AttemptStartFailure::NotLiveAfterQueue,
        );
        return;
    }

    // Validate any carried credential binding against the authoritative active session row.
    if let Some(binding) = credential_binding.as_ref() {
        let active_binding = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                send_start_failed(&actor_tx, attempt_id, session_id, AttemptStartFailure::Cancelled);
                return;
            }
            res = deps.store.active_session_credential_binding(&session_id) => res,
        };
        let session_still_open = match &active_binding {
            Ok(Some(_)) => true,
            Ok(None) => {
                deps.store
                    .active_session_id(&streamer_id)
                    .await
                    .ok()
                    .flatten()
                    .as_deref()
                    == Some(session_id.as_str())
            }
            Err(_) => false,
        };
        match active_binding {
            Ok(Some(current))
                if current.epoch == binding.epoch && current.identity == binding.identity => {}
            Ok(None) if session_still_open => {}
            _ => {
                debug!(%session_id, "Ignoring stale credential handoff during startup");
                send_start_failed(
                    &actor_tx,
                    attempt_id,
                    session_id,
                    AttemptStartFailure::StaleBinding,
                );
                return;
            }
        }
    }

    let sanitized_streamer = sanitize_filename_for_template(&streamer_name);
    let sanitized_title = sanitize_title_for_template(&title);
    let platform = streamer_metadata
        .as_ref()
        .map_or("unknown", |metadata| metadata.platform());
    let streamer_url = streamer_metadata
        .as_ref()
        .map(|metadata| metadata.url.clone())
        .unwrap_or_default();

    let dir = merged_config
        .output_folder
        .replace("{streamer}", &sanitized_streamer)
        .replace("{title}", &sanitized_title)
        .replace("{session_id}", &session_id)
        .replace("{platform}", platform);
    let output_dir = expand_path_template(&dir);

    // 1. Preflight.
    let preflight_req = PreflightRequest {
        streamer_id: streamer_id.clone(),
        streamer_name: streamer_name.clone(),
        session_id: session_id.clone(),
        output_dir: output_dir.clone().into(),
        engine_id: Some(merged_config.download_engine.clone()),
        engines_override: merged_config.engines_override.clone(),
    };
    let preflight = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            send_start_failed(&actor_tx, attempt_id, session_id, AttemptStartFailure::Cancelled);
            return;
        }
        engine = deps.download_manager.preflight_classified(preflight_req.clone()) => engine,
    };
    let mut engine = match preflight {
        Ok(engine) => engine,
        Err(PreflightFailure::RejectedEmitted) => {
            // DownloadManager already published `DownloadTerminalEvent::Rejected`
            // via the reserved feedback sink.
            return;
        }
        Err(PreflightFailure::FeedbackBusy) => {
            send_start_failed(
                &actor_tx,
                attempt_id,
                session_id,
                AttemptStartFailure::RecorderFeedbackBusy,
            );
            return;
        }
        Err(PreflightFailure::Other(message)) => {
            warn!(%streamer_id, %message, "Download preflight failed");
            send_start_failed(
                &actor_tx,
                attempt_id,
                session_id,
                AttemptStartFailure::Other { message },
            );
            return;
        }
    };

    if cancel.is_cancelled() {
        send_start_failed(
            &actor_tx,
            attempt_id,
            session_id,
            AttemptStartFailure::Cancelled,
        );
        return;
    }

    // 2. Acquire slot.
    let acquire_req = AcquireRequest {
        session_id: session_id.clone(),
        streamer_id: streamer_id.clone(),
        streamer_name: streamer_name.clone(),
        engine_type: engine.engine_type,
        priority: if is_high_priority {
            QueuePriority::High
        } else {
            QueuePriority::Normal
        },
    };
    let slot = match deps
        .download_manager
        .acquire_slot(acquire_req, cancel.clone())
        .await
    {
        Ok(slot) => slot,
        Err(error) => {
            debug!(%streamer_id, %error, "acquire_slot exited without a slot");
            let reason = if cancel.is_cancelled() {
                AttemptStartFailure::Cancelled
            } else {
                AttemptStartFailure::Other {
                    message: error.to_string(),
                }
            };
            send_start_failed(&actor_tx, attempt_id, session_id, reason);
            return;
        }
    };

    let waited_ms = slot.waited_ms();

    // 3. Freshness check.
    let needs_fresh_probe = !using_recovered_media
        && (managed
            || streams.is_empty()
            || waited_ms > deps.download_manager.queue_freshness_threshold_ms());
    if using_recovered_media {
        debug!(%session_id, "Starting attempt with freshly recovered diagnostic media");
    } else if needs_fresh_probe {
        debug!(
            %streamer_id,
            waited_ms,
            managed,
            "Running post-slot freshness probe"
        );
        let probe_req = ProbeRequest {
            probe_id: 0,
            streamer_id: streamer_id.clone(),
            purpose: ProbePurpose::QueueStart,
            session_id: Some(session_id.clone()),
            credential_binding: credential_binding.clone(),
            is_live_watchdog: false,
        };
        let fresh = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(&actor_tx, attempt_id, session_id, AttemptStartFailure::Cancelled);
                return;
            }
            res = deps.prober.probe_with_hint(probe_req) => res,
        };
        match fresh {
            Ok((Ok(ProbeStatus::Live(live)), _)) => {
                if live.streams.is_empty() {
                    deps.download_manager.emit_dequeued_for_slot(
                        &slot,
                        &streamer_id,
                        &streamer_name,
                    );
                    send_start_failed(
                        &actor_tx,
                        attempt_id,
                        session_id,
                        AttemptStartFailure::NotLiveAfterQueue,
                    );
                    return;
                }
                streams = live.streams;
                media_headers = live.media_headers;
                media_extras = live.media_extras;
                credential_binding = live.credential_binding;
                credential_snapshot = live.credential_snapshot;
            }
            Ok((Ok(_), _)) => {
                debug!(%streamer_id, "Streamer no longer live after queue wait");
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(
                    &actor_tx,
                    attempt_id,
                    session_id,
                    AttemptStartFailure::NotLiveAfterQueue,
                );
                return;
            }
            Ok((Err(ProbeFailure::CredentialSourceChanged), _)) => {
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(
                    &actor_tx,
                    attempt_id,
                    session_id,
                    AttemptStartFailure::CredentialsChanged,
                );
                return;
            }
            Ok((Err(ProbeFailure::CredentialUnavailable { .. }), _)) => {
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(
                    &actor_tx,
                    attempt_id,
                    session_id,
                    AttemptStartFailure::CredentialsUnavailable,
                );
                return;
            }
            Ok((Err(failure), _)) => {
                if managed || streams.is_empty() {
                    warn!(%streamer_id, ?failure, "Freshness probe failed without fallback media");
                    deps.download_manager.emit_dequeued_for_slot(
                        &slot,
                        &streamer_id,
                        &streamer_name,
                    );
                    send_start_failed(
                        &actor_tx,
                        attempt_id,
                        session_id,
                        AttemptStartFailure::Other {
                            message: format!("{failure:?}"),
                        },
                    );
                    return;
                }
                warn!(%streamer_id, ?failure, "Freshness probe failed; falling back to cached media");
            }
            Err(error) => {
                if managed || streams.is_empty() {
                    warn!(%streamer_id, %error, "Freshness probe error without fallback media");
                    deps.download_manager.emit_dequeued_for_slot(
                        &slot,
                        &streamer_id,
                        &streamer_name,
                    );
                    send_start_failed(
                        &actor_tx,
                        attempt_id,
                        session_id,
                        AttemptStartFailure::Other {
                            message: error.to_string(),
                        },
                    );
                    return;
                }
                warn!(%streamer_id, %error, "Freshness probe errored; falling back to cached media");
            }
        }
    } else if !from_hysteresis_resume {
        let meta = deps.streamer_manager.get_streamer(&streamer_id);
        let permits_start = meta
            .as_ref()
            .is_some_and(|m| m.state == StreamerState::Live && !m.is_disabled());
        if !permits_start {
            debug!(
                %streamer_id,
                state = ?meta.as_ref().map(|m| m.state),
                "Streamer no longer LIVE after short queue wait"
            );
            deps.download_manager
                .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
            send_start_failed(
                &actor_tx,
                attempt_id,
                session_id,
                AttemptStartFailure::NotLiveAfterQueue,
            );
            return;
        }
    }

    if cancel.is_cancelled() {
        deps.download_manager
            .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
        send_start_failed(
            &actor_tx,
            attempt_id,
            session_id,
            AttemptStartFailure::Cancelled,
        );
        return;
    }

    // Commit the session's bound credential epoch before launching the engine.
    let mut committed_binding: Option<CredentialBinding> = None;
    if let Some(binding) = credential_binding {
        if credential_snapshot.is_none() {
            warn!(%session_id, "Managed download startup missing credential snapshot");
            deps.download_manager
                .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
            send_start_failed(
                &actor_tx,
                attempt_id,
                session_id,
                AttemptStartFailure::CredentialsUnavailable,
            );
            return;
        }
        let commit_result = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(&actor_tx, attempt_id, session_id, AttemptStartFailure::Cancelled);
                return;
            }
            res = deps.store.commit(DbCommitOp::CommitCredentialBinding {
                streamer_id: streamer_id.clone(),
                session_id: session_id.clone(),
                binding,
            }) => res,
        };
        match commit_result {
            Ok(DbCommitOutcome::CredentialBindingCommitted { binding, .. }) => {
                committed_binding = Some(binding);
            }
            Ok(other) => {
                warn!(%session_id, ?other, "Unexpected outcome committing credential binding");
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(
                    &actor_tx,
                    attempt_id,
                    session_id,
                    AttemptStartFailure::Other {
                        message: "unexpected outcome committing credential binding".to_owned(),
                    },
                );
                return;
            }
            Err(DbCommitFailure::CredentialBindingConflict { message }) => {
                debug!(%session_id, %message, "Credential binding conflict before engine start");
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(
                    &actor_tx,
                    attempt_id,
                    session_id,
                    AttemptStartFailure::CredentialsChanged,
                );
                return;
            }
            Err(DbCommitFailure::Transient { message }) => {
                warn!(%session_id, %message, "Transient error committing credential binding");
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(
                    &actor_tx,
                    attempt_id,
                    session_id,
                    AttemptStartFailure::Other { message },
                );
                return;
            }
        }
    }

    let Some(best_stream) = streams.first() else {
        deps.download_manager
            .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
        send_start_failed(
            &actor_tx,
            attempt_id,
            session_id,
            AttemptStartFailure::NotLiveAfterQueue,
        );
        return;
    };

    // 4. Bigo native streams require Mesio.
    if NativeRoom::parse(&best_stream.url).is_some() && engine.engine_type != EngineType::Mesio {
        info!(
            %streamer_id,
            configured = %engine.engine_type,
            "Recording native Bigo stream with the mesio engine"
        );
        let request = PreflightRequest {
            engine_id: Some(EngineType::Mesio.as_str().to_owned()),
            ..preflight_req
        };
        let preflight = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(&actor_tx, attempt_id, session_id, AttemptStartFailure::Cancelled);
                return;
            }
            res = deps.download_manager.preflight_classified(request) => res,
        };
        match preflight {
            Ok(mesio) => engine = mesio,
            Err(PreflightFailure::RejectedEmitted) => {
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                return;
            }
            Err(PreflightFailure::FeedbackBusy) => {
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(
                    &actor_tx,
                    attempt_id,
                    session_id,
                    AttemptStartFailure::RecorderFeedbackBusy,
                );
                return;
            }
            Err(PreflightFailure::Other(message)) => {
                warn!(%streamer_id, %message, "Preflight of the mesio engine failed");
                deps.download_manager
                    .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                send_start_failed(
                    &actor_tx,
                    attempt_id,
                    session_id,
                    AttemptStartFailure::Other { message },
                );
                return;
            }
        }
    }

    let stream_format = best_stream.stream_format.as_str();
    let media_format = best_stream.media_format.as_str();

    // 5. Monotonic session segment index (MAX + 1).
    let next_index = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            deps.download_manager
                .emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
            send_start_failed(&actor_tx, attempt_id, session_id, AttemptStartFailure::Cancelled);
            return;
        }
        res = deps.store.next_session_segment_index(&session_id) => res,
    };
    let initial_segment_index = match next_index {
        Ok(index) => index,
        Err(error) => {
            warn!(
                %session_id,
                %streamer_id,
                %error,
                "Failed to load next persisted session segment index; starting from zero"
            );
            0
        }
    };

    let mut headers = media_headers.unwrap_or_default();
    if let Some(extras) = best_stream.extras.as_ref() {
        if let Some(extra_headers) = extras.get("headers").and_then(|value| value.as_object()) {
            for (key, value) in extra_headers {
                if let Some(value) = value.as_str() {
                    headers.retain(|existing, _| !existing.eq_ignore_ascii_case(key));
                    headers.insert(key.clone(), value.to_string());
                }
            }
        }
        if let Some(host_header) = extras.get("host_header").and_then(|value| value.as_str()) {
            headers.retain(|existing, _| !existing.eq_ignore_ascii_case("host"));
            headers.insert("Host".to_string(), host_header.to_string());
        }
    }

    let extractor_cookie_updates: Vec<String> = if managed {
        let updates = headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("cookie"))
            .map(|(_, value)| value.clone())
            .collect();
        headers.retain(|key, _| !key.eq_ignore_ascii_case("cookie"));
        updates
    } else {
        Vec::new()
    };

    let protocol = DownloadProtocol::from_format_label(stream_format);
    let mut config = DownloadConfig::new(
        best_stream.url.clone(),
        output_dir,
        streamer_id.clone(),
        streamer_name.clone(),
        session_id.clone(),
    )
    .with_initial_segment_index(initial_segment_index)
    .with_filename_template(
        merged_config
            .output_filename_template
            .replace("{streamer}", &sanitized_streamer)
            .replace("{title}", &sanitized_title)
            .replace("{platform}", platform),
    )
    .with_output_format(&merged_config.output_file_format)
    .with_protocol(protocol)
    .with_max_segment_duration(merged_config.max_download_duration_secs as u64)
    .with_max_segment_size(merged_config.max_part_size_bytes as u64)
    .with_engines_override(merged_config.engines_override.clone());
    config.managed_credentials = managed;
    config.credential_binding = committed_binding;

    let cookies = credential_snapshot
        .as_ref()
        .map(|snapshot: &Arc<CredentialSnapshot>| {
            crate::credentials::merge_cookie_updates(
                &snapshot.material.cookies,
                extractor_cookie_updates.iter().map(String::as_str),
            )
        });
    if let Some(ref cookies) = cookies {
        config = config.with_cookies(cookies);
    }

    let route = credential_snapshot.as_ref().map_or_else(
        || merged_config.proxy_route.clone(),
        |snapshot| snapshot.route.clone(),
    );
    config = config.with_proxy(route.target.clone());
    for (key, value) in headers {
        config = config.with_header(key, value);
    }

    info!(
        %streamer_id,
        %session_id,
        stream_format,
        media_format,
        queue_wait_ms = waited_ms,
        initial_segment_index,
        "Starting download with extracted media"
    );

    let danmu_statistics = merged_config.danmu_statistics.clone();
    let leases_for_cb = deps.leases.clone();
    // 6. StartEngine (runs post-slot credential_start_validator internally and
    // registers `download_id -> attempt_id` synchronously before publishing `DownloadStarted`).
    let started_download_id = match deps
        .download_manager
        .start_with_slot_tracked(slot, config, engine, &cancel, move |download_id| {
            leases_for_cb.register_download_attempt(download_id, attempt_id, protocol);
        })
        .await
    {
        Ok(Some(download_id)) => {
            lease.disarm_dequeued_guard();
            info!(
                %download_id,
                %streamer_id,
                priority = if is_high_priority { "high" } else { "normal" },
                "Started download attempt"
            );
            download_id
        }
        Ok(None) => {
            debug!(%streamer_id, "Download startup cancelled during admission");
            send_start_failed(
                &actor_tx,
                attempt_id,
                session_id,
                AttemptStartFailure::Cancelled,
            );
            return;
        }
        Err(crate::Error::RecorderFeedbackBusy { .. }) => {
            info!(%streamer_id, "Recording admission deferred until feedback capacity recovers");
            send_start_failed(
                &actor_tx,
                attempt_id,
                session_id,
                AttemptStartFailure::RecorderFeedbackBusy,
            );
            return;
        }
        Err(crate::Error::CredentialProfile(ProfileError::SourceChanged)) => {
            send_start_failed(
                &actor_tx,
                attempt_id,
                session_id,
                AttemptStartFailure::CredentialsChanged,
            );
            return;
        }
        Err(crate::Error::CredentialUnavailable(_)) => {
            send_start_failed(
                &actor_tx,
                attempt_id,
                session_id,
                AttemptStartFailure::CredentialsUnavailable,
            );
            return;
        }
        Err(error) => {
            warn!(%streamer_id, %error, "Failed to start download engine");
            send_start_failed(
                &actor_tx,
                attempt_id,
                session_id,
                AttemptStartFailure::Other {
                    message: error.to_string(),
                },
            );
            return;
        }
    };

    let _ = started_download_id;

    // 7. Danmu collection start or authentication update (keeping existing collector & DanmuTaps).
    if !merged_config.record_danmu || cancel.is_cancelled() {
        // `on_attempt_started` may have already transitioned `open.danmu` into
        // `DanmuSessionState::Starting` based on the actor's cached `record_danmu`;
        // report `NotRegistered` whenever no collector is actually running so
        // `danmu_collecting` and `segment_guard.danmu_stopped` do not stay stuck.
        if !deps.danmu_service.is_collecting(&session_id) {
            let _ = actor_tx.send(ActorMessage::Input(FsmInput::DanmuCollectionStopped {
                session_id,
                total_count: 0,
                result: DanmuStopResult::NotRegistered,
            }));
        }
        return;
    }

    let danmu_proxy =
        match crate::proxies::danmu_proxy_for_route(&route, crate::proxies::SystemProxy::current())
        {
            Ok(proxy) => proxy,
            Err(error) => {
                warn!(%session_id, %error, "Danmu not collected: its proxy cannot be used");
                let _ = actor_tx.send(ActorMessage::Input(FsmInput::DanmuCollectionStopped {
                    session_id,
                    total_count: 0,
                    result: DanmuStopResult::NotRegistered,
                }));
                return;
            }
        };

    if deps.danmu_service.is_collecting(&session_id) {
        if !deps
            .danmu_service
            .update_authentication(&session_id, cookies, media_extras, danmu_proxy)
            .await
        {
            debug!(%session_id, "Danmu collection ended before authentication could be updated");
        }
    } else {
        match deps
            .danmu_service
            .start_collection_cancellable(
                CollectionSpec {
                    session_id: session_id.clone(),
                    streamer_id: streamer_id.clone(),
                    streamer_url,
                    cookies,
                    extras: media_extras,
                    proxy: danmu_proxy,
                    statistics: danmu_statistics,
                },
                &cancel,
            )
            .await
        {
            Ok(Some(handle)) => {
                info!(
                    session_id = %handle.session_id(),
                    %streamer_id,
                    "Started danmu collection"
                );
            }
            Ok(None) => {
                debug!(%session_id, "Danmu startup cancelled and contained");
                let _ = actor_tx.send(ActorMessage::Input(FsmInput::DanmuCollectionStopped {
                    session_id,
                    total_count: 0,
                    result: DanmuStopResult::NotRegistered,
                }));
            }
            Err(error) => {
                warn!(%streamer_id, %error, "Failed to start danmu collection");
                let _ = actor_tx.send(ActorMessage::Input(FsmInput::DanmuCollectionStopped {
                    session_id,
                    total_count: 0,
                    result: DanmuStopResult::NotRegistered,
                }));
            }
        }
    }
}
