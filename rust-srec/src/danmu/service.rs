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
use crate::database::models::{DanmuRateEntry, GiftTallyEntry, TopTalkerEntry, WordFrequencyEntry};
use crate::database::repositories::SessionRepository;
use crate::error::{Error, Result};
use platforms_parser::danmaku::ConnectionConfig;

use super::events::{CollectionCommand, DanmuEvent};
use super::runner::{CollectionRunner, RunnerParams};

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
    /// Signals when the runner has fully stopped (including final XML
    /// flush/finalize), carrying its final statistics.
    done_rx: Option<oneshot::Receiver<DanmuStatistics>>,
}

/// Danmu collection service.
pub struct DanmuService {
    /// Provider registry
    providers: Arc<ProviderRegistry>,
    /// Active collections (session_id -> state)
    collections: Arc<DashMap<String, CollectionState>>,
    /// Reverse index for fast lookups (streamer_id -> session_id).
    sessions_by_streamer: Arc<DashMap<String, String>>,
    /// Event sender
    event_tx: broadcast::Sender<DanmuEvent>,
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
    const MAX_TOP_TALKERS: usize = 32;
    const MAX_WORDS: usize = 50;
    const RATE_BUCKET_SECS: u64 = 10;

    /// Create a new danmu service.
    pub fn new() -> Self {
        Self::with_providers(ProviderRegistry::with_defaults())
    }

    /// Create a new danmu service with custom providers.
    pub fn with_providers(providers: ProviderRegistry) -> Self {
        let (event_tx, _) = broadcast::channel(256);

        Self {
            providers: Arc::new(providers),
            collections: Arc::new(DashMap::new()),
            sessions_by_streamer: Arc::new(DashMap::new()),
            event_tx,
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

    /// Get the session repository (if set).
    pub fn session_repo(
        &self,
    ) -> Option<&Arc<dyn crate::database::repositories::SessionRepository>> {
        self.session_repo.as_ref()
    }

    /// Subscribe to danmu events.
    pub fn subscribe(&self) -> broadcast::Receiver<DanmuEvent> {
        self.event_tx.subscribe()
    }

    /// Start danmu collection for a session.
    /// Returns a handle that can be used to control segment file writing.
    pub async fn start_collection(
        &self,
        session_id: &str,
        streamer_id: &str,
        streamer_url: &str,
        cookies: Option<String>,
        extras: Option<std::collections::HashMap<String, String>>,
    ) -> Result<CollectionHandle> {
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

        // Check if already collecting
        if self.collections.contains_key(session_id) {
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
            .get(streamer_id)
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
        let provider = self.providers.get_by_url(streamer_url).ok_or_else(|| {
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
                    .or_else(|| provider.extract_room_id(streamer_url))
            }
            "douyin" => {
                // Douyin uses id_str (room_id) for danmu connection
                extras
                    .as_ref()
                    .and_then(|e| e.get("id_str"))
                    .cloned()
                    .or_else(|| provider.extract_room_id(streamer_url))
            }
            "douyu" => {
                // Douyu uses rid for danmu connection
                extras
                    .as_ref()
                    .and_then(|e| e.get("rid"))
                    .cloned()
                    .or_else(|| provider.extract_room_id(streamer_url))
            }
            "soop" => {
                // SOOP chat path uses bj id; chat host/FTK arrive via MediaInfo extras.
                extras
                    .as_ref()
                    .and_then(|e| e.get("bjid").or_else(|| e.get("channel_id")))
                    .cloned()
                    .or_else(|| provider.extract_room_id(streamer_url))
            }
            "bigo" => {
                // Bigo WS enter needs studio roomId (not siteId from the URL)
                extras
                    .as_ref()
                    .and_then(|e| e.get("room_id"))
                    .cloned()
                    .or_else(|| provider.extract_room_id(streamer_url))
            }
            _ => provider.extract_room_id(streamer_url),
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

        // Build bounded per-session statistics state, resuming from a checkpoint
        // when this session was already being collected before a restart.
        let stats = super::checkpoint::load_or_new(
            self.session_repo.as_ref(),
            session_id,
            platforms_parser::danmaku::StatisticsConfig {
                top_talkers: Self::MAX_TOP_TALKERS,
                top_words: Self::MAX_WORDS,
                rate_bucket_secs: Self::RATE_BUCKET_SECS,
                ..Default::default()
            },
        )
        .await;
        let cancel_token = self.cancel_token.child_token();

        let (ready_tx, ready_rx) = oneshot::channel::<Result<()>>();
        let (done_tx, done_rx) = oneshot::channel::<DanmuStatistics>();

        let state = CollectionState {
            streamer_id: streamer_id.to_string(),
            cancel_token: cancel_token.clone(),
            command_tx: command_tx.clone(),
            done_rx: Some(done_rx),
        };

        self.collections.insert(session_id.to_string(), state);
        self.sessions_by_streamer
            .insert(streamer_id.to_string(), session_id.to_string());

        // Start collection task
        let session_id_clone = session_id.to_string();
        let streamer_id_clone = streamer_id.to_string();
        let room_id_clone = room_id.clone();
        let event_tx = self.event_tx.clone();
        let collections = self.collections.clone();
        let sessions_by_streamer = self.sessions_by_streamer.clone();
        let session_repo = self.session_repo.clone();
        let provider = Arc::clone(&provider);
        let conn_config = connection_config;
        let cancel_token_task = cancel_token.clone();

        tokio::spawn(async move {
            let (runner, items) = match tokio::time::timeout(
                CONNECT_TIMEOUT,
                CollectionRunner::new(RunnerParams {
                    session_id: session_id_clone.clone(),
                    streamer_id: streamer_id_clone.clone(),
                    room_id: room_id_clone,
                    provider: Arc::clone(&provider),
                    conn_config,
                    stats,
                    session_repo: session_repo.clone(),
                    event_tx: event_tx.clone(),
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
                    let _ = event_tx.send(DanmuEvent::Error {
                        session_id: session_id_clone.clone(),
                        error: error_message,
                    });
                    let _ = ready_tx.send(Err(e));
                    remove_collection(&collections, &sessions_by_streamer, &session_id_clone);
                    // No `CollectionStopped` here on purpose: collection never
                    // started, so `PipelineCoordinator` never set
                    // `danmu_observed` and the session-complete gate does not
                    // wait on a danmu arm.
                    return;
                }
                Err(_) => {
                    let message = format!(
                        "Danmu connection timed out after {:?} (session_id={})",
                        CONNECT_TIMEOUT, session_id_clone
                    );
                    let _ = ready_tx.send(Err(Error::from(
                        platforms_parser::danmaku::DanmakuError::connection(message.clone()),
                    )));
                    let _ = event_tx.send(DanmuEvent::Error {
                        session_id: session_id_clone.clone(),
                        error: message,
                    });
                    remove_collection(&collections, &sessions_by_streamer, &session_id_clone);
                    return;
                }
            };

            let outcome = runner.run(command_rx, items, cancel_token_task).await;
            if let Some(error) = &outcome.error {
                let _ = event_tx.send(DanmuEvent::Error {
                    session_id: session_id_clone.clone(),
                    error: error.to_string(),
                });
            }

            // Clean up the in-memory state when the runner exits on its own — a
            // transport failure, a `StreamClosed` control event, or external
            // cancellation — rather than through `stop_collection`.
            //
            // `CollectionStopped` is emitted on every one of those paths, failure
            // included: `PipelineCoordinator::apply_event` treats it as the only
            // signal that the session's danmu arm is finished (it is the sole
            // writer of `danmu_complete`), so withholding it after a failure
            // leaves `CreateSessionCompleteDag` permanently ungated for the
            // session. The statistics are final either way.
            //
            // If `stop_collection` already removed the entry it emits the event
            // itself, and `remove` returning None keeps this from duplicating it.
            // Only a session that is genuinely over releases its checkpoint;
            // collection interrupted by a process shutdown keeps it so the next
            // start resumes rather than recounting from zero.
            if outcome.session_ended {
                super::checkpoint::discard(session_repo.as_ref(), &session_id_clone).await;
            }

            if remove_collection(&collections, &sessions_by_streamer, &session_id_clone) {
                persist_statistics(
                    session_repo.as_deref(),
                    &session_id_clone,
                    &outcome.statistics,
                )
                .await;
                let _ = event_tx.send(DanmuEvent::CollectionStopped {
                    session_id: session_id_clone.clone(),
                    total_count: outcome.statistics.total_count,
                });
            }

            let _ = done_tx.send(outcome.statistics);
        });

        tokio::select! {
            ready = ready_rx => {
                match ready {
                    Ok(Ok(())) => {
                        let _ = self.event_tx.send(DanmuEvent::CollectionStarted {
                            session_id: session_id.to_string(),
                            streamer_id: streamer_id.to_string(),
                        });
                    }
                    Ok(Err(e)) => {
                        remove_collection(&self.collections, &self.sessions_by_streamer, session_id);
                        return Err(e);
                    }
                    Err(_) => {
                        remove_collection(&self.collections, &self.sessions_by_streamer, session_id);
                        return Err(Error::from(platforms_parser::danmaku::DanmakuError::connection(
                            "Danmu collection task stopped before it became ready",
                        )));
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                remove_collection(&self.collections, &self.sessions_by_streamer, session_id);
                return Err(Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    "Danmu collection cancelled before it became ready",
                )));
            }
        }

        Ok(CollectionHandle {
            session_id: session_id.to_string(),
            command_tx,
        })
    }

    /// Stop danmu collection for a session.
    pub async fn stop_collection(&self, session_id: &str) -> Result<DanmuStatistics> {
        // Get and remove state
        let (_, state) = self.collections.remove(session_id).ok_or_else(|| {
            Error::from(platforms_parser::danmaku::DanmakuError::connection(
                format!("No active collection for session {}", session_id),
            ))
        })?;

        release_streamer_slot(&self.sessions_by_streamer, &state.streamer_id, session_id);

        // Send stop command
        if state
            .command_tx
            .send(CollectionCommand::Stop)
            .await
            .is_err()
        {
            tracing::debug!(session_id, "Danmu collection task already stopped");
        }

        // Cancel the collection task
        state.cancel_token.cancel();

        if let Some(done_rx) = state.done_rx {
            const STOP_TIMEOUT: Duration = Duration::from_secs(10);
            match tokio::time::timeout(STOP_TIMEOUT, done_rx).await {
                Ok(Ok(statistics)) => {
                    persist_statistics(self.session_repo.as_deref(), session_id, &statistics).await;
                    let _ = self.event_tx.send(DanmuEvent::CollectionStopped {
                        session_id: session_id.to_string(),
                        total_count: statistics.total_count,
                    });
                    return Ok(statistics);
                }
                // The runner task ended without reporting. It removed its own
                // entry and emitted `CollectionStopped` on that path, so there is
                // nothing left to do here.
                Ok(Err(_)) => {
                    tracing::debug!(
                        session_id,
                        "Danmu collection task ended without reporting statistics"
                    );
                    return Ok(DanmuStatistics::default());
                }
                Err(_) => {
                    warn!(
                        "Danmu collection stop timed out after {:?} (session_id={})",
                        STOP_TIMEOUT, session_id
                    );
                }
            }
        }

        // Reached only on the stop timeout above, where the runner is wedged and
        // may never report. Emit `CollectionStopped` anyway: it is the sole writer
        // of the coordinator's `danmu_complete`, so staying silent would leave the
        // session-complete pipeline ungated forever. A late duplicate from the
        // runner is harmless — finalization is idempotent.
        let _ = self.event_tx.send(DanmuEvent::CollectionStopped {
            session_id: session_id.to_string(),
            total_count: 0,
        });

        Ok(DanmuStatistics::default())
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
            if let Err(error) = self.stop_collection(&session_id).await {
                warn!(session_id, %error, "Failed to stop danmu collection during shutdown");
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

fn saturating_u64_to_i64(value: u64) -> i64 {
    if value > i64::MAX as u64 {
        i64::MAX
    } else {
        value as i64
    }
}

/// Serialize and upsert danmu statistics for a session.
///
/// Called from `stop_collection`/runner-exit with final statistics, and from
/// `CollectionRunner::run`'s periodic persist tick with in-progress snapshots,
/// so `GET /api/sessions/{id}/danmu-statistics` reflects live sessions and a
/// crash loses at most one persist interval of data.
pub(super) async fn persist_statistics(
    session_repo: Option<&dyn SessionRepository>,
    session_id: &str,
    statistics: &DanmuStatistics,
) {
    let Some(repo) = session_repo else {
        return;
    };

    // Serialize each aggregate independently: a failed field degrades to NULL
    // instead of dropping the whole upsert.
    fn to_json_field<T: serde::Serialize>(
        session_id: &str,
        field: &'static str,
        value: &T,
    ) -> Option<String> {
        match serde_json::to_string(value) {
            Ok(value) => Some(value),
            Err(error) => {
                warn!(session_id, field, %error, "Failed to serialize danmu statistics field");
                None
            }
        }
    }

    let rate_timeseries: Vec<_> = statistics
        .rate_timeseries
        .iter()
        .map(|entry| DanmuRateEntry {
            ts: entry.timestamp.timestamp_millis(),
            count: saturating_u64_to_i64(entry.count),
        })
        .collect();

    let to_talker_entry = |entry: &platforms_parser::danmaku::TopTalker| TopTalkerEntry {
        user_id: entry.user_id.clone(),
        username: entry.username.clone(),
        message_count: saturating_u64_to_i64(entry.message_count),
        error: saturating_u64_to_i64(entry.error),
    };

    let top_talkers: Vec<_> = statistics.top_talkers.iter().map(to_talker_entry).collect();
    let top_gifters: Vec<_> = statistics.top_gifters.iter().map(to_talker_entry).collect();

    let top_gifts: Vec<_> = statistics
        .top_gifts
        .iter()
        .map(|entry| GiftTallyEntry {
            name: entry.name.clone(),
            count: saturating_u64_to_i64(entry.count),
        })
        .collect();

    let word_frequency: Vec<_> = statistics
        .word_frequency
        .iter()
        .map(|entry| WordFrequencyEntry {
            word: entry.word.clone(),
            count: saturating_u64_to_i64(entry.count),
            error: saturating_u64_to_i64(entry.error),
        })
        .collect();

    let mut model = crate::database::models::DanmuStatisticsDbModel::new(session_id);
    model.total_danmus = saturating_u64_to_i64(statistics.total_count);
    model.unique_talkers = Some(saturating_u64_to_i64(statistics.unique_talkers));
    model.chat_count = Some(saturating_u64_to_i64(statistics.chat_count));
    model.gift_count = Some(saturating_u64_to_i64(statistics.gift_count));
    model.duration_secs = Some(saturating_u64_to_i64(statistics.duration_secs));
    model.start_time = statistics.start_time.map(|time| time.timestamp_millis());
    model.end_time = statistics.end_time.map(|time| time.timestamp_millis());
    model.rate_bucket_secs = Some(saturating_u64_to_i64(statistics.bucket_duration_secs));
    model.danmu_rate_timeseries = to_json_field(session_id, "rate_timeseries", &rate_timeseries);
    model.top_talkers = to_json_field(session_id, "top_talkers", &top_talkers);
    model.top_gifters = to_json_field(session_id, "top_gifters", &top_gifters);
    model.top_gifts = to_json_field(session_id, "top_gifts", &top_gifts);
    model.word_frequency = to_json_field(session_id, "word_frequency", &word_frequency);

    if let Err(error) = repo.upsert_danmu_statistics(&model).await {
        warn!(session_id, %error, "Failed to persist danmu statistics");
    }
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
    ) -> oneshot::Sender<DanmuStatistics> {
        let (command_tx, _command_rx) = mpsc::channel(8);
        let (done_tx, done_rx) = oneshot::channel();
        let cancel_token = service.cancel_token.child_token();
        let state = CollectionState {
            streamer_id: streamer_id.to_string(),
            cancel_token,
            command_tx,
            done_rx: Some(done_rx),
        };
        service.collections.insert(session_id.to_string(), state);
        service
            .sessions_by_streamer
            .insert(streamer_id.to_string(), session_id.to_string());
        done_tx
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
        let service = DanmuService::new();
        let streamer_id = "streamer-1";
        let old_session = "session-old";
        let new_session = "session-new";

        // Seed a prior collection for the streamer. Resolve the done_tx so
        // `stop_collection`'s await returns promptly instead of waiting on
        // its 10s timeout.
        let done_tx = seed_active_collection(&service, streamer_id, old_session);
        done_tx.send(DanmuStatistics::default()).unwrap();

        assert!(
            service.is_collecting(old_session),
            "fixture: prior collector seeded"
        );
        assert_eq!(
            service.get_session_by_streamer(streamer_id).as_deref(),
            Some(old_session)
        );

        // `start_collection` will fail at provider lookup for our fake URL
        // (no provider is registered for `https://example.com/test`). The
        // abort logic runs *before* the provider lookup, so by the time the
        // call returns the error, the prior collector should already be gone.
        let outcome = service
            .start_collection(
                new_session,
                streamer_id,
                "https://example.com/test",
                None,
                None,
            )
            .await;
        assert!(
            outcome.is_err(),
            "expected provider-lookup failure for fake URL"
        );

        // The old collection must have been removed by the abort path,
        // even though the new spawn ultimately failed.
        assert!(
            !service.is_collecting(old_session),
            "prior collector for {old_session} must have been aborted"
        );
        assert!(
            !service.is_collecting(new_session),
            "the new spawn failed at provider lookup so its slot must be empty too"
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
            .start_collection("session-1", "streamer-1", FakeProvider::URL, None, None)
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

    /// Calling `start_collection` twice with the same `session_id` keeps
    /// the existing "already active" error path. The new abort logic must
    /// not fire for self-replace because of the `old_sid != session_id`
    /// guard.
    #[tokio::test]
    async fn start_collection_idempotent_for_same_session_id() {
        let service = DanmuService::new();
        let streamer_id = "streamer-1";
        let session_id = "session-1";

        let _done_tx = seed_active_collection(&service, streamer_id, session_id);

        let result = service
            .start_collection(
                session_id,
                streamer_id,
                "https://example.com/test",
                None,
                None,
            )
            .await;

        assert!(
            result.is_err(),
            "second start_collection for same session must return the 'already active' error"
        );
        // The seeded collector must still be present — the early-return at
        // line 250 short-circuits before the abort logic could touch it.
        assert!(service.is_collecting(session_id));
    }
}
