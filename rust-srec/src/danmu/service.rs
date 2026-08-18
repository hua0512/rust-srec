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

use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

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
    /// Session repository for persistence
    session_repo: Option<Arc<dyn crate::database::repositories::SessionRepository>>,
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
        let CollectionSpec {
            session_id,
            streamer_id,
            streamer_url,
            cookies,
            extras,
            statistics,
        } = spec;

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
            match self.stop_collection(&old_sid).await {
                Ok(_) => info!(
                    streamer_id,
                    old_session_id = old_sid.as_str(),
                    new_session_id = session_id,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "danmu: replaced previous collector for streamer"
                ),
                // Non-fatal: the new collector still spawns. Logged so a
                // wedged prior runner is visible in operator dashboards.
                Err(e) => warn!(
                    streamer_id,
                    old_session_id = old_sid.as_str(),
                    error = %e,
                    "danmu: failed to stop prior collector; new collector will spawn anyway"
                ),
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

        tokio::spawn(collection_task(CollectionTaskContext {
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
        }));

        tokio::select! {
            ready = ready_rx => {
                match ready {
                    Ok(Ok(())) => {
                        self.events.publish(DanmuEvent::CollectionStarted {
                            session_id: session_id.clone(),
                            streamer_id: streamer_id.clone(),
                        });
                    }
                    Ok(Err(e)) => {
                        remove_collection(&self.collections, &self.sessions_by_streamer, &session_id);
                        return Err(e);
                    }
                    Err(_) => {
                        remove_collection(&self.collections, &self.sessions_by_streamer, &session_id);
                        return Err(Error::from(platforms_parser::danmaku::DanmakuError::connection(
                            "Danmu collection task stopped before it became ready",
                        )));
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                remove_collection(&self.collections, &self.sessions_by_streamer, &session_id);
                return Err(Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    "Danmu collection cancelled before it became ready",
                )));
            }
        }

        Ok(CollectionHandle {
            session_id,
            command_tx,
        })
    }

    /// Stop danmu collection for a session.
    pub async fn stop_collection(&self, session_id: &str) -> Result<DanmuStatistics> {
        self.stop_collection_with_reason(session_id, CollectionStopReason::SessionEnded)
            .await
    }

    async fn stop_collection_with_reason(
        &self,
        session_id: &str,
        reason: CollectionStopReason,
    ) -> Result<DanmuStatistics> {
        let (command_tx, cancel_token, done_rx) = {
            let mut state = self.collections.get_mut(session_id).ok_or_else(|| {
                Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    format!("No active collection for session {}", session_id),
                ))
            })?;
            let done_rx = state.done_rx.take().ok_or_else(|| {
                Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    format!("Collection is already stopping for session {}", session_id),
                ))
            })?;
            (
                state.command_tx.clone(),
                state.cancel_token.clone(),
                done_rx,
            )
        };

        if command_tx
            .send(CollectionCommand::Stop(reason))
            .await
            .is_err()
        {
            tracing::debug!(session_id, "Danmu collection task already stopped");
        }

        const STOP_TIMEOUT: Duration = Duration::from_secs(10);
        match tokio::time::timeout(STOP_TIMEOUT, done_rx).await {
            Ok(Ok(outcome)) => {
                tracing::debug!(session_id, reason = ?outcome.reason, "Danmu collection stopped");
                Ok(outcome.statistics)
            }
            Ok(Err(_)) => {
                tracing::debug!(
                    session_id,
                    "Danmu collection task ended without reporting an outcome"
                );
                Ok(DanmuStatistics::default())
            }
            Err(_) => {
                warn!(
                    "Danmu collection stop timed out after {:?} (session_id={})",
                    STOP_TIMEOUT, session_id
                );
                cancel_token.cancel();

                // A wedged task cannot own completion, so the timeout fallback
                // releases the coordinator gate. A late task observes the
                // missing entry and cannot emit a duplicate terminal event.
                if remove_collection(&self.collections, &self.sessions_by_streamer, session_id) {
                    self.events.publish(DanmuEvent::CollectionStopped {
                        session_id: session_id.to_string(),
                        total_count: 0,
                    });
                }
                Ok(DanmuStatistics::default())
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

    /// Shutdown the service.
    pub async fn shutdown(&self) {
        // Cancel all collections
        self.cancel_token.cancel();

        // Stop all active collections
        let session_ids: Vec<_> = self.collections.iter().map(|r| r.key().clone()).collect();
        for session_id in session_ids {
            if let Err(error) = self
                .stop_collection_with_reason(&session_id, CollectionStopReason::ServiceShutdown)
                .await
            {
                warn!(session_id, %error, "Failed to stop danmu collection during shutdown");
            }
        }
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
}

/// One collection, from connect to terminal event.
///
/// The task is the sole owner of normal completion: registry cleanup,
/// persistence (inside `CollectionRunner::run`), and `CollectionStopped`.
/// Callers only request a stop and await the outcome.
async fn collection_task(ctx: CollectionTaskContext) {
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
    } = ctx;

    // Armed for the whole task: every early return below either disarms it
    // or has already released the session, and a panic in between is what
    // it exists for.
    let mut cleanup = CollectionCleanupGuard {
        collections: collections.clone(),
        sessions_by_streamer: sessions_by_streamer.clone(),
        events: events.clone(),
        session_id: session_id.clone(),
        armed: true,
    };

    let (runner, items) = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        CollectionRunner::new(RunnerParams {
            session_id: session_id.clone(),
            streamer_id,
            room_id,
            provider,
            conn_config,
            statistics,
            events: events.clone(),
        }),
    )
    .await
    {
        Ok(Ok(started)) => {
            let _ = ready_tx.send(Ok(()));
            started
        }
        Ok(Err(e)) => {
            let error_message = e.to_string();
            events.publish(DanmuEvent::Error {
                session_id: session_id.clone(),
                error: error_message,
            });
            let _ = ready_tx.send(Err(e));
            cleanup.disarm();
            remove_collection(&collections, &sessions_by_streamer, &session_id);
            // No `CollectionStopped` here on purpose: collection never
            // started, so `PipelineCoordinator` never set `danmu_observed`
            // and the session-complete gate does not wait on a danmu arm.
            return;
        }
        Err(_) => {
            let message = format!(
                "Danmu connection timed out after {:?} (session_id={})",
                CONNECT_TIMEOUT, session_id
            );
            let _ = ready_tx.send(Err(Error::from(
                platforms_parser::danmaku::DanmakuError::connection(message.clone()),
            )));
            events.publish(DanmuEvent::Error {
                session_id: session_id.clone(),
                error: message,
            });
            cleanup.disarm();
            remove_collection(&collections, &sessions_by_streamer, &session_id);
            return;
        }
    };

    let outcome = runner.run(command_rx, items, cancel_token).await;
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

    let _ = done_tx.send(outcome);
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
    armed: bool,
}

impl CollectionCleanupGuard {
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
            self.events.publish(DanmuEvent::CollectionStopped {
                session_id: self.session_id.clone(),
                total_count: 0,
            });
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
/// Returns whether this call was the one that removed the entry; callers use
/// that to emit `DanmuEvent::CollectionStopped` exactly once when the runner
/// task and `stop_collection` race.
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

        // The old collection must have been removed by the abort path,
        // even though the new spawn ultimately failed.
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
