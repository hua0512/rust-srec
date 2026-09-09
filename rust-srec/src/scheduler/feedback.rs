//! Retained lifecycle admission and generation-bound actor application.
//!
//! Admission is synchronous: a producer must never wait for an actor that may
//! itself be waiting for that producer's startup. Only lifecycle envelopes are
//! retained here; progress uses the ordinary lossy observer path.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::FutureExt;
use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::downloader::{DownloadManagerEvent, DownloadProgressEvent};
use crate::utils::task_supervisor::TaskSupervisor;

use super::actor::{ActorHandle, PlatformConfig, PlatformMessage, StreamerConfig, StreamerMessage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FeedbackDisposition {
    Applied,
    Superseded,
    Retired,
    MonitoringStopped,
    ActorUnavailable,
}

pub(crate) type ApplicationReceipt = oneshot::Receiver<Result<FeedbackDisposition, String>>;

#[derive(Debug)]
pub struct LifecycleEnvelope {
    pub(crate) sequence: u64,
    pub(crate) epoch: u64,
    pub(crate) current_epoch: Arc<AtomicU64>,
    pub(crate) event: Arc<DownloadManagerEvent>,
    pub(crate) lease: Arc<FeedbackPermit>,
    pub(crate) applied: oneshot::Sender<Result<FeedbackDisposition, String>>,
}

#[derive(Clone, Debug)]
pub struct DesiredConfig {
    pub(crate) generation: u64,
    pub(crate) revision: u64,
    pub(crate) config: StreamerConfig,
}

/// Finite retained lifecycle capacity, including envelopes being applied.
const MAX_FEEDBACK_ENVELOPES: usize = 1024;
const MAX_STREAMER_ENVELOPES: usize = 32;

#[derive(Debug)]
pub(crate) struct FeedbackPermit {
    streamer_id: String,
    _global: OwnedSemaphorePermit,
    _streamer: OwnedSemaphorePermit,
}

struct PendingEvent {
    permit: Arc<FeedbackPermit>,
    epoch: u64,
    current_epoch: Arc<AtomicU64>,
    sequence: u64,
    event: Arc<DownloadManagerEvent>,
    applied: oneshot::Sender<Result<FeedbackDisposition, String>>,
}

struct StreamQueue {
    events: mpsc::UnboundedSender<PendingEvent>,
    config: watch::Sender<Option<DesiredConfig>>,
    recheck: watch::Sender<u64>,
    check: watch::Sender<u64>,
    retired: CancellationToken,
}

type PlatformConfigUpdate = (ActorHandle<PlatformMessage>, PlatformConfig);

#[derive(Default)]
struct FeedbackState {
    stopped: bool,
    capacity_limit: usize,
    sequence: u64,
    targets: HashMap<String, ActorHandle<StreamerMessage>>,
    queues: HashMap<String, StreamQueue>,
    retired: HashMap<String, FeedbackDisposition>,
    active_downloads: HashMap<String, (String, String)>,
    latest_attempts: HashMap<String, (String, String)>,
    epochs: HashMap<String, Arc<AtomicU64>>,
    capacity: HashMap<String, Arc<Semaphore>>,
    platforms: HashMap<String, watch::Sender<Option<PlatformConfigUpdate>>>,
}

pub(crate) struct SchedulerFeedback {
    state: Mutex<FeedbackState>,
    targets_changed: watch::Sender<u64>,
    stop: CancellationToken,
    tasks: TaskSupervisor,
    capacity: Arc<Semaphore>,
    failure_tx: watch::Sender<Option<String>>,
    shutdown_gate: tokio::sync::Mutex<()>,
}

impl SchedulerFeedback {
    pub(crate) fn new() -> Arc<Self> {
        let stop = CancellationToken::new();
        let (targets_changed, _) = watch::channel(0);
        let (failure_tx, _) = watch::channel(None);
        Arc::new(Self {
            state: Mutex::new(FeedbackState {
                capacity_limit: MAX_FEEDBACK_ENVELOPES,
                ..FeedbackState::default()
            }),
            failure_tx,
            shutdown_gate: tokio::sync::Mutex::new(()),
            targets_changed,
            tasks: TaskSupervisor::with_cancellation(stop.clone()),
            stop,
            capacity: Arc::new(Semaphore::new(MAX_FEEDBACK_ENVELOPES)),
        })
    }

    /// Preserve room for both lifecycle envelopes of every configured slot.
    /// Shrinking download concurrency does not revoke already-reserved capacity.
    pub(crate) fn ensure_attempt_capacity(&self, attempts: usize) {
        let needed = attempts
            .saturating_mul(2)
            .saturating_add(MAX_FEEDBACK_ENVELOPES)
            .min(Semaphore::MAX_PERMITS);
        let mut state = self.state.lock();
        if needed > state.capacity_limit {
            self.capacity.add_permits(needed - state.capacity_limit);
            state.capacity_limit = needed;
        }
    }

    /// Reserve capacity before an attempt is admitted. Failure is explicit and
    /// never waits for the actor that may be awaiting this attempt's startup.
    pub(crate) fn reserve(&self, id: &str) -> Result<FeedbackPermit, String> {
        let global = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| "scheduler lifecycle capacity is full".to_owned())?;
        let capacity = self
            .state
            .lock()
            .capacity
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Semaphore::new(MAX_STREAMER_ENVELOPES)))
            .clone();
        let streamer = capacity
            .try_acquire_owned()
            .map_err(|_| "streamer lifecycle capacity is full".to_owned())?;
        Ok(FeedbackPermit {
            streamer_id: id.to_owned(),
            _global: global,
            _streamer: streamer,
        })
    }

    pub(crate) fn reserve_attempt(
        &self,
        id: &str,
    ) -> Result<(FeedbackPermit, FeedbackPermit), String> {
        Ok((self.reserve(id)?, self.reserve(id)?))
    }

    /// Retain before returning. The returned receipt observes application,
    /// never producer admission; producers must not await it during startup.
    pub(crate) fn publish_reserved(
        self: &Arc<Self>,
        event: &DownloadManagerEvent,
        permit: FeedbackPermit,
    ) -> ApplicationReceipt {
        let id = event.streamer_id().to_owned();
        let (applied, receipt) = oneshot::channel();
        if permit.streamer_id != id {
            let _ = applied.send(Err(
                "scheduler feedback reservation belongs to another streamer".to_owned(),
            ));
            return receipt;
        }
        let mut state = self.state.lock();
        // The recording owner controls identity even while monitoring is
        // retired. A later re-enable must not resurrect a completed attempt.
        let superseded = match event {
            DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
                download_id,
                session_id,
                ..
            }) => {
                state
                    .active_downloads
                    .insert(id.clone(), (download_id.clone(), session_id.clone()));
                state
                    .latest_attempts
                    .insert(id.clone(), (download_id.clone(), session_id.clone()));
                let epoch = state
                    .epochs
                    .entry(id.clone())
                    .or_insert_with(|| Arc::new(AtomicU64::new(0)));
                // Publisher updates are serialized by `state`; actors only read this marker.
                epoch.store(
                    epoch.load(Ordering::Acquire).saturating_add(1),
                    Ordering::Release,
                );
                false
            }
            DownloadManagerEvent::Terminal(terminal) => {
                let superseded = if let Some(download_id) = terminal.download_id() {
                    state
                        .latest_attempts
                        .get(&id)
                        .is_some_and(|(download, session)| {
                            download_id != download || terminal.session_id() != session
                        })
                } else {
                    state.active_downloads.contains_key(&id)
                };
                if !superseded {
                    state.active_downloads.remove(&id);
                }
                superseded
            }
            _ => true,
        };
        let disposition = if state.stopped {
            Some(FeedbackDisposition::MonitoringStopped)
        } else if let Some(disposition) = state.retired.get(&id) {
            Some(*disposition)
        } else if superseded {
            Some(FeedbackDisposition::Superseded)
        } else {
            None
        };
        if let Some(disposition) = disposition {
            let _ = applied.send(Ok(disposition));
            return receipt;
        }
        if self.stop.is_cancelled() {
            let _ = applied.send(Err("scheduler feedback owner failed".to_owned()));
            return receipt;
        }
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        let current_epoch = state
            .epochs
            .entry(id.clone())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone();
        let epoch = current_epoch.load(Ordering::Acquire);
        self.ensure_queue(&mut state, &id);
        if let Some(queue) = state.queues.get(&id)
            && let Err(error) = queue.events.send(PendingEvent {
                permit: Arc::new(permit),
                epoch,
                current_epoch,
                sequence,
                event: Arc::new(event.clone()),
                applied,
            })
        {
            let _ = error
                .0
                .applied
                .send(Err("scheduler feedback dispatcher stopped".to_owned()));
        }
        receipt
    }

    fn ensure_queue(self: &Arc<Self>, state: &mut FeedbackState, id: &str) {
        if state.queues.contains_key(id) {
            return;
        }
        // Every message holds leases from both finite capacity pools. Transport
        // admission stays synchronous so startup cannot wait for its own actor.
        let (events, receiver) = mpsc::unbounded_channel();
        let (config, desired) = watch::channel(None);
        let (recheck, checks) = watch::channel(0);
        let (check, forced_checks) = watch::channel(0);
        let retired = CancellationToken::new();
        state.queues.insert(
            id.to_owned(),
            StreamQueue {
                events,
                config,
                recheck,
                check,
                retired: retired.clone(),
            },
        );
        let owner = self.clone();
        let id = id.to_owned();
        let retry_owner = self.clone();
        let retry_id = id.clone();
        let retry_retired = retired.clone();
        self.tasks.spawn("scheduler admission retry", async move {
            let outcome =
                AssertUnwindSafe(retry_owner.admission_rechecks(retry_id, checks, retry_retired))
                    .catch_unwind()
                    .await;
            if outcome.is_err() {
                retry_owner
                    .failure_tx
                    .send_replace(Some("scheduler admission retry worker panicked".to_owned()));
                retry_owner.stop.cancel();
            }
        });
        self.tasks
            .spawn("scheduler feedback dispatcher", async move {
                let result = AssertUnwindSafe(owner.clone().dispatch(
                    id,
                    receiver,
                    desired,
                    forced_checks,
                    retired,
                ))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err("scheduler feedback dispatcher panicked".to_owned()));
                if let Err(error) = result {
                    owner.failure_tx.send_replace(Some(error));
                    owner.stop.cancel();
                }
            });
    }

    pub(crate) fn pending_streamers(&self) -> Vec<String> {
        self.state.lock().queues.keys().cloned().collect()
    }

    pub(crate) fn update_targets(&self, targets: &HashMap<String, ActorHandle<StreamerMessage>>) {
        let mut state = self.state.lock();
        let changed = targets.len() != state.targets.len()
            || targets.iter().any(|(id, handle)| {
                state
                    .targets
                    .get(id)
                    .is_none_or(|old| old.generation() != handle.generation())
            });
        if changed {
            state.targets = targets.clone();
            self.targets_changed
                .send_modify(|revision| *revision = revision.wrapping_add(1));
        }
    }

    pub(crate) fn activate(&self, id: &str) {
        self.state.lock().retired.remove(id);
    }

    pub(crate) fn retire(&self, id: &str) {
        self.stop_streamer(id, FeedbackDisposition::Retired);
    }

    pub(crate) fn unavailable(&self, id: &str) {
        self.stop_streamer(id, FeedbackDisposition::ActorUnavailable);
    }

    fn retirement_disposition(&self, id: &str) -> FeedbackDisposition {
        self.state
            .lock()
            .retired
            .get(id)
            .copied()
            .unwrap_or(FeedbackDisposition::Retired)
    }

    fn stop_streamer(&self, id: &str, disposition: FeedbackDisposition) {
        let mut state = self.state.lock();
        state.retired.insert(id.to_owned(), disposition);
        state.targets.remove(id);
        if let Some(queue) = state.queues.remove(id) {
            queue.retired.cancel();
        }
        self.targets_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub(crate) fn configure(self: &Arc<Self>, id: &str, config: DesiredConfig) {
        let mut state = self.state.lock();
        if state.stopped || state.retired.contains_key(id) {
            return;
        }
        self.ensure_queue(&mut state, id);
        if let Some(queue) = state.queues.get(id) {
            queue.config.send_replace(Some(config));
        }
    }

    pub(crate) fn active_download(&self, id: &str) -> Option<(String, String)> {
        self.state.lock().active_downloads.get(id).cloned()
    }

    pub(crate) fn is_current_download(&self, id: &str, download: &str, session: &str) -> bool {
        self.state
            .lock()
            .active_downloads
            .get(id)
            .is_some_and(|current| current.0 == download && current.1 == session)
    }

    pub(crate) async fn failure(&self) -> String {
        let mut changes = self.failure_tx.subscribe();
        loop {
            if let Some(error) = changes.borrow().clone() {
                return error;
            }
            if changes.changed().await.is_err() {
                return "scheduler feedback failure channel closed".to_owned();
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn drain_configurations(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let pending = self
                    .state
                    .lock()
                    .queues
                    .values()
                    .any(|queue| queue.config.borrow().is_some());
                if !pending {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retained configuration must reach the test actors");
    }

    pub(crate) fn request_recheck(self: &Arc<Self>, id: &str) {
        let mut state = self.state.lock();
        if state.stopped || state.retired.contains_key(id) {
            return;
        }
        self.ensure_queue(&mut state, id);
        if let Some(queue) = state.queues.get(id) {
            queue
                .recheck
                .send_modify(|revision| *revision = revision.wrapping_add(1));
        }
    }

    pub(crate) fn request_check(self: &Arc<Self>, id: &str) {
        let mut state = self.state.lock();
        if state.stopped || state.retired.contains_key(id) {
            return;
        }
        self.ensure_queue(&mut state, id);
        if let Some(queue) = state.queues.get(id) {
            queue
                .check
                .send_modify(|revision| *revision = revision.wrapping_add(1));
        }
    }

    pub(crate) fn configure_platform(
        &self,
        id: &str,
        handle: ActorHandle<PlatformMessage>,
        config: PlatformConfig,
    ) {
        let mut state = self.state.lock();
        if state.stopped {
            return;
        }
        let sender = state.platforms.entry(id.to_owned()).or_insert_with(|| {
            let (sender, mut receiver) =
                watch::channel::<Option<(ActorHandle<PlatformMessage>, PlatformConfig)>>(None);
            let stop = self.stop.clone();
            self.tasks.spawn("platform configuration delivery", async move {
                loop {
                    let desired = receiver.borrow_and_update().clone();
                    if let Some((handle, config)) = desired {
                        tokio::select! {
                            biased;
                            _ = stop.cancelled() => return,
                            changed = receiver.changed() => { if changed.is_err() { return; } continue; },
                            result = handle.send_reliable(PlatformMessage::ConfigUpdate(config)) => {
                                if let Err(error) = result {
                                    tracing::debug!(%error, "Platform generation stopped before configuration delivery; restart uses the retained configuration");
                                }
                            }
                        }
                    }
                    tokio::select! {
                        biased;
                        _ = stop.cancelled() => return,
                        changed = receiver.changed() => if changed.is_err() { return; }
                    }
                }
            });
            sender
        });
        sender.send_replace(Some((handle, config)));
    }

    pub(crate) fn monitoring_stopped(&self) -> bool {
        self.state.lock().stopped
    }

    pub(crate) fn stop_monitoring(&self) {
        self.state.lock().stopped = true;
        self.stop.cancel();
    }

    pub(crate) async fn shutdown(&self) {
        self.stop_monitoring();
        let _drain = self.shutdown_gate.lock().await;
        self.tasks.shutdown(Duration::from_secs(2)).await;
    }

    pub(crate) async fn abort(&self, deadline: tokio::time::Instant) {
        self.stop_monitoring();
        if let Ok(_drain) = tokio::time::timeout_at(deadline, self.shutdown_gate.lock()).await {
            self.tasks.abort_all(deadline).await;
        }
    }

    async fn dispatch(
        self: Arc<Self>,
        id: String,
        mut events: mpsc::UnboundedReceiver<PendingEvent>,
        mut config: watch::Receiver<Option<DesiredConfig>>,
        mut forced_checks: watch::Receiver<u64>,
        retired: CancellationToken,
    ) -> Result<(), String> {
        let mut targets = self.targets_changed.subscribe();
        loop {
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => break,
                _ = retired.cancelled() => break,
                event = events.recv() => {
                    let Some(event) = event else { break; };
                    let result = self.deliver(&id, &event, &mut targets, &retired).await;
                    let failed = result.as_ref().err().cloned();
                    let _ = event.applied.send(result);
                    if let Some(error) = failed { return Err(error); }
                }
                changed = forced_checks.changed() => {
                    if changed.is_err() { break; }
                    forced_checks.borrow_and_update();
                    self.deliver_control(&id, false, &mut targets, &retired).await;
                }
                changed = config.changed() => {
                    if changed.is_err() { break; }
                    let desired = config.borrow_and_update().clone();
                    if let Some(desired) = desired {
                        self.deliver_configuration(&id, &desired, &mut targets, &retired).await;
                        let state = self.state.lock();
                        if let Some(queue) = state.queues.get(&id) {
                            queue.config.send_if_modified(|current| {
                                if current.as_ref().is_some_and(|current| current.revision == desired.revision && current.generation == desired.generation) { *current = None; true } else { false }
                            });
                        }
                    }
                }
            }
        }
        let disposition = if retired.is_cancelled() {
            self.retirement_disposition(&id)
        } else {
            FeedbackDisposition::MonitoringStopped
        };
        events.close();
        while let Some(event) = events.recv().await {
            let _ = event.applied.send(Ok(disposition));
        }
        Ok(())
    }

    /// Wait outside the lifecycle dispatcher: that dispatcher must remain free
    /// to apply the envelopes whose leases admission is waiting to recover.
    async fn admission_rechecks(
        &self,
        id: String,
        mut requests: watch::Receiver<u64>,
        retired: CancellationToken,
    ) {
        let mut targets = self.targets_changed.subscribe();
        loop {
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => return,
                _ = retired.cancelled() => return,
                changed = requests.changed() => if changed.is_err() { return; }
            }
            let streamer = self
                .state
                .lock()
                .capacity
                .entry(id.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(MAX_STREAMER_ENVELOPES)))
                .clone();
            let recovered = async {
                let streamer = streamer.acquire_many_owned(2).await.ok()?;
                let global = self.capacity.clone().acquire_many_owned(2).await.ok()?;
                Some((streamer, global))
            };
            let capacity = tokio::select! {
                biased;
                _ = self.stop.cancelled() => return,
                _ = retired.cancelled() => return,
                capacity = recovered => capacity,
            };
            let Some(capacity) = capacity else {
                return;
            };
            // All overload requests received during pressure represent one
            // retry. Release the probe leases before the actor starts again.
            requests.borrow_and_update();
            drop(capacity);
            self.deliver_control(&id, true, &mut targets, &retired)
                .await;
        }
    }

    async fn deliver_control(
        &self,
        id: &str,
        admission_retry: bool,
        targets: &mut watch::Receiver<u64>,
        retired: &CancellationToken,
    ) {
        loop {
            targets.borrow_and_update();
            let target = self.state.lock().targets.get(id).cloned();
            if let Some(target) = target {
                let message = if admission_retry {
                    StreamerMessage::RetryAdmission
                } else {
                    StreamerMessage::CheckStatus
                };
                tokio::select! {
                    biased;
                    _ = self.stop.cancelled() => return,
                    _ = retired.cancelled() => return,
                    result = target.send_reliable(message) => if result.is_ok() { return; }
                }
            }
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => return,
                _ = retired.cancelled() => return,
                changed = targets.changed() => if changed.is_err() { return; }
            }
        }
    }

    async fn deliver_configuration(
        &self,
        id: &str,
        desired: &DesiredConfig,
        targets: &mut watch::Receiver<u64>,
        retired: &CancellationToken,
    ) {
        targets.borrow_and_update();
        let target = self.state.lock().targets.get(id).cloned();
        let Some(target) = target.filter(|target| target.generation() == desired.generation) else {
            return;
        };
        let (applied, receipt) = oneshot::channel();
        let delivery = async {
            if target
                .send_reliable(StreamerMessage::RetainedConfig {
                    config: desired.clone(),
                    applied,
                })
                .await
                .is_ok()
                && let Err(error) = receipt.await
            {
                tracing::debug!(%error, "Actor generation stopped before configuration acknowledgement");
            }
        };
        tokio::pin!(delivery);
        loop {
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => return,
                _ = retired.cancelled() => return,
                changed = targets.changed() => {
                    if changed.is_err() { return; }
                    if self.state.lock().targets.get(id).is_none_or(|current| current.generation() != desired.generation) { return; }
                }
                _ = &mut delivery => return,
            }
        }
    }

    async fn deliver(
        &self,
        id: &str,
        event: &PendingEvent,
        targets: &mut watch::Receiver<u64>,
        retired: &CancellationToken,
    ) -> Result<FeedbackDisposition, String> {
        'retry: loop {
            if event.current_epoch.load(Ordering::Acquire) != event.epoch {
                return Ok(FeedbackDisposition::Superseded);
            }
            targets.borrow_and_update();
            let target = self.state.lock().targets.get(id).cloned();
            if let Some(target) = target {
                let (applied, receipt) = oneshot::channel();
                let message = StreamerMessage::LifecycleFeedback(Box::new(LifecycleEnvelope {
                    sequence: event.sequence,
                    epoch: event.epoch,
                    current_epoch: event.current_epoch.clone(),
                    event: event.event.clone(),
                    lease: event.permit.clone(),
                    applied,
                }));
                let delivery = async {
                    if target.send_reliable(message).await.is_err() {
                        return None;
                    }
                    receipt.await.ok()
                };
                tokio::pin!(delivery);
                loop {
                    tokio::select! {
                        biased;
                        _ = self.stop.cancelled() => return Ok(FeedbackDisposition::MonitoringStopped),
                        _ = retired.cancelled() => return Ok(self.retirement_disposition(id)),
                        changed = targets.changed() => {
                            if changed.is_err() { return Err("scheduler target registry stopped".to_owned()); }
                            let generation = self.state.lock().targets.get(id).map(|handle| handle.generation());
                            if generation != Some(target.generation()) {
                                continue 'retry;
                            }
                        }
                        result = &mut delivery => {
                            if let Some(result) = result { return result; }
                            break;
                        }
                    }
                }
                // The registry owns replacement. Retain the event while its
                // completion is reaped and a fresh generation is resolved.
            }
            if targets.has_changed().unwrap_or(false) {
                continue;
            }
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => return Ok(FeedbackDisposition::MonitoringStopped),
                _ = retired.cancelled() => return Ok(self.retirement_disposition(id)),
                changed = targets.changed() => if changed.is_err() { return Err("scheduler target registry stopped".to_owned()); }
            }
        }
    }
}

#[cfg(test)]
mod tests;
