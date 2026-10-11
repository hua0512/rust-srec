//! The DASH muxer: per-track assembler output → one fragmented MP4 stream.
//!
//! Each selected track runs its own reactor and assembler, whose ordered
//! output arrives here on its own channel. Both channels are read
//! concurrently, so neither track ever blocks the other's assembler; what
//! couples them is only the output order decided below.
//!
//! Output is led by one track (video when selected, else audio). Each output
//! item is one leader segment followed by the follower (audio) fragments
//! that overlap it: a fragment is included when it starts before the leader
//! segment ends and does not end before the segment starts. Item duration is
//! the leader segment's coverage. Follower fragments that end before the
//! slice they would belong to — audio preceding the first video segment, or
//! audio arriving after its slice was written without it — are dropped and
//! counted. When the leader ends, the remaining follower fragments are
//! written as a final item.
//!
//! All timing comes from the fragments' `tfdt`/`trun`, shifted onto the MPD
//! presentation timeline by `round(Period@start × timescale) − PTO`. MPD
//! timeline values are never used for output timing: packagers may describe
//! segments with the encoder-priming offset removed, or with durations that
//! are not frame-aligned, so they disagree with the media's decode times.
//!
//! Inits are merged only within an *epoch*: one Period with one leader
//! Representation and at most one follower Representation. When either
//! track moves to a new epoch the output gets an end marker
//! (`StreamStructureChange`) and a new merged init; fragments of different
//! epochs never share an init.
//!
//! Audio never fails a recording. A video segment waits for the audio it
//! overlaps at most `max_audio_lag` after it arrived here (less under buffer
//! pressure), then is written with what has arrived; a `TrackStalled` event
//! reports each such episode, and the audio that comes later is dropped as
//! late. An audio pipeline failure, protected audio, or audio the merger
//! rejects drops the audio track for the rest of the session with a
//! `TrackDropped` event.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use chrono::{DateTime, TimeDelta, Utc};
use hls::{HlsData, M4sData, M4sInitSegmentData, M4sSegmentData, SplitReason};
use mp4::merge::{FragmentWriter, MergeError, MergedInit, merge_init};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::DownloadError;
use crate::dash::config::DashMuxConfig;
use crate::dash::planner::PeriodKey;
use crate::redact::redact_url;
use crate::segment_engine::assembler::{AssembledItem, AssemblerSink, SinkClosed};
use crate::segment_engine::control::EngineNotice;
use crate::segment_engine::descriptor::{
    DashSegmentMeta, MediaTrack, SegmentDescriptor, SegmentMeta,
};
use crate::segment_engine::payload::SegmentPayload;
use crate::session::{DownloadEvent, EventSink};

/// Consecutive leader segments the merger may reject before the stream
/// fails. One corrupt segment is skipped; a systematic rejection is an error
/// rather than a recording that silently writes nothing.
const MAX_CONSECUTIVE_LEADER_REJECTIONS: u32 = 3;

pub(crate) type TrackItem = Result<AssembledItem, DownloadError>;

/// Assembler sink of one DASH track: forwards ordered items to the muxer.
pub(crate) struct TrackSink {
    tx: mpsc::Sender<TrackItem>,
}

impl TrackSink {
    pub(crate) fn new(tx: mpsc::Sender<TrackItem>) -> Self {
        Self { tx }
    }
}

impl AssemblerSink for TrackSink {
    async fn send(&mut self, item: TrackItem) -> Result<(), SinkClosed> {
        self.tx.send(item).await.map_err(|_| SinkClosed)
    }
}

/// One track's input to the muxer.
pub(crate) struct MuxTrack {
    pub track: MediaTrack,
    pub rx: mpsc::Receiver<TrackItem>,
    /// Stops this track's pipeline when the muxer drops the track.
    pub cancel: CancellationToken,
}

/// How the muxer finished.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum MuxOutcome {
    /// Every track ended and all output was delivered.
    Ended,
    Cancelled,
    DownstreamClosed,
    Failed(Arc<str>),
}

impl std::fmt::Debug for MuxOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A failure reason can quote server responses; keep it out of logs.
        f.write_str(match self {
            Self::Ended => "Ended",
            Self::Cancelled => "Cancelled",
            Self::DownstreamClosed => "DownstreamClosed",
            Self::Failed(_) => "Failed([redacted])",
        })
    }
}

/// Bookkeeping for drops the muxer decides, logged when it finishes.
#[derive(Debug, Default)]
struct MuxStats {
    items: u64,
    late_fragments: u64,
    stale_fragments: u64,
    slices_without_waiting: u64,
    rejected_leader_segments: u64,
}

/// A track's initialization segment and what is derived from it.
#[derive(Debug)]
struct TrackInit {
    period: PeriodKey,
    representation: Arc<str>,
    data: Bytes,
    descriptor: Arc<SegmentDescriptor>,
    /// The init merged on its own: validates it, and parses this track's
    /// media for timing before the output init is known. `None` when the
    /// merger rejected it and the leader falls back to passthrough.
    probe: Option<MergedInit>,
    timescale: u32,
    /// Ticks added to every decode time to reach the MPD presentation
    /// timeline.
    time_shift: i64,
}

impl TrackInit {
    fn same_epoch(&self, other: &Self) -> bool {
        self.period.same(&other.period) && self.representation == other.representation
    }
}

#[derive(Debug)]
struct QueuedMedia {
    init: Arc<TrackInit>,
    data: Bytes,
    descriptor: Arc<SegmentDescriptor>,
    /// `(start, end)` of each fragment in the track timescale, on the
    /// presentation timeline. Empty for a passthrough leader.
    fragments: Vec<(u64, u64)>,
    /// Fragments at the front already written or dropped.
    consumed: usize,
    /// When the muxer received it; a leader segment waits for its follower
    /// data at most `max_audio_lag` from here.
    arrived: Instant,
}

impl QueuedMedia {
    fn span(&self) -> Option<(u64, u64)> {
        let start = self.fragments.iter().map(|&(start, _)| start).min()?;
        let end = self.fragments.iter().map(|&(_, end)| end).max()?;
        Some((start, end))
    }
}

#[derive(Debug)]
enum Queued {
    Init(Arc<TrackInit>),
    Media(QueuedMedia),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Active,
    /// Authoritative end received.
    Ended,
    /// Closed without an end (cancellation) or dropped by the muxer.
    Closed,
}

struct TrackState {
    track: MediaTrack,
    status: Status,
    /// Dropped by the muxer after a failure; never resumes.
    dropped: bool,
    cancel: CancellationToken,
    queue: VecDeque<Queued>,
    queued_bytes: usize,
    /// The newest init received; media references it.
    latest_init: Option<Arc<TrackInit>>,
    /// Follower only: the newest init the slice logic has moved past.
    current_init: Option<Arc<TrackInit>>,
}

impl TrackState {
    fn new(track: MediaTrack, cancel: CancellationToken) -> Self {
        Self {
            track,
            status: Status::Active,
            dropped: false,
            cancel,
            queue: VecDeque::new(),
            queued_bytes: 0,
            latest_init: None,
            current_init: None,
        }
    }

    fn pop_front(&mut self) -> Option<Queued> {
        let item = self.queue.pop_front()?;
        if let Queued::Media(media) = &item {
            self.queued_bytes = self.queued_bytes.saturating_sub(media.data.len());
        }
        Some(item)
    }
}

/// The merged init currently in effect downstream.
struct Output {
    leader: Arc<TrackInit>,
    follower: Option<Arc<TrackInit>>,
    /// `None` for passthrough of a leader the merger rejected.
    init: Option<MergedInit>,
}

/// Follower fragments chosen for one slice: a run of `count` fragments
/// starting at `first` of one queued segment.
struct FollowerPart {
    init: Arc<TrackInit>,
    data: Bytes,
    first: usize,
    count: usize,
}

enum Collect {
    Wait,
    Ready(usize),
}

enum Step {
    /// Output was produced or state advanced; pump again.
    Continue,
    /// More input is needed, from the follower until the deadline, if any.
    Wait(Option<Instant>),
    /// Every track is done.
    Finished,
}

struct MuxState {
    config: DashMuxConfig,
    events: Option<EventSink>,
    leader: TrackState,
    follower: Option<TrackState>,
    output: Option<Output>,
    writer: FragmentWriter,
    out: Vec<HlsData>,
    /// When the current wait for follower data gives up.
    wait_deadline: Option<Instant>,
    /// Set when output stopped waiting for the follower; cleared by the next
    /// follower input. One stall event per episode.
    follower_stalled: bool,
    /// When the leader's authoritative end arrived.
    leader_ended_at: Option<Instant>,
    /// End of the newest leader slice written, with its timescale; follower
    /// fragments ending by then arrived too late for any slice.
    leader_end: Option<(u64, u32)>,
    leader_rejections: u32,
    finished: bool,
    stats: MuxStats,
}

/// A failure that ends the stream.
struct Fatal(DownloadError);

pub(crate) async fn run_muxer(
    leader: MuxTrack,
    follower: Option<MuxTrack>,
    out: mpsc::Sender<Result<HlsData, DownloadError>>,
    events: Option<EventSink>,
    config: DashMuxConfig,
    cancel: CancellationToken,
) -> MuxOutcome {
    let mut leader_rx = Some(leader.rx);
    let (follower_state, mut follower_rx) = match follower {
        Some(track) => (
            Some(TrackState::new(track.track, track.cancel)),
            Some(track.rx),
        ),
        None => (None, None),
    };
    let mut state = MuxState {
        config,
        events,
        leader: TrackState::new(leader.track, leader.cancel),
        follower: follower_state,
        output: None,
        writer: FragmentWriter::new(),
        out: Vec::new(),
        wait_deadline: None,
        follower_stalled: false,
        leader_ended_at: None,
        leader_end: None,
        leader_rejections: 0,
        finished: false,
        stats: MuxStats::default(),
    };

    let outcome = loop {
        if let Err(Fatal(error)) = state.pump() {
            let reason: Arc<str> = Arc::from(error.to_string());
            state.out.clear();
            if out.send(Err(error)).await.is_err() {
                debug!("DASH consumer closed before the stream error was delivered");
            }
            break MuxOutcome::Failed(reason);
        }
        let mut closed = false;
        for item in state.out.drain(..) {
            if out.send(Ok(item)).await.is_err() {
                closed = true;
                break;
            }
        }
        if closed {
            // Dropping the item stream cancels the session first, so a
            // closed output under a cancelled token is a cancellation.
            break if cancel.is_cancelled() {
                MuxOutcome::Cancelled
            } else {
                MuxOutcome::DownstreamClosed
            };
        }
        if state.finished {
            break match state.leader.status {
                Status::Ended => MuxOutcome::Ended,
                _ => MuxOutcome::Cancelled,
            };
        }
        if state
            .follower
            .as_ref()
            .is_none_or(|follower| follower.status != Status::Active)
        {
            follower_rx = None;
        }

        let leader_open = state.leader.status == Status::Active;
        let follower_open = follower_rx.is_some()
            && state
                .follower
                .as_ref()
                .is_some_and(|f| f.queued_bytes < state.config.max_buffered_bytes);
        let deadline = state.wait_deadline;

        tokio::select! {
            biased;
            _ = cancel.cancelled() => break MuxOutcome::Cancelled,
            item = recv(&mut leader_rx), if leader_open => {
                if let Err(Fatal(error)) = state.accept_leader(item) {
                    let reason: Arc<str> = Arc::from(error.to_string());
                    if out.send(Err(error)).await.is_err() {
                        debug!("DASH consumer closed before the stream error was delivered");
                    }
                    break MuxOutcome::Failed(reason);
                }
            }
            item = recv(&mut follower_rx), if follower_open => state.accept_follower(item),
            // Wakes the next pump, which stops waiting past the deadline.
            _ = sleep_until(deadline), if deadline.is_some() => {}
        }
    };

    state.leader.cancel.cancel();
    if let Some(follower) = &state.follower {
        follower.cancel.cancel();
    }
    let stats = &state.stats;
    info!(
        items = stats.items,
        late_audio_fragments = stats.late_fragments,
        stale_audio_fragments = stats.stale_fragments,
        slices_without_waiting = stats.slices_without_waiting,
        rejected_segments = stats.rejected_leader_segments,
        ?outcome,
        "DASH muxer finished"
    );
    outcome
}

async fn recv(rx: &mut Option<mpsc::Receiver<TrackItem>>) -> Option<TrackItem> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

impl MuxState {
    fn emit(&self, event: DownloadEvent) {
        if let Some(events) = &self.events {
            events.emit(event);
        }
    }

    // --- Input ---

    fn accept_leader(&mut self, item: Option<TrackItem>) -> Result<(), Fatal> {
        let item = match item {
            None => {
                if self.leader.status == Status::Active {
                    debug!(track = %self.leader.track, "leader closed without an end");
                    self.leader.status = Status::Closed;
                }
                return Ok(());
            }
            Some(Err(error)) => return Err(Fatal(error)),
            Some(Ok(item)) => item,
        };
        match item {
            AssembledItem::Payload(SegmentPayload::Mp4Init { data, descriptor }) => {
                let init = build_init(data, descriptor, true).map_err(|error| {
                    Fatal(match error {
                        MergeError::Protected { fourcc } => DownloadError::Unsupported {
                            protocol: "dash",
                            feature: format!("protected sample entries (`{fourcc}`)"),
                        },
                        other => DownloadError::InvalidContent {
                            protocol: "dash",
                            reason: format!(
                                "{} initialization segment: {other}",
                                self.leader.track
                            ),
                        },
                    })
                })?;
                if init.probe.is_none() {
                    warn!(
                        track = %self.leader.track,
                        "initialization segment cannot be merged; passing the track through unchanged"
                    );
                }
                self.leader.latest_init = Some(Arc::new(init));
            }
            AssembledItem::Payload(SegmentPayload::Mp4Media { data, descriptor }) => {
                let Some(init) = self.leader.latest_init.clone() else {
                    warn!(track = %self.leader.track, "media segment without an initialization segment; dropped");
                    return Ok(());
                };
                match probe_fragments(&init, &data) {
                    Ok(fragments) => {
                        self.leader_rejections = 0;
                        self.leader.queued_bytes += data.len();
                        self.leader.queue.push_back(Queued::Media(QueuedMedia {
                            init,
                            data,
                            descriptor,
                            fragments,
                            consumed: 0,
                            arrived: Instant::now(),
                        }));
                    }
                    Err(error) => self.reject_leader_segment(&error)?,
                }
            }
            AssembledItem::Payload(other) => {
                return Err(Fatal(DownloadError::InvalidContent {
                    protocol: "dash",
                    reason: format!(
                        "{} segment is not fragmented MP4 (msn {})",
                        self.leader.track,
                        other.msn()
                    ),
                }));
            }
            AssembledItem::StreamEnded => {
                self.leader.status = Status::Ended;
                self.leader_ended_at = Some(Instant::now());
            }
            AssembledItem::Notice(EngineNotice::ManifestRefreshed {
                sequence_base,
                target_duration,
            }) => self.emit(DownloadEvent::PlaylistRefreshed {
                media_sequence_base: sequence_base,
                target_duration,
            }),
            other => self.forward_engine_event(other),
        }
        Ok(())
    }

    fn accept_follower(&mut self, item: Option<TrackItem>) {
        self.follower_stalled = false;
        let Some(follower) = self.follower.as_mut() else {
            return;
        };
        let item = match item {
            None => {
                if follower.status == Status::Active {
                    follower.status = Status::Closed;
                }
                return;
            }
            Some(Err(error)) => {
                self.drop_follower(format!("pipeline failed: {error}"));
                return;
            }
            Some(Ok(item)) => item,
        };
        match item {
            AssembledItem::Payload(SegmentPayload::Mp4Init { data, descriptor }) => {
                match build_init(data, descriptor, false) {
                    Ok(init) => {
                        let init = Arc::new(init);
                        follower.latest_init = Some(Arc::clone(&init));
                        follower.queue.push_back(Queued::Init(init));
                    }
                    Err(error) => {
                        self.drop_follower(format!("initialization segment rejected: {error}"))
                    }
                }
            }
            AssembledItem::Payload(SegmentPayload::Mp4Media { data, descriptor }) => {
                let Some(init) = follower.latest_init.clone() else {
                    warn!(track = %follower.track, "media segment without an initialization segment; dropped");
                    return;
                };
                match probe_fragments(&init, &data) {
                    Ok(fragments) => {
                        follower.queued_bytes += data.len();
                        follower.queue.push_back(Queued::Media(QueuedMedia {
                            init,
                            data,
                            descriptor,
                            fragments,
                            consumed: 0,
                            arrived: Instant::now(),
                        }));
                    }
                    Err(error) => self.drop_follower(format!("media segment rejected: {error}")),
                }
            }
            AssembledItem::Payload(_) => {
                self.drop_follower("segment is not fragmented MP4".to_owned());
            }
            AssembledItem::StreamEnded => follower.status = Status::Ended,
            other => self.forward_engine_event(other),
        }
    }

    fn forward_engine_event(&self, item: AssembledItem) {
        match item {
            AssembledItem::GapSkipped {
                from_sequence,
                to_sequence,
                reason,
            } => self.emit(DownloadEvent::GapSkipped {
                from_sequence,
                to_sequence,
                reason,
            }),
            AssembledItem::SegmentTimeout {
                sequence_number,
                waited_duration,
            } => self.emit(DownloadEvent::SegmentTimeout {
                sequence_number,
                waited: waited_duration,
            }),
            // DASH descriptors never carry discontinuities: epoch changes are
            // detected from the inits. The end notice precedes StreamEnded.
            AssembledItem::Discontinuity
            | AssembledItem::Notice(_)
            | AssembledItem::Payload(_)
            | AssembledItem::StreamEnded => {}
        }
    }

    fn reject_leader_segment(&mut self, error: &MergeError) -> Result<(), Fatal> {
        self.leader_rejections += 1;
        self.stats.rejected_leader_segments += 1;
        warn!(track = %self.leader.track, %error, "segment rejected by the merger; skipped");
        if self.leader_rejections >= MAX_CONSECUTIVE_LEADER_REJECTIONS {
            return Err(Fatal(DownloadError::InvalidContent {
                protocol: "dash",
                reason: format!(
                    "{} consecutive {} segments rejected: {error}",
                    self.leader_rejections, self.leader.track
                ),
            }));
        }
        Ok(())
    }

    /// Stops recording the follower; output continues with the leader.
    fn drop_follower(&mut self, reason: String) {
        let Some(follower) = self.follower.as_mut() else {
            return;
        };
        if follower.dropped {
            return;
        }
        warn!(track = %follower.track, %reason, "dropping track; recording continues without it");
        follower.dropped = true;
        follower.status = Status::Closed;
        follower.queue.clear();
        follower.queued_bytes = 0;
        follower.cancel.cancel();
        let track = follower.track;
        self.emit(DownloadEvent::TrackDropped {
            track,
            reason: Arc::from(reason),
        });
    }

    /// Output gives up waiting for the follower; reported once per episode.
    fn follower_stall(&mut self) {
        self.stats.slices_without_waiting += 1;
        if self.follower_stalled {
            return;
        }
        self.follower_stalled = true;
        if let Some(follower) = &self.follower {
            warn!(
                track = %follower.track,
                waited_ms = self.config.max_audio_lag.as_millis() as u64,
                "track is behind; writing output without waiting for it"
            );
            self.emit(DownloadEvent::TrackStalled {
                track: follower.track,
                waited: self.config.max_audio_lag,
            });
        }
    }

    // --- Output ---

    fn pump(&mut self) -> Result<(), Fatal> {
        self.wait_deadline = None;
        while !self.finished {
            match self.step(Instant::now())? {
                Step::Continue => {}
                Step::Wait(deadline) => {
                    self.wait_deadline = deadline;
                    break;
                }
                Step::Finished => self.finished = true,
            }
        }
        Ok(())
    }

    /// The deadline of a wait for follower data that began at `since`.
    fn deadline(&self, since: Instant) -> Instant {
        let lag = self
            .config
            .max_audio_lag
            .min(Duration::from_secs(24 * 3600));
        since.checked_add(lag).unwrap_or(since)
    }

    fn under_buffer_limit(&self) -> bool {
        let buffered = self.leader.queued_bytes
            + self
                .follower
                .as_ref()
                .map_or(0, |follower| follower.queued_bytes);
        buffered < self.config.max_buffered_bytes
    }

    fn follower_active(&self) -> bool {
        self.follower
            .as_ref()
            .is_some_and(|follower| follower.status == Status::Active)
    }

    fn step(&mut self, now: Instant) -> Result<Step, Fatal> {
        let Some(Queued::Media(head)) = self.leader.queue.front() else {
            return Ok(self.leader_drained(now));
        };
        let Some((start, end)) = head.span() else {
            let Some(Queued::Media(media)) = self.leader.pop_front() else {
                return Ok(Step::Continue);
            };
            self.write_passthrough(media);
            return Ok(Step::Continue);
        };
        let period = head.init.period.clone();
        let timescale = head.init.timescale;
        let deadline = self.deadline(head.arrived);
        let in_time = now < deadline;
        let may_wait = in_time && self.under_buffer_limit();
        let picks = match self.collect(&period, start, end, timescale, may_wait) {
            Collect::Wait => return Ok(Step::Wait(Some(deadline))),
            Collect::Ready(picks) => picks,
        };
        if !may_wait && self.follower_active() {
            if in_time {
                // Buffer pressure, not lateness.
                self.stats.slices_without_waiting += 1;
            } else {
                self.follower_stall();
            }
        }
        let parts = self.take_follower(picks);
        let Some(Queued::Media(media)) = self.leader.pop_front() else {
            return Ok(Step::Continue);
        };
        self.write_slice(media, parts, start, end)?;
        Ok(Step::Continue)
    }

    /// The leader has no queued segment.
    fn leader_drained(&mut self, now: Instant) -> Step {
        match self.leader.status {
            Status::Active => Step::Wait(None),
            Status::Closed => Step::Finished,
            Status::Ended => {
                // Completion is shared: the follower's tail belongs in the
                // final item, so its end is awaited for one more lag.
                let deadline = self.deadline(self.leader_ended_at.unwrap_or(now));
                if self.follower_active() && now < deadline && self.under_buffer_limit() {
                    return Step::Wait(Some(deadline));
                }
                if self.follower_active() {
                    debug!("follower did not end in time; finishing without its tail");
                }
                self.flush_follower();
                self.out
                    .push(HlsData::end_marker_with_reason(SplitReason::EndOfStream));
                Step::Finished
            }
        }
    }

    /// Decides which follower fragments belong to the leader slice
    /// `[start, end)` (leader timescale) of `period`. Drops follower data of
    /// earlier Periods and fragments that end before the slice.
    fn collect(
        &mut self,
        period: &PeriodKey,
        start: u64,
        end: u64,
        timescale: u32,
        may_wait: bool,
    ) -> Collect {
        let Some(follower) = self.follower.as_mut() else {
            return Collect::Ready(0);
        };
        let mut picks = 0usize;
        let mut pick_init: Option<Arc<TrackInit>> = None;
        let mut index = 0usize;
        loop {
            let Some(item) = follower.queue.get_mut(index) else {
                return if follower.status == Status::Active && may_wait {
                    Collect::Wait
                } else {
                    Collect::Ready(picks)
                };
            };
            match item {
                Queued::Init(init) => {
                    if picks > 0 || is_after(&init.period, period) {
                        return Collect::Ready(picks);
                    }
                    // Nothing is picked, so this init is at the head.
                    let init = Arc::clone(init);
                    follower.pop_front();
                    follower.current_init = Some(init);
                }
                Queued::Media(media) => {
                    if is_after(&media.init.period, period) {
                        return Collect::Ready(picks);
                    }
                    if is_before(&media.init.period, period) {
                        if picks > 0 {
                            return Collect::Ready(picks);
                        }
                        self.stats.stale_fragments +=
                            (media.fragments.len() - media.consumed) as u64;
                        follower.pop_front();
                        continue;
                    }
                    if pick_init
                        .as_ref()
                        .is_some_and(|init| !Arc::ptr_eq(init, &media.init))
                    {
                        return Collect::Ready(picks);
                    }
                    let follower_timescale = media.init.timescale;
                    // Fragments of this segment picked so far; picks begin at
                    // the queue head, so only the head can have dropped ones.
                    let mut taken = 0usize;
                    while let Some(&(fragment_start, fragment_end)) =
                        media.fragments.get(media.consumed + taken)
                    {
                        if picks == 0 && ends_by(fragment_end, follower_timescale, start, timescale)
                        {
                            media.consumed += 1;
                            self.stats.late_fragments += 1;
                            continue;
                        }
                        if starts_before(fragment_start, follower_timescale, end, timescale) {
                            picks += 1;
                            taken += 1;
                            pick_init.get_or_insert_with(|| Arc::clone(&media.init));
                            // Decode order: the next fragment starts where
                            // this one ends at the earliest, so one reaching
                            // the slice end completes the slice without
                            // waiting for the next to arrive.
                            if !starts_before(fragment_end, follower_timescale, end, timescale) {
                                return Collect::Ready(picks);
                            }
                            continue;
                        }
                        return Collect::Ready(picks);
                    }
                    if picks == 0 {
                        follower.pop_front();
                    } else {
                        index += 1;
                    }
                }
            }
        }
    }

    /// Removes the first `picks` fragments from the follower queue.
    fn take_follower(&mut self, mut picks: usize) -> Vec<FollowerPart> {
        let mut parts = Vec::new();
        let Some(follower) = self.follower.as_mut() else {
            return parts;
        };
        while picks > 0 {
            let Some(Queued::Media(media)) = follower.queue.front_mut() else {
                break;
            };
            let count = picks.min(media.fragments.len() - media.consumed);
            parts.push(FollowerPart {
                init: Arc::clone(&media.init),
                data: media.data.clone(),
                first: media.consumed,
                count,
            });
            media.consumed += count;
            picks -= count;
            if media.consumed == media.fragments.len() {
                follower.pop_front();
            }
        }
        parts
    }

    /// The follower init the next slice uses when it has no follower
    /// fragments: keep the output's (a merged init with an idle track is
    /// fine and avoids a split), else the follower's init for this Period.
    fn idle_follower_init(&self, leader: &Arc<TrackInit>) -> Option<Arc<TrackInit>> {
        if let Some(output) = &self.output
            && Arc::ptr_eq(&output.leader, leader)
        {
            return output.follower.clone();
        }
        let follower = self.follower.as_ref()?;
        if follower.dropped {
            return None;
        }
        follower
            .current_init
            .clone()
            .filter(|init| init.period.same(&leader.period))
    }

    /// Makes `(leader, follower)` the output epoch, writing an end marker
    /// and the merged init when it changes.
    fn ensure_output(&mut self, leader: &Arc<TrackInit>, follower: Option<Arc<TrackInit>>) {
        if let Some(output) = &self.output
            && Arc::ptr_eq(&output.leader, leader)
            && match (&output.follower, &follower) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
        {
            return;
        }
        if let Some(output) = &self.output {
            let description = epoch_change(output, leader, follower.as_deref());
            info!(%description, "DASH output epoch changes");
            self.out.push(HlsData::end_marker_with_reason(
                SplitReason::StreamStructureChange { description },
            ));
        }
        let (init, follower) = match (&leader.probe, follower) {
            (Some(_), Some(follower)) => {
                match merge_init(&[leader.data.clone(), follower.data.clone()]) {
                    Ok(merged) => (Some(merged), Some(follower)),
                    Err(error) => {
                        self.drop_follower(format!(
                            "initialization segments cannot be merged: {error}"
                        ));
                        (leader.probe.clone(), None)
                    }
                }
            }
            (None, Some(_)) => {
                self.drop_follower("the leading track is passed through unmerged".to_owned());
                (None, None)
            }
            (probe, None) => (probe.clone(), None),
        };
        let data = init
            .as_ref()
            .map_or_else(|| leader.data.clone(), |init| init.data().clone());
        self.out
            .push(HlsData::M4sData(M4sData::InitSegment(M4sInitSegmentData {
                segment: init_media_segment(&leader.descriptor),
                data,
            })));
        self.output = Some(Output {
            leader: Arc::clone(leader),
            follower,
            init,
        });
    }

    fn write_slice(
        &mut self,
        media: QueuedMedia,
        parts: Vec<FollowerPart>,
        start: u64,
        end: u64,
    ) -> Result<(), Fatal> {
        let follower = match parts.first() {
            Some(part) => Some(Arc::clone(&part.init)),
            None => self.idle_follower_init(&media.init),
        };
        self.ensure_output(&media.init, follower);
        let Some(output) = &self.output else {
            return Ok(());
        };
        let Some(init) = &output.init else {
            self.write_passthrough(media);
            return Ok(());
        };

        let mut data = Vec::with_capacity(
            media.data.len() + parts.iter().map(|part| part.data.len()).sum::<usize>(),
        );
        let leader_written = init
            .parse_media_segment(0, &media.data, media.init.time_shift)
            .and_then(|fragments| {
                fragments
                    .into_iter()
                    .try_for_each(|fragment| self.writer.write(fragment, &mut data))
            });
        if let Err(error) = leader_written {
            return self.reject_leader_segment(&error);
        }
        self.leader_rejections = 0;

        let follower_written = match &output.follower {
            Some(_) => parts.iter().try_for_each(|part| {
                init.parse_media_segment(1, &part.data, part.init.time_shift)
                    .and_then(|fragments| {
                        fragments
                            .into_iter()
                            .skip(part.first)
                            .take(part.count)
                            .try_for_each(|fragment| self.writer.write(fragment, &mut data))
                    })
            }),
            None => Ok(()),
        };
        if let Err(error) = follower_written {
            // The leader's fragments are complete; what follower data was
            // written before the error stays, nothing after it.
            self.drop_follower(format!("media segment rejected: {error}"));
        }

        let timescale = media.init.timescale;
        self.leader_end = Some((end, timescale));
        let segment = media_segment(&media.descriptor, start, end, timescale);
        self.out
            .push(HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                segment,
                data: Bytes::from(data),
            })));
        self.stats.items += 1;
        Ok(())
    }

    fn write_passthrough(&mut self, media: QueuedMedia) {
        self.ensure_output(&media.init, None);
        let duration = match &media.descriptor.meta {
            SegmentMeta::Dash(meta) => meta.duration_secs() as f32,
            SegmentMeta::Hls(segment) => segment.duration,
        };
        let segment = m3u8_rs::MediaSegment {
            uri: redact_url(&media.descriptor.parsed_url).to_string(),
            duration,
            ..Default::default()
        };
        self.out
            .push(HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                segment,
                data: media.data,
            })));
        self.stats.items += 1;
    }

    /// Writes the follower fragments left after the leader ended, when they
    /// belong to the output's epoch.
    fn flush_follower(&mut self) {
        let Some(output) = &self.output else {
            return;
        };
        let (Some(init), Some(output_follower)) = (&output.init, &output.follower) else {
            return;
        };
        let Some(follower) = self.follower.as_mut() else {
            return;
        };
        let mut data = Vec::new();
        let mut last: Option<Arc<SegmentDescriptor>> = None;
        while let Some(item) = follower.pop_front() {
            let Queued::Media(media) = item else {
                continue;
            };
            if !media.init.same_epoch(output_follower) {
                self.stats.stale_fragments += (media.fragments.len() - media.consumed) as u64;
                continue;
            }
            let follower_timescale = media.init.timescale;
            let late = |&(_, fragment_end): &(u64, u64)| {
                self.leader_end.is_some_and(|(end, timescale)| {
                    ends_by(fragment_end, follower_timescale, end, timescale)
                })
            };
            let keep: Vec<bool> = media.fragments.iter().map(|span| !late(span)).collect();
            self.stats.late_fragments +=
                keep[media.consumed..].iter().filter(|keep| !**keep).count() as u64;
            let written = init
                .parse_media_segment(1, &media.data, media.init.time_shift)
                .and_then(|fragments| {
                    fragments
                        .into_iter()
                        .zip(keep)
                        .skip(media.consumed)
                        .filter(|(_, keep)| *keep)
                        .try_for_each(|(fragment, _)| self.writer.write(fragment, &mut data))
                });
            if let Err(error) = written {
                warn!(%error, "trailing audio rejected by the merger; dropped");
                break;
            }
            last = Some(media.descriptor);
        }
        if let Some(descriptor) = last
            && !data.is_empty()
        {
            let mut segment = init_media_segment(&descriptor);
            segment.duration = 0.0;
            self.out
                .push(HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                    segment,
                    data: Bytes::from(data),
                })));
            self.stats.items += 1;
        }
    }
}

fn is_before(a: &PeriodKey, b: &PeriodKey) -> bool {
    !a.same(b) && a.start < b.start
}

fn is_after(a: &PeriodKey, b: &PeriodKey) -> bool {
    !a.same(b) && a.start > b.start
}

/// `a / a_scale ≤ b / b_scale`, exactly.
fn ends_by(a: u64, a_scale: u32, b: u64, b_scale: u32) -> bool {
    u128::from(a) * u128::from(b_scale) <= u128::from(b) * u128::from(a_scale)
}

/// `a / a_scale < b / b_scale`, exactly.
fn starts_before(a: u64, a_scale: u32, b: u64, b_scale: u32) -> bool {
    u128::from(a) * u128::from(b_scale) < u128::from(b) * u128::from(a_scale)
}

fn dash_meta(descriptor: &SegmentDescriptor) -> Option<&DashSegmentMeta> {
    match &descriptor.meta {
        SegmentMeta::Dash(meta) => Some(meta),
        SegmentMeta::Hls(_) => None,
    }
}

/// Builds a track init from its payload. `Err` only for protected content
/// (both roles) or, for the follower, any rejection; a rejected leader init
/// yields a passthrough init (`probe: None`).
fn build_init(
    data: Bytes,
    descriptor: Arc<SegmentDescriptor>,
    leader: bool,
) -> Result<TrackInit, MergeError> {
    let (period, representation, period_start, pto, mpd_timescale) = match dash_meta(&descriptor) {
        Some(meta) => (
            PeriodKey {
                id: meta.period_id.clone(),
                start: meta.period_start,
            },
            Arc::clone(&meta.representation_id),
            meta.period_start,
            meta.presentation_time_offset,
            meta.timescale,
        ),
        None => (
            PeriodKey {
                id: None,
                start: Duration::ZERO,
            },
            Arc::from(""),
            Duration::ZERO,
            0,
            1,
        ),
    };
    let probe = match merge_init(std::slice::from_ref(&data)) {
        Ok(probe) => Some(probe),
        Err(error @ MergeError::Protected { .. }) => return Err(error),
        Err(error) if !leader => return Err(error),
        Err(error) => {
            debug!(%error, "leader initialization segment rejected by the merger");
            None
        }
    };
    let timescale = probe
        .as_ref()
        .and_then(|probe| probe.tracks().first())
        .map_or(0, |track| track.timescale());
    let time_shift = match &probe {
        Some(_) => time_shift(period_start, pto, mpd_timescale, timescale)
            .ok_or(MergeError::Overflow("time shift"))?,
        None => 0,
    };
    Ok(TrackInit {
        period,
        representation,
        data,
        descriptor,
        probe,
        timescale,
        time_shift,
    })
}

/// `round(period_start × timescale) − PTO`, with the PTO converted from the
/// MPD timescale to the media timescale.
fn time_shift(period_start: Duration, pto: u64, mpd_timescale: u64, timescale: u32) -> Option<i64> {
    let timescale = i128::from(timescale);
    let nanos = i128::try_from(period_start.as_nanos()).ok()?;
    let start = (nanos.checked_mul(timescale)? + 500_000_000) / 1_000_000_000;
    let mpd_timescale = i128::from(mpd_timescale.max(1));
    let pto = (i128::from(pto).checked_mul(timescale)? + mpd_timescale / 2) / mpd_timescale;
    i64::try_from(start - pto).ok()
}

/// Fragment spans of a segment, from the track's own init.
fn probe_fragments(init: &TrackInit, data: &Bytes) -> Result<Vec<(u64, u64)>, MergeError> {
    let Some(probe) = &init.probe else {
        return Ok(Vec::new());
    };
    let fragments = probe.parse_media_segment(0, data, init.time_shift)?;
    if fragments.is_empty() {
        return Err(MergeError::MissingBox {
            fourcc: mp4::FourCc(*b"moof"),
            parent: mp4::FourCc(*b"file"),
        });
    }
    fragments
        .iter()
        .map(|fragment| {
            let start = fragment.decode_time();
            start
                .checked_add(fragment.duration())
                .map(|end| (start, end))
                .ok_or(MergeError::Overflow("fragment end time"))
        })
        .collect()
}

fn epoch_change(output: &Output, leader: &TrackInit, follower: Option<&TrackInit>) -> String {
    let describe = |init: Option<&TrackInit>| match init {
        Some(init) => format!("{}@{}", init.representation, period_label(&init.period)),
        None => "none".to_owned(),
    };
    format!(
        "DASH tracks changed from {}/{} to {}/{}",
        describe(Some(&output.leader)),
        describe(output.follower.as_deref()),
        describe(Some(leader)),
        describe(follower),
    )
}

fn period_label(period: &PeriodKey) -> String {
    match &period.id {
        Some(id) => format!("period {id}"),
        None => format!("period at {:?}", period.start),
    }
}

fn init_media_segment(descriptor: &SegmentDescriptor) -> m3u8_rs::MediaSegment {
    m3u8_rs::MediaSegment {
        uri: redact_url(&descriptor.parsed_url).to_string(),
        ..Default::default()
    }
}

/// The synthetic playlist entry describing an output item: leader coverage
/// as duration and, for dynamic presentations, the wall-clock time of its
/// start.
fn media_segment(
    descriptor: &SegmentDescriptor,
    start: u64,
    end: u64,
    timescale: u32,
) -> m3u8_rs::MediaSegment {
    let duration = if timescale == 0 {
        0.0
    } else {
        (end.saturating_sub(start) as f64 / f64::from(timescale)) as f32
    };
    let program_date_time = dash_meta(descriptor)
        .and_then(|meta| meta.availability_start_time)
        .and_then(|anchor| presentation_instant(anchor, start, timescale))
        .map(|time| time.fixed_offset());
    m3u8_rs::MediaSegment {
        uri: redact_url(&descriptor.parsed_url).to_string(),
        duration,
        program_date_time,
        ..Default::default()
    }
}

fn presentation_instant(
    anchor: DateTime<Utc>,
    ticks: u64,
    timescale: u32,
) -> Option<DateTime<Utc>> {
    if timescale == 0 {
        return None;
    }
    let nanos = u128::from(ticks) * 1_000_000_000 / u128::from(timescale);
    let delta = TimeDelta::nanoseconds(i64::try_from(nanos).ok()?);
    anchor.checked_add_signed(delta)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::StreamExt;
    use mp4::independent::IndependentFragmentCheck;
    use url::Url;

    use super::*;
    use crate::segment_engine::descriptor::SegmentSource;
    use crate::segment_engine::identity::{SegmentKey, SegmentKind};
    use crate::session::DownloadEventStream;

    const VIDEO_INIT: &[u8] = include_bytes!("../../../mp4/tests/fixtures/dash/video-init.mp4");
    const VIDEO_1: &[u8] = include_bytes!("../../../mp4/tests/fixtures/dash/video-1.m4s");
    const VIDEO_2: &[u8] = include_bytes!("../../../mp4/tests/fixtures/dash/video-2.m4s");
    const AUDIO_INIT: &[u8] = include_bytes!("../../../mp4/tests/fixtures/dash/audio-init.mp4");
    const AUDIO_1: &[u8] = include_bytes!("../../../mp4/tests/fixtures/dash/audio-1.m4s");
    const AUDIO_2: &[u8] = include_bytes!("../../../mp4/tests/fixtures/dash/audio-2.m4s");
    const AUDIO_3: &[u8] = include_bytes!("../../../mp4/tests/fixtures/dash/audio-3.m4s");
    const WAIT: Duration = Duration::from_secs(5);

    fn descriptor(
        track: MediaTrack,
        kind: SegmentKind,
        period: Option<&str>,
        period_start_secs: u64,
        number: u64,
    ) -> Arc<SegmentDescriptor> {
        let url = Url::parse(&format!(
            "https://cdn.example.com/{track}/{number}.m4s?token=secret"
        ))
        .unwrap();
        Arc::new(SegmentDescriptor {
            key: SegmentKey {
                kind,
                uri: Arc::from(url.as_str()),
                byte_range: None,
                scope: Some(Arc::from(format!("{track}/{period:?}"))),
            },
            msn: number,
            source: SegmentSource::Playlist,
            parsed_url: Arc::new(url),
            discontinuity: false,
            encryption: None,
            init_key: None,
            meta: SegmentMeta::Dash(Arc::new(DashSegmentMeta {
                track,
                period_id: period.map(Arc::from),
                period_start: Duration::from_secs(period_start_secs),
                representation_id: Arc::from(track.as_str()),
                number,
                time: 0,
                duration: 0,
                timescale: 1000,
                presentation_time_offset: 0,
                availability_start_time: None,
            })),
        })
    }

    fn init(track: MediaTrack, period: Option<&str>, start: u64, data: &[u8]) -> TrackItem {
        Ok(AssembledItem::Payload(SegmentPayload::Mp4Init {
            data: Bytes::copy_from_slice(data),
            descriptor: descriptor(track, SegmentKind::Init, period, start, 0),
        }))
    }

    fn media(
        track: MediaTrack,
        period: Option<&str>,
        start: u64,
        number: u64,
        data: Bytes,
    ) -> TrackItem {
        Ok(AssembledItem::Payload(SegmentPayload::Mp4Media {
            data,
            descriptor: descriptor(track, SegmentKind::Media, period, start, number),
        }))
    }

    /// `segment` with its (version 1) `tfdt` set to `decode_time`.
    fn with_tfdt(segment: &[u8], decode_time: u64) -> Bytes {
        let mut data = segment.to_vec();
        let at = data
            .windows(4)
            .position(|window| window == b"tfdt")
            .expect("segment has a tfdt");
        assert_eq!(data[at + 4], 1, "fixture tfdt is version 1");
        data[at + 8..at + 16].copy_from_slice(&decode_time.to_be_bytes());
        Bytes::from(data)
    }

    /// `(fourcc, body)` of each box in `data[start..end]`.
    fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 8 <= data.len() {
            let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            let fourcc: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
            out.push((fourcc, &data[at + 8..at + size]));
            at += size;
        }
        out
    }

    /// `(mfhd sequence, track ID)` of every fragment in `data`.
    fn fragments(data: &[u8]) -> Vec<(u32, u32)> {
        boxes(data)
            .into_iter()
            .filter(|(fourcc, _)| fourcc == b"moof")
            .map(|(_, moof)| {
                let children = boxes(moof);
                let mfhd = children.iter().find(|(f, _)| f == b"mfhd").unwrap().1;
                let traf = children.iter().find(|(f, _)| f == b"traf").unwrap().1;
                let tfhd = boxes(traf)
                    .into_iter()
                    .find(|(f, _)| f == b"tfhd")
                    .unwrap()
                    .1;
                (
                    u32::from_be_bytes(mfhd[4..8].try_into().unwrap()),
                    u32::from_be_bytes(tfhd[4..8].try_into().unwrap()),
                )
            })
            .collect()
    }

    struct Harness {
        video: mpsc::Sender<TrackItem>,
        audio: Option<mpsc::Sender<TrackItem>>,
        out: mpsc::Receiver<Result<HlsData, DownloadError>>,
        events: DownloadEventStream,
        muxer: tokio::task::JoinHandle<MuxOutcome>,
    }

    fn start(leader: MediaTrack, with_follower: bool, config: DashMuxConfig) -> Harness {
        let (video, video_rx) = mpsc::channel(64);
        let (out_tx, out) = mpsc::channel(64);
        let (sink, events) = EventSink::channel(64);
        let (audio, follower) = if with_follower {
            let (audio, audio_rx) = mpsc::channel(64);
            (
                Some(audio),
                Some(MuxTrack {
                    track: MediaTrack::Audio,
                    rx: audio_rx,
                    cancel: CancellationToken::new(),
                }),
            )
        } else {
            (None, None)
        };
        let muxer = tokio::spawn(run_muxer(
            MuxTrack {
                track: leader,
                rx: video_rx,
                cancel: CancellationToken::new(),
            },
            follower,
            out_tx,
            Some(sink),
            config,
            CancellationToken::new(),
        ));
        Harness {
            video,
            audio,
            out,
            events,
            muxer,
        }
    }

    impl Harness {
        async fn video(&self, item: TrackItem) {
            self.video.send(item).await.unwrap();
        }

        async fn audio(&self, item: TrackItem) {
            self.audio.as_ref().unwrap().send(item).await.unwrap();
        }

        async fn finish(mut self) -> (Vec<HlsData>, MuxOutcome, Vec<DownloadEvent>) {
            let mut items = Vec::new();
            while let Some(item) = tokio::time::timeout(WAIT, self.out.recv())
                .await
                .expect("muxer output within the timeout")
            {
                match item {
                    Ok(item) => items.push(item),
                    Err(error) => panic!("unexpected stream error: {error}"),
                }
            }
            let outcome = tokio::time::timeout(WAIT, self.muxer)
                .await
                .expect("muxer finishes")
                .unwrap();
            drop(self.video);
            drop(self.audio);
            let mut events = Vec::new();
            while let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(50), self.events.next()).await
            {
                events.push(event);
            }
            (items, outcome, events)
        }
    }

    fn init_data(items: &[HlsData]) -> Vec<&Bytes> {
        items
            .iter()
            .filter_map(|item| match item {
                HlsData::M4sData(M4sData::InitSegment(init)) => Some(&init.data),
                _ => None,
            })
            .collect()
    }

    fn media_data(items: &[HlsData]) -> Vec<&Bytes> {
        items
            .iter()
            .filter_map(|item| match item {
                HlsData::M4sData(M4sData::Segment(segment)) => Some(&segment.data),
                _ => None,
            })
            .collect()
    }

    fn lag(lag: Duration) -> DashMuxConfig {
        DashMuxConfig {
            max_audio_lag: lag,
            ..DashMuxConfig::default()
        }
    }

    #[tokio::test]
    async fn merges_separate_tracks_into_one_two_track_stream() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
        h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(media(v, Some("p0"), 0, 2, Bytes::from_static(VIDEO_2)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;
        h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
        for (number, data) in [(1, AUDIO_1), (2, AUDIO_2), (3, AUDIO_3)] {
            h.audio(media(a, Some("p0"), 0, number, Bytes::from_static(data)))
                .await;
        }
        h.audio(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        assert!(matches!(
            items.last(),
            Some(HlsData::EndMarker(Some(SplitReason::EndOfStream)))
        ));
        let inits = init_data(&items);
        assert_eq!(inits.len(), 1);
        let info = mp4::isobmff::parse_init_segment(inits[0]);
        assert!(info.has_h264 && info.has_aac, "{info:?}");
        let check = IndependentFragmentCheck::from_init(inits[0]).expect("two-track init");

        let segments = media_data(&items);
        assert!(
            check.is_independent(segments[0]),
            "first item holds both tracks"
        );
        let all: Vec<(u32, u32)> = segments.iter().flat_map(|data| fragments(data)).collect();
        assert_eq!(all.iter().filter(|(_, track)| *track == 1).count(), 2);
        assert_eq!(all.iter().filter(|(_, track)| *track == 2).count(), 3);
        let sequences: Vec<u32> = all.iter().map(|(sequence, _)| *sequence).collect();
        assert!(
            sequences.windows(2).all(|pair| pair[0] < pair[1]),
            "{sequences:?}"
        );
        // Each item starts with its video fragment.
        for data in &segments[..2] {
            assert_eq!(fragments(data)[0].1, 1);
        }
        for item in &items {
            if let HlsData::M4sData(M4sData::Segment(segment)) = item {
                assert!(!segment.segment.uri.contains("secret"));
            }
        }
    }

    #[tokio::test]
    async fn stalled_audio_is_not_waited_for_and_late_audio_is_dropped() {
        let h = start(MediaTrack::Video, true, lag(Duration::from_millis(50)));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
        h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
        h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(media(v, Some("p0"), 0, 2, Bytes::from_static(VIDEO_2)))
            .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Belongs to the first video segment, which was written without it.
        h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
            .await;
        h.audio(Ok(AssembledItem::StreamEnded)).await;
        h.video(media(v, Some("p0"), 0, 3, with_tfdt(VIDEO_1, 2 * 12288)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, events) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        let segments = media_data(&items);
        assert_eq!(segments.len(), 3);
        for data in segments {
            assert!(fragments(data).iter().all(|(_, track)| *track == 1));
        }
        assert!(events.iter().any(|event| matches!(
            event,
            DownloadEvent::TrackStalled {
                track: MediaTrack::Audio,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn failed_audio_is_dropped_and_video_continues() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let v = MediaTrack::Video;
        h.audio(Err(DownloadError::Timeout {
            reason: "stalled".to_owned(),
        }))
        .await;
        h.video(init(v, None, 0, VIDEO_INIT)).await;
        h.video(media(v, None, 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(media(v, None, 0, 2, Bytes::from_static(VIDEO_2)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, events) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        assert_eq!(media_data(&items).len(), 2);
        let inits = init_data(&items);
        assert!(!mp4::isobmff::parse_init_segment(inits[0]).has_aac);
        assert!(events.iter().any(|event| matches!(
            event,
            DownloadEvent::TrackDropped {
                track: MediaTrack::Audio,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn period_change_starts_a_new_epoch_with_its_own_init() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
        h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
        h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
            .await;
        // Period p1 restarts the media timeline at 0 and starts at 1 s.
        h.video(init(v, Some("p1"), 1, VIDEO_INIT)).await;
        h.video(media(v, Some("p1"), 1, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;
        h.audio(init(a, Some("p1"), 1, AUDIO_INIT)).await;
        h.audio(media(a, Some("p1"), 1, 1, Bytes::from_static(AUDIO_1)))
            .await;
        h.audio(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        let kinds: Vec<&str> = items
            .iter()
            .map(|item| match item {
                HlsData::M4sData(M4sData::InitSegment(_)) => "init",
                HlsData::M4sData(M4sData::Segment(_)) => "media",
                HlsData::EndMarker(Some(SplitReason::StreamStructureChange { .. })) => "epoch",
                HlsData::EndMarker(Some(SplitReason::EndOfStream)) => "end",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["init", "media", "epoch", "init", "media", "end"]);
        for data in media_data(&items) {
            let tracks: Vec<u32> = fragments(data).iter().map(|(_, track)| *track).collect();
            assert_eq!(tracks, [1, 2], "one fragment of each track per epoch");
        }
    }

    #[tokio::test]
    async fn single_track_is_normalized_through_the_merger() {
        let h = start(MediaTrack::Audio, false, lag(WAIT));
        let a = MediaTrack::Audio;
        h.video(init(a, None, 0, AUDIO_INIT)).await;
        h.video(media(a, None, 0, 1, Bytes::from_static(AUDIO_1)))
            .await;
        h.video(media(a, None, 0, 2, Bytes::from_static(AUDIO_2)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        let segments = media_data(&items);
        assert_eq!(segments.len(), 2);
        for data in segments {
            // `styp` and `sidx` of the source are gone.
            assert_eq!(&data[4..8], b"moof");
            assert_eq!(fragments(data).len(), 1);
        }
        let sequences: Vec<u32> = media_data(&items)
            .iter()
            .flat_map(|data| fragments(data))
            .map(|(sequence, _)| sequence)
            .collect();
        assert_eq!(sequences, [1, 2]);
    }

    #[tokio::test]
    async fn video_ending_first_waits_for_the_audio_tail() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        h.video(init(v, None, 0, VIDEO_INIT)).await;
        h.video(media(v, None, 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(media(v, None, 0, 2, Bytes::from_static(VIDEO_2)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;
        h.audio(init(a, None, 0, AUDIO_INIT)).await;
        h.audio(media(a, None, 0, 1, Bytes::from_static(AUDIO_1)))
            .await;
        h.audio(media(a, None, 0, 2, Bytes::from_static(AUDIO_2)))
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        h.audio(media(a, None, 0, 3, Bytes::from_static(AUDIO_3)))
            .await;
        h.audio(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        let segments = media_data(&items);
        let tail = segments.last().unwrap();
        assert!(
            fragments(tail).iter().all(|(_, track)| *track == 2),
            "audio tail item"
        );
        let audio = segments
            .iter()
            .flat_map(|data| fragments(data))
            .filter(|(_, track)| *track == 2)
            .count();
        assert_eq!(audio, 3);
    }

    #[tokio::test]
    async fn a_multiplexed_representation_is_passed_through_unchanged() {
        // One init carrying both tracks, as a muxed Representation serves it.
        let muxed = merge_init(&[
            Bytes::from_static(VIDEO_INIT),
            Bytes::from_static(AUDIO_INIT),
        ])
        .unwrap();
        let h = start(MediaTrack::Video, false, lag(WAIT));
        let v = MediaTrack::Video;
        h.video(init(v, None, 0, muxed.data())).await;
        h.video(media(v, None, 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        assert_eq!(init_data(&items), [muxed.data()]);
        assert_eq!(media_data(&items), [&Bytes::from_static(VIDEO_1)]);
        let duration = match &items[1] {
            HlsData::M4sData(M4sData::Segment(segment)) => segment.segment.duration,
            other => panic!("expected media, got {other:?}"),
        };
        assert_eq!(duration, 0.0, "the MPD duration of the test descriptor");
    }

    #[tokio::test]
    async fn protected_video_fails_with_a_typed_error() {
        let mut protected = VIDEO_INIT.to_vec();
        let at = protected
            .windows(4)
            .position(|window| window == b"avc1")
            .unwrap();
        protected[at..at + 4].copy_from_slice(b"encv");
        let mut h = start(MediaTrack::Video, false, lag(WAIT));
        h.video(init(MediaTrack::Video, None, 0, &protected)).await;

        let item = tokio::time::timeout(WAIT, h.out.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            item,
            Err(DownloadError::Unsupported {
                protocol: "dash",
                ..
            })
        ));
        let outcome = tokio::time::timeout(WAIT, h.muxer).await.unwrap().unwrap();
        assert!(matches!(outcome, MuxOutcome::Failed(_)));
    }

    #[test]
    fn time_shift_moves_media_time_onto_the_presentation_timeline() {
        // Period at 10 s, PTO 2 s in a 1 kHz MPD timescale, 48 kHz media.
        assert_eq!(
            time_shift(Duration::from_secs(10), 2_000, 1_000, 48_000),
            Some(10 * 48_000 - 2 * 48_000)
        );
        assert_eq!(time_shift(Duration::ZERO, 0, 1, 90_000), Some(0));
        assert_eq!(time_shift(Duration::MAX, 0, 1, u32::MAX), None);
    }
}
