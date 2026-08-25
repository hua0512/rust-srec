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
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::danmu::{DanmuStatistics, ProviderRegistry};
use crate::error::{Error, Result};
use platforms_parser::danmaku::ConnectionConfig;

use super::events::{CollectionCommand, DanmuCoordinationSender, DanmuEvent, DanmuEventPublisher};
use super::lifecycle::{CollectionOutcome, CollectionSpec, CollectionStopReason};
use super::runner::{CollectionRunner, RunnerParams};
use super::statistics_session::StatisticsSession;

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
        start_time: chrono::DateTime<chrono::Utc>,
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

    /// End the current segment file (finalize XML).
    pub async fn end_segment(&self, segment_id: &str) -> Result<()> {
        self.command_tx
            .send(CollectionCommand::EndSegment {
                segment_id: segment_id.to_string(),
            })
            .await
            .map_err(|_| {
                Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    "Collection task not running",
                ))
            })
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
    done_rx: Option<oneshot::Receiver<CollectionOutcome>>,
}

/// Danmu collection service.
pub struct DanmuService {
    /// Provider registry
    providers: Arc<ProviderRegistry>,
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
    /// Serializes duplicate checks, replacement, registry insertion, and spawn.
    start_serialization: tokio::sync::Mutex<()>,
    accepting: AtomicBool,
    /// Owns collection tasks through final XML flush and terminal events.
    collection_tasks: Mutex<JoinSet<CollectionTaskReport>>,
    /// Cleanup failures from tasks reaped before shutdown.
    collection_failures: Mutex<Vec<String>>,
    /// Session repository for persistence
    session_repo: Option<Arc<dyn crate::database::repositories::SessionRepository>>,
}

#[derive(Debug, Default)]
pub(crate) struct DanmuShutdownReport {
    /// Errors reported by collections that had already ended when
    /// `shutdown_until` closed admission: connect failures, runner errors and
    /// per-session XML cleanup errors drained from `collection_failures`, plus
    /// the reports of tasks joined during the graceful phase.
    ///
    /// A collection can end this way at any point in a long run while the
    /// process keeps recording, so these say nothing about whether shutdown
    /// itself finalized cleanly. Callers report them and continue.
    pub(crate) runtime_failures: Vec<String>,
    /// Failures of the shutdown phase itself: the start-admission deadline or
    /// the graceful collection deadline elapsing, and collection tasks that
    /// could not be joined. Each one means `shutdown_until` could not prove a
    /// collector reached its final flush, so callers treat them as fatal.
    pub(crate) shutdown_failures: Vec<String>,
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
    /// both cases — or another caller already took `done_rx` and owns the wait.
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
            collections: Arc::new(DashMap::new()),
            sessions_by_streamer: Arc::new(DashMap::new()),
            events: DanmuEventPublisher::new(256),
            cancel_token: CancellationToken::new(),
            start_gate: tokio::sync::RwLock::new(()),
            start_serialization: tokio::sync::Mutex::new(()),
            accepting: AtomicBool::new(true),
            collection_tasks: Mutex::new(JoinSet::new()),
            collection_failures: Mutex::new(Vec::new()),
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
        let _start_guard = self.start_gate.read().await;
        if !self.accepting.load(Ordering::Acquire) {
            return Err(Error::Other("danmu service is shutting down".to_string()));
        }

        let CollectionSpec {
            session_id,
            streamer_id,
            streamer_url,
            cookies,
            extras,
            statistics,
        } = spec;

        // Only this short setup section may reserve collection ownership.
        // Expensive connection establishment happens in the owned task after
        // the registry entry and JoinSet slot are installed.
        let setup_guard = self.start_serialization.lock().await;
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
            match self
                .stop_collection_with_reason(&old_sid, CollectionStopReason::SessionEnded)
                .await
            {
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

        // Find provider for URL
        let provider = self.providers.get_by_url(&streamer_url).ok_or_else(|| {
            Error::from(platforms_parser::danmaku::DanmakuError::connection(
                format!("No danmu provider for URL: {}", streamer_url),
            ))
        })?;

        // Extract room ID - use platform-specific extras when available
        // - Huya: uses "presenter_uid" from extras
        // - Douyin: uses "id_str" from extras
        // - Douyu: uses "rid" from extras
        // - Bigo: uses studio "room_id" from extras (not siteId from the URL)
        // - Others: fallback to URL-based extraction
        let platform = provider.platform();
        let room_id = match platform {
            "huya" => {
                // Huya uses presenter_uid for danmu connection
                extras
                    .as_ref()
                    .and_then(|e| e.get("presenter_uid"))
                    .cloned()
                    .or_else(|| provider.extract_room_id(&streamer_url))
            }
            "douyin" => {
                // Douyin uses id_str (room_id) for danmu connection
                extras
                    .as_ref()
                    .and_then(|e| e.get("id_str"))
                    .cloned()
                    .or_else(|| provider.extract_room_id(&streamer_url))
            }
            "douyu" => {
                // Douyu uses rid for danmu connection
                extras
                    .as_ref()
                    .and_then(|e| e.get("rid"))
                    .cloned()
                    .or_else(|| provider.extract_room_id(&streamer_url))
            }
            "soop" => {
                // SOOP chat path uses bj id; chat host/FTK arrive via MediaInfo extras.
                extras
                    .as_ref()
                    .and_then(|e| e.get("bjid").or_else(|| e.get("channel_id")))
                    .cloned()
                    .or_else(|| provider.extract_room_id(&streamer_url))
            }
            "bigo" => {
                // Bigo WS enter needs studio roomId (not siteId from the URL)
                extras
                    .as_ref()
                    .and_then(|e| e.get("room_id"))
                    .cloned()
                    .or_else(|| provider.extract_room_id(&streamer_url))
            }
            _ => provider.extract_room_id(&streamer_url),
        }
        .ok_or_else(|| {
            Error::from(platforms_parser::danmaku::DanmakuError::connection(
                format!("Could not extract room ID from URL: {}", streamer_url),
            ))
        })?;

        // Build connection config
        let mut connection_config = ConnectionConfig::with_cookies(cookies.clone());
        if let Some(e) = extras {
            // Remove common fields that are used for room ID extraction but might be useful as extras too
            // We keep them in extras for now as it's cleaner
            connection_config = connection_config.with_extras(e);
        }

        // Create command channel
        let (command_tx, command_rx) = mpsc::channel(32);

        // With statistics disabled the aggregator is left empty and never
        // persisted, so the session has no statistics row and the API reports
        // them as unavailable. XML recording is unaffected.
        let statistics =
            StatisticsSession::load(session_id.clone(), self.session_repo.clone(), statistics)
                .await;
        if !self.accepting.load(Ordering::Acquire) {
            return Err(Error::Other("danmu service is shutting down".to_string()));
        }
        let cancel_token = self.cancel_token.child_token();

        let (ready_tx, ready_rx) = oneshot::channel::<Result<()>>();
        let (done_tx, done_rx) = oneshot::channel::<CollectionOutcome>();

        let state = CollectionState {
            streamer_id: streamer_id.clone(),
            cancel_token: cancel_token.clone(),
            command_tx: command_tx.clone(),
            done_rx: Some(done_rx),
        };

        self.collections.insert(session_id.clone(), state);
        self.sessions_by_streamer
            .insert(streamer_id.clone(), session_id.clone());

        let collection = collection_task(CollectionTaskContext {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
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
        drop(_start_guard);

        match ready_rx.await {
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

        Ok(CollectionHandle {
            session_id,
            command_tx,
        })
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

    /// Request a stop and report what it proved about the collector.
    ///
    /// See [`CollectionStopOutcome`] for how each return maps onto whether the
    /// task still holds its `collections` entry.
    async fn stop_collection_with_reason(
        &self,
        session_id: &str,
        reason: CollectionStopReason,
    ) -> CollectionStopOutcome {
        let (command_tx, cancel_token, done_rx) = {
            let Some(mut state) = self.collections.get_mut(session_id) else {
                return CollectionStopOutcome::NotRegistered;
            };
            let Some(done_rx) = state.done_rx.take() else {
                // Another stop owns the outcome wait, so this call cannot
                // observe the task finishing.
                return CollectionStopOutcome::Unconfirmed(Error::from(
                    platforms_parser::danmaku::DanmakuError::connection(format!(
                        "Collection is already stopping for session {}",
                        session_id
                    )),
                ));
            };
            (
                state.command_tx.clone(),
                state.cancel_token.clone(),
                done_rx,
            )
        };

        const STOP_TIMEOUT: Duration = Duration::from_secs(10);
        let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        match tokio::time::timeout_at(deadline, command_tx.send(CollectionCommand::Stop(reason)))
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                // The receiver is gone, so the task is past its command loop.
                // The `done_rx` wait below decides whether it finished.
                debug!(session_id, "Danmu collection task already stopped");
            }
            Err(_) => {
                cancel_token.cancel();
                return CollectionStopOutcome::Unconfirmed(Error::Other(format!(
                    "danmu collection {session_id} stop command timed out after {STOP_TIMEOUT:?}"
                )));
            }
        }

        match tokio::time::timeout_at(deadline, done_rx).await {
            Ok(Ok(outcome)) => {
                debug!(session_id, reason = ?outcome.reason, "Danmu collection stopped");
                let mut errors = outcome
                    .error
                    .into_iter()
                    .chain(outcome.cleanup_errors)
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>();
                // Sorted so a multi-error stop formats the same way every run.
                errors.sort();
                CollectionStopOutcome::Terminated {
                    statistics: outcome.statistics,
                    errors,
                }
            }
            // `done_tx` was dropped rather than sent, which only happens once
            // the task's locals — `CollectionCleanupGuard` among them — have
            // dropped, so the registry entry is already released.
            Ok(Err(_)) => CollectionStopOutcome::Terminated {
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

    /// Check if collection is active for a session.
    pub fn is_collecting(&self, session_id: &str) -> bool {
        self.collections.contains_key(session_id)
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
    /// run; stopping the service can neither cause nor repair them, so they are
    /// logged and the shutdown still counts as clean.
    pub async fn shutdown(&self) -> Result<()> {
        let report = self
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(10))
            .await?;
        for failure in &report.runtime_failures {
            warn!(%failure, "Danmu collection ended with an error during the run");
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
    ) -> Result<DanmuShutdownReport> {
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

        let mut tasks = {
            let mut owned = self.collection_tasks.lock();
            std::mem::take(&mut *owned)
        };
        drop(closed_admission);

        // Errors reaped from tasks that ended earlier in the run describe those
        // collections, not this shutdown, so they stay out of
        // `shutdown_failures`.
        let mut runtime_failures = std::mem::take(&mut *self.collection_failures.lock());
        let mut shutdown_failures = Vec::new();
        if admission_deadline_exceeded {
            shutdown_failures.push(
                "graceful danmu start-admission deadline exceeded; admitted starts were contained to completion"
                    .to_string(),
            );
        }
        let mut forced_session_ids = Vec::new();
        let mut graceful_deadline_exceeded = false;
        while !tasks.is_empty() {
            let result = if graceful_deadline_exceeded {
                tasks.join_next().await
            } else {
                match tokio::time::timeout_at(deadline, tasks.join_next()).await {
                    Ok(result) => result,
                    Err(_) => {
                        forced_session_ids = self.active_sessions();
                        forced_session_ids.sort();
                        shutdown_failures.push(format!(
                            "graceful collection deadline exceeded; drained cancelled sessions to completion: {forced_session_ids:?}"
                        ));
                        graceful_deadline_exceeded = true;
                        continue;
                    }
                }
            };

            match result {
                Some(Ok(report)) => append_collection_report(&mut runtime_failures, report),
                // A task that cannot be joined panicked or was aborted, so this
                // shutdown cannot show its collection reached a final flush.
                Some(Err(error)) => {
                    warn!(%error, "Danmu collection task failed while joining");
                    shutdown_failures.push(error.to_string());
                }
                None => break,
            }
        }

        Ok(DanmuShutdownReport {
            runtime_failures,
            shutdown_failures,
            forced_session_ids,
        })
    }
}

/// Everything one collection task owns for its lifetime.
///
/// Named rather than captured by a closure so `start_collection` reads as
/// setup + spawn + ready-handshake, and the task body — which owns registry
/// cleanup and the terminal event — is reviewable on its own.
struct CollectionTaskContext {
    session_id: String,
    streamer_id: String,
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
fn append_collection_report(runtime_failures: &mut Vec<String>, report: CollectionTaskReport) {
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
    runtime_failures.extend(cleanup_errors);
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
    // `is_some_and` consumes the guard, so the read is released before `remove`
    // touches the same shard.
    let owns_slot = sessions_by_streamer
        .get(streamer_id)
        .is_some_and(|sid| sid.value() == session_id);
    if owns_slot {
        sessions_by_streamer.remove(streamer_id);
    }
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
mod tests {
    use super::*;

    struct PendingConnectProvider {
        entered: tokio::sync::Notify,
    }

    impl PendingConnectProvider {
        const URL: &'static str = "https://pending.test/room-1";

        fn new() -> Self {
            Self {
                entered: tokio::sync::Notify::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl platforms_parser::danmaku::DanmuProvider for PendingConnectProvider {
        fn platform(&self) -> &str {
            "pending"
        }

        async fn connect(
            &self,
            _room_id: &str,
            _config: ConnectionConfig,
        ) -> platforms_parser::danmaku::error::Result<platforms_parser::danmaku::DanmuStream>
        {
            self.entered.notify_one();
            std::future::pending().await
        }

        async fn disconnect(
            &self,
            _connection: &mut platforms_parser::danmaku::DanmuConnection,
        ) -> platforms_parser::danmaku::error::Result<()> {
            Ok(())
        }

        fn supports_url(&self, url: &str) -> bool {
            url.contains("pending.test")
        }

        fn extract_room_id(&self, _url: &str) -> Option<String> {
            Some("room-1".to_string())
        }
    }

    /// Seed the service's maps as if a prior collector had spawned for this
    /// `(streamer_id, session_id)`, without actually running a connection
    /// task. Returns a oneshot tx the test can use to drive the runner's
    /// "done" signal — `stop_collection` awaits the rx with a 10s timeout,
    /// so resolving the tx makes the abort path return promptly.
    fn seed_active_collection(
        service: &DanmuService,
        streamer_id: &str,
        session_id: &str,
    ) -> (oneshot::Sender<CollectionOutcome>, CancellationToken) {
        let (command_tx, _command_rx) = mpsc::channel(8);
        let (done_tx, done_rx) = oneshot::channel();
        let cancel_token = service.cancel_token.child_token();
        let state = CollectionState {
            streamer_id: streamer_id.to_string(),
            cancel_token: cancel_token.clone(),
            command_tx,
            done_rx: Some(done_rx),
        };
        service.collections.insert(session_id.to_string(), state);
        service
            .sessions_by_streamer
            .insert(streamer_id.to_string(), session_id.to_string());
        (done_tx, cancel_token)
    }

    fn collection_spec(session_id: &str, streamer_id: &str, streamer_url: &str) -> CollectionSpec {
        CollectionSpec {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            streamer_url: streamer_url.to_string(),
            cookies: None,
            extras: None,
            statistics: crate::domain::DanmuStatisticsConfig::default(),
        }
    }

    #[tokio::test]
    async fn test_danmu_service_creation() {
        let service = DanmuService::new();

        assert!(service.active_sessions().is_empty());
    }

    #[tokio::test]
    async fn shutdown_finalizes_xml_and_joins_collection_before_returning() {
        use crate::danmu::test_support::{FakeProvider, temp_xml_path};

        let (_items_tx, items_rx) = mpsc::channel(8);
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
        let service = DanmuService::with_providers(registry);
        let mut events = service.subscribe();
        let xml_path = temp_xml_path("service-shutdown");

        let handle = service
            .start_collection(collection_spec(
                "session-shutdown",
                "streamer-shutdown",
                FakeProvider::URL,
            ))
            .await
            .expect("collection should start");
        handle
            .start_segment("0", xml_path.clone(), chrono::Utc::now())
            .await
            .expect("segment start should be accepted");
        loop {
            let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("segment start event should arrive")
                .expect("event channel should remain open");
            if matches!(event, DanmuEvent::SegmentStarted { .. }) {
                break;
            }
        }

        service
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(2))
            .await
            .expect("collection should finalize within the shutdown deadline");

        let xml = tokio::fs::read_to_string(&xml_path)
            .await
            .expect("finalized XML should remain readable");
        assert!(xml.trim_end().ends_with("</i>"));
        assert!(!service.is_collecting("session-shutdown"));
        assert!(service.collection_tasks.lock().is_empty());

        let mut completed_positions = Vec::new();
        let mut stopped_positions = Vec::new();
        let mut position = 0;
        while let Ok(event) = events.try_recv() {
            match event {
                DanmuEvent::SegmentCompleted {
                    ref output_path, ..
                } => {
                    assert_eq!(output_path, &xml_path);
                    completed_positions.push(position);
                }
                DanmuEvent::CollectionStopped { .. } => stopped_positions.push(position),
                _ => {}
            }
            position += 1;
        }
        assert_eq!(completed_positions.len(), 1);
        assert_eq!(stopped_positions.len(), 1);
        assert!(completed_positions[0] < stopped_positions[0]);

        tokio::fs::remove_file(xml_path)
            .await
            .expect("test XML should be removable after shutdown");
    }

    #[tokio::test]
    async fn shutdown_rejects_late_collection_starts() {
        let service = DanmuService::new();
        service
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .expect("empty service should stop");

        let result = service
            .start_collection(collection_spec(
                "late-session",
                "late-streamer",
                "https://example.invalid/live",
            ))
            .await;
        let Err(error) = result else {
            panic!("late collection must be rejected");
        };
        assert!(error.to_string().contains("shutting down"));
    }

    #[tokio::test]
    async fn shutdown_report_retains_genuine_collection_start_failure() {
        use crate::danmu::test_support::FakeProvider;

        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(Vec::new())));
        let service = DanmuService::with_providers(registry);

        let start_result = service
            .start_collection(collection_spec(
                "failed-session",
                "failed-streamer",
                FakeProvider::URL,
            ))
            .await;
        let Err(start_error) = start_result else {
            panic!("provider without a stream must reject collection startup");
        };
        assert!(
            start_error
                .to_string()
                .contains("fake provider has no streams left")
        );

        let report = service
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .expect("failed task should still be joined");
        assert!(
            report.runtime_failures.iter().any(|failure| {
                failure.contains("failed-session")
                    && failure.contains("fake provider has no streams left")
            }),
            "a collection that never connected must still be reported"
        );
        assert!(
            report.shutdown_failures.is_empty(),
            "a collection that failed to start is not a shutdown-phase failure: {:?}",
            report.shutdown_failures
        );
    }

    /// `shutdown` reports a collection that ended badly during the run and
    /// still returns `Ok`. Only `DanmuShutdownReport::shutdown_failures` may
    /// fail it, because callers turn that `Err` into a failed process exit.
    #[tokio::test]
    async fn shutdown_succeeds_when_only_a_collection_failed_during_the_run() {
        use crate::danmu::test_support::FakeProvider;

        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(Vec::new())));
        let service = DanmuService::with_providers(registry);

        let start_result = service
            .start_collection(collection_spec(
                "failed-session",
                "failed-streamer",
                FakeProvider::URL,
            ))
            .await;
        assert!(
            start_result.is_err(),
            "fixture: a provider without a stream must reject collection startup"
        );

        service
            .shutdown()
            .await
            .expect("a collection that failed during the run must not fail shutdown");
    }

    #[tokio::test]
    async fn shutdown_report_retains_collection_runtime_failure() {
        use crate::danmu::test_support::{FakeProvider, temp_xml_path};

        let (_items_tx, items_rx) = mpsc::channel(8);
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
        let service = DanmuService::with_providers(registry);
        let mut events = service.subscribe();
        let handle = service
            .start_collection(collection_spec(
                "runtime-failure-session",
                "runtime-failure-streamer",
                FakeProvider::URL,
            ))
            .await
            .expect("collection should become ready");

        // A regular file cannot be used as the parent directory of the XML.
        let blocker = temp_xml_path("service-runtime-failure-blocker");
        tokio::fs::write(&blocker, b"not a directory")
            .await
            .expect("blocker file should be created");
        handle
            .start_segment(
                "0",
                blocker.join("child").join("segment.xml"),
                chrono::Utc::now(),
            )
            .await
            .expect("failing segment command should be admitted");

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let event = events
                    .recv()
                    .await
                    .expect("event channel should remain available");
                if matches!(event, DanmuEvent::CollectionStopped { .. }) {
                    break;
                }
            }
        })
        .await
        .expect("failed collection should publish its terminal event");

        let report = service
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .expect("failed collection task should be joined");
        tokio::fs::remove_file(blocker)
            .await
            .expect("blocker file should be removable");

        assert!(
            report
                .runtime_failures
                .iter()
                .any(|failure| failure.contains("danmu collection runtime-failure-session failed")),
            "the runner's error must still be reported"
        );
        assert!(
            report.shutdown_failures.is_empty(),
            "a collection that ended before shutdown must not fail the shutdown: {:?}",
            report.shutdown_failures
        );
    }

    #[tokio::test]
    async fn shutdown_cancellation_during_startup_is_not_a_runtime_failure() {
        let provider = Arc::new(PendingConnectProvider::new());
        let mut registry = ProviderRegistry::new();
        registry.register(provider.clone());
        let service = Arc::new(DanmuService::with_providers(registry));

        let start_service = service.clone();
        let start = tokio::spawn(async move {
            start_service
                .start_collection(collection_spec(
                    "cancelled-session",
                    "cancelled-streamer",
                    PendingConnectProvider::URL,
                ))
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), provider.entered.notified())
            .await
            .expect("provider connect should begin");

        let report = service
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .expect("startup cancellation should settle cleanly");
        let start_result = start.await.expect("start task should join");
        let Err(start_error) = start_result else {
            panic!("shutdown must cancel startup");
        };

        assert!(start_error.to_string().contains("cancelled during startup"));
        assert!(report.runtime_failures.is_empty());
        assert!(report.shutdown_failures.is_empty());
        assert!(report.forced_session_ids.is_empty());
    }

    #[tokio::test]
    async fn shutdown_drains_collection_cleanup_after_graceful_deadline() {
        let service = Arc::new(DanmuService::new());
        let cancelled = Arc::new(tokio::sync::Notify::new());
        let release_cleanup = Arc::new(tokio::sync::Notify::new());
        let task_token = service.cancel_token.child_token();
        let task_cancelled = cancelled.clone();
        let task_release = release_cleanup.clone();
        service.collection_tasks.lock().spawn(async move {
            task_token.cancelled().await;
            task_cancelled.notify_one();
            task_release.notified().await;
            CollectionTaskReport {
                session_id: "slow-cleanup-session".to_string(),
                runtime_error: None,
                cleanup_errors: Vec::new(),
            }
        });

        let shutdown_service = service.clone();
        let mut shutdown = tokio::spawn(async move {
            shutdown_service
                .shutdown_until(tokio::time::Instant::now() + Duration::from_millis(10))
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), cancelled.notified())
            .await
            .expect("owned task should observe service cancellation");
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut shutdown)
                .await
                .is_err(),
            "deadline expiry must not abort an owned cleanup future"
        );

        release_cleanup.notify_one();
        let report = tokio::time::timeout(Duration::from_secs(1), shutdown)
            .await
            .expect("shutdown should finish after cleanup is released")
            .expect("shutdown task should join")
            .expect("post-deadline cleanup should remain contained");
        assert!(
            report
                .shutdown_failures
                .iter()
                .any(|failure| failure.contains("graceful collection deadline exceeded")),
            "a missed graceful deadline is a shutdown-phase failure"
        );
    }

    #[tokio::test]
    async fn test_is_collecting() {
        let service = DanmuService::new();

        assert!(!service.is_collecting("session1"));
    }

    #[tokio::test]
    async fn test_get_session_by_streamer_empty() {
        let service = DanmuService::new();

        // Should return None when no sessions exist
        assert!(service.get_session_by_streamer("streamer1").is_none());
    }

    /// `start_collection` for a different session of an already-tracked
    /// streamer must abort the prior collector before claiming the
    /// streamer-level slot; otherwise the old collector keeps running
    /// orphaned alongside the new session's collector.
    #[tokio::test]
    async fn start_collection_aborts_previous_for_same_streamer() {
        use crate::danmu::test_support::FakeProvider;

        let (_first_items_tx, first_items_rx) = mpsc::channel(8);
        let (_second_items_tx, second_items_rx) = mpsc::channel(8);
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(vec![
            first_items_rx,
            second_items_rx,
        ])));
        let service = DanmuService::with_providers(registry);
        let streamer_id = "streamer-1";
        let old_session = "session-old";
        let new_session = "session-new";

        service
            .start_collection(collection_spec(old_session, streamer_id, FakeProvider::URL))
            .await
            .expect("first collection starts");

        assert!(
            service.is_collecting(old_session),
            "fixture: prior collector seeded"
        );
        assert_eq!(
            service.get_session_by_streamer(streamer_id).as_deref(),
            Some(old_session)
        );

        service
            .start_collection(collection_spec(new_session, streamer_id, FakeProvider::URL))
            .await
            .expect("replacement collection starts");

        // The old collection must be gone before the replacement claims the
        // streamer-level slot.
        assert!(
            !service.is_collecting(old_session),
            "prior collector for {old_session} must have been aborted"
        );
        assert!(
            service.is_collecting(new_session),
            "the replacement collector must own the session slot"
        );
        service
            .stop_collection(new_session)
            .await
            .expect("replacement stops");
    }

    /// A prior collector that already delivered its `CollectionOutcome` has
    /// released its `collections` entry, so there is nothing for the new
    /// collector to overlap and the errors it reported must not deny the next
    /// session a collector. Only the `CollectionStopOutcome::Unconfirmed`
    /// timeout paths refuse.
    ///
    /// The errors are still reported: they reach `collection_failures` and come
    /// back as `DanmuShutdownReport::runtime_failures`, which never fails a
    /// shutdown.
    #[tokio::test]
    async fn replacement_starts_after_a_prior_collector_terminated_with_errors() {
        use crate::danmu::lifecycle::CollectionExitReason;
        use crate::danmu::test_support::FakeProvider;

        let (_items_tx, items_rx) = mpsc::channel(8);
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
        let service = DanmuService::with_providers(registry);
        let streamer_id = "streamer-1";
        let old_session = "session-old";
        let new_session = "session-new";
        let cleanup_error = "failed to finalize danmu XML for session-old";

        // Stands in for a collector that finished its cleanup and reported a
        // failed XML flush. Resolving `done_tx` up front is what the real task
        // does after `remove_collection`, so the stop's outcome wait returns
        // immediately with the errors attached. The owned task separately
        // returns the same failure through its JoinSet report; that report is
        // the single path that records the failure for shutdown diagnostics.
        let (done_tx, _cancel_token) = seed_active_collection(&service, streamer_id, old_session);
        done_tx
            .send(CollectionOutcome {
                statistics: DanmuStatistics::default(),
                reason: CollectionExitReason::Failed,
                error: None,
                cleanup_errors: vec![Error::Other(cleanup_error.to_string())],
            })
            .expect("the seeded collector should deliver its outcome");
        service.collection_tasks.lock().spawn(async move {
            CollectionTaskReport {
                session_id: old_session.to_string(),
                runtime_error: None,
                cleanup_errors: vec![cleanup_error.to_string()],
            }
        });

        service
            .start_collection(collection_spec(new_session, streamer_id, FakeProvider::URL))
            .await
            .expect("a terminated prior collector must not block the replacement");

        assert!(
            service.is_collecting(new_session),
            "the replacement collector must own the session slot"
        );
        assert_eq!(
            service.get_session_by_streamer(streamer_id).as_deref(),
            Some(new_session),
            "the replacement must claim the streamer-level slot"
        );

        let report = service
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .expect("shutdown should settle");
        let matching_failures = report
            .runtime_failures
            .iter()
            .filter(|failure| failure.contains(cleanup_error))
            .count();
        assert_eq!(
            matching_failures, 1,
            "the prior collector's error must be reported exactly once: {:?}",
            report.runtime_failures
        );
        assert!(
            report.shutdown_failures.is_empty(),
            "and must not fail the shutdown: {:?}",
            report.shutdown_failures
        );
    }

    /// A stop that timed out leaves the prior task possibly holding its
    /// websocket and its `collections` entry, so `start_collection` must refuse
    /// rather than run two collectors for one streamer.
    #[tokio::test(start_paused = true)]
    async fn replacement_is_refused_while_the_prior_collector_may_still_run() {
        use crate::danmu::test_support::FakeProvider;

        let (_items_tx, items_rx) = mpsc::channel(8);
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
        let service = DanmuService::with_providers(registry);
        let streamer_id = "streamer-1";
        let old_session = "session-old";

        // `done_tx` is held, so the outcome wait runs out its `STOP_TIMEOUT`
        // without ever proving the task released its entry.
        let (_done_tx, _cancel_token) = seed_active_collection(&service, streamer_id, old_session);

        let result = service
            .start_collection(collection_spec(
                "session-new",
                streamer_id,
                FakeProvider::URL,
            ))
            .await;
        let Err(error) = result else {
            panic!("an unconfirmed prior collector must block the replacement");
        };

        assert!(
            error.to_string().contains("stop timed out"),
            "the refusal must carry the stop timeout: {error}"
        );
        assert!(
            !service.is_collecting("session-new"),
            "no replacement collector may be registered"
        );
    }

    /// The guarantee `PipelineCoordinator` depends on: a collector that ends on a
    /// failure rather than a stop request must still emit `CollectionStopped`.
    ///
    /// `apply_event`'s `DanmuCollectionStopped` arm is the only writer of
    /// `danmu_complete`, and `is_ready` will not emit `CreateSessionCompleteDag`
    /// while a session has `danmu_expected && danmu_observed` but not
    /// `danmu_complete` — so withholding the event after a failure strands the
    /// session-complete pipeline for the rest of the process's life.
    #[tokio::test(start_paused = true)]
    async fn collection_stopped_is_emitted_when_the_runner_fails() {
        use crate::danmu::test_support::{FakeProvider, temp_xml_path};

        let (_items_tx, items_rx) = mpsc::channel(8);
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
        let service = DanmuService::with_providers(registry);
        let mut events = service.subscribe();

        let handle = service
            .start_collection(collection_spec(
                "session-1",
                "streamer-1",
                FakeProvider::URL,
            ))
            .await
            .expect("collection starts against the fake provider");
        assert_eq!(handle.session_id(), "session-1");

        // A segment path whose parent is a regular file cannot be created, so the
        // runner's loop ends on an error instead of a stop request.
        let blocker = temp_xml_path("service-loop-error");
        tokio::fs::write(&blocker, b"not a directory")
            .await
            .expect("write blocker file");
        handle
            .start_segment(
                "0",
                blocker.join("child").join("seg.xml"),
                chrono::Utc::now(),
            )
            .await
            .expect("command is accepted; the failure happens in the runner");

        let stopped = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match events.recv().await {
                    Ok(DanmuEvent::CollectionStopped { session_id, .. }) => return session_id,
                    Ok(_) => continue,
                    Err(error) => panic!("danmu event stream ended early: {error}"),
                }
            }
        })
        .await
        .expect("CollectionStopped must follow a runner failure");
        let _ = tokio::fs::remove_file(&blocker).await;

        assert_eq!(stopped, "session-1");
        assert!(
            !service.is_collecting("session-1"),
            "the collection slot must be released"
        );
        assert!(
            service.get_session_by_streamer("streamer-1").is_none(),
            "the streamer reverse index must be released"
        );
    }

    /// The last line of defence for the session-complete gate: if the collector
    /// task unwinds, `CollectionCleanupGuard::drop` must still release the session
    /// and emit `CollectionStopped`.
    ///
    /// Without it a panic inside `CollectionRunner::run` leaves the `collections`
    /// entry behind — so `is_collecting` keeps reporting a live collector, which
    /// the health probe trusts — and withholds the only event that sets the
    /// coordinator's `danmu_complete`.
    #[tokio::test]
    async fn cleanup_guard_releases_the_session_when_the_task_unwinds() {
        let service = DanmuService::new();
        let mut events = service.subscribe();
        let (_done_tx, _) = seed_active_collection(&service, "streamer-1", "session-1");
        assert!(service.is_collecting("session-1"));

        // Dropping without disarming is what an unwind through the task body does.
        {
            let _guard = CollectionCleanupGuard {
                collections: service.collections.clone(),
                sessions_by_streamer: service.sessions_by_streamer.clone(),
                events: service.events.clone(),
                session_id: "session-1".to_string(),
                started: true,
                armed: true,
            };
        }

        assert!(
            !service.is_collecting("session-1"),
            "the collection slot must be released"
        );
        assert!(
            service.get_session_by_streamer("streamer-1").is_none(),
            "the streamer reverse index must be released"
        );
        assert!(
            matches!(
                events.try_recv(),
                Ok(DanmuEvent::CollectionStopped { session_id, .. }) if session_id == "session-1"
            ),
            "the coordinator's danmu arm must still be released"
        );
    }

    /// A disarmed guard is inert, so the normal exit path — which does its own
    /// cleanup with real statistics — cannot be duplicated by it.
    #[tokio::test]
    async fn cleanup_guard_is_inert_once_disarmed() {
        let service = DanmuService::new();
        let mut events = service.subscribe();
        let (_done_tx, _) = seed_active_collection(&service, "streamer-1", "session-1");

        {
            let mut guard = CollectionCleanupGuard {
                collections: service.collections.clone(),
                sessions_by_streamer: service.sessions_by_streamer.clone(),
                events: service.events.clone(),
                session_id: "session-1".to_string(),
                started: true,
                armed: true,
            };
            guard.disarm();
        }

        assert!(
            service.is_collecting("session-1"),
            "a disarmed guard must not touch the collection"
        );
        assert!(events.try_recv().is_err(), "and must not emit an event");
    }

    /// Calling `start_collection` twice with the same `session_id` keeps
    /// the existing "already active" error path. The new abort logic must
    /// not fire for self-replace because of the `old_sid != session_id`
    /// guard.
    #[tokio::test]
    async fn start_collection_idempotent_for_same_session_id() {
        use crate::danmu::test_support::FakeProvider;

        let (_items_tx, items_rx) = mpsc::channel(8);
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
        let service = DanmuService::with_providers(registry);
        let streamer_id = "streamer-1";
        let session_id = "session-1";

        service
            .start_collection(collection_spec(session_id, streamer_id, FakeProvider::URL))
            .await
            .expect("first collection starts");

        let result = service
            .start_collection(collection_spec(session_id, streamer_id, FakeProvider::URL))
            .await;

        assert!(
            result.is_err(),
            "second start_collection for same session must return the 'already active' error"
        );
        assert!(service.is_collecting(session_id));
        service
            .stop_collection(session_id)
            .await
            .expect("collection stops");
    }

    #[tokio::test]
    async fn concurrent_starts_reserve_a_session_exactly_once() {
        use crate::danmu::test_support::FakeProvider;

        let (_items_tx, items_rx) = mpsc::channel(8);
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
        let service = Arc::new(DanmuService::with_providers(registry));

        let first = {
            let service = service.clone();
            tokio::spawn(async move {
                service
                    .start_collection(collection_spec(
                        "session-1",
                        "streamer-1",
                        FakeProvider::URL,
                    ))
                    .await
            })
        };
        let second = {
            let service = service.clone();
            tokio::spawn(async move {
                service
                    .start_collection(collection_spec(
                        "session-1",
                        "streamer-1",
                        FakeProvider::URL,
                    ))
                    .await
            })
        };

        let first = first.await.expect("first start task should join");
        let second = second.await.expect("second start task should join");
        assert_ne!(first.is_ok(), second.is_ok());
        assert_eq!(service.active_sessions(), vec!["session-1".to_string()]);

        service
            .shutdown()
            .await
            .expect("the winning collection should stop cleanly");
    }

    #[tokio::test]
    async fn normal_stop_does_not_cancel_the_collection_token() {
        use crate::danmu::test_support::FakeProvider;

        let (_items_tx, items_rx) = mpsc::channel(8);
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
        let service = DanmuService::with_providers(registry);
        let mut events = service.subscribe();
        service
            .start_collection(collection_spec(
                "session-1",
                "streamer-1",
                FakeProvider::URL,
            ))
            .await
            .expect("collection starts");
        let cancel_token = service
            .collections
            .get("session-1")
            .expect("active collection")
            .cancel_token
            .clone();

        service
            .stop_collection("session-1")
            .await
            .expect("collection stops");

        assert!(
            !cancel_token.is_cancelled(),
            "a normal session stop must not be treated as process shutdown"
        );
        let stopped_events = std::iter::from_fn(|| events.try_recv().ok())
            .filter(|event| matches!(event, DanmuEvent::CollectionStopped { .. }))
            .count();
        assert_eq!(
            stopped_events, 1,
            "normal completion must have one terminal event owner"
        );
    }

    #[tokio::test]
    async fn disabled_statistics_are_not_persisted_on_stop() {
        use crate::database::models::{LiveSessionDbModel, StreamerDbModel};
        use crate::database::repositories::{
            SessionRepository as _, SqlxSessionRepository, SqlxStreamerRepository,
            StreamerRepository as _,
        };
        use crate::database::{init_pool_with_size, run_migrations};

        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let mut streamer = StreamerDbModel::new(
            "Streamer One",
            "https://example.com/streamer-1",
            "platform-twitch",
        );
        streamer.id = "streamer-1".to_string();
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();

        let mut session = LiveSessionDbModel::new("streamer-1");
        session.id = "session-1".to_string();
        let repo = Arc::new(SqlxSessionRepository::new(pool.clone(), pool));
        repo.create_session(&session).await.unwrap();

        use crate::danmu::test_support::FakeProvider;

        let (_items_tx, items_rx) = mpsc::channel(8);
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
        let service = DanmuService::with_providers(registry).with_session_repository(repo.clone());
        let mut spec = collection_spec("session-1", "streamer-1", FakeProvider::URL);
        spec.statistics.enabled = false;
        service
            .start_collection(spec)
            .await
            .expect("collection starts");

        service
            .stop_collection("session-1")
            .await
            .expect("collection stops");

        assert!(
            repo.get_danmu_statistics("session-1")
                .await
                .unwrap()
                .is_none(),
            "disabled collection must not create an empty statistics row"
        );
    }
}
