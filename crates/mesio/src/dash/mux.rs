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
//! Slicing works on signed times, since a segment overlapping the Period
//! start lies partly before it; the output adds one session-wide origin to
//! every track, chosen with the first item so no written decode time is
//! negative, which keeps the tracks' relative offsets.
//!
//! Inits are merged only within an *epoch*: one Period with one leader
//! Representation and at most one follower Representation. When either
//! track moves to a new epoch the output gets an end marker
//! (`StreamStructureChange`) and a new merged init, unless the new merged
//! init is byte-identical to the one in effect: then output continues
//! without either, as Periods that only split the timeline (ad markers,
//! packager-inserted boundaries) commonly repeat the same initialization.
//! Fragments are never written under an init other than their own or a
//! byte-identical one. Audio of the old epoch that is still queued
//! when the video moves to a new Period is written first, under the old
//! init.
//!
//! A Period without video (an audio-only interlude, or audio carried past
//! the last video Period) is recorded as its own audio-only epoch: one item
//! per audio segment under the audio track's own init, its duration the
//! audio coverage. A Period counts as video-less once the video planner
//! reports it (`TrackAbsent`), video arrives from a later Period, or the
//! video track has ended.
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
use mp4::merge::{FragmentWriter, MergeError, MergedInit, merge_init, validate_init};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::DownloadError;
use crate::dash::config::DashMuxConfig;
use crate::dash::planner::{ConsumedFrontier, PeriodKey};
use crate::redact::redact_url;
use crate::segment_engine::assembler::{AssembledItem, AssemblerSink, SinkClosed};
use crate::segment_engine::control::EngineNotice;
use crate::segment_engine::descriptor::{
    DashSegmentMeta, MediaTrack, SegmentDescriptor, SegmentMeta, WallClockAnchor,
};
use crate::segment_engine::payload::SegmentPayload;
use crate::session::{DownloadEvent, EventSink};

/// Consecutive leader segments the merger may reject before the stream
/// fails. One corrupt segment is skipped; a systematic rejection is an error
/// rather than a recording that silently writes nothing.
const MAX_CONSECUTIVE_LEADER_REJECTIONS: u32 = 3;

/// Leader-less Periods remembered; only the newest few can still matter.
const MAX_ABSENT_PERIODS: usize = 16;

pub(crate) type TrackItem = Result<AssembledItem, DownloadError>;

/// Assembler sink of one DASH track: forwards ordered items to the muxer
/// and publishes the track's consumed frontier to its planner.
pub(crate) struct TrackSink {
    tx: mpsc::Sender<TrackItem>,
    frontier: Arc<ConsumedFrontier>,
}

impl TrackSink {
    pub(crate) fn new(tx: mpsc::Sender<TrackItem>, frontier: Arc<ConsumedFrontier>) -> Self {
        Self { tx, frontier }
    }
}

impl AssemblerSink for TrackSink {
    async fn send(&mut self, item: TrackItem) -> Result<(), SinkClosed> {
        match &item {
            Ok(AssembledItem::Payload(payload)) if !payload.is_init() => {
                self.frontier.advance_to(payload.msn().saturating_add(1));
            }
            Ok(AssembledItem::GapSkipped { to_sequence, .. }) => {
                self.frontier.advance_to(*to_sequence);
            }
            _ => {}
        }
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
    track: MediaTrack,
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
    /// Whether both govern the same media: the planner gives every epoch
    /// (Period, Representation and init fingerprint) its own init key, so a
    /// same-ID Representation whose init or timing changed is a new epoch.
    fn same_epoch(&self, other: &Self) -> bool {
        self.descriptor.key == other.descriptor.key
    }
}

#[derive(Debug)]
struct QueuedMedia {
    init: Arc<TrackInit>,
    data: Bytes,
    descriptor: Arc<SegmentDescriptor>,
    /// `(start, end)` of each fragment in the track timescale, on the
    /// presentation timeline (signed: a segment overlapping the Period start
    /// begins before it). Empty for a passthrough leader.
    fragments: Vec<(i128, i128)>,
    /// Fragments at the front already written or dropped.
    consumed: usize,
    /// When the muxer received it; a leader segment waits for its follower
    /// data at most `max_audio_lag` from here.
    arrived: Instant,
}

impl QueuedMedia {
    fn span(&self) -> Option<(i128, i128)> {
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

/// A new output epoch, announced only together with its first media item.
struct PendingOutput {
    output: Output,
    /// The end marker (when an epoch precedes it) and the init.
    header: Vec<HlsData>,
}

/// Follower fragments chosen for one slice: a run of `count` fragments
/// starting at `first` of one queued segment.
struct FollowerPart {
    init: Arc<TrackInit>,
    data: Bytes,
    first: usize,
    count: usize,
    /// Presentation start of the first fragment and end of the last, in
    /// the follower timescale.
    start: i128,
    end: i128,
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
    /// Output groups, each delivered whole: an epoch's end marker and init
    /// travel with its first media item, so a consumer that opens a file on
    /// the init never sees the init without media.
    out: Vec<Vec<HlsData>>,
    /// When the current wait for follower data gives up.
    wait_deadline: Option<Instant>,
    /// Set when output stopped waiting for the follower; cleared by the next
    /// follower input. One stall event per episode.
    follower_stalled: bool,
    /// When the leader's authoritative end arrived.
    leader_ended_at: Option<Instant>,
    /// When the follower last delivered anything.
    follower_input_at: Option<Instant>,
    /// When a media item was last written (or the muxer started).
    last_media_at: Instant,
    /// Periods the leader's planner reported without leader content, with
    /// the leader sequence number they follow.
    leader_absent: VecDeque<(PeriodKey, u64)>,
    /// The leader sequence number after the newest one received or skipped.
    leader_next_sequence: u64,
    /// End of the newest leader slice written, with its timescale; follower
    /// fragments ending by then arrived too late for any slice.
    leader_end: Option<(i128, u32)>,
    /// Added to every presentation time written, in nanoseconds; chosen with
    /// the first item.
    origin_nanos: Option<i128>,
    /// Presentation end of the media written so far, per track, in
    /// nanoseconds on the MPD timeline.
    written_ends: Vec<(MediaTrack, i128)>,
    /// Whether the follower's tail of the current output epoch was written
    /// ahead of a Period change.
    tail_flushed: bool,
    leader_rejections: u32,
    finished: bool,
    stats: MuxStats,
}

/// A failure that ends the stream.
struct Fatal(DownloadError);

/// `media_stall`: fail the session when no media item was written for this
/// long. Track-level watchdogs cannot cover this: a track that is absent by
/// design keeps reporting so, which would keep a session whose remaining
/// track failed alive forever.
pub(crate) async fn run_muxer(
    leader: MuxTrack,
    follower: Option<MuxTrack>,
    out: mpsc::Sender<Result<HlsData, DownloadError>>,
    events: Option<EventSink>,
    config: DashMuxConfig,
    media_stall: Option<Duration>,
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
        follower_input_at: None,
        last_media_at: Instant::now(),
        leader_absent: VecDeque::new(),
        leader_next_sequence: 0,
        leader_end: None,
        written_ends: Vec::new(),
        origin_nanos: None,
        tail_flushed: false,
        leader_rejections: 0,
        finished: false,
        stats: MuxStats::default(),
    };

    let outcome = loop {
        if let Err(Fatal(error)) = state.pump() {
            state.out.clear();
            break fail(&out, error, &cancel).await;
        }
        let mut stopped = None;
        for group in state.out.drain(..) {
            if let Err(stop) = send_group(&out, group, &cancel).await {
                stopped = Some(stop);
                break;
            }
            // Time blocked on a slow consumer is not a media stall.
            state.last_media_at = Instant::now();
        }
        if let Some(stop) = stopped {
            break stop;
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
        let stall_deadline = media_stall.map(|stall| {
            state
                .last_media_at
                .checked_add(stall.min(Duration::from_secs(24 * 3600)))
                .unwrap_or(state.last_media_at)
        });

        tokio::select! {
            biased;
            _ = cancel.cancelled() => break MuxOutcome::Cancelled,
            item = recv(&mut leader_rx), if leader_open => {
                if let Err(Fatal(error)) = state.accept_leader(item, cancel.is_cancelled()) {
                    break fail(&out, error, &cancel).await;
                }
            }
            item = recv(&mut follower_rx), if follower_open => {
                state.accept_follower(item, cancel.is_cancelled());
            }
            // Wakes the next pump, which stops waiting past the deadline.
            _ = sleep_until(deadline), if deadline.is_some() => {}
            _ = sleep_until(stall_deadline), if stall_deadline.is_some() => {
                let stalled_for = media_stall.unwrap_or_default();
                warn!(
                    stalled_ms = stalled_for.as_millis() as u64,
                    "no DASH media was written; failing the session"
                );
                break fail(
                    &out,
                    DownloadError::Timeout {
                        reason: format!(
                            "no media was written for {} s",
                            stalled_for.as_secs_f32()
                        ),
                    },
                    &cancel,
                )
                .await;
            }
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

/// Delivers `item` unless the session is cancelled first: a consumer that
/// stopped reading must not keep a cancelled session alive.
async fn send(
    out: &mpsc::Sender<Result<HlsData, DownloadError>>,
    item: Result<HlsData, DownloadError>,
    cancel: &CancellationToken,
) -> Result<(), MuxOutcome> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(MuxOutcome::Cancelled),
        sent = out.send(item) => sent.map_err(|_| {
            // Dropping the item stream cancels the session first, so a
            // closed output under a cancelled token is a cancellation.
            if cancel.is_cancelled() {
                MuxOutcome::Cancelled
            } else {
                MuxOutcome::DownstreamClosed
            }
        }),
    }
}

/// Delivers `group` whole, or nothing of it when the session is cancelled
/// first: permits for every item are reserved before any is sent.
async fn send_group(
    out: &mpsc::Sender<Result<HlsData, DownloadError>>,
    group: Vec<HlsData>,
    cancel: &CancellationToken,
) -> Result<(), MuxOutcome> {
    let closed = || {
        if cancel.is_cancelled() {
            MuxOutcome::Cancelled
        } else {
            MuxOutcome::DownstreamClosed
        }
    };
    if group.len() > out.max_capacity() {
        // Cannot be reserved at once; only a channel smaller than a group
        // (three items) gets here.
        for item in group {
            send(out, Ok(item), cancel).await?;
        }
        return Ok(());
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(MuxOutcome::Cancelled),
        permits = out.reserve_many(group.len()) => {
            let permits = permits.map_err(|_| closed())?;
            for (permit, item) in permits.zip(group) {
                permit.send(Ok(item));
            }
            Ok(())
        }
    }
}

/// Delivers the stream's terminal error and reports the failure.
async fn fail(
    out: &mpsc::Sender<Result<HlsData, DownloadError>>,
    error: DownloadError,
    cancel: &CancellationToken,
) -> MuxOutcome {
    let reason: Arc<str> = Arc::from(error.to_string());
    match send(out, Err(error), cancel).await {
        Ok(()) | Err(MuxOutcome::DownstreamClosed) => MuxOutcome::Failed(reason),
        Err(stop) => stop,
    }
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

    /// `cancelled`: whether the session was cancelled, which explains a
    /// channel closing without an end or an error.
    fn accept_leader(&mut self, item: Option<TrackItem>, cancelled: bool) -> Result<(), Fatal> {
        let item = match item {
            None => {
                if self.leader.status == Status::Active {
                    if !cancelled {
                        return Err(Fatal(DownloadError::Internal {
                            reason: format!(
                                "{} pipeline stopped without an end or an error",
                                self.leader.track
                            ),
                        }));
                    }
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
                self.leader_next_sequence = self
                    .leader_next_sequence
                    .max(descriptor.msn.saturating_add(1));
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
            AssembledItem::Notice(EngineNotice::TrackAbsent {
                section_id,
                section_start,
                after_sequence,
            }) => {
                let period = PeriodKey {
                    id: section_id,
                    start: section_start,
                };
                if !self
                    .leader_absent
                    .iter()
                    .any(|(known, _)| known.same(&period))
                {
                    if self.leader_absent.len() == MAX_ABSENT_PERIODS {
                        self.leader_absent.pop_front();
                    }
                    self.leader_absent.push_back((period, after_sequence));
                }
            }
            AssembledItem::GapSkipped {
                from_sequence,
                to_sequence,
                reason,
            } => {
                self.leader_next_sequence = self.leader_next_sequence.max(to_sequence);
                self.forward_engine_event(AssembledItem::GapSkipped {
                    from_sequence,
                    to_sequence,
                    reason,
                });
            }
            other => self.forward_engine_event(other),
        }
        Ok(())
    }

    fn accept_follower(&mut self, item: Option<TrackItem>, cancelled: bool) {
        self.follower_stalled = false;
        self.follower_input_at = Some(Instant::now());
        let Some(follower) = self.follower.as_mut() else {
            return;
        };
        let item = match item {
            None => {
                if follower.status == Status::Active {
                    if cancelled {
                        follower.status = Status::Closed;
                    } else {
                        self.drop_follower("pipeline stopped without an end", "");
                    }
                }
                return;
            }
            Some(Err(error)) => {
                self.drop_follower("pipeline failed", &error);
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
                    Err(error) => self.drop_follower("initialization segment rejected", &error),
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
                    Err(error) => self.drop_follower("media segment rejected", &error),
                }
            }
            AssembledItem::Payload(_) => {
                self.drop_follower("segment is not fragmented MP4", "");
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

    /// Stops recording the follower; output continues with the leader. The
    /// event carries `reason` only; `detail` may quote server responses and
    /// is not logged.
    fn drop_follower(&mut self, reason: &'static str, detail: impl std::fmt::Display) {
        let Some(follower) = self.follower.as_mut() else {
            return;
        };
        if follower.dropped {
            return;
        }
        warn!(
            track = %follower.track,
            reason,
            detail = %crate::redact::redact_diagnostic(detail),
            "dropping track; recording continues without it"
        );
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

    /// Time added to `init`'s decode times on output: its shift onto the
    /// presentation timeline plus the session origin.
    fn output_shift(&self, init: &TrackInit) -> Result<i64, MergeError> {
        let origin = self.origin_nanos.unwrap_or(0);
        let timescale = i128::from(init.timescale);
        // Rounded up, so a time the origin was chosen to lift to zero is not
        // left one tick below it.
        let origin_ticks = (origin * timescale + NANOS_PER_SEC - 1) / NANOS_PER_SEC;
        i64::try_from(i128::from(init.time_shift) + origin_ticks)
            .map_err(|_| MergeError::Overflow("time shift"))
    }

    /// Chooses the session origin with the first item: the lift that makes
    /// its earliest presentation time zero, or none when nothing is negative.
    fn choose_origin(&mut self, starts: impl Iterator<Item = (i128, u32)>) {
        if self.origin_nanos.is_some() {
            return;
        }
        let earliest = starts
            .filter(|&(_, timescale)| timescale > 0)
            .map(|(ticks, timescale)| {
                ticks
                    .saturating_mul(NANOS_PER_SEC)
                    .div_euclid(i128::from(timescale))
            })
            .min()
            .unwrap_or(0);
        let origin = (-earliest).max(0);
        if origin > 0 {
            info!(
                origin_ms = (origin / 1_000_000) as i64,
                "DASH output starts before the Period; shifting the session timeline"
            );
        }
        self.origin_nanos = Some(origin);
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
        let head_period = match self.leader.queue.front() {
            Some(Queued::Media(head)) => Some(head.init.period.clone()),
            _ => None,
        };
        if self.emit_gap_audio(head_period.as_ref()) {
            return Ok(Step::Continue);
        }
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
        let period_change = self
            .output
            .as_ref()
            .is_some_and(|output| !output.leader.period.same(&period));
        if period_change && !self.tail_flushed {
            if self.follower_tail_pending(may_wait) {
                return Ok(Step::Wait(Some(deadline)));
            }
            self.flush_follower(true);
            self.tail_flushed = true;
            // Audio of Periods without video may come next.
            return Ok(Step::Continue);
        }
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
                // final items, so its end is awaited while it keeps
                // delivering, up to one lag after its last input.
                let since = self
                    .leader_ended_at
                    .into_iter()
                    .chain(self.follower_input_at)
                    .max()
                    .unwrap_or(now);
                let deadline = self.deadline(since);
                if self.follower_active() && now < deadline {
                    if !self.under_buffer_limit() {
                        // Write what is queued rather than stop reading:
                        // the buffer bound must not end the recording.
                        self.flush_follower(false);
                    }
                    return Step::Wait(Some(deadline));
                }
                if self.follower_active() {
                    debug!("follower did not end in time; finishing without its tail");
                }
                self.flush_follower(false);
                self.out.push(vec![HlsData::end_marker_with_reason(
                    SplitReason::EndOfStream,
                )]);
                Step::Finished
            }
        }
    }

    /// Whether the follower may still deliver audio of the output's Period
    /// (an A/V or an audio-only one): everything queued belongs to that
    /// Period (under any init) and the track is active.
    fn follower_tail_pending(&self, may_wait: bool) -> bool {
        let (Some(follower), Some(output)) = (&self.follower, &self.output) else {
            return false;
        };
        let period = &output.leader.period;
        may_wait
            && follower.status == Status::Active
            && follower.queue.iter().all(|item| match item {
                Queued::Media(media) => media.init.period.same(period),
                Queued::Init(init) => init.period.same(period),
            })
    }

    /// Decides which follower fragments belong to the leader slice
    /// `[start, end)` (leader timescale) of `period`. Drops follower data of
    /// earlier Periods and fragments that end before the slice.
    fn collect(
        &mut self,
        period: &PeriodKey,
        start: i128,
        end: i128,
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
                start: media.fragments[media.consumed].0,
                end: media.fragments[media.consumed + count - 1].1,
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

    /// The output epoch `(leader, follower)` when it differs from the
    /// current one, with the end marker and merged init that announce it.
    /// `starts` are the presentation starts `(track, ticks, timescale)` of
    /// the tracks in the epoch's first media item. Nothing is emitted or
    /// committed here: the caller sends the header with the epoch's first
    /// media item, or drops it with that item.
    fn prepare_output(
        &mut self,
        leader: &Arc<TrackInit>,
        follower: Option<Arc<TrackInit>>,
        starts: &[(MediaTrack, i128, u32)],
    ) -> Option<PendingOutput> {
        if let Some(output) = &self.output
            && Arc::ptr_eq(&output.leader, leader)
            && match (&output.follower, &follower) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
        {
            return None;
        }
        let (init, follower) = match (&leader.probe, follower) {
            (Some(_), Some(follower)) => {
                match merge_init(&[leader.data.clone(), follower.data.clone()]) {
                    Ok(merged) => (Some(merged), Some(follower)),
                    Err(error) => {
                        self.drop_follower("initialization segments cannot be merged", &error);
                        (leader.probe.clone(), None)
                    }
                }
            }
            (None, Some(_)) => {
                self.drop_follower("the leading track is passed through unmerged", "");
                (None, None)
            }
            (probe, None) => (probe.clone(), None),
        };
        let mut header = Vec::with_capacity(2);
        match &self.output {
            Some(output) if self.continues_output(output, init.as_ref(), starts) => {
                debug!(
                    description = %epoch_change(output, leader, follower.as_deref()),
                    "DASH epoch changes under an identical initialization segment; output continues"
                );
            }
            current => {
                if let Some(output) = current {
                    let description = epoch_change(output, leader, follower.as_deref());
                    header.push(HlsData::end_marker_with_reason(
                        SplitReason::StreamStructureChange { description },
                    ));
                }
                let data = init
                    .as_ref()
                    .map_or_else(|| leader.data.clone(), |init| init.data().clone());
                header.push(HlsData::M4sData(M4sData::InitSegment(M4sInitSegmentData {
                    segment: init_media_segment(&leader.descriptor),
                    data,
                })));
            }
        }
        Some(PendingOutput {
            output: Output {
                leader: Arc::clone(leader),
                follower,
                init,
            },
            header,
        })
    }

    /// Whether a new output epoch continues the current output, with neither
    /// a split nor a new init: its merged init is byte-identical to the one
    /// in effect, so everything written under either decodes under the one
    /// already written, and no track of its first item (`starts`, see
    /// [`Self::prepare_output`]) starts before that track's written media
    /// ends, so decode times keep increasing within the output. Every
    /// epoch's fragments are shifted onto the one presentation timeline, so
    /// nothing else differs. Passthrough output keeps the source's own
    /// decode times and always splits.
    fn continues_output(
        &self,
        output: &Output,
        init: Option<&MergedInit>,
        starts: &[(MediaTrack, i128, u32)],
    ) -> bool {
        let (Some(current), Some(next)) = (&output.init, init) else {
            return false;
        };
        current.data() == next.data()
            && starts.iter().all(|&(track, start, timescale)| {
                let written = self
                    .written_ends
                    .iter()
                    .find(|(written, _)| *written == track)
                    .map(|&(_, end)| end);
                match (written, to_nanos(start, timescale)) {
                    (Some(end), Some(start)) => start >= end,
                    (None, Some(_)) => true,
                    (_, None) => false,
                }
            })
    }

    /// Records that `track`'s output reaches `end` (ticks of `timescale`).
    fn note_written(&mut self, track: MediaTrack, end: i128, timescale: u32) {
        let Some(end) = to_nanos(end, timescale) else {
            return;
        };
        match self
            .written_ends
            .iter_mut()
            .find(|(written, _)| *written == track)
        {
            Some((_, written)) => *written = (*written).max(end),
            None => self.written_ends.push((track, end)),
        }
    }

    /// Emits `item`, preceded by the header of `pending` when it opens a new
    /// output epoch, and commits that epoch.
    fn emit_media(&mut self, pending: Option<PendingOutput>, item: HlsData) {
        let mut group = Vec::with_capacity(3);
        if let Some(pending) = pending {
            if let Some(HlsData::EndMarker(Some(SplitReason::StreamStructureChange {
                description,
            }))) = pending.header.first()
            {
                info!(%description, "DASH output epoch changes");
            }
            group.extend(pending.header);
            self.output = Some(pending.output);
            self.tail_flushed = false;
        }
        group.push(item);
        self.out.push(group);
        self.stats.items += 1;
        self.last_media_at = Instant::now();
    }

    fn write_slice(
        &mut self,
        media: QueuedMedia,
        parts: Vec<FollowerPart>,
        start: i128,
        end: i128,
    ) -> Result<(), Fatal> {
        let follower = match parts.first() {
            Some(part) => Some(Arc::clone(&part.init)),
            None => self.idle_follower_init(&media.init),
        };
        let starts: Vec<(MediaTrack, i128, u32)> =
            std::iter::once((media.init.track, start, media.init.timescale))
                .chain(
                    parts
                        .iter()
                        .map(|part| (part.init.track, part.start, part.init.timescale)),
                )
                .collect();
        let pending = self.prepare_output(&media.init, follower, &starts);
        let passthrough = match &pending {
            Some(pending) => pending.output.init.is_none(),
            None => self
                .output
                .as_ref()
                .is_some_and(|output| output.init.is_none()),
        };
        if passthrough {
            self.write_passthrough_with(media, pending);
            return Ok(());
        }
        self.choose_origin(
            std::iter::once((start, media.init.timescale))
                .chain(parts.iter().map(|part| (part.start, part.init.timescale))),
        );
        let leader_shift = self.output_shift(&media.init);
        let part_shifts: Vec<Result<i64, MergeError>> = parts
            .iter()
            .map(|part| self.output_shift(&part.init))
            .collect();
        let Some(output) = pending
            .as_ref()
            .map(|pending| &pending.output)
            .or(self.output.as_ref())
        else {
            return Ok(());
        };
        let Some(init) = &output.init else {
            return Ok(());
        };
        let has_follower = output.follower.is_some();

        let mut data = Vec::with_capacity(
            media.data.len() + parts.iter().map(|part| part.data.len()).sum::<usize>(),
        );
        // Parsed unshifted and shifted fragment by fragment: only what is
        // written must land at or after the output origin.
        let leader_written = leader_shift.and_then(|shift| {
            init.parse_media_segment(0, &media.data, 0)?
                .into_iter()
                .try_for_each(|mut fragment| {
                    fragment.shift_decode_time(shift)?;
                    self.writer.write(fragment, &mut data)
                })
        });
        if let Err(error) = leader_written {
            // An epoch whose first item is rejected is not announced.
            return self.reject_leader_segment(&error);
        }
        self.leader_rejections = 0;

        let follower_written = match has_follower {
            true => parts.iter().zip(part_shifts).try_for_each(|(part, shift)| {
                let shift = shift?;
                init.parse_media_segment(1, &part.data, 0)?
                    .into_iter()
                    .skip(part.first)
                    .take(part.count)
                    .try_for_each(|mut fragment| {
                        fragment.shift_decode_time(shift)?;
                        self.writer.write(fragment, &mut data)
                    })
            }),
            false => Ok(()),
        };
        match follower_written {
            Ok(()) => {
                for part in &parts {
                    self.note_written(part.init.track, part.end, part.init.timescale);
                }
            }
            // The leader's fragments are complete; what follower data was
            // written before the error stays, nothing after it.
            Err(error) => self.drop_follower("media segment rejected", &error),
        }

        let timescale = media.init.timescale;
        self.note_written(media.init.track, end, timescale);
        self.leader_end = Some((end, timescale));
        let segment = media_segment(&media.descriptor, start, end, timescale);
        self.emit_media(
            pending,
            HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                segment,
                data: Bytes::from(data),
            })),
        );
        Ok(())
    }

    fn write_passthrough(&mut self, media: QueuedMedia) {
        let pending = self.prepare_output(&media.init, None, &[]);
        self.write_passthrough_with(media, pending);
    }

    fn write_passthrough_with(&mut self, media: QueuedMedia, pending: Option<PendingOutput>) {
        let duration = match &media.descriptor.meta {
            SegmentMeta::Dash(meta) => meta.duration_secs() as f32,
            SegmentMeta::Hls(segment) => segment.duration,
        };
        let segment = m3u8_rs::MediaSegment {
            uri: redact_url(&media.descriptor.parsed_url).to_string(),
            duration,
            ..Default::default()
        };
        self.emit_media(
            pending,
            HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                segment,
                data: media.data,
            })),
        );
    }

    /// The Period of the first queued follower media.
    fn next_follower_period(&self) -> Option<PeriodKey> {
        self.follower
            .as_ref()?
            .queue
            .iter()
            .find_map(|item| match item {
                Queued::Media(media) => Some(media.init.period.clone()),
                Queued::Init(_) => None,
            })
    }

    /// Whether `period` is known to have no leader content: leader media
    /// already comes from a later Period, the leader ended, or its planner
    /// reported the Period.
    fn leader_skips(&self, period: &PeriodKey, head: Option<&PeriodKey>) -> bool {
        match head {
            Some(head) => is_before(period, head),
            None => {
                self.leader.status == Status::Ended
                    || self.leader_absent.iter().any(|(absent, after)| {
                        absent.same(period) && self.leader_next_sequence >= *after
                    })
            }
        }
    }

    /// Writes follower runs of Periods without leader content as audio-only
    /// epochs, after the output Period's own follower tail. Returns whether
    /// anything was written.
    fn emit_gap_audio(&mut self, head: Option<&PeriodKey>) -> bool {
        let mut written = false;
        loop {
            let Some(run_period) = self.next_follower_period() else {
                return written;
            };
            let output_period = self
                .output
                .as_ref()
                .map(|output| output.leader.period.clone());
            let after_output = output_period
                .as_ref()
                .is_none_or(|output| is_after(&run_period, output));
            let audio_only_output = self
                .output
                .as_ref()
                .is_some_and(|output| output.leader.track != self.leader.track);
            if audio_only_output
                && output_period
                    .as_ref()
                    .is_some_and(|output| run_period.same(output))
            {
                // An audio-only epoch keeps writing its Period's audio as it
                // arrives; nothing else will.
                for media in self.pop_follower_run() {
                    self.write_audio_only(media);
                }
                written = true;
                continue;
            }
            if !after_output {
                // The output Period's tail goes first, once audio of a
                // leader-less Period is queued behind it.
                let gap_behind = output_period
                    .as_ref()
                    .is_some_and(|output| run_period.same(output))
                    && self.follower.as_ref().is_some_and(|follower| {
                        follower.queue.iter().any(|item| {
                            matches!(item, Queued::Media(media)
                                if output_period.as_ref().is_some_and(|output| is_after(&media.init.period, output))
                                    && self.leader_skips(&media.init.period, head))
                        })
                    });
                if !gap_behind {
                    return written;
                }
                self.flush_follower(true);
                self.tail_flushed = true;
                written = true;
                continue;
            }
            if !self.leader_skips(&run_period, head) {
                return written;
            }
            let run = self.pop_follower_run();
            if run.is_empty() {
                return written;
            }
            for media in run {
                self.write_audio_only(media);
            }
            written = true;
        }
    }

    /// Removes the next run of queued follower media under one init.
    fn pop_follower_run(&mut self) -> Vec<QueuedMedia> {
        let mut run = Vec::new();
        let Some(follower) = self.follower.as_mut() else {
            return run;
        };
        while matches!(follower.queue.front(), Some(Queued::Init(_))) {
            follower.pop_front();
        }
        let Some(Queued::Media(head)) = follower.queue.front() else {
            return run;
        };
        let run_init = Arc::clone(&head.init);
        while matches!(
            follower.queue.front(),
            Some(Queued::Media(media)) if media.init.same_epoch(&run_init)
        ) {
            if let Some(Queued::Media(media)) = follower.pop_front() {
                run.push(media);
            }
        }
        run
    }

    /// Writes one follower segment as an item of an audio-only epoch under
    /// the follower's own init.
    fn write_audio_only(&mut self, media: QueuedMedia) {
        let Some(&(start, _)) = media.fragments.get(media.consumed) else {
            return;
        };
        let end = media.fragments.last().map_or(start, |&(_, end)| end);
        let starts = [(media.init.track, start, media.init.timescale)];
        let pending = self.prepare_output(&media.init, None, &starts);
        self.choose_origin(std::iter::once((start, media.init.timescale)));
        let shift = self.output_shift(&media.init);
        let Some(init) = pending
            .as_ref()
            .map(|pending| &pending.output)
            .or(self.output.as_ref())
            .and_then(|output| output.init.as_ref())
        else {
            return;
        };
        let mut data = Vec::new();
        let written = shift.and_then(|shift| {
            init.parse_media_segment(0, &media.data, 0)?
                .into_iter()
                .skip(media.consumed)
                .try_for_each(|mut fragment| {
                    fragment.shift_decode_time(shift)?;
                    self.writer.write(fragment, &mut data)
                })
        });
        if let Err(error) = written {
            warn!(%error, "audio of a Period without video rejected by the merger; dropped");
            return;
        }
        self.note_written(media.init.track, end, media.init.timescale);
        let segment = media_segment(&media.descriptor, start, end, media.init.timescale);
        self.emit_media(
            pending,
            HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                segment,
                data: Bytes::from(data),
            })),
        );
    }

    /// Writes queued follower fragments left after the slices. `prefix_only`:
    /// only those of the output's Period at the head of the queue (ahead of a
    /// Period change), each run under a changed init announced with it.
    /// Otherwise the queue is drained (at the end of the stream), Periods
    /// after the output's as audio-only epochs and earlier ones dropped.
    fn flush_follower(&mut self, prefix_only: bool) {
        loop {
            let Some(output) = &self.output else {
                return;
            };
            let Some(run_period) = self.next_follower_period() else {
                return;
            };
            let output_period = output.leader.period.clone();
            let audio_only = output.leader.track != self.leader.track;
            if !run_period.same(&output_period) {
                if prefix_only {
                    return;
                }
                let run = self.pop_follower_run();
                if is_before(&run_period, &output_period) {
                    self.stats.stale_fragments += run
                        .iter()
                        .map(|media| (media.fragments.len() - media.consumed) as u64)
                        .sum::<u64>();
                } else {
                    for media in run {
                        self.write_audio_only(media);
                    }
                }
                continue;
            }
            let run = self.pop_follower_run();
            let Some(run_init) = run.first().map(|media| Arc::clone(&media.init)) else {
                return;
            };
            let Some(output) = &self.output else {
                return;
            };
            if audio_only {
                for media in run {
                    self.write_audio_only(media);
                }
            } else if output
                .follower
                .as_ref()
                .is_some_and(|epoch| epoch.same_epoch(&run_init))
            {
                self.write_tail(run, None);
            } else if output.init.is_some() {
                let leader = Arc::clone(&output.leader);
                let starts: Vec<(MediaTrack, i128, u32)> = run
                    .first()
                    .and_then(|media| media.fragments.get(media.consumed))
                    .map(|&(start, _)| (run_init.track, start, run_init.timescale))
                    .into_iter()
                    .collect();
                let pending = self.prepare_output(&leader, Some(run_init), &starts);
                self.write_tail(run, pending);
            } else {
                self.stats.stale_fragments += run
                    .iter()
                    .map(|media| (media.fragments.len() - media.consumed) as u64)
                    .sum::<u64>();
            }
        }
    }

    /// Writes a tail run as one audio-only item under the output's merged
    /// init, or under `pending` (announced with the item) when the run's
    /// init is not the output's.
    fn write_tail(&mut self, run: Vec<QueuedMedia>, pending: Option<PendingOutput>) {
        let Some(run_init) = run.first().map(|media| Arc::clone(&media.init)) else {
            return;
        };
        let shift = match self.output_shift(&run_init) {
            Ok(shift) => shift,
            Err(error) => {
                warn!(%error, "trailing audio cannot be placed on the output timeline; dropped");
                return;
            }
        };
        let Some(output) = pending
            .as_ref()
            .map(|pending| &pending.output)
            .or(self.output.as_ref())
        else {
            return;
        };
        let (Some(init), Some(_)) = (&output.init, &output.follower) else {
            return;
        };
        let follower_timescale = run_init.timescale;
        let leader_end = self.leader_end;
        let late = |&(_, fragment_end): &(i128, i128)| {
            leader_end.is_some_and(|(end, timescale)| {
                ends_by(fragment_end, follower_timescale, end, timescale)
            })
        };
        let mut data = Vec::new();
        let mut last = None;
        let mut written_end = None;
        let mut late_fragments = 0u64;
        for media in &run {
            let keep: Vec<bool> = media.fragments.iter().map(|span| !late(span)).collect();
            late_fragments += keep[media.consumed..].iter().filter(|keep| !**keep).count() as u64;
            let kept_end = media
                .fragments
                .iter()
                .zip(&keep)
                .skip(media.consumed)
                .filter(|(_, keep)| **keep)
                .map(|(&(_, end), _)| end)
                .max();
            // Parsed unshifted, so a dropped fragment's time never matters.
            let written = init
                .parse_media_segment(1, &media.data, 0)
                .and_then(|fragments| {
                    fragments
                        .into_iter()
                        .zip(keep)
                        .skip(media.consumed)
                        .filter(|(_, keep)| *keep)
                        .try_for_each(|(mut fragment, _)| {
                            fragment.shift_decode_time(shift)?;
                            self.writer.write(fragment, &mut data)
                        })
                });
            if let Err(error) = written {
                warn!(%error, "trailing audio rejected by the merger; dropped");
                break;
            }
            written_end = written_end.max(kept_end);
            last = Some(Arc::clone(&media.descriptor));
        }
        self.stats.late_fragments += late_fragments;
        if let Some(end) = written_end {
            self.note_written(run_init.track, end, follower_timescale);
        }
        if let Some(descriptor) = last
            && !data.is_empty()
        {
            let mut segment = init_media_segment(&descriptor);
            segment.duration = 0.0;
            self.emit_media(
                pending,
                HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                    segment,
                    data: Bytes::from(data),
                })),
            );
        }
    }
}

fn is_before(a: &PeriodKey, b: &PeriodKey) -> bool {
    !a.same(b) && a.start < b.start
}

fn is_after(a: &PeriodKey, b: &PeriodKey) -> bool {
    !a.same(b) && a.start > b.start
}

const NANOS_PER_SEC: i128 = 1_000_000_000;

/// `ticks` of `timescale` in nanoseconds, floored; `None` for a zero
/// timescale. Equal times in different timescales floor alike.
fn to_nanos(ticks: i128, timescale: u32) -> Option<i128> {
    (timescale > 0).then(|| {
        ticks
            .saturating_mul(NANOS_PER_SEC)
            .div_euclid(i128::from(timescale))
    })
}

/// `a / a_scale ≤ b / b_scale`, exactly. Times are 64-bit decode times plus
/// a 64-bit shift and timescales 32-bit, so the products fit in `i128`.
fn ends_by(a: i128, a_scale: u32, b: i128, b_scale: u32) -> bool {
    a.saturating_mul(i128::from(b_scale)) <= b.saturating_mul(i128::from(a_scale))
}

/// `a / a_scale < b / b_scale`, exactly.
fn starts_before(a: i128, a_scale: u32, b: i128, b_scale: u32) -> bool {
    a.saturating_mul(i128::from(b_scale)) < b.saturating_mul(i128::from(a_scale))
}

fn dash_meta(descriptor: &SegmentDescriptor) -> Option<&DashSegmentMeta> {
    match &descriptor.meta {
        SegmentMeta::Dash(meta) => Some(meta),
        SegmentMeta::Hls(_) => None,
    }
}

/// Builds a track init from its payload. A leader init that already
/// carries several tracks (a multiplexed Representation) is passed through
/// unmerged (`probe: None`) once [`validate_init`] accepts every track;
/// anything else the merger rejects is an error.
fn build_init(
    data: Bytes,
    descriptor: Arc<SegmentDescriptor>,
    leader: bool,
) -> Result<TrackInit, MergeError> {
    let track = dash_meta(&descriptor).map_or(MediaTrack::Video, |meta| meta.track);
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
        Err(MergeError::MultipleTracks { count }) if leader => {
            validate_init(&data)?;
            debug!(
                tracks = count,
                "multiplexed initialization segment passed through"
            );
            None
        }
        Err(error) => return Err(error),
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
        track,
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

/// Fragment spans of a segment on the presentation timeline, from the
/// track's own init. Parsed unshifted, since a shifted decode time may be
/// negative.
fn probe_fragments(init: &TrackInit, data: &Bytes) -> Result<Vec<(i128, i128)>, MergeError> {
    let Some(probe) = &init.probe else {
        return Ok(Vec::new());
    };
    let fragments = probe.parse_media_segment(0, data, 0)?;
    if fragments.is_empty() {
        return Err(MergeError::MissingBox {
            fourcc: mp4::FourCc(*b"moof"),
            parent: mp4::FourCc(*b"file"),
        });
    }
    Ok(fragments
        .iter()
        .map(|fragment| {
            let start = i128::from(fragment.decode_time()) + i128::from(init.time_shift);
            (start, start + i128::from(fragment.duration()))
        })
        .collect())
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
/// as duration and, for dynamic presentations whose timing anchor the
/// planner still trusts, the wall-clock time of its start.
fn media_segment(
    descriptor: &SegmentDescriptor,
    start: i128,
    end: i128,
    timescale: u32,
) -> m3u8_rs::MediaSegment {
    let duration = if timescale == 0 {
        0.0
    } else {
        ((end - start).max(0) as f64 / f64::from(timescale)) as f32
    };
    let program_date_time = dash_meta(descriptor)
        .and_then(|meta| meta.availability_start_time.as_ref())
        .and_then(WallClockAnchor::get)
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
    ticks: i128,
    timescale: u32,
) -> Option<DateTime<Utc>> {
    if timescale == 0 {
        return None;
    }
    let nanos = ticks.checked_mul(NANOS_PER_SEC)? / i128::from(timescale);
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
        descriptor_with_pto(track, kind, period, period_start_secs, number, 0)
    }

    /// A descriptor whose Representation has a presentation time offset
    /// (in milliseconds: the test MPD timescale is 1000).
    fn descriptor_with_pto(
        track: MediaTrack,
        kind: SegmentKind,
        period: Option<&str>,
        period_start_secs: u64,
        number: u64,
        pto_ms: u64,
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
                presentation_time_offset: pto_ms,
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

    /// `init` as another encoder configuration would produce it: equal in
    /// all but its `mdhd` modification time, so the merged init differs.
    fn changed(init: &[u8]) -> Vec<u8> {
        let mut data = init.to_vec();
        let at = data
            .windows(4)
            .position(|window| window == b"mdhd")
            .expect("init has an mdhd");
        // Version 0: creation, then modification time, 32 bits each.
        assert_eq!(data[at + 4], 0, "fixture mdhd is version 0");
        data[at + 12..at + 16].copy_from_slice(&0x5eed_u32.to_be_bytes());
        data
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
            None,
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
        // Period p1 restarts the media timeline at 0 and starts at 1 s,
        // with another video configuration.
        h.video(init(v, Some("p1"), 1, &changed(VIDEO_INIT))).await;
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
    async fn identical_inits_continue_the_output_across_a_period_change() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
        h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
        h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
            .await;
        // p1 repeats both inits and restarts the media timeline at 1 s.
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
        let kinds: Vec<&str> = items.iter().map(kind).collect();
        assert_eq!(kinds, ["init", "media", "media", "end"]);
        let all: Vec<(u32, u32)> = media_data(&items)
            .iter()
            .flat_map(|data| fragments(data))
            .collect();
        assert_eq!(
            all.iter().map(|(_, track)| *track).collect::<Vec<_>>(),
            [1, 2, 1, 2],
            "one fragment of each track per Period"
        );
        assert!(
            all.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "fragment sequence numbers continue: {all:?}"
        );
    }

    #[tokio::test]
    async fn an_identical_init_whose_media_overlaps_the_output_splits_it() {
        let h = start(MediaTrack::Video, false, lag(WAIT));
        let v = MediaTrack::Video;
        h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
        h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(media(v, Some("p0"), 0, 2, with_tfdt(VIDEO_1, 12_288)))
            .await;
        // p1 starts at 1 s, inside the 0–2 s already written.
        h.video(init(v, Some("p1"), 1, VIDEO_INIT)).await;
        h.video(media(v, Some("p1"), 1, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        let kinds: Vec<&str> = items.iter().map(kind).collect();
        assert_eq!(
            kinds,
            ["init", "media", "media", "epoch", "init", "media", "end"]
        );
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

    /// One init and one media segment carrying both fixture tracks, as a
    /// multiplexed Representation serves them.
    fn multiplexed() -> (Bytes, Bytes) {
        let init = merge_init(&[
            Bytes::from_static(VIDEO_INIT),
            Bytes::from_static(AUDIO_INIT),
        ])
        .unwrap();
        let mut segment = Vec::new();
        let mut writer = FragmentWriter::new();
        for (track, data) in [(0, VIDEO_1), (1, AUDIO_1)] {
            for fragment in init
                .parse_media_segment(track, &Bytes::from_static(data), 0)
                .unwrap()
            {
                writer.write(fragment, &mut segment).unwrap();
            }
        }
        (init.data().clone(), Bytes::from(segment))
    }

    #[tokio::test]
    async fn a_multiplexed_representation_is_passed_through_unchanged() {
        let (muxed_init, muxed_media) = multiplexed();
        let h = start(MediaTrack::Video, false, lag(WAIT));
        let v = MediaTrack::Video;
        h.video(init(v, None, 0, &muxed_init)).await;
        h.video(media(v, None, 0, 1, muxed_media.clone())).await;
        h.video(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        assert_eq!(init_data(&items), [&muxed_init]);
        assert_eq!(media_data(&items), [&muxed_media]);
        let tracks: Vec<u32> = fragments(&muxed_media).iter().map(|(_, id)| *id).collect();
        assert_eq!(tracks, [1, 2], "genuine two-track media");
        let check = IndependentFragmentCheck::from_init(&muxed_init).unwrap();
        assert!(check.is_independent(&muxed_media));
    }

    #[tokio::test]
    async fn protected_or_malformed_multiplexed_inits_are_not_passed_through() {
        let (muxed_init, _) = multiplexed();
        let mut protected = muxed_init.to_vec();
        let at = protected
            .windows(4)
            .position(|window| window == b"mp4a")
            .unwrap();
        protected[at..at + 4].copy_from_slice(b"enca");
        let malformed = {
            // Drop the audio track's `trex`: its fragments could not be read.
            let mut data = muxed_init.to_vec();
            let at = data
                .windows(4)
                .rposition(|window| window == b"trex")
                .unwrap()
                - 4;
            data.drain(at..at + 32);
            let mvex = data
                .windows(4)
                .position(|window| window == b"mvex")
                .unwrap()
                - 4;
            let size = u32::from_be_bytes(data[mvex..mvex + 4].try_into().unwrap()) - 32;
            data[mvex..mvex + 4].copy_from_slice(&size.to_be_bytes());
            let moov = data
                .windows(4)
                .position(|window| window == b"moov")
                .unwrap()
                - 4;
            let size = u32::from_be_bytes(data[moov..moov + 4].try_into().unwrap()) - 32;
            data[moov..moov + 4].copy_from_slice(&size.to_be_bytes());
            data
        };
        for (data, protected) in [(protected, true), (malformed, false)] {
            let mut h = start(MediaTrack::Video, false, lag(WAIT));
            h.video(init(MediaTrack::Video, None, 0, &data)).await;
            let item = tokio::time::timeout(WAIT, h.out.recv())
                .await
                .unwrap()
                .unwrap();
            match item {
                Err(DownloadError::Unsupported { .. }) => assert!(protected),
                Err(DownloadError::InvalidContent { .. }) => assert!(!protected),
                other => panic!("expected a typed rejection, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn audio_left_at_a_period_change_is_written_under_its_own_init() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
        h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
        h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
            .await;
        // Starts at 1 s, after the only video segment of p0 ends.
        h.audio(media(a, Some("p0"), 0, 2, with_tfdt(AUDIO_1, 48_000)))
            .await;
        h.video(init(v, Some("p1"), 2, &changed(VIDEO_INIT))).await;
        h.video(media(v, Some("p1"), 2, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;
        h.audio(init(a, Some("p1"), 2, AUDIO_INIT)).await;
        h.audio(media(a, Some("p1"), 2, 1, Bytes::from_static(AUDIO_1)))
            .await;
        h.audio(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        let kinds: Vec<String> = items
            .iter()
            .map(|item| match item {
                HlsData::M4sData(M4sData::InitSegment(_)) => "init".to_owned(),
                HlsData::M4sData(M4sData::Segment(segment)) => {
                    let tracks: Vec<String> = fragments(&segment.data)
                        .iter()
                        .map(|(_, track)| track.to_string())
                        .collect();
                    format!("media[{}]", tracks.join(","))
                }
                HlsData::EndMarker(Some(SplitReason::StreamStructureChange { .. })) => {
                    "epoch".to_owned()
                }
                HlsData::EndMarker(Some(SplitReason::EndOfStream)) => "end".to_owned(),
                _ => "other".to_owned(),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "init",
                "media[1,2]",
                "media[2]",
                "epoch",
                "init",
                "media[1,2]",
                "end"
            ]
        );
    }

    #[tokio::test]
    async fn fragments_straddling_the_period_start_get_a_common_origin() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        // PTO 0.5 s: both tracks' first fragments start half a second before
        // the Period.
        let item = |track, kind, number, data: Bytes| -> TrackItem {
            let descriptor = descriptor_with_pto(track, kind, None, 0, number, 500);
            Ok(AssembledItem::Payload(match kind {
                SegmentKind::Init => SegmentPayload::Mp4Init { data, descriptor },
                SegmentKind::Media => SegmentPayload::Mp4Media { data, descriptor },
            }))
        };
        h.video(item(
            v,
            SegmentKind::Init,
            0,
            Bytes::from_static(VIDEO_INIT),
        ))
        .await;
        h.video(item(v, SegmentKind::Media, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;
        h.audio(item(
            a,
            SegmentKind::Init,
            0,
            Bytes::from_static(AUDIO_INIT),
        ))
        .await;
        h.audio(item(a, SegmentKind::Media, 1, Bytes::from_static(AUDIO_1)))
            .await;
        h.audio(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        let segments = media_data(&items);
        assert_eq!(segments.len(), 1, "nothing was rejected");
        let decode_times: Vec<(u32, u64)> = boxes(segments[0])
            .into_iter()
            .filter(|(fourcc, _)| fourcc == b"moof")
            .map(|(_, moof)| {
                let traf = boxes(moof)
                    .into_iter()
                    .find(|(f, _)| f == b"traf")
                    .unwrap()
                    .1;
                let children = boxes(traf);
                let tfhd = children.iter().find(|(f, _)| f == b"tfhd").unwrap().1;
                let tfdt = children.iter().find(|(f, _)| f == b"tfdt").unwrap().1;
                (
                    u32::from_be_bytes(tfhd[4..8].try_into().unwrap()),
                    u64::from_be_bytes(tfdt[4..12].try_into().unwrap()),
                )
            })
            .collect();
        // Lifted by the same 0.5 s: both start at zero, keeping their offset.
        assert_eq!(decode_times, [(1, 0), (2, 0)]);
    }

    #[tokio::test]
    async fn fragments_dropped_before_the_origin_do_not_reject_their_segment() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        // Audio PTO 1 s: the segment's first fragment ends before the video
        // starts and is dropped; only the second is written.
        let audio = |kind, number, data: Bytes| -> TrackItem {
            let descriptor = descriptor_with_pto(a, kind, None, 0, number, 1000);
            Ok(AssembledItem::Payload(match kind {
                SegmentKind::Init => SegmentPayload::Mp4Init { data, descriptor },
                SegmentKind::Media => SegmentPayload::Mp4Media { data, descriptor },
            }))
        };
        let mut two_fragments = AUDIO_1.to_vec();
        two_fragments.extend_from_slice(&with_tfdt(AUDIO_1, 48_000));
        h.video(init(v, None, 0, VIDEO_INIT)).await;
        h.video(media(v, None, 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;
        h.audio(audio(SegmentKind::Init, 0, Bytes::from_static(AUDIO_INIT)))
            .await;
        h.audio(audio(SegmentKind::Media, 1, Bytes::from(two_fragments)))
            .await;
        h.audio(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, events) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, DownloadEvent::TrackDropped { .. })),
            "{events:?}"
        );
        let segments = media_data(&items);
        assert_eq!(segments.len(), 1);
        let tracks: Vec<u32> = fragments(segments[0]).iter().map(|(_, id)| *id).collect();
        assert_eq!(tracks, [1, 2], "the kept audio fragment is written");
    }

    /// `item` re-keyed into another epoch scope: the same Period and
    /// Representation id under a new init, as the planner issues after a PTO
    /// or sample entry change.
    fn rescoped(item: TrackItem, scope: &str) -> TrackItem {
        let Ok(AssembledItem::Payload(payload)) = item else {
            panic!("test items are payloads");
        };
        let rekey = |descriptor: &Arc<SegmentDescriptor>| {
            let mut descriptor = descriptor.as_ref().clone();
            descriptor.key.scope = Some(Arc::from(scope));
            Arc::new(descriptor)
        };
        Ok(AssembledItem::Payload(match payload {
            SegmentPayload::Mp4Init { data, descriptor } => SegmentPayload::Mp4Init {
                data,
                descriptor: rekey(&descriptor),
            },
            SegmentPayload::Mp4Media { data, descriptor } => SegmentPayload::Mp4Media {
                data,
                descriptor: rekey(&descriptor),
            },
            other => other,
        }))
    }

    fn absent(period: &str, start_secs: u64, after_sequence: u64) -> TrackItem {
        Ok(AssembledItem::Notice(EngineNotice::TrackAbsent {
            section_id: Some(Arc::from(period)),
            section_start: Duration::from_secs(start_secs),
            after_sequence,
        }))
    }

    #[tokio::test]
    async fn an_audio_only_period_announced_by_the_video_planner_is_recorded_live() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
        h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
        h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
            .await;
        // An ad break without video: no video item will come for p1, whose
        // gap follows video sequence number 1.
        h.video(absent("p1", 1, 2)).await;
        h.audio(init(a, Some("p1"), 1, AUDIO_INIT)).await;
        h.audio(media(a, Some("p1"), 1, 1, Bytes::from_static(AUDIO_1)))
            .await;
        let mut h = h;
        let mut seen = Vec::new();
        while seen.len() < 5 {
            let item = tokio::time::timeout(WAIT, h.out.recv())
                .await
                .expect("the audio-only Period is written without waiting for video")
                .unwrap()
                .unwrap();
            seen.push(kind(&item));
        }
        assert_eq!(seen, ["init", "media", "epoch", "init", "media"]);
        // Video returns with the next Period.
        h.video(init(v, Some("p2"), 2, VIDEO_INIT)).await;
        h.video(media(v, Some("p2"), 2, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;
        h.audio(init(a, Some("p2"), 2, AUDIO_INIT)).await;
        h.audio(media(a, Some("p2"), 2, 1, Bytes::from_static(AUDIO_1)))
            .await;
        h.audio(Ok(AssembledItem::StreamEnded)).await;
        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        let kinds: Vec<&str> = items.iter().map(kind).collect();
        assert_eq!(kinds, ["epoch", "init", "media", "end"]);
        let tracks: Vec<u32> = fragments(media_data(&items)[0])
            .iter()
            .map(|(_, id)| *id)
            .collect();
        assert_eq!(tracks, [1, 2], "back to video and audio");
    }

    /// A p0 A/V start, then `count` audio segments of the audio-only p1, under
    /// a buffer far smaller than that audio.
    async fn long_audio_only_period(video_ended: bool, count: u64) -> Vec<&'static str> {
        let config = DashMuxConfig {
            max_audio_lag: WAIT,
            max_buffered_bytes: 16 * 1024,
        };
        let mut h = start(MediaTrack::Video, true, config);
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
        h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        if video_ended {
            h.video(Ok(AssembledItem::StreamEnded)).await;
        } else {
            h.video(absent("p1", 1, 2)).await;
        }
        h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
        h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
            .await;
        h.audio(init(a, Some("p1"), 1, AUDIO_INIT)).await;
        let audio = h.audio.take().unwrap();
        let feeder = tokio::spawn(async move {
            for number in 1..=count {
                audio
                    .send(media(
                        a,
                        Some("p1"),
                        1,
                        number,
                        with_tfdt(AUDIO_1, (number - 1) * 48_000),
                    ))
                    .await
                    .unwrap();
            }
            audio
        });
        let mut kinds = Vec::new();
        // Everything of p1 is written while the video track stays silent
        // (or has ended) and audio keeps coming.
        while kinds.iter().filter(|kind| **kind == "media").count() < 1 + count as usize {
            let item = tokio::time::timeout(WAIT, h.out.recv())
                .await
                .expect("audio of the audio-only Period keeps being written")
                .unwrap()
                .unwrap();
            kinds.push(kind(&item));
        }
        let audio = feeder.await.unwrap();
        audio.send(Ok(AssembledItem::StreamEnded)).await.unwrap();
        if !video_ended {
            h.video(Ok(AssembledItem::StreamEnded)).await;
        }
        let ending = tokio::time::timeout(WAIT, h.out.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        kinds.push(kind(&ending));
        let outcome = tokio::time::timeout(WAIT, h.muxer).await.unwrap().unwrap();
        assert_eq!(outcome, MuxOutcome::Ended);
        kinds
    }

    #[tokio::test]
    async fn an_audio_only_period_is_written_completely_while_video_is_silent() {
        let kinds = long_audio_only_period(false, 40).await;
        let audio_only = kinds
            .iter()
            .skip(4)
            .filter(|kind| **kind == "media")
            .count();
        assert_eq!(&kinds[..4], ["init", "media", "epoch", "init"]);
        assert_eq!(audio_only, 40);
        assert_eq!(kinds.last(), Some(&"end"));
    }

    #[tokio::test]
    async fn an_audio_only_period_is_written_completely_after_video_ended() {
        let kinds = long_audio_only_period(true, 40).await;
        let audio_only = kinds
            .iter()
            .skip(4)
            .filter(|kind| **kind == "media")
            .count();
        assert_eq!(&kinds[..4], ["init", "media", "epoch", "init"]);
        assert_eq!(audio_only, 40);
        assert_eq!(kinds.last(), Some(&"end"));
    }

    #[tokio::test]
    async fn absence_notices_cannot_keep_a_session_without_media_alive() {
        let (video, video_rx) = mpsc::channel(64);
        let (out_tx, mut out) = mpsc::channel(64);
        let muxer = tokio::spawn(run_muxer(
            MuxTrack {
                track: MediaTrack::Video,
                rx: video_rx,
                cancel: CancellationToken::new(),
            },
            None,
            out_tx,
            None,
            lag(WAIT),
            Some(Duration::from_millis(200)),
            CancellationToken::new(),
        ));
        let v = MediaTrack::Video;
        video
            .send(init(v, Some("p0"), 0, VIDEO_INIT))
            .await
            .unwrap();
        video
            .send(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await
            .unwrap();
        // The only other track failed; video keeps reporting it is absent.
        let notices = tokio::spawn(async move {
            for _ in 0..100 {
                if video.send(absent("p1", 1, 2)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let mut last = None;
        while let Some(item) = tokio::time::timeout(WAIT, out.recv())
            .await
            .expect("the session ends")
        {
            last = Some(item);
        }
        assert!(
            matches!(last, Some(Err(DownloadError::Timeout { .. }))),
            "{last:?}"
        );
        let outcome = tokio::time::timeout(WAIT, muxer).await.unwrap().unwrap();
        assert!(matches!(outcome, MuxOutcome::Failed(_)));
        notices.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn time_blocked_on_a_slow_consumer_is_not_a_media_stall() {
        let (video, video_rx) = mpsc::channel(64);
        // Room for the first group only; the next delivery blocks.
        let (out_tx, mut out) = mpsc::channel(2);
        let stall = Duration::from_secs(60);
        let muxer = tokio::spawn(run_muxer(
            MuxTrack {
                track: MediaTrack::Video,
                rx: video_rx,
                cancel: CancellationToken::new(),
            },
            None,
            out_tx,
            None,
            lag(WAIT),
            Some(stall),
            CancellationToken::new(),
        ));
        let v = MediaTrack::Video;
        video
            .send(init(v, Some("p0"), 0, VIDEO_INIT))
            .await
            .unwrap();
        for number in 1..=2 {
            video
                .send(media(
                    v,
                    Some("p0"),
                    0,
                    number,
                    with_tfdt(VIDEO_1, (number - 1) * 12_288),
                ))
                .await
                .unwrap();
        }
        // The consumer reads nothing for longer than the stall setting.
        tokio::time::sleep(stall + Duration::from_secs(30)).await;
        for expected in ["init", "media", "media"] {
            let item = out.recv().await.unwrap().unwrap();
            assert_eq!(kind(&item), expected);
        }
        // A quiet stretch shorter than the stall setting follows.
        for _ in 0..5 {
            video.send(absent("p1", 1, 3)).await.unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        assert!(out.try_recv().is_err(), "no timeout error");
        assert!(!muxer.is_finished());
        video.send(Ok(AssembledItem::StreamEnded)).await.unwrap();
        assert_eq!(kind(&out.recv().await.unwrap().unwrap()), "end");
        assert_eq!(muxer.await.unwrap(), MuxOutcome::Ended);
    }

    #[tokio::test]
    async fn changed_init_audio_tails_survive_a_period_change() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
        h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.audio(rescoped(init(a, Some("p0"), 0, AUDIO_INIT), "a#1"))
            .await;
        h.audio(rescoped(
            media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)),
            "a#1",
        ))
        .await;
        // p0's audio switches init after the only video segment.
        h.audio(rescoped(
            init(a, Some("p0"), 0, &changed(AUDIO_INIT)),
            "a#2",
        ))
        .await;
        h.audio(rescoped(
            media(a, Some("p0"), 0, 2, with_tfdt(AUDIO_1, 48_000)),
            "a#2",
        ))
        .await;
        h.video(init(v, Some("p1"), 2, &changed(VIDEO_INIT))).await;
        h.video(media(v, Some("p1"), 2, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;
        h.audio(init(a, Some("p1"), 2, AUDIO_INIT)).await;
        h.audio(media(a, Some("p1"), 2, 1, Bytes::from_static(AUDIO_1)))
            .await;
        h.audio(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        let kinds: Vec<&str> = items.iter().map(kind).collect();
        assert_eq!(
            kinds,
            [
                "init", "media", "epoch", "init", "media", "epoch", "init", "media", "end"
            ]
        );
        let tail = media_data(&items)[1];
        assert!(
            fragments(tail).iter().all(|(_, id)| *id == 2),
            "the a#2 tail"
        );
    }

    #[tokio::test]
    async fn an_audio_tail_under_a_changed_init_is_announced_with_it() {
        let h = start(MediaTrack::Video, true, lag(WAIT));
        let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
        // Same Period and Representation id, but a new init (as the planner
        // issues after a PTO or sample entry change): a new epoch scope.
        h.video(init(v, None, 0, VIDEO_INIT)).await;
        h.video(media(v, None, 0, 1, Bytes::from_static(VIDEO_1)))
            .await;
        h.video(Ok(AssembledItem::StreamEnded)).await;
        h.audio(rescoped(init(a, None, 0, AUDIO_INIT), "a#1")).await;
        h.audio(rescoped(
            media(a, None, 0, 1, Bytes::from_static(AUDIO_1)),
            "a#1",
        ))
        .await;
        h.audio(rescoped(init(a, None, 0, &changed(AUDIO_INIT)), "a#2"))
            .await;
        h.audio(rescoped(
            media(a, None, 0, 2, with_tfdt(AUDIO_1, 48_000)),
            "a#2",
        ))
        .await;
        h.audio(Ok(AssembledItem::StreamEnded)).await;

        let (items, outcome, _) = h.finish().await;
        assert_eq!(outcome, MuxOutcome::Ended);
        let kinds: Vec<&str> = items.iter().map(kind).collect();
        assert_eq!(kinds, ["init", "media", "epoch", "init", "media", "end"]);
        let tail = media_data(&items)[1];
        assert!(fragments(tail).iter().all(|(_, id)| *id == 2));
    }

    #[tokio::test]
    async fn an_unexplained_leader_closure_is_a_pipeline_error() {
        let mut h = start(MediaTrack::Video, false, lag(WAIT));
        h.video(init(MediaTrack::Video, None, 0, VIDEO_INIT)).await;
        drop(h.video);
        let item = tokio::time::timeout(WAIT, h.out.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(item, Err(DownloadError::Internal { .. })));
        let outcome = tokio::time::timeout(WAIT, h.muxer).await.unwrap().unwrap();
        assert!(matches!(outcome, MuxOutcome::Failed(_)));
    }

    /// A single-track muxer with its cancel token, for tests that end the
    /// session themselves.
    type OutputRx = mpsc::Receiver<Result<HlsData, DownloadError>>;

    fn start_cancellable() -> (
        mpsc::Sender<TrackItem>,
        OutputRx,
        CancellationToken,
        tokio::task::JoinHandle<MuxOutcome>,
    ) {
        let (video, video_rx) = mpsc::channel(64);
        let (out_tx, out) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let muxer = tokio::spawn(run_muxer(
            MuxTrack {
                track: MediaTrack::Video,
                rx: video_rx,
                cancel: CancellationToken::new(),
            },
            None,
            out_tx,
            None,
            lag(WAIT),
            None,
            cancel.clone(),
        ));
        (video, out, cancel, muxer)
    }

    fn kind(item: &HlsData) -> &'static str {
        match item {
            HlsData::M4sData(M4sData::InitSegment(_)) => "init",
            HlsData::M4sData(M4sData::Segment(_)) => "media",
            HlsData::EndMarker(Some(SplitReason::StreamStructureChange { .. })) => "epoch",
            HlsData::EndMarker(Some(SplitReason::EndOfStream)) => "end",
            _ => "other",
        }
    }

    async fn next_kind(out: &mut OutputRx) -> &'static str {
        let item = tokio::time::timeout(WAIT, out.recv())
            .await
            .expect("muxer output")
            .expect("open output")
            .expect("no stream error");
        kind(&item)
    }

    #[tokio::test]
    async fn an_epoch_without_media_is_never_announced() {
        let v = MediaTrack::Video;
        for cancel_session in [false, true] {
            let (video, mut out, cancel, muxer) = start_cancellable();
            video
                .send(init(v, Some("p0"), 0, VIDEO_INIT))
                .await
                .unwrap();
            video
                .send(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
                .await
                .unwrap();
            assert_eq!(next_kind(&mut out).await, "init");
            assert_eq!(next_kind(&mut out).await, "media");
            // The next Period's init arrives; its media never does.
            video
                .send(init(v, Some("p1"), 1, VIDEO_INIT))
                .await
                .unwrap();
            if cancel_session {
                tokio::time::sleep(Duration::from_millis(50)).await;
                cancel.cancel();
            } else {
                video.send(Ok(AssembledItem::StreamEnded)).await.unwrap();
            }
            let outcome = tokio::time::timeout(WAIT, muxer).await.unwrap().unwrap();
            let mut rest = Vec::new();
            while let Ok(item) = out.try_recv() {
                rest.push(kind(&item.unwrap()));
            }
            if cancel_session {
                assert_eq!(outcome, MuxOutcome::Cancelled);
                assert!(rest.is_empty(), "{rest:?}");
            } else {
                assert_eq!(outcome, MuxOutcome::Ended);
                assert_eq!(rest, ["end"]);
            }
        }
    }

    #[tokio::test]
    async fn an_epoch_is_announced_together_with_its_first_media() {
        let v = MediaTrack::Video;
        let (video, mut out, cancel, muxer) = start_cancellable();
        video
            .send(init(v, Some("p0"), 0, VIDEO_INIT))
            .await
            .unwrap();
        video
            .send(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await
            .unwrap();
        video
            .send(init(v, Some("p1"), 1, &changed(VIDEO_INIT)))
            .await
            .unwrap();
        video
            .send(media(v, Some("p1"), 1, 1, Bytes::from_static(VIDEO_1)))
            .await
            .unwrap();
        assert_eq!(next_kind(&mut out).await, "init");
        assert_eq!(next_kind(&mut out).await, "media");
        assert_eq!(next_kind(&mut out).await, "epoch");
        // Cancelling right after the barrier cannot strand its init: the
        // whole group was delivered at once.
        cancel.cancel();
        assert_eq!(next_kind(&mut out).await, "init");
        assert_eq!(next_kind(&mut out).await, "media");
        tokio::time::timeout(WAIT, muxer).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn cancellation_is_not_blocked_by_a_consumer_that_stopped_reading() {
        let (video, video_rx) = mpsc::channel(64);
        // Never read; room for exactly the first group (init + media).
        let (out_tx, _held) = mpsc::channel(2);
        let probe = out_tx.clone();
        let cancel = CancellationToken::new();
        let muxer = tokio::spawn(run_muxer(
            MuxTrack {
                track: MediaTrack::Video,
                rx: video_rx,
                cancel: CancellationToken::new(),
            },
            None,
            out_tx,
            None,
            lag(WAIT),
            None,
            cancel.clone(),
        ));
        video
            .send(init(MediaTrack::Video, None, 0, VIDEO_INIT))
            .await
            .unwrap();
        for number in 1..=4 {
            video
                .send(media(
                    MediaTrack::Video,
                    None,
                    0,
                    number,
                    with_tfdt(VIDEO_1, (number - 1) * 12_288),
                ))
                .await
                .unwrap();
        }
        // The output is full and the muxer has taken the second media item,
        // whose delivery it is now blocked on: it reads nothing further.
        tokio::time::timeout(WAIT, async {
            while probe.capacity() != 0 || video.capacity() != 62 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the muxer blocks on a full output");
        cancel.cancel();
        let outcome = tokio::time::timeout(WAIT, muxer)
            .await
            .expect("cancellation finishes the muxer")
            .unwrap();
        assert_eq!(outcome, MuxOutcome::Cancelled);
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
