//! Non-blocking single-streamer actor driving [`StreamerFsm`].
//!
//! One [`tokio::select!`] loop concurrently polls:
//! - the actor's mailbox,
//! - at most one in-flight `CommitDb` future (owned by `committed_write_supervisor`
//!   through [`RecorderStore`]),
//! - at most one in-flight `Probe` future,
//! - an optional batch-precheck live-hint `watch::Receiver<bool>`,
//! - the earliest [`StreamerFsm::next_deadline`] timer, and
//! - completion of asynchronous segment/pipeline side-effect tasks.
//!
//! Neither database commits nor engine/danmu stops ever block outside `select!`.

pub mod effects;
pub mod startup_task;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use jiff::Timestamp;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tracing::{debug, trace, warn};

use crate::config::ConfigService;
use crate::danmu::DanmuService;
use crate::database::repositories::config::SqlxConfigRepository;
use crate::database::repositories::streamer::SqlxStreamerRepository;
use crate::downloader::{DownloadManager, RecorderFeedbackPermit};
use crate::recorder::fsm::{FsmInput, StepContext, StreamerFsm};
use crate::recorder::ports::{
    ActorStopReason, DbCommitFailure, DbCommitOutcome, InitialStreamerSnapshot, ProbeFailure,
    ProbeStatus, RecorderPipelinePort, RecorderStorePort, StreamerRuntimeView,
};
use crate::recorder::prober::RecorderProber;
use crate::recorder::store::RecorderStore;
use crate::streamer::StreamerManager;

use self::effects::{
    LoopEffectSignals, dispatch_danmu_fifo_effects, dispatch_effects,
    dispatch_segment_completed_effects, dispatch_segment_started_effects, publish_runtime_view,
};
use self::startup_task::StartupLeaseRegistry;

type BoxedCommitFuture =
    Pin<Box<dyn Future<Output = std::result::Result<DbCommitOutcome, DbCommitFailure>> + Send>>;
type ProbeFutureOutput = crate::Result<(
    std::result::Result<ProbeStatus, ProbeFailure>,
    Option<watch::Receiver<bool>>,
)>;
type BoxedProbeFuture = Pin<Box<dyn Future<Output = ProbeFutureOutput> + Send>>;

/// Messages delivered to a [`StreamerActor`]'s mailbox.
pub(crate) enum ActorMessage {
    /// Standard FSM input with no caller receipt.
    Input(FsmInput),
    /// Reserved-capacity download lifecycle delivery (`AttemptStarted` / `DownloadEnded`),
    /// holding its bounded permit until applied to the FSM.
    ReservedDownload {
        input: FsmInput,
        _permit: RecorderFeedbackPermit,
    },
    /// Required `SegmentStarted` event carrying its acknowledgement channel.
    SegmentStarted {
        input: FsmInput,
        ack: oneshot::Sender<std::result::Result<(), String>>,
    },
    /// Required `SegmentCompleted` event carrying its SQLite persistence acknowledgement.
    SegmentCompleted {
        input: FsmInput,
        ack: oneshot::Sender<std::result::Result<(), String>>,
    },
    /// Required FIFO danmu coordination event carrying its completion receipt.
    DanmuFifo {
        input: FsmInput,
        done: oneshot::Sender<std::result::Result<(), String>>,
    },
}

/// Shared runtime services injected into every [`StreamerActor`].
#[derive(Clone)]
pub(crate) struct ActorRuntimeDeps {
    pub store: Arc<RecorderStore>,
    pub prober: Arc<RecorderProber>,
    pub download_manager: Arc<DownloadManager>,
    pub danmu_service: Arc<DanmuService>,
    pub pipeline: Arc<dyn RecorderPipelinePort>,
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    pub config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    pub leases: Arc<StartupLeaseRegistry>,
    pub runtime_views: Arc<DashMap<String, StreamerRuntimeView>>,
}

/// Lightweight handle to a running [`StreamerActor`].
#[derive(Clone)]
pub(crate) struct ActorHandle {
    pub tx: mpsc::UnboundedSender<ActorMessage>,
}

impl ActorHandle {
    pub(crate) fn send_input(&self, input: FsmInput) -> bool {
        self.tx.send(ActorMessage::Input(input)).is_ok()
    }

    pub(crate) fn send_message(&self, msg: ActorMessage) -> bool {
        self.tx.send(msg).is_ok()
    }
}

/// Single-streamer actor owning one [`StreamerFsm`] and driving its effects
/// non-blockingly inside a single [`tokio::select!`] loop.
pub(crate) struct StreamerActor {
    streamer_id: String,
    generation: u64,
    fsm: StreamerFsm,
    rx: mpsc::UnboundedReceiver<ActorMessage>,
    self_tx: mpsc::UnboundedSender<ActorMessage>,
    deps: ActorRuntimeDeps,
    in_flight_commit: Option<(u64, BoxedCommitFuture)>,
    in_flight_probe: Option<(u64, BoxedProbeFuture)>,
    batch_live_rx: Option<watch::Receiver<bool>>,
    effects_tasks: JoinSet<()>,
}

impl StreamerActor {
    /// Construct a new [`StreamerActor`] and its mailbox [`ActorHandle`].
    pub(crate) fn new(
        generation: u64,
        snapshot: InitialStreamerSnapshot,
        deps: ActorRuntimeDeps,
    ) -> (Self, ActorHandle) {
        let now = Instant::now();
        let wall_now = Timestamp::now();
        let streamer_id = snapshot.streamer_id.clone();
        let fsm = StreamerFsm::new(snapshot, now, wall_now);
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = ActorHandle { tx: tx.clone() };
        let actor = Self {
            streamer_id,
            generation,
            fsm,
            rx,
            self_tx: tx,
            deps,
            in_flight_commit: None,
            in_flight_probe: None,
            batch_live_rx: None,
            effects_tasks: JoinSet::new(),
        };
        (actor, handle)
    }

    #[cfg(test)]
    pub(crate) fn fsm_mut(&mut self) -> &mut StreamerFsm {
        &mut self.fsm
    }

    /// Run the actor loop until [`FsmEffect::ActorStopped`] is emitted or the
    /// mailbox closes.
    pub(crate) async fn run(mut self) -> ActorStopReason {
        publish_runtime_view(&self.deps.runtime_views, self.fsm.runtime_view());
        trace!(
            streamer_id = %self.streamer_id,
            generation = self.generation,
            "StreamerActor started"
        );

        let stop_reason = loop {
            let now = Instant::now();
            let wall_now = Timestamp::now();
            let next_deadline = self
                .fsm
                .next_deadline(now, wall_now)
                .map(tokio::time::Instant::from_std);

            let stopped = tokio::select! {
                msg = self.rx.recv() => {
                    match msg {
                        Some(msg) => self.handle_message(msg),
                        None => Some(ActorStopReason::Shutdown),
                    }
                }
                commit_done = poll_in_flight_commit(&mut self.in_flight_commit), if self.in_flight_commit.is_some() => {
                    let (op_id, result) = commit_done;
                    self.step_and_dispatch(FsmInput::DbCommitted { op_id, result })
                }
                probe_done = poll_in_flight_probe(&mut self.in_flight_probe), if self.in_flight_probe.is_some() => {
                    let (probe_id, outcome) = probe_done;
                    match outcome {
                        Ok((result, live_hint)) => {
                            if let Some(hint) = live_hint {
                                self.batch_live_rx = Some(hint);
                            } else if !matches!(result, Ok(ProbeStatus::Offline { .. })) {
                                self.batch_live_rx = None;
                            }
                            self.step_and_dispatch(FsmInput::ProbeCompleted { probe_id, result })
                        }
                        Err(error) => {
                            self.batch_live_rx = None;
                            self.step_and_dispatch(FsmInput::ProbeCompleted {
                                probe_id,
                                result: Err(ProbeFailure::Transient {
                                    message: error.to_string(),
                                }),
                            })
                        }
                    }
                }
                hint_ready = wait_batch_live_hint(&mut self.batch_live_rx), if self.batch_live_rx.is_some() => {
                    if hint_ready {
                        self.batch_live_rx = None;
                        self.step_and_dispatch(FsmInput::BatchLiveHint)
                    } else {
                        self.batch_live_rx = None;
                        None
                    }
                }
                () = sleep_until_deadline(next_deadline), if next_deadline.is_some() => {
                    self.step_and_dispatch(FsmInput::TimerFired)
                }
                Some(joined) = self.effects_tasks.join_next(), if !self.effects_tasks.is_empty() => {
                    if let Err(error) = joined {
                        warn!(
                            streamer_id = %self.streamer_id,
                            generation = self.generation,
                            %error,
                            "Recorder actor side-effect task panicked"
                        );
                    }
                    None
                }
            };

            if let Some(reason) = stopped {
                break reason;
            }
        };

        // Cancel any in-flight probe (status probes are pure reads).
        self.in_flight_probe = None;
        self.batch_live_rx = None;

        // Wait for spawned segment persistence and pipeline notification tasks
        // so trailing segment rows and `SessionEnded` pipeline notifications
        // settle before this actor reports exit.
        while let Some(joined) = self.effects_tasks.join_next().await {
            if let Err(error) = joined {
                warn!(
                    streamer_id = %self.streamer_id,
                    generation = self.generation,
                    %error,
                    "Recorder actor side-effect task panicked during drain"
                );
            }
        }

        // Close the mailbox cleanly. Dropping `ack`/`done` on unconsumed segment
        // or danmu events signals the coordination bridge to run the session-level
        // trailing persistence fallback so no closed segment is ever lost on exit.
        self.rx.close();
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ActorMessage::SegmentStarted { ack, .. } => {
                    let _ = ack.send(Err("recorder actor stopped before segment start".into()));
                }
                ActorMessage::SegmentCompleted { ack, .. } => {
                    drop(ack);
                }
                ActorMessage::DanmuFifo { done, .. } => {
                    drop(done);
                }
                ActorMessage::Input(_) | ActorMessage::ReservedDownload { .. } => {}
            }
        }

        debug!(
            streamer_id = %self.streamer_id,
            generation = self.generation,
            ?stop_reason,
            "StreamerActor stopped"
        );
        stop_reason
    }

    fn next_jitter_bps(&self) -> u16 {
        // ±10% jitter (9000..=11000 bps) derived without holding shared RNG state.
        let nanos = Timestamp::now().as_nanosecond() as u64;
        let mixed = nanos
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.generation.wrapping_mul(1_442_695_040_888_963_407));
        9_000 + ((mixed >> 32) % 2_001) as u16
    }

    fn step_ctx(&self) -> StepContext {
        StepContext::new(Instant::now(), Timestamp::now(), self.next_jitter_bps())
    }

    fn handle_message(&mut self, msg: ActorMessage) -> Option<ActorStopReason> {
        match msg {
            ActorMessage::Input(input) => self.step_and_dispatch(input),
            ActorMessage::ReservedDownload { input, _permit } => {
                let stopped = self.step_and_dispatch(input);
                drop(_permit);
                stopped
            }
            ActorMessage::SegmentStarted { input, ack } => {
                let ctx = self.step_ctx();
                let effects = self.fsm.step(ctx, input);
                let signals = dispatch_segment_started_effects(
                    &self.streamer_id,
                    effects,
                    ack,
                    &self.deps,
                    &self.self_tx,
                    &mut self.effects_tasks,
                );
                self.apply_loop_signals(signals)
            }
            ActorMessage::SegmentCompleted { input, ack } => {
                let ctx = self.step_ctx();
                let effects = self.fsm.step(ctx, input);
                let signals = dispatch_segment_completed_effects(
                    &self.streamer_id,
                    effects,
                    ack,
                    &self.deps,
                    &self.self_tx,
                    &mut self.effects_tasks,
                );
                self.apply_loop_signals(signals)
            }
            ActorMessage::DanmuFifo { input, done } => {
                let ctx = self.step_ctx();
                let effects = self.fsm.step(ctx, input);
                let signals = dispatch_danmu_fifo_effects(
                    &self.streamer_id,
                    effects,
                    done,
                    &self.deps,
                    &self.self_tx,
                    &mut self.effects_tasks,
                );
                self.apply_loop_signals(signals)
            }
        }
    }

    fn step_and_dispatch(&mut self, input: FsmInput) -> Option<ActorStopReason> {
        let ctx = self.step_ctx();
        let effects = self.fsm.step(ctx, input);
        let signals = dispatch_effects(
            &self.streamer_id,
            effects,
            &self.deps,
            &self.self_tx,
            &mut self.effects_tasks,
        );
        self.apply_loop_signals(signals)
    }

    fn apply_loop_signals(&mut self, signals: LoopEffectSignals) -> Option<ActorStopReason> {
        if let Some(probe_id) = signals.cancel_probe
            && self
                .in_flight_probe
                .as_ref()
                .is_some_and(|(current_id, _)| *current_id == probe_id)
        {
            self.in_flight_probe = None;
        }
        if let Some(req) = signals.spawn_probe {
            let probe_id = req.probe_id;
            let prober = self.deps.prober.clone();
            self.in_flight_probe = Some((
                probe_id,
                Box::pin(async move { prober.probe_with_hint(req).await }),
            ));
        }
        if let Some((op_id, op)) = signals.commit_db {
            debug_assert!(
                self.in_flight_commit.is_none(),
                "FSM must dispatch at most one CommitDb in flight at a time"
            );
            let store = self.deps.store.clone();
            self.in_flight_commit = Some((op_id, Box::pin(async move { store.commit(op).await })));
        }
        signals.stopped
    }
}

async fn poll_in_flight_commit(
    slot: &mut Option<(u64, BoxedCommitFuture)>,
) -> (u64, std::result::Result<DbCommitOutcome, DbCommitFailure>) {
    let Some((op_id, fut)) = slot.as_mut() else {
        std::future::pending().await
    };
    let op_id = *op_id;
    let result = fut.await;
    *slot = None;
    (op_id, result)
}

async fn poll_in_flight_probe(
    slot: &mut Option<(u64, BoxedProbeFuture)>,
) -> (u64, ProbeFutureOutput) {
    let Some((probe_id, fut)) = slot.as_mut() else {
        std::future::pending().await
    };
    let probe_id = *probe_id;
    let result = fut.await;
    *slot = None;
    (probe_id, result)
}

async fn wait_batch_live_hint(slot: &mut Option<watch::Receiver<bool>>) -> bool {
    let Some(rx) = slot.as_mut() else {
        std::future::pending().await
    };
    if *rx.borrow_and_update() {
        return true;
    }
    loop {
        match rx.changed().await {
            Ok(()) => {
                if *rx.borrow_and_update() {
                    return true;
                }
            }
            Err(_) => return false,
        }
    }
}

async fn sleep_until_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
