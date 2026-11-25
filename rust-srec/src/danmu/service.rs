//! Danmu collection service.
//!
//! Manages danmu collection for live sessions with segment-based file writing.
//!
//! The danmu collection follows this model:
//! - **Session level**: WebSocket connection stays alive, statistics are aggregated
//! - **Segment level**: XML files are created/finalized per download segment
//!
//! When a new segment starts → create new danmu XML file
//! When segment closes → finalize that XML file, but keep collecting danmu
//! When session ends → stop collection entirely

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::danmu::{DanmuStatistics, ProviderRegistry};
use crate::error::{Error, Result};
use crate::utils::failure_samples::FailureSamples;
use crate::utils::task_supervisor::DrainedTasks;
use platforms_parser::danmaku::ConnectionConfig;

use super::events::{
    CollectionCommand, DanmuAuthentication, DanmuCoordinationSender, DanmuEvent,
    DanmuEventPublisher,
};
use super::lifecycle::{
    CollectionExitReason, CollectionOutcome, CollectionSpec, CollectionStopReason,
};
use super::runner::{CollectionRunner, RunnerParams};
use super::statistics_session::StatisticsSession;
use super::tap::{DanmuTaps, TapRegistration};

const STOP_TIMEOUT: Duration = Duration::from_secs(10);

type SharedCollectionCompletion = Shared<BoxFuture<'static, Option<Arc<CollectionOutcome>>>>;

fn shared_completion(receiver: oneshot::Receiver<CollectionOutcome>) -> SharedCollectionCompletion {
    receiver
        .map(|result| result.ok().map(Arc::new))
        .boxed()
        .shared()
}

/// Handle for controlling a danmu collection session.
#[derive(Clone)]
pub struct CollectionHandle {
    session_id: String,
    command_tx: mpsc::Sender<CollectionCommand>,
}

impl CollectionHandle {
    /// Start writing to a new segment file.
    ///
    /// The `start_time` is used to calculate danmu timestamp offsets.
    pub async fn start_segment(
        &self,
        segment_id: &str,
        output_path: PathBuf,
        start_time: jiff::Timestamp,
    ) -> Result<()> {
        self.command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: segment_id.to_string(),
                output_path,
                start_time,
            })
            .await
            .map_err(|_| {
                Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    "Collection task not running",
                ))
            })
    }

    /// End the current segment file (finalize XML and publish completion).
    pub async fn end_segment(&self, segment_id: &str) -> Result<()> {
        self.close_segment(segment_id, false).await
    }

    /// Close the current segment file and wait for the runner's acknowledgement.
    ///
    /// When `discard` is `true`, the runner closes and deletes the `.xml` file
    /// and suppresses [`DanmuEvent::SegmentCompleted`] before acknowledging.
    pub async fn close_segment(&self, segment_id: &str, discard: bool) -> Result<()> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.command_tx
            .send(CollectionCommand::CloseSegment {
                segment_id: segment_id.to_string(),
                discard,
                ack: ack_tx,
            })
            .await
            .map_err(|_| {
                Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    "Collection task not running",
                ))
            })?;
        // If the runner exited concurrently before replying, treat the close
        // handshake as settled; shutdown finalization or discard reverse-cleanup
        // handles any resulting file state.
        let _ = ack_rx.await;
        Ok(())
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// State for an active danmu collection session.
struct CollectionState {
    /// Streamer ID
    streamer_id: String,
    /// Cancellation token for this collection
    cancel_token: CancellationToken,
    /// Command sender
    command_tx: mpsc::Sender<CollectionCommand>,
    /// Signals when the task has completed all normal finalization.
    done: SharedCollectionCompletion,
    /// Only one caller enqueues Stop; all callers retain the same completion future.
    stop_requested: bool,
}

/// Danmu collection service.
pub struct DanmuService {
    /// Provider registry
    providers: Arc<ProviderRegistry>,
    taps: Arc<DanmuTaps>,
    /// Active collections (session_id -> state)
    collections: Arc<DashMap<String, CollectionState>>,
    /// Reverse index for fast lookups (streamer_id -> session_id).
    sessions_by_streamer: Arc<DashMap<String, String>>,
    /// Routes required coordination events separately from observer events.
    events: DanmuEventPublisher,
    /// Global cancellation token
    cancel_token: CancellationToken,
    /// Tracks collection setup through task admission so shutdown cannot miss
    /// an in-flight start.
    start_gate: tokio::sync::RwLock<()>,
    /// Serializes duplicate checks, replacement, registry insertion, and spawn
    /// for one streamer.
    ///
    /// Keyed by streamer rather than process-global because the section it
    /// guards can await `stop_collection_with_reason` for the whole
    /// `STOP_TIMEOUT` while replacing that streamer's prior collector. A single
    /// shared mutex would hold every other streamer's `start_collection` behind
    /// that wait, leaving unrelated recordings with no collector.
    /// Entries are removed after their final setup owner or waiter drops.
    start_serialization: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
    accepting: AtomicBool,
    /// Owns collection tasks through final XML flush and terminal events.
    collection_tasks: Mutex<JoinSet<CollectionTaskReport>>,
    /// Recent cleanup failures from tasks reaped before shutdown, with a total count.
    collection_failures: Mutex<FailureSamples>,
    /// Session repository for persistence
    session_repo: Option<Arc<dyn crate::database::repositories::SessionRepository>>,
}

/// Keeps one streamer's setup lock registered while it has owners or waiters.
/// The mutex guard borrows this lease, so it is released before the last lease
/// can remove the entry, including when a startup future is cancelled.
struct StreamerStartLock<'a> {
    // Struct fields drop in declaration order: release the strong owner
    // before checking whether only the map's owner remains.
    mutex: Arc<tokio::sync::Mutex<()>>,
    _registration: StreamerStartRegistration<'a>,
}

struct StreamerStartRegistration<'a> {
    registry: &'a DashMap<String, Arc<tokio::sync::Mutex<()>>>,
    streamer_id: &'a str,
    mutex: std::sync::Weak<tokio::sync::Mutex<()>>,
}

impl Drop for StreamerStartRegistration<'_> {
    fn drop(&mut self) {
        self.registry.remove_if(self.streamer_id, |_, current| {
            // Lookup clones under this same shard lock. A new caller either
            // keeps this mutex alive or arrives after every prior owner left.
            Arc::as_ptr(current) == self.mutex.as_ptr() && Arc::strong_count(current) == 1
        });
    }
}

#[derive(Debug, Default)]
pub(crate) struct DanmuShutdownReport {
    /// Errors reported by collections that had already ended when
    /// `shutdown_until` closed admission: connect failures, runner errors and
    /// per-session XML cleanup errors drained from `collection_failures`, plus
    /// the reports of tasks joined during the graceful phase. Details are
    /// bounded; an additional entry reports the total and omitted sample count.
    ///
    /// A collection can end this way at any point in a long run while the
    /// process keeps recording, so these say nothing about whether shutdown
    /// itself finalized cleanly. Callers report them and continue.
    pub(crate) runtime_failures: Vec<String>,
    /// Failures of the shutdown phase itself: collection tasks that could not
    /// be joined. Each one means `shutdown_until` could not prove a collector
    /// reached its final flush, so callers treat them as fatal.
    pub(crate) shutdown_failures: Vec<String>,
    /// Soft-budget overruns — the start-admission or graceful collection
    /// deadline elapsing — that were still contained to completion afterwards.
    /// Reported so operators can raise the budget, never fatal.
    pub(crate) overruns: Vec<String>,
    /// Sessions still active when the graceful deadline elapsed. Their
    /// cancellation cleanup was joined to completion before returning.
    pub(crate) forced_session_ids: Vec<String>,
}

/// What a stop request proved about the collector it targeted.
///
/// `start_collection` replaces a prior collector for the same streamer only
/// when the stop proved the prior task released its `collections` entry;
/// otherwise two collectors would hold one streamer's websocket at once.
enum CollectionStopOutcome {
    /// The task is gone. `collection_task` runs `remove_collection` before it
    /// sends on `done_tx`, and on an unwind `CollectionCleanupGuard::drop` runs
    /// it before `done_tx` is dropped, so a delivered outcome and a closed
    /// channel both prove the registry entry is free. `errors` carries what the
    /// collector reported on its way out.
    Terminated {
        statistics: DanmuStatistics,
        errors: Vec<String>,
    },
    /// The session has no `collections` entry, so no collector holds its slot:
    /// either it never had one or its task already ran `remove_collection`.
    NotRegistered,
    /// The stop is unproven and the task may still own its websocket and its
    /// `collections` entry: `STOP_TIMEOUT` elapsed on the command send or on
    /// the outcome wait — the collection's cancellation token is cancelled in
    /// both cases. Timeout or caller cancellation does not consume shared completion.
    Unconfirmed(Error),
}

impl Default for DanmuService {
    fn default() -> Self {
        Self::new()
    }
}

impl DanmuService {
    /// Create a new danmu service.
    pub fn new() -> Self {
        Self::with_providers(ProviderRegistry::with_defaults())
    }

    /// Create a new danmu service with custom providers.
    pub fn with_providers(providers: ProviderRegistry) -> Self {
        Self {
            providers: Arc::new(providers),
            taps: Arc::default(),
            collections: Arc::new(DashMap::new()),
            sessions_by_streamer: Arc::new(DashMap::new()),
            events: DanmuEventPublisher::new(256),
            cancel_token: CancellationToken::new(),
            start_gate: tokio::sync::RwLock::new(()),
            start_serialization: DashMap::new(),
            accepting: AtomicBool::new(true),
            collection_tasks: Mutex::new(JoinSet::new()),
            collection_failures: Mutex::new(FailureSamples::default()),
            session_repo: None,
        }
    }

    /// Set the session repository for persistence.
    pub fn with_session_repository(
        mut self,
        repo: Arc<dyn crate::database::repositories::SessionRepository>,
    ) -> Self {
        self.session_repo = Some(repo);
        self
    }

    /// Install the required runtime coordination path.
    pub(crate) fn with_coordination_sender(mut self, sender: DanmuCoordinationSender) -> Self {
        self.events = self.events.with_coordination_sender(sender);
        self
    }

    /// Subscribe to danmu events.
    pub fn subscribe(&self) -> broadcast::Receiver<DanmuEvent> {
        self.events.subscribe()
    }

    /// Start danmu collection for a session.
    /// Returns a handle that can be used to control segment file writing.
    pub async fn start_collection(&self, spec: CollectionSpec) -> Result<CollectionHandle> {
        self.start_collection_cancellable(spec, &CancellationToken::new())
            .await?
            .ok_or_else(|| Error::Other("danmu collection startup cancelled".to_owned()))
    }

    fn start_lock<'a>(&'a self, streamer_id: &'a str) -> StreamerStartLock<'a> {
        let mutex = self
            .start_serialization
            .entry(streamer_id.to_owned())
            .or_default()
            .clone();
        StreamerStartLock {
            _registration: StreamerStartRegistration {
                registry: &self.start_serialization,
                streamer_id,
                mutex: Arc::downgrade(&mutex),
            },
            mutex,
        }
    }

    /// Cancellation before publication leaves no collection. After publication,
    /// cancel the owned task and await its cleanup before acknowledging startup
    /// cancellation; dropping a readiness waiter must not orphan that task.
    pub(crate) async fn start_collection_cancellable(
        &self,
        spec: CollectionSpec,
        cancel: &CancellationToken,
    ) -> Result<Option<CollectionHandle>> {
        let _start_guard = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(None),
            guard = self.start_gate.read() => guard,
        };
        if !self.accepting.load(Ordering::Acquire) {
            return Err(Error::Other("danmu service is shutting down".to_string()));
        }

        let CollectionSpec {
            session_id,
            streamer_id,
            streamer_url,
            cookies,
            extras,
            proxy,
            statistics,
        } = spec;

        // Only this short setup section may reserve collection ownership.
        // Expensive connection establishment happens in the owned task after
        // the registry entry and JoinSet slot are installed.
        let streamer_setup_lock = self.start_lock(&streamer_id);
        let handoff_deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        let setup = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(None),
            guard = tokio::time::timeout_at(handoff_deadline, streamer_setup_lock.mutex.lock()) => guard,
        };
        let setup_guard = setup
            .map_err(|_| Error::Other(format!(
                "danmu collection startup handoff timed out after {STOP_TIMEOUT:?} (streamer_id={streamer_id})"
            )))?;
        if !self.accepting.load(Ordering::Acquire) {
            return Err(Error::Other("danmu service is shutting down".to_string()));
        }

        // Check if already collecting
        if self.collections.contains_key(&session_id) {
            return Err(Error::from(
                platforms_parser::danmaku::DanmakuError::connection(format!(
                    "Collection already active for session {}",
                    session_id
                )),
            ));
        }

        // If this streamer already has a collector for a *different* session,
        // abort it before claiming the streamer-level slot. Without this, each
        // live cycle leaks one collector: the previous task keeps its
        // websocket open with a stale session_id until the platform finally
        // sends StreamClosed — at which point N stale collectors all fire
        // end-of-stream events for N different sessions in sequence.
        //
        // Important: clone the value out of the DashMap guard before any
        // .await — holding a parking_lot read across an await is a deadlock
        // with `stop_collection` (which mutates the same shard).
        let prior_session_id: Option<String> = self
            .sessions_by_streamer
            .get(&streamer_id)
            .map(|entry| entry.value().clone());

        if let Some(old_sid) = prior_session_id
            && old_sid != session_id
            && self.collections.contains_key(&old_sid)
        {
            let started = std::time::Instant::now();
            let stopped = tokio::select! {
                biased;
                // Only the stop waiter is dropped. The prior task retains its
                // registry and JoinSet ownership; its stop sender's drop guard
                // signals cancellation if the command was not enqueued yet.
                _ = cancel.cancelled() => return Ok(None),
                stopped = self.stop_collection_until(
                    &old_sid,
                    CollectionStopReason::SessionEnded,
                    handoff_deadline,
                ) => stopped,
            };
            match stopped {
                CollectionStopOutcome::Terminated { errors, .. } => {
                    info!(
                        streamer_id,
                        old_session_id = old_sid.as_str(),
                        new_session_id = session_id,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "danmu: replaced previous collector for streamer"
                    );
                    // The prior task released its `collections` entry, so the
                    // replacement cannot overlap it and the errors it reported
                    // are not a reason to deny this session a collector. They
                    // are still returned by the owned task's
                    // `CollectionTaskReport`; the JoinSet is the single owner
                    // that records them in `collection_failures`.
                    if !errors.is_empty() {
                        warn!(
                            streamer_id,
                            old_session_id = old_sid.as_str(),
                            errors = ?errors,
                            "danmu: prior collector terminated with errors"
                        );
                    }
                }
                CollectionStopOutcome::NotRegistered => {
                    // The prior task ran `remove_collection` between the
                    // `contains_key` check above and the stop request.
                    debug!(
                        streamer_id,
                        old_session_id = old_sid.as_str(),
                        "danmu: prior collector released its slot before the stop request"
                    );
                }
                CollectionStopOutcome::Unconfirmed(error) => {
                    warn!(
                        streamer_id,
                        old_session_id = old_sid.as_str(),
                        error = %error,
                        "danmu: refusing to overlap a new collector with a prior collector"
                    );
                    return Err(error);
                }
            }
        }

        let super::room::DanmuRoom {
            provider,
            room_id,
            config: connection_config,
        } = super::room::resolve_room(&self.providers, &streamer_url, cookies, extras, proxy)?;

        // Create command channel
        let (command_tx, command_rx) = mpsc::channel(32);

        // With statistics disabled the aggregator is left empty and never
        // persisted, so the session has no statistics row and the API reports
        // them as unavailable. XML recording is unaffected.
        let statistics = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(None),
            statistics = StatisticsSession::load(session_id.clone(), self.session_repo.clone(), statistics) => statistics,
        };
        if !self.accepting.load(Ordering::Acquire) {
            return Err(Error::Other("danmu service is shutting down".to_string()));
        }
        let cancel_token = self.cancel_token.child_token();
        let startup_guard = cancel_token.clone().drop_guard();

        let (ready_tx, ready_rx) = oneshot::channel::<Result<()>>();
        let (done_tx, done_rx) = oneshot::channel::<CollectionOutcome>();
        let done = shared_completion(done_rx);

        let state = CollectionState {
            streamer_id: streamer_id.clone(),
            cancel_token: cancel_token.clone(),
            command_tx: command_tx.clone(),
            done: done.clone(),
            stop_requested: false,
        };

        self.collections.insert(session_id.clone(), state);
        self.sessions_by_streamer
            .insert(streamer_id.clone(), session_id.clone());

        let tap = self.taps.register(provider.platform(), &room_id);
        let collection = collection_task(CollectionTaskContext {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            tap,
            room_id,
            provider: Arc::clone(&provider),
            conn_config: connection_config,
            statistics,
            command_rx,
            cancel_token: cancel_token.clone(),
            collections: self.collections.clone(),
            sessions_by_streamer: self.sessions_by_streamer.clone(),
            events: self.events.clone(),
            ready_tx,
            done_tx,
            cleanup: CollectionCleanupGuard {
                collections: self.collections.clone(),
                sessions_by_streamer: self.sessions_by_streamer.clone(),
                events: self.events.clone(),
                session_id: session_id.clone(),
                started: false,
                armed: true,
            },
        });
        {
            let mut tasks = self.collection_tasks.lock();
            while let Some(result) = tasks.try_join_next() {
                match result {
                    Ok(report) => self.record_collection_report(report),
                    Err(error) => {
                        warn!(%error, "Danmu collection task failed before reap");
                        self.collection_failures.lock().push(error.to_string());
                    }
                }
            }
            tasks.spawn(collection);
        }
        drop(setup_guard);
        drop(streamer_setup_lock);
        drop(_start_guard);

        let readiness = tokio::select! {
            biased;
            _ = cancel.cancelled() => return cancelled_collection_start(&session_id, &cancel_token, done).await,
            readiness = ready_rx => readiness,
        };
        match readiness {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(Error::from(
                    platforms_parser::danmaku::DanmakuError::connection(
                        "Danmu collection task stopped before it became ready",
                    ),
                ));
            }
        }

        if cancel.is_cancelled() {
            return cancelled_collection_start(&session_id, &cancel_token, done).await;
        }
        drop(startup_guard.disarm());
        Ok(Some(CollectionHandle {
            session_id,
            command_tx,
        }))
    }

    /// Stop danmu collection for a session.
    ///
    /// Anything but a clean stop is an error here, including a collector that
    /// terminated while reporting errors. `start_collection` calls
    /// [`Self::stop_collection_with_reason`] directly instead, because it has
    /// to tell a collector that is gone from one that may still be running.
    pub async fn stop_collection(&self, session_id: &str) -> Result<DanmuStatistics> {
        match self
            .stop_collection_with_reason(session_id, CollectionStopReason::SessionEnded)
            .await
        {
            CollectionStopOutcome::Terminated { statistics, errors } if errors.is_empty() => {
                Ok(statistics)
            }
            CollectionStopOutcome::Terminated { errors, .. } => Err(Error::Other(format!(
                "danmu collection {session_id} stopped with errors: {}",
                errors.join("; ")
            ))),
            CollectionStopOutcome::NotRegistered => Err(Error::from(
                platforms_parser::danmaku::DanmakuError::connection(format!(
                    "No active collection for session {}",
                    session_id
                )),
            )),
            CollectionStopOutcome::Unconfirmed(error) => Err(error),
        }
    }

    /// Request a stop with an explicit reason and report the structured stop outcome.
    pub(crate) async fn stop_collection_with_outcome(
        &self,
        session_id: &str,
        reason: CollectionStopReason,
    ) -> crate::recorder::DanmuStopResult {
        match self.stop_collection_with_reason(session_id, reason).await {
            CollectionStopOutcome::Terminated { .. } => {
                crate::recorder::DanmuStopResult::Terminated
            }
            CollectionStopOutcome::NotRegistered => crate::recorder::DanmuStopResult::NotRegistered,
            CollectionStopOutcome::Unconfirmed(error) => {
                crate::recorder::DanmuStopResult::Unconfirmed {
                    error: error.to_string(),
                }
            }
        }
    }

    /// Returns `true` while any danmu collector is registered for `streamer_id`.
    pub(crate) fn has_active_collection_for_streamer(&self, streamer_id: &str) -> bool {
        self.sessions_by_streamer.contains_key(streamer_id)
            || self
                .collections
                .iter()
                .any(|entry| entry.streamer_id == streamer_id)
    }

    /// Request a stop and report what it proved about the collector.
    ///
    /// See [`CollectionStopOutcome`] for how each return maps onto whether the
    /// task still holds its `collections` entry.
    async fn stop_collection_with_reason(
        &self,
        session_id: &str,
        reason: CollectionStopReason,
    ) -> CollectionStopOutcome {
        self.stop_collection_until(
            session_id,
            reason,
            tokio::time::Instant::now() + STOP_TIMEOUT,
        )
        .await
    }

    async fn stop_collection_until(
        &self,
        session_id: &str,
        reason: CollectionStopReason,
        deadline: tokio::time::Instant,
    ) -> CollectionStopOutcome {
        let (command_tx, cancel_token, done, send_stop) = {
            let Some(mut state) = self.collections.get_mut(session_id) else {
                return CollectionStopOutcome::NotRegistered;
            };
            let send_stop = !state.stop_requested;
            state.stop_requested = true;
            (
                state.command_tx.clone(),
                state.cancel_token.clone(),
                state.done.clone(),
                send_stop,
            )
        };

        if send_stop {
            // If the elected sender is cancelled while waiting for channel capacity, its
            // followers still need a stop signal. Once queued, the task owns graceful stop.
            let send_guard = cancel_token.clone().drop_guard();
            match tokio::time::timeout_at(
                deadline,
                command_tx.send(CollectionCommand::Stop(reason)),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    // The receiver is gone, so the task is past its command loop.
                    // The shared completion below decides whether it finished.
                    debug!(session_id, "Danmu collection task already stopped");
                }
                Err(_) => {
                    return CollectionStopOutcome::Unconfirmed(Error::Other(format!(
                        "danmu collection {session_id} stop command timed out after {STOP_TIMEOUT:?}"
                    )));
                }
            }
            drop(send_guard.disarm());
        }

        match tokio::time::timeout_at(deadline, done).await {
            Ok(Some(outcome)) => {
                debug!(session_id, reason = ?outcome.reason, "Danmu collection stopped");
                let mut errors = outcome
                    .error
                    .iter()
                    .chain(outcome.cleanup_errors.iter())
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>();
                // Sorted so a multi-error stop formats the same way every run.
                errors.sort();
                CollectionStopOutcome::Terminated {
                    statistics: outcome.statistics.clone(),
                    errors,
                }
            }
            // `done_tx` was dropped rather than sent, which only happens once
            // the task's locals — `CollectionCleanupGuard` among them — have
            // dropped, so the registry entry is already released.
            Ok(None) => CollectionStopOutcome::Terminated {
                statistics: DanmuStatistics::default(),
                errors: vec![format!(
                    "danmu collection {session_id} ended without reporting an outcome"
                )],
            },
            Err(_) => {
                warn!(
                    "Danmu collection stop timed out after {:?} (session_id={})",
                    STOP_TIMEOUT, session_id
                );
                cancel_token.cancel();
                CollectionStopOutcome::Unconfirmed(Error::Other(format!(
                    "danmu collection {session_id} stop timed out after {STOP_TIMEOUT:?}"
                )))
            }
        }
    }

    /// Get a handle for an existing collection.
    pub fn get_handle(&self, session_id: &str) -> Option<CollectionHandle> {
        self.collections
            .get(session_id)
            .map(|state| CollectionHandle {
                session_id: session_id.to_string(),
                command_tx: state.command_tx.clone(),
            })
    }

    /// Point a running collection at the account and proxy a later attempt of
    /// the same recording uses. The collector reconnects at once when either
    /// differs, so danmu does not keep authenticating as a replaced account or
    /// leaving through its old network path. Returns `false` when no collection
    /// is registered for the session.
    pub async fn update_authentication(
        &self,
        session_id: &str,
        cookies: Option<String>,
        extras: Option<std::collections::HashMap<String, String>>,
        proxy: Option<platforms_parser::danmaku::DanmuProxy>,
    ) -> bool {
        // Clone the sender out of the DashMap guard before awaiting.
        let Some(command_tx) = self
            .collections
            .get(session_id)
            .map(|state| state.command_tx.clone())
        else {
            return false;
        };
        command_tx
            .send(CollectionCommand::UpdateAuthentication(
                DanmuAuthentication {
                    cookies,
                    extras,
                    proxy,
                },
            ))
            .await
            .is_ok()
    }

    /// Check if collection is active for a session.
    pub fn is_collecting(&self, session_id: &str) -> bool {
        self.collections.contains_key(session_id)
    }

    /// The rooms this service is collecting, for live viewers to follow.
    pub fn taps(&self) -> Arc<DanmuTaps> {
        self.taps.clone()
    }

    /// Get all active session IDs.
    pub fn active_sessions(&self) -> Vec<String> {
        self.collections.iter().map(|r| r.key().clone()).collect()
    }

    /// Get the session ID for a streamer if one exists.
    ///
    /// Iterates over active collections to find a session matching the given streamer ID.
    /// Returns the session ID if found, None otherwise.
    pub fn get_session_by_streamer(&self, streamer_id: &str) -> Option<String> {
        self.sessions_by_streamer
            .get(streamer_id)
            .map(|entry| entry.value().clone())
    }

    fn record_collection_report(&self, report: CollectionTaskReport) {
        append_collection_report(&mut self.collection_failures.lock(), report);
    }

    /// Seed `collection_failures` with an error as if a collection had ended
    /// badly earlier in the run, so tests outside this module can build a
    /// service that carries pre-shutdown failures into `shutdown_until`.
    #[cfg(test)]
    pub(crate) fn seed_runtime_failure_for_test(&self, failure: String) {
        self.collection_failures.lock().push(failure);
    }

    /// Shutdown the service using its standalone timeout.
    ///
    /// Only [`DanmuShutdownReport::shutdown_failures`] fails this call.
    /// `runtime_failures` describes collections that already ended during the
    /// run, and `overruns` describes a drain that took longer than its grace
    /// period but still finalized; neither can be caused or repaired by
    /// stopping the service, so both are logged and the shutdown counts as
    /// clean.
    pub async fn shutdown(&self) -> Result<()> {
        let report = self
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(10))
            .await;
        for failure in &report.runtime_failures {
            warn!(%failure, "Danmu collection ended with an error during the run");
        }
        for overrun in &report.overruns {
            warn!(%overrun, "Danmu shutdown exceeded its grace period but was contained");
        }
        if report.shutdown_failures.is_empty() {
            Ok(())
        } else {
            Err(Error::Other(format!(
                "danmu collection task shutdown failed: {}",
                report.shutdown_failures.join("; ")
            )))
        }
    }

    /// Close start admission and join every collection task. The deadline
    /// bounds the graceful phase; cancellation cleanup that is still running at
    /// that point remains owned and is joined before this method returns.
    pub(crate) async fn shutdown_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> DanmuShutdownReport {
        self.accepting.store(false, Ordering::Release);

        // Wait until every start that passed the initial admission check has
        // either installed an owned task or rejected itself. The ready/connect
        // handshake is performed by that owned task and is not inside this gate.
        let (closed_admission, admission_deadline_exceeded) =
            match tokio::time::timeout_at(deadline, self.start_gate.write()).await {
                Ok(guard) => (guard, false),
                Err(_) => {
                    // Existing collectors and any admitted starter use children of
                    // this token. Their owning JoinSet cannot be taken until the
                    // start gate is acquired, because the admitted starter may
                    // still install a task. Cancel first, then retain ownership
                    // until that admission path has settled.
                    self.cancel_token.cancel();
                    warn!("Danmu start-admission grace period exceeded; awaiting containment");
                    (self.start_gate.write().await, true)
                }
            };

        for state in self.collections.iter() {
            if let Err(error) = state.command_tx.try_send(CollectionCommand::Stop(
                CollectionStopReason::ServiceShutdown,
            )) {
                debug!(
                    session_id = state.key(),
                    %error,
                    "Danmu collection will use cancellation fallback during shutdown"
                );
            }
        }
        self.cancel_token.cancel();

        // `DrainedTasks` returns the set to `collection_tasks` if this future is
        // dropped mid-join, which is what `shutdown_with_schedule` does at its
        // force deadline. Taking the `JoinSet` into a plain local instead would
        // abort every collection where it stands, truncating the segment XML
        // that `CollectionTaskContext` is finalizing.
        let mut tasks = DrainedTasks::take_from(&self.collection_tasks);
        drop(closed_admission);

        // Errors reaped from tasks that ended earlier in the run describe those
        // collections, not this shutdown, so they stay out of
        // `shutdown_failures`.
        let mut runtime_failures = std::mem::take(&mut *self.collection_failures.lock());
        let mut shutdown_failures = Vec::new();
        let mut overruns = Vec::new();
        if admission_deadline_exceeded {
            overruns.push(
                "graceful danmu start-admission deadline exceeded; admitted starts were contained to completion"
                    .to_string(),
            );
        }
        let mut forced_session_ids = Vec::new();
        tasks
            .join_with_grace(
                deadline,
                |_| {
                    forced_session_ids = self.active_sessions();
                    forced_session_ids.sort();
                    overruns.push(format!(
                        "graceful collection deadline exceeded; drained cancelled sessions to completion: {forced_session_ids:?}"
                    ));
                },
                |result, _| match result {
                    Ok(report) => append_collection_report(&mut runtime_failures, report),
                    // A task that cannot be joined panicked or was aborted, so this
                    // shutdown cannot show its collection reached a final flush.
                    Err(error) => {
                        warn!(%error, "Danmu collection task failed while joining");
                        shutdown_failures.push(error.to_string());
                    }
                },
            )
            .await;

        DanmuShutdownReport {
            runtime_failures: runtime_failures.into_report(),
            shutdown_failures,
            overruns,
            forced_session_ids,
        }
    }

    /// Close start admission and abort every collection task instead of joining
    /// it, then join the aborted tasks so their futures are dropped.
    ///
    /// [`Self::shutdown_until`] keeps joining past its deadline, so a collector
    /// that never returns holds it pending. Callers with no parent process to
    /// force-kill them use this; aborting loses whatever segment XML the task
    /// had not finalized, so it is the last resort rather than the drain.
    ///
    /// Returns the session ids that were still collecting.
    pub(crate) async fn abort_collections(&self, deadline: tokio::time::Instant) -> Vec<String> {
        self.accepting.store(false, Ordering::Release);
        self.cancel_token.cancel();

        let mut session_ids = self.active_sessions();
        session_ids.sort();

        let unfinished = DrainedTasks::take_from(&self.collection_tasks)
            .abort_and_reap(deadline, "Danmu collection")
            .await;
        if unfinished > 0 {
            warn!(
                unfinished,
                "Aborted danmu collections did not settle before the reap deadline"
            );
        }

        session_ids
    }
}

async fn cancelled_collection_start(
    session_id: &str,
    cancel: &CancellationToken,
    done: SharedCollectionCompletion,
) -> Result<Option<CollectionHandle>> {
    cancel.cancel();
    let outcome = done.await.ok_or_else(|| {
        Error::Other(format!(
            "cancelled danmu startup {session_id} ended without a cleanup outcome"
        ))
    })?;
    let errors: Vec<_> = outcome
        .error
        .iter()
        .chain(outcome.cleanup_errors.iter())
        .map(ToString::to_string)
        .collect();
    if !errors.is_empty() {
        return Err(Error::Other(format!(
            "cancelled danmu startup {session_id} cleanup failed: {}",
            errors.join("; ")
        )));
    }
    Ok(None)
}

/// Everything one collection task owns for its lifetime.
///
/// Named rather than captured by a closure so `start_collection` reads as
/// setup + spawn + ready-handshake, and the task body — which owns registry
/// cleanup and the terminal event — is reviewable on its own.
struct CollectionTaskContext {
    session_id: String,
    streamer_id: String,
    /// The room claim for live viewers.
    tap: TapRegistration,
    room_id: String,
    provider: Arc<dyn platforms_parser::danmaku::DanmuProvider>,
    conn_config: ConnectionConfig,
    statistics: StatisticsSession,
    command_rx: mpsc::Receiver<CollectionCommand>,
    cancel_token: CancellationToken,
    collections: Arc<DashMap<String, CollectionState>>,
    sessions_by_streamer: Arc<DashMap<String, String>>,
    events: DanmuEventPublisher,
    /// Resolved once, to tell `start_collection` whether the connect succeeded.
    ready_tx: oneshot::Sender<Result<()>>,
    /// Carries the final outcome to a waiting `stop_collection`.
    done_tx: oneshot::Sender<CollectionOutcome>,
    /// Constructed before spawn so an unpolled task still owns registry cleanup.
    cleanup: CollectionCleanupGuard,
}

#[derive(Debug)]
struct CollectionTaskReport {
    session_id: String,
    runtime_error: Option<String>,
    cleanup_errors: Vec<String>,
}

/// Fold one collection's ending into the runtime-failure class.
///
/// Everything appended here describes how a single collection ended, which
/// `shutdown_until` returns as [`DanmuShutdownReport::runtime_failures`].
fn append_collection_report(runtime_failures: &mut FailureSamples, report: CollectionTaskReport) {
    let CollectionTaskReport {
        session_id,
        runtime_error,
        cleanup_errors,
    } = report;
    if let Some(error) = runtime_error {
        warn!(%session_id, %error, "Danmu collection ended with an error");
        runtime_failures.push(format!("danmu collection {session_id} failed: {error}"));
    } else {
        debug!(%session_id, "Danmu collection task reaped");
    }
    for error in cleanup_errors {
        runtime_failures.push(error);
    }
}

/// One collection, from connect to terminal event.
///
/// The task is the sole owner of normal completion: registry cleanup,
/// persistence (inside `CollectionRunner::run`), and `CollectionStopped`.
/// Callers only request a stop and await the outcome.
async fn collection_task(ctx: CollectionTaskContext) -> CollectionTaskReport {
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
    let CollectionTaskContext {
        session_id,
        streamer_id,
        tap,
        room_id,
        provider,
        conn_config,
        statistics,
        command_rx,
        cancel_token,
        collections,
        sessions_by_streamer,
        events,
        ready_tx,
        done_tx,
        mut cleanup,
    } = ctx;

    let (start_result, cancelled_during_startup) = tokio::select! {
        biased;
        _ = cancel_token.cancelled() => (
            Err(Error::Other(
                format!("danmu collection {session_id} cancelled during startup")
            )),
            true,
        ),
        result = tokio::time::timeout(
            CONNECT_TIMEOUT,
            CollectionRunner::new(RunnerParams {
                session_id: session_id.clone(),
                streamer_id: streamer_id.clone(),
                room_id,
                provider,
                conn_config,
                statistics,
                events: events.clone(),
                tap: Some(tap),
            }),
        ) => (
            match result {
                Ok(result) => result,
                Err(_) => Err(Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    format!(
                        "Danmu connection timed out after {:?} (session_id={})",
                        CONNECT_TIMEOUT, session_id
                    ),
                ))),
            },
            false,
        ),
    };

    let (runner, items) = match start_result {
        Ok(started) => started,
        Err(error) => {
            let error_message = error.to_string();
            cleanup.disarm();
            remove_collection(&collections, &sessions_by_streamer, &session_id);
            // A settled connection attempt has no running collector, but must
            // still acknowledge cleanup. Missing completion is reserved for a
            // panic/abort, where registry removal alone cannot prove finalization.
            let outcome = CollectionOutcome {
                statistics: DanmuStatistics::default(),
                reason: if cancelled_during_startup {
                    CollectionExitReason::Cancelled
                } else {
                    CollectionExitReason::Failed
                },
                error: (!cancelled_during_startup).then(|| Error::Other(error_message.clone())),
                cleanup_errors: Vec::new(),
            };
            if done_tx.send(outcome).is_err() {
                debug!(%session_id, "Danmu startup cleanup waiter was dropped");
            }
            if ready_tx.send(Err(error)).is_err() {
                debug!(%session_id, "Danmu collection starter dropped before startup failure was delivered");
            }
            if !cancelled_during_startup {
                events.publish(DanmuEvent::Error {
                    session_id: session_id.clone(),
                    error: error_message.clone(),
                });
            }
            return CollectionTaskReport {
                session_id,
                runtime_error: (!cancelled_during_startup).then_some(error_message),
                cleanup_errors: Vec::new(),
            };
        }
    };

    cleanup.mark_started();
    events.publish(DanmuEvent::CollectionStarted {
        session_id: session_id.clone(),
        streamer_id,
    });
    if ready_tx.send(Ok(())).is_err() {
        debug!(%session_id, "Danmu collection starter dropped before readiness was delivered");
    }

    let outcome = runner.run(command_rx, items, cancel_token).await;
    let runtime_error = outcome.error.as_ref().map(ToString::to_string);
    let cleanup_errors = outcome
        .cleanup_errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if let Some(error) = &outcome.error {
        events.publish(DanmuEvent::Error {
            session_id: session_id.clone(),
            error: error.to_string(),
        });
    }
    for error in &outcome.cleanup_errors {
        events.publish(DanmuEvent::Error {
            session_id: session_id.clone(),
            error: error.to_string(),
        });
    }

    if remove_collection(&collections, &sessions_by_streamer, &session_id) {
        events.publish(DanmuEvent::CollectionStopped {
            session_id: session_id.clone(),
            total_count: outcome.statistics.total_count,
        });
    }
    cleanup.disarm();

    if done_tx.send(outcome).is_err() {
        debug!(%session_id, "Danmu collection stop waiter dropped before outcome was delivered");
    }
    CollectionTaskReport {
        session_id,
        runtime_error,
        cleanup_errors,
    }
}

/// Releases a session's collection state if the collector task unwinds.
///
/// The normal exit paths do this themselves, with final statistics, and call
/// [`Self::disarm`]. This covers the path that cannot: a panic inside
/// `CollectionRunner::run` would otherwise leave the `collections` entry behind —
/// so `is_collecting` keeps reporting a live collector and the `danmu_service`
/// health probe sees nothing wrong — and withhold `CollectionStopped`, which is
/// the only writer of `PipelineCoordinator`'s `danmu_complete` and therefore the
/// only thing that lets the session-complete pipeline run.
///
/// Both operations are synchronous — `DashMap::remove` and
/// `DanmuEventPublisher::publish` — so the whole cleanup fits in `Drop`. The daemon's
/// release profile deliberately keeps unwinding (see the comment on
/// `[profile.release]` in the workspace manifest), so this runs on a panic rather
/// than the process aborting.
struct CollectionCleanupGuard {
    collections: Arc<DashMap<String, CollectionState>>,
    sessions_by_streamer: Arc<DashMap<String, String>>,
    events: DanmuEventPublisher,
    session_id: String,
    started: bool,
    armed: bool,
}

impl CollectionCleanupGuard {
    fn mark_started(&mut self) {
        self.started = true;
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CollectionCleanupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // `remove_collection` returning false means an exit path already
        // released the session, so there is nothing to announce.
        if remove_collection(
            &self.collections,
            &self.sessions_by_streamer,
            &self.session_id,
        ) {
            warn!(
                session_id = %self.session_id,
                "danmu: collector task ended without completing; releasing the session"
            );
            if self.started {
                self.events.publish(DanmuEvent::CollectionStopped {
                    session_id: self.session_id.clone(),
                    total_count: 0,
                });
            }
        }
    }
}

/// Drop the streamer reverse-index entry only when it still points at
/// `session_id`; a newer collector that already claimed the streamer must keep
/// its slot.
fn release_streamer_slot(
    sessions_by_streamer: &DashMap<String, String>,
    streamer_id: &str,
    session_id: &str,
) {
    // One shard lock covers the check and the removal, so a collector that
    // claims the slot in between cannot lose it.
    sessions_by_streamer.remove_if(streamer_id, |_, sid| sid == session_id);
}

/// Remove a session's collection state along with its streamer reverse-index
/// entry.
///
/// Returns whether this call was the one that removed the entry. The owned
/// collection task is the only caller that publishes `CollectionStopped`.
fn remove_collection(
    collections: &DashMap<String, CollectionState>,
    sessions_by_streamer: &DashMap<String, String>,
    session_id: &str,
) -> bool {
    let Some((_, state)) = collections.remove(session_id) else {
        return false;
    };
    release_streamer_slot(sessions_by_streamer, &state.streamer_id, session_id);
    true
}

#[cfg(test)]
mod tests;
