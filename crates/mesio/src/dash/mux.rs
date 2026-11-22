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
//! A new epoch may begin inside media already written: while a dynamic
//! MPD's last Period is open, its segments are planned past the start of a
//! Period that a later update reveals, and that Period is planned from its
//! start. Each track remembers the intervals of media it recently wrote
//! and where its newest media ends. Until a track writes a fragment of its
//! new epoch, fragments lying within one written interval, starting at
//! most [`MAX_REPEATED_SEGMENTS`] of their segment's duration before that
//! end, are dropped as already written; from its first fragment written,
//! the epoch's media is never judged against earlier epochs. Written media,
//! not downloads, decides, so exactly one copy is kept whether the earlier
//! Period's segments were written, failed or skipped, and a hole the
//! earlier Period left is filled. The first fragment kept, if it still
//! starts before the written end (filling a hole, straddling the end, or
//! further back: a timeline reset), starts a new file and is written
//! whole, so decode times never go back within a file and nothing beyond a
//! repeat is dropped. A file continues across a byte-identical init only
//! when every track's first media of the new epoch follows on.
//!
//! A Period without video (an audio-only interlude, or audio carried past
//! the last video Period) is recorded as its own audio-only epoch: one item
//! per audio segment under the audio track's own init, its duration the
//! audio coverage. A Period counts as video-less once the video planner
//! reports it (`TrackAbsent`), video arrives from a later Period, or the
//! video track has ended.
//!
//! The reverse is not detected: the audio planner's `TrackAbsent` is a
//! snapshot of an MPD that later updates may contradict, and it arrives
//! ahead of earlier audio, so it cannot prove that no audio of a Period is
//! still to come. A Period without audio is therefore waited for like late
//! audio (below): its video is written video-only once no audio has arrived
//! within up to `max_audio_lag`.
//!
//! Audio never fails a recording. A video segment waits for the audio it
//! overlaps up to `max_audio_lag` after it arrived here, excluding time
//! actually held by bounded rate limiting (less under buffer pressure).
//! It is then written with what has arrived; a
//! `TrackStalled` event reports each such episode, and audio arriving later
//! is dropped as late. An audio pipeline failure, protected audio, or audio the merger
//! rejects drops the audio track for the rest of the session with a
//! `TrackDropped` event.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use hls::{HlsData, M4sData, M4sInitSegmentData, M4sSegmentData, SplitReason};
use jiff::{SignedDuration, Timestamp};
use mp4::merge::{
    FragmentWriter, MergeError, MergedInit, SourceFragment, merge_init, validate_init,
};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::DownloadError;
use crate::dash::config::DashMuxConfig;
use crate::dash::manifest::NANOS_PER_SEC;
use crate::dash::planner::{ConsumedFrontier, PeriodKey};
use crate::redact::redact_url;
use crate::segment_engine::assembler::{AssembledItem, AssemblerSink, SinkClosed};
use crate::segment_engine::control::EngineNotice;
use crate::segment_engine::descriptor::{
    DashSegmentMeta, MediaTrack, SegmentDescriptor, SegmentMeta, WallClockAnchor,
};
use crate::segment_engine::payload::SegmentPayload;
use crate::segment_engine::throttle::ThrottleClock;
use crate::session::{DownloadEvent, EventSink};

/// Consecutive leader segments the merger may reject before the stream
/// fails. One corrupt segment is skipped; a systematic rejection is an error
/// rather than a recording that silently writes nothing.
const MAX_CONSECUTIVE_LEADER_REJECTIONS: u32 = 3;

/// Leader-less Periods remembered; only the newest few can still matter.
const MAX_ABSENT_PERIODS: usize = 16;

/// How far behind a track's written end a new epoch's fragment may start,
/// in durations of its segment, and still be dropped as a repeat. A repeat
/// is what an open Period's template planned past the start of the Period
/// an update reveals, so the MPD's staleness bounds it: refreshed at the
/// MPD's update period, about one or two segments. Media further back is
/// a timeline reset, which must never be dropped: it starts a new file.
const MAX_REPEATED_SEGMENTS: i128 = 3;

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
    /// Follower only: actual time held by rate limiting at its emit cursor.
    pub rate_limit_clock: Option<watch::Receiver<ThrottleClock>>,
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
    repeated_fragments: u64,
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
    /// Two-track probe (`track_id == 2` at index 1) for follower tracks so
    /// `probe_fragments` produces ready-to-write `SourceFragment`s for merged
    /// output without re-parsing MP4 boxes on emit.
    follower_probe: Option<MergedInit>,
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
    /// Unshifted fragments parsed during `probe_fragments`, cached so slice
    /// and tail emission never re-parse MP4 boxes.
    parsed: Vec<SourceFragment>,
    /// Fragments at the front already written or dropped.
    consumed: usize,
    /// When the muxer received it; the leader's ordinary audio wait starts here.
    arrived: Instant,
    /// Leader only: follower pause counter when this slice arrived.
    follower_paused_at: Duration,
}

impl QueuedMedia {
    /// How many fragments are not yet written or dropped.
    fn remaining(&self) -> usize {
        self.fragments.len() - self.consumed
    }

    /// The span of the fragments not yet written or dropped.
    fn span(&self) -> Option<(i128, i128)> {
        let fragments = self.fragments.get(self.consumed..)?;
        let start = fragments.iter().map(|&(start, _)| start).min()?;
        let end = fragments.iter().map(|&(_, end)| end).max()?;
        Some((start, end))
    }

    /// How many fragments, from the first not yet written or dropped,
    /// repeat media the track already wrote (see the module documentation).
    fn repeated_prefix(&self, written: Option<&Written>) -> usize {
        let Some(written) = written.filter(|written| !Arc::ptr_eq(&written.init, &self.init))
        else {
            return 0;
        };
        let timescale = self.init.timescale;
        let segment_start = self.fragments.iter().map(|&(start, _)| start).min();
        let segment_end = self.fragments.iter().map(|&(_, end)| end).max();
        let Some(window) = segment_start
            .zip(segment_end)
            .and_then(|(start, end)| to_nanos(end - start, timescale))
            .map(|duration| duration.saturating_mul(MAX_REPEATED_SEGMENTS))
        else {
            return 0;
        };
        self.fragments
            .get(self.consumed..)
            .unwrap_or_default()
            .iter()
            .take_while(|&&(start, end)| {
                matches!(
                    (to_nanos(start, timescale), to_nanos(end, timescale)),
                    (Some(start), Some(end))
                        if written.end - start <= window && written.contains(start, end)
                )
            })
            .count()
    }
}

/// Written intervals a track remembers. Gaps between them (segments that
/// failed or were skipped) are what keeps intervals apart, so a few cover
/// any repeat window.
const MAX_WRITTEN_INTERVALS: usize = 16;

/// What a track wrote to the output.
#[derive(Debug, Clone)]
struct Written {
    /// End of the media it last wrote, in nanoseconds on the presentation
    /// timeline.
    end: i128,
    /// Its init: media under another one is a new epoch of the track.
    init: Arc<TrackInit>,
    /// Whether the current file holds media of the track.
    in_file: bool,
    /// The newest written intervals, in nanoseconds, ascending and
    /// coalesced where they touch; restarted when written media goes back
    /// in time, as what lies ahead then belongs to an earlier timeline.
    intervals: VecDeque<(i128, i128)>,
}

impl Written {
    fn new(init: Arc<TrackInit>) -> Self {
        Self {
            end: i128::MIN,
            init,
            in_file: false,
            intervals: VecDeque::new(),
        }
    }

    /// Whether `[start, end)` lies within one written interval.
    fn contains(&self, start: i128, end: i128) -> bool {
        self.intervals
            .iter()
            .any(|&(from, to)| from <= start && end <= to)
    }

    /// Records media of `init` written over `[start, end)`.
    fn record(&mut self, init: &Arc<TrackInit>, start: i128, end: i128) {
        if start < self.end {
            self.intervals.clear();
        }
        match self.intervals.back_mut() {
            Some(last) if last.1 >= start => last.1 = last.1.max(end),
            _ => self.intervals.push_back((start, end)),
        }
        while self.intervals.len() > MAX_WRITTEN_INTERVALS {
            self.intervals.pop_front();
        }
        self.end = end;
        self.init = Arc::clone(init);
        self.in_file = true;
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
    /// Closed without an end (cancellation).
    Closed,
    /// Dropped by the muxer after a failure; never resumes.
    Dropped,
}

struct TrackState {
    track: MediaTrack,
    status: Status,
    cancel: CancellationToken,
    queue: VecDeque<Queued>,
    queued_bytes: usize,
    /// The newest init received; media references it.
    latest_init: Option<Arc<TrackInit>>,
    /// Follower only: the newest init the slice logic has moved past.
    current_init: Option<Arc<TrackInit>>,
    /// What the track last wrote.
    written: Option<Written>,
}

impl TrackState {
    fn new(track: MediaTrack, cancel: CancellationToken) -> Self {
        Self {
            track,
            status: Status::Active,
            cancel,
            queue: VecDeque::new(),
            queued_bytes: 0,
            latest_init: None,
            current_init: None,
            written: None,
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
    /// The end marker (when an epoch precedes it) and the init; empty when
    /// the epoch continues the current file.
    header: Vec<HlsData>,
    /// Whether the epoch's merged init repeats the current one byte for
    /// byte, so the current file continues (see [`MuxState::emit_media`]).
    continued: bool,
}

/// What an output item holds of one track: fragments under `init`, each
/// `(start, end)` on the presentation timeline in its timescale, in order.
/// Each is recorded as written on its own, so a hole between two (a
/// segment that failed or arrived late) is never taken for written media.
struct TrackSpan {
    init: Arc<TrackInit>,
    fragments: Vec<(i128, i128)>,
}

/// Follower fragments chosen for one slice: a run of `count` fragments
/// starting at `first` of one queued segment.
struct FollowerPart {
    init: Arc<TrackInit>,
    data: Bytes,
    first: usize,
    count: usize,
    /// Presentation start of the first fragment, in the follower timescale.
    start: i128,
    /// `(start, end)` of each fragment, in the follower timescale.
    fragments: Vec<(i128, i128)>,
    /// Cached `SourceFragment`s for the selected sub-slice (`first..first + count`).
    parsed: Vec<SourceFragment>,
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
    follower_clock: ThrottleClock,
    /// Set when output stopped waiting for the follower; cleared by the next
    /// follower input. One stall event per episode.
    follower_stalled: bool,
    /// When the leader's authoritative end arrived.
    leader_ended_at: Option<(Instant, Duration)>,
    /// When the follower last delivered anything.
    follower_input_at: Option<(Instant, Duration)>,
    /// When a media item was last written (or the muxer started).
    last_media_at: Instant,
    media_clock: ThrottleClock,
    media_paused_at: Duration,
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
    /// Whether the follower's tail of the current output epoch was written
    /// ahead of a Period change.
    tail_flushed: bool,
    leader_rejections: u32,
    finished: bool,
    stats: MuxStats,
}

/// A failure that ends the stream.
struct Fatal(DownloadError);

/// The session watchdog follows the union of both tracks' actual pauses.
pub(crate) struct MediaStall {
    pub timeout: Duration,
    pub clock: watch::Receiver<ThrottleClock>,
}

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
    media_stall: Option<MediaStall>,
    cancel: CancellationToken,
) -> MuxOutcome {
    let media_timeout = media_stall.as_ref().map(|stall| stall.timeout);
    let mut media_clock_rx = media_stall.map(|stall| stall.clock);
    let media_clock = media_clock_rx
        .as_mut()
        .map_or_else(ThrottleClock::default, |rx| *rx.borrow_and_update());
    let now = Instant::now();
    let mut leader_rx = Some(leader.rx);
    let (follower_state, mut follower_rx, mut follower_clock_rx) = match follower {
        Some(track) => (
            Some(TrackState::new(track.track, track.cancel)),
            Some(track.rx),
            track.rate_limit_clock,
        ),
        None => (None, None, None),
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
        follower_clock: ThrottleClock::default(),
        follower_stalled: false,
        leader_ended_at: None,
        follower_input_at: None,
        last_media_at: now,
        media_clock,
        media_paused_at: media_clock.paused_at(now.into_std()),
        leader_absent: VecDeque::new(),
        leader_next_sequence: 0,
        leader_end: None,
        origin_nanos: None,
        tail_flushed: false,
        leader_rejections: 0,
        finished: false,
        stats: MuxStats::default(),
    };

    let outcome = loop {
        read_clock(&mut follower_clock_rx, &mut state.follower_clock);
        read_clock(&mut media_clock_rx, &mut state.media_clock);
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
            read_clock(&mut media_clock_rx, &mut state.media_clock);
            state.last_media_at = Instant::now();
            state.media_paused_at = state.media_clock.paused_at(state.last_media_at.into_std());
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
        let stall_deadline =
            media_timeout.map(|timeout| state.media_deadline(timeout, Instant::now()));

        tokio::select! {
            biased;
            _ = cancel.cancelled() => break MuxOutcome::Cancelled,
            item = recv(&mut leader_rx), if leader_open => {
                read_clock(&mut follower_clock_rx, &mut state.follower_clock);
                if let Err(Fatal(error)) = state.accept_leader(item, cancel.is_cancelled()) {
                    break fail(&out, error, &cancel).await;
                }
            }
            item = recv(&mut follower_rx), if follower_open => {
                read_clock(&mut follower_clock_rx, &mut state.follower_clock);
                state.accept_follower(item, cancel.is_cancelled());
            }
            _ = clock_changed(&mut follower_clock_rx), if follower_clock_rx.is_some() => {}
            _ = clock_changed(&mut media_clock_rx), if media_clock_rx.is_some() => {}
            // Wakes the next pump, which stops waiting past the deadline.
            _ = sleep_until(deadline), if deadline.is_some() => {}
            _ = sleep_until(stall_deadline), if stall_deadline.is_some() => {
                // A producer can publish after the watch arm was polled but
                // before this timer became ready in the same select.
                read_clock(&mut media_clock_rx, &mut state.media_clock);
                let stalled_for = media_timeout.unwrap_or_default();
                let now = Instant::now();
                if state.media_deadline(stalled_for, now) > now {
                    continue;
                }
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
        repeated_fragments = stats.repeated_fragments,
        ?outcome,
        "DASH muxer finished"
    );
    outcome
}

fn read_clock(rx: &mut Option<watch::Receiver<ThrottleClock>>, clock: &mut ThrottleClock) {
    if let Some(rx) = rx {
        *clock = *rx.borrow_and_update();
    }
}

async fn clock_changed(rx: &mut Option<watch::Receiver<ThrottleClock>>) {
    if let Some(receiver) = rx
        && receiver.changed().await.is_err()
    {
        *rx = None;
    }
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
                    Ok((fragments, parsed)) => {
                        let arrived = Instant::now();
                        self.leader_rejections = 0;
                        self.leader.queued_bytes += data.len();
                        self.leader.queue.push_back(Queued::Media(QueuedMedia {
                            init,
                            data,
                            descriptor,
                            fragments,
                            parsed,
                            consumed: 0,
                            arrived,
                            follower_paused_at: self.follower_clock.paused_at(arrived.into_std()),
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
                let now = Instant::now();
                self.leader_ended_at = Some((now, self.follower_clock.paused_at(now.into_std())));
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
        let now = Instant::now();
        self.follower_input_at = Some((now, self.follower_clock.paused_at(now.into_std())));
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
                    Ok((fragments, parsed)) => {
                        follower.queued_bytes += data.len();
                        follower.queue.push_back(Queued::Media(QueuedMedia {
                            init,
                            data,
                            descriptor,
                            fragments,
                            parsed,
                            consumed: 0,
                            arrived: Instant::now(),
                            follower_paused_at: Duration::ZERO,
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
        if follower.status == Status::Dropped {
            return;
        }
        warn!(
            track = %follower.track,
            reason,
            detail = %crate::redact::redact_diagnostic(detail),
            "dropping track; recording continues without it"
        );
        follower.status = Status::Dropped;
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
            .filter_map(|(ticks, timescale)| to_nanos(ticks, timescale))
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

    fn follower_deadline(&self, since: Instant, paused_at: Duration, now: Instant) -> Instant {
        Instant::from_std(self.follower_clock.deadline(
            since.into_std(),
            paused_at,
            self.config.max_audio_lag,
            now.into_std(),
        ))
    }

    /// When the session fails for writing no media.
    fn media_deadline(&self, timeout: Duration, now: Instant) -> Instant {
        Instant::from_std(self.media_clock.deadline(
            self.last_media_at.into_std(),
            self.media_paused_at,
            timeout,
            now.into_std(),
        ))
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
        if self.drop_repeated_leader() {
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
        let deadline = self.follower_deadline(head.arrived, head.follower_paused_at, now);
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
            Status::Closed | Status::Dropped => Step::Finished,
            Status::Ended => {
                // Completion is shared: the follower's tail belongs in the
                // final items, so its end is awaited while it keeps
                // delivering, up to one lag after its last input.
                let (since, paused_at) = self
                    .leader_ended_at
                    .into_iter()
                    .chain(self.follower_input_at)
                    .max()
                    .unwrap_or((now, self.follower_clock.paused_at(now.into_std())));
                let deadline = self.follower_deadline(since, paused_at, now);
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
                    if picks > 0 || init.period.is_after(period) {
                        return Collect::Ready(picks);
                    }
                    // Nothing is picked, so this init is at the head.
                    let init = Arc::clone(init);
                    follower.pop_front();
                    follower.current_init = Some(init);
                }
                Queued::Media(media) => {
                    if media.init.period.is_after(period) {
                        return Collect::Ready(picks);
                    }
                    if media.init.period.is_before(period) {
                        if picks > 0 {
                            return Collect::Ready(picks);
                        }
                        self.stats.stale_fragments += media.remaining() as u64;
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
                    if picks == 0 {
                        let repeated = media.repeated_prefix(follower.written.as_ref());
                        media.consumed += repeated;
                        self.stats.repeated_fragments += repeated as u64;
                    }
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
            let count = picks.min(media.remaining());
            let parsed = if media.consumed == 0 && count == media.parsed.len() {
                std::mem::take(&mut media.parsed)
            } else {
                media
                    .parsed
                    .get(media.consumed..media.consumed + count)
                    .map(<[SourceFragment]>::to_vec)
                    .unwrap_or_default()
            };
            parts.push(FollowerPart {
                init: Arc::clone(&media.init),
                data: media.data.clone(),
                first: media.consumed,
                count,
                start: media.fragments[media.consumed].0,
                fragments: media.fragments[media.consumed..media.consumed + count].to_vec(),
                parsed,
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
        if follower.status == Status::Dropped {
            return None;
        }
        follower
            .current_init
            .clone()
            .filter(|init| init.period.same(&leader.period))
    }

    /// The output epoch `(leader, follower)` when it differs from the
    /// current one, with the end marker and merged init that announce it;
    /// without either when its merged init repeats the current one, which
    /// continues the file. Nothing is emitted or committed here: the caller
    /// sends the header with the epoch's first media item, or drops it with
    /// that item.
    fn prepare_output(
        &mut self,
        leader: &Arc<TrackInit>,
        follower: Option<Arc<TrackInit>>,
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
        let continued = self
            .output
            .as_ref()
            .is_some_and(|output| repeats_init(output, init.as_ref()));
        match &self.output {
            Some(output) if continued => {
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
                header.push(init_item(leader, init.as_ref()));
            }
        }
        Some(PendingOutput {
            output: Output {
                leader: Arc::clone(leader),
                follower,
                init,
            },
            header,
            continued,
        })
    }

    fn track_mut(&mut self, track: MediaTrack) -> Option<&mut TrackState> {
        if self.leader.track == track {
            return Some(&mut self.leader);
        }
        self.follower
            .as_mut()
            .filter(|follower| follower.track == track)
    }

    fn written_of(&self, track: MediaTrack) -> Option<&Written> {
        if self.leader.track == track {
            return self.leader.written.as_ref();
        }
        self.follower
            .as_ref()
            .filter(|follower| follower.track == track)?
            .written
            .as_ref()
    }

    /// Drops the leader head's fragments that repeat written media (see the
    /// module documentation); returns whether the whole segment went.
    fn drop_repeated_leader(&mut self) -> bool {
        let Some(Queued::Media(head)) = self.leader.queue.front_mut() else {
            return false;
        };
        let repeated = head.repeated_prefix(self.leader.written.as_ref());
        if repeated == 0 {
            return false;
        }
        head.consumed += repeated;
        self.stats.repeated_fragments += repeated as u64;
        debug!(
            track = %self.leader.track,
            msn = head.descriptor.msn,
            fragments = repeated,
            "media already written from the previous epoch; dropped"
        );
        if head.consumed < head.fragments.len() {
            return false;
        }
        self.leader.pop_front();
        true
    }

    /// Emits `item`, which holds `spans` of media, preceded by the header of
    /// `pending` when it opens a new output epoch, and commits that epoch.
    ///
    /// A track's first media of a new epoch, in this item or a later one
    /// (inits and media of the tracks arrive independently), that starts
    /// before what the current file holds of the track starts a new file
    /// under the same init, so decode times never go back within a file.
    fn emit_media(&mut self, pending: Option<PendingOutput>, item: HlsData, spans: &[TrackSpan]) {
        let continued = pending.as_ref().map(|pending| pending.continued);
        let overlapping = spans
            .iter()
            .find(|span| {
                self.written_of(span.init.track).is_some_and(|written| {
                    !Arc::ptr_eq(&written.init, &span.init)
                        && written.in_file
                        && span.fragments.first().is_none_or(|&(start, _)| {
                            to_nanos(start, span.init.timescale)
                                .is_none_or(|start| start < written.end)
                        })
                })
            })
            .map(|span| span.init.track);
        let mut group = Vec::with_capacity(3);
        let mut split = continued == Some(false);
        if !split
            && let Some(track) = overlapping
            && let Some(output) = output_for(pending.as_ref(), self.output.as_ref())
        {
            let description = format!(
                "DASH {track} media of {} starts before the file's",
                output.leader.period
            );
            group.push(HlsData::end_marker_with_reason(
                SplitReason::StreamStructureChange { description },
            ));
            group.push(init_item(&output.leader, output.init.as_ref()));
            split = true;
        }
        if let Some(pending) = pending {
            group.extend(pending.header);
            self.output = Some(pending.output);
            self.tail_flushed = false;
        }
        if let Some(HlsData::EndMarker(Some(SplitReason::StreamStructureChange { description }))) =
            group.first()
        {
            info!(%description, "DASH output epoch changes");
        }
        if split {
            let tracks = std::iter::once(&mut self.leader).chain(self.follower.as_mut());
            for written in tracks.filter_map(|track| track.written.as_mut()) {
                written.in_file = false;
            }
        }
        for span in spans {
            let timescale = span.init.timescale;
            let Some(track) = self.track_mut(span.init.track) else {
                continue;
            };
            let written = track
                .written
                .get_or_insert_with(|| Written::new(Arc::clone(&span.init)));
            for &(start, end) in &span.fragments {
                if let (Some(start), Some(end)) =
                    (to_nanos(start, timescale), to_nanos(end, timescale))
                {
                    written.record(&span.init, start, end);
                }
            }
        }
        group.push(item);
        self.out.push(group);
        self.stats.items += 1;
        self.last_media_at = Instant::now();
        self.media_paused_at = self.media_clock.paused_at(self.last_media_at.into_std());
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
        let pending = self.prepare_output(&media.init, follower);
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
        let Some(output) = output_for(pending.as_ref(), self.output.as_ref()) else {
            return Ok(());
        };
        let Some(init) = &output.init else {
            return Ok(());
        };
        let has_follower = output.follower.is_some();

        let mut data = Vec::with_capacity(
            media.data.len() + parts.iter().map(|part| part.data.len()).sum::<usize>(),
        );
        // Every part counts toward recorded spans, as one rejected midway may
        // be partly written.
        let timescale = media.init.timescale;
        let spans: Vec<TrackSpan> = std::iter::once(TrackSpan {
            init: Arc::clone(&media.init),
            fragments: media.fragments[media.consumed..].to_vec(),
        })
        .chain(parts.iter().map(|part| TrackSpan {
            init: Arc::clone(&part.init),
            fragments: part.fragments.clone(),
        }))
        .collect();
        let media_descriptor = Arc::clone(&media.descriptor);
        let consumed = media.consumed;
        let leader_data = media.data;
        let leader_parsed = media.parsed;

        // Cached unshifted fragments are shifted fragment by fragment: only
        // what is written must land at or after the output origin.
        let leader_written = leader_shift.and_then(|shift| {
            let fragments = if !leader_parsed.is_empty() {
                leader_parsed
            } else {
                init.parse_media_segment(0, &leader_data, 0)?
            };
            write_shifted(
                &mut self.writer,
                fragments.into_iter().skip(consumed),
                shift,
                &mut data,
            )
        });
        if let Err(error) = leader_written {
            // An epoch whose first item is rejected is not announced.
            return self.reject_leader_segment(&error);
        }
        self.leader_rejections = 0;

        let follower_written = match has_follower {
            true => parts
                .into_iter()
                .zip(part_shifts)
                .try_for_each(|(part, shift)| {
                    let shift = shift?;
                    let fragments = if part
                        .parsed
                        .first()
                        .is_some_and(|fragment| fragment.track_index() == 1)
                    {
                        part.parsed
                    } else {
                        init.parse_media_segment(1, &part.data, 0)?
                            .into_iter()
                            .skip(part.first)
                            .take(part.count)
                            .collect()
                    };
                    write_shifted(&mut self.writer, fragments, shift, &mut data)
                }),
            false => Ok(()),
        };
        if let Err(error) = follower_written {
            // The leader's fragments are complete; what follower data was
            // written before the error stays, nothing after it.
            self.drop_follower("media segment rejected", &error);
        }

        self.leader_end = Some((end, timescale));
        let segment = media_segment(&media_descriptor, start, end, timescale);
        self.emit_media(
            pending,
            HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                segment,
                data: Bytes::from(data),
            })),
            &spans,
        );
        Ok(())
    }

    fn write_passthrough(&mut self, media: QueuedMedia) {
        let pending = self.prepare_output(&media.init, None);
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
        // Passthrough media is not parsed, so what the track wrote is unknown:
        // nothing later is judged a repeat of it. Passthrough epochs never
        // continue a file.
        if let Some(track) = self.track_mut(media.init.track) {
            track.written = None;
        }
        self.emit_media(
            pending,
            HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                segment,
                data: media.data,
            })),
            &[],
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
            Some(head) => period.is_before(head),
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
                .is_none_or(|output| run_period.is_after(output));
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
                                if output_period.as_ref().is_some_and(|output| media.init.period.is_after(output))
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
    fn write_audio_only(&mut self, mut media: QueuedMedia) {
        let repeated = media.repeated_prefix(self.written_of(media.init.track));
        media.consumed += repeated;
        self.stats.repeated_fragments += repeated as u64;
        let Some(&(start, _)) = media.fragments.get(media.consumed) else {
            return;
        };
        let end = media.fragments.last().map_or(start, |&(_, end)| end);
        let pending = self.prepare_output(&media.init, None);
        self.choose_origin(std::iter::once((start, media.init.timescale)));
        let shift = self.output_shift(&media.init);
        let Some(init) = output_for(pending.as_ref(), self.output.as_ref())
            .and_then(|output| output.init.as_ref())
        else {
            return;
        };
        let mut data = Vec::with_capacity(media.data.len());
        let consumed = media.consumed;
        let media_init = Arc::clone(&media.init);
        let media_descriptor = Arc::clone(&media.descriptor);
        let media_spans = media.fragments[consumed..].to_vec();
        let media_data = media.data;
        let media_parsed = media.parsed;
        let written = shift.and_then(|shift| {
            let fragments = if media_parsed
                .first()
                .is_some_and(|fragment| fragment.track_index() == 0)
            {
                media_parsed
            } else {
                init.parse_media_segment(0, &media_data, 0)?
            };
            write_shifted(
                &mut self.writer,
                fragments.into_iter().skip(consumed),
                shift,
                &mut data,
            )
        });
        if let Err(error) = written {
            warn!(%error, "audio of a Period without video rejected by the merger; dropped");
            return;
        }
        let timescale = media_init.timescale;
        let segment = media_segment(&media_descriptor, start, end, timescale);
        self.emit_media(
            pending,
            HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                segment,
                data: Bytes::from(data),
            })),
            &[TrackSpan {
                init: media_init,
                fragments: media_spans,
            }],
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
                if run_period.is_before(&output_period) {
                    self.stats.stale_fragments += run
                        .iter()
                        .map(|media| media.remaining() as u64)
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
                let pending = self.prepare_output(&leader, Some(run_init));
                self.write_tail(run, pending);
            } else {
                self.stats.stale_fragments += run
                    .iter()
                    .map(|media| media.remaining() as u64)
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
        let Some(output) = output_for(pending.as_ref(), self.output.as_ref()) else {
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
        // Repeats are judged only until the run writes media: from then on
        // the track has written media of this epoch.
        let mut judge_repeats = true;
        let mut data = Vec::with_capacity(run.iter().map(|media| media.data.len()).sum());
        let mut last = None;
        let mut written_fragments: Vec<(i128, i128)> = Vec::new();
        let mut late_fragments = 0u64;
        for mut media in run {
            let previous = self.written_of(run_init.track).filter(|_| judge_repeats);
            let repeated = media.repeated_prefix(previous);
            media.consumed += repeated;
            self.stats.repeated_fragments += repeated as u64;
            let keep: Vec<bool> = media.fragments.iter().map(|span| !late(span)).collect();
            late_fragments += keep[media.consumed..].iter().filter(|keep| !**keep).count() as u64;
            let kept: Vec<(i128, i128)> = media
                .fragments
                .iter()
                .zip(&keep)
                .skip(media.consumed)
                .filter(|(_, keep)| **keep)
                .map(|(&span, _)| span)
                .collect();
            let consumed = media.consumed;
            let media_descriptor = Arc::clone(&media.descriptor);
            // Cached unshifted fragments, so a dropped fragment's time never matters.
            let written = if media
                .parsed
                .first()
                .is_some_and(|fragment| fragment.track_index() == 1)
            {
                let fragments = media
                    .parsed
                    .into_iter()
                    .zip(keep)
                    .skip(consumed)
                    .filter_map(|(fragment, keep)| keep.then_some(fragment));
                write_shifted(&mut self.writer, fragments, shift, &mut data)
            } else {
                init.parse_media_segment(1, &media.data, 0)
                    .and_then(|fragments| {
                        let fragments = fragments
                            .into_iter()
                            .zip(keep)
                            .skip(consumed)
                            .filter_map(|(fragment, keep)| keep.then_some(fragment));
                        write_shifted(&mut self.writer, fragments, shift, &mut data)
                    })
            };
            if let Err(error) = written {
                warn!(%error, "trailing audio rejected by the merger; dropped");
                break;
            }
            if !kept.is_empty() {
                judge_repeats = false;
            }
            written_fragments.extend(kept);
            last = Some(media_descriptor);
        }
        self.stats.late_fragments += late_fragments;
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
                &[TrackSpan {
                    init: Arc::clone(&run_init),
                    fragments: written_fragments,
                }],
            );
        }
    }
}

/// The output epoch an item is written under: the one `pending` opens, else
/// the current one.
fn output_for<'a>(
    pending: Option<&'a PendingOutput>,
    current: Option<&'a Output>,
) -> Option<&'a Output> {
    pending.map(|pending| &pending.output).or(current)
}

/// Writes `fragments`, parsed unshifted, to `out` with their decode times
/// shifted by `shift`.
fn write_shifted(
    writer: &mut FragmentWriter,
    fragments: impl IntoIterator<Item = SourceFragment>,
    shift: i64,
    out: &mut Vec<u8>,
) -> Result<(), MergeError> {
    fragments.into_iter().try_for_each(|mut fragment| {
        fragment.shift_decode_time(shift)?;
        writer.write(fragment, out)
    })
}

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
    let follower_probe = if !leader && probe.is_some() {
        merge_init(&[data.clone(), data.clone()]).ok()
    } else {
        None
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
        follower_probe,
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

type ProbedFragments = (Vec<(i128, i128)>, Vec<SourceFragment>);

/// Fragment spans of a segment on the presentation timeline and its unshifted
/// `SourceFragment`s, parsed once from the track's own init (or its two-track
/// follower probe so follower fragments carry `track_id == 2`).
fn probe_fragments(init: &TrackInit, data: &Bytes) -> Result<ProbedFragments, MergeError> {
    let Some(probe) = &init.probe else {
        return Ok((Vec::new(), Vec::new()));
    };
    let fragments = match &init.follower_probe {
        Some(follower_probe) => follower_probe.parse_media_segment(1, data, 0)?,
        None => probe.parse_media_segment(0, data, 0)?,
    };
    if fragments.is_empty() {
        return Err(MergeError::MissingBox {
            fourcc: mp4::FourCc(*b"moof"),
            parent: mp4::FourCc(*b"file"),
        });
    }
    let spans = fragments
        .iter()
        .map(|fragment| {
            let start = i128::from(fragment.decode_time()) + i128::from(init.time_shift);
            (start, start + i128::from(fragment.duration()))
        })
        .collect();
    Ok((spans, fragments))
}

/// Whether `init`, an epoch's merged init, repeats `output`'s byte for
/// byte, so that everything written under either decodes under the one
/// already written. Every epoch's fragments are shifted onto the one
/// presentation timeline, so nothing else differs. Passthrough output keeps
/// the source's own decode times and never repeats.
fn repeats_init(output: &Output, init: Option<&MergedInit>) -> bool {
    match (&output.init, init) {
        (Some(current), Some(next)) => current.data() == next.data(),
        _ => false,
    }
}

/// The init item announcing an epoch led by `leader`: its merged init, else
/// the leader's own for passthrough.
fn init_item(leader: &TrackInit, init: Option<&MergedInit>) -> HlsData {
    let data = init.map_or_else(|| leader.data.clone(), |init| init.data().clone());
    HlsData::M4sData(M4sData::InitSegment(M4sInitSegmentData {
        segment: init_media_segment(&leader.descriptor),
        data,
    }))
}

fn epoch_change(output: &Output, leader: &TrackInit, follower: Option<&TrackInit>) -> String {
    let describe = |init: Option<&TrackInit>| match init {
        Some(init) => format!("{}@{}", init.representation, init.period),
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
        .and_then(|time| time.to_string().parse().ok());
    m3u8_rs::MediaSegment {
        uri: redact_url(&descriptor.parsed_url).to_string(),
        duration,
        program_date_time,
        ..Default::default()
    }
}

fn presentation_instant(anchor: Timestamp, ticks: i128, timescale: u32) -> Option<Timestamp> {
    if timescale == 0 {
        return None;
    }
    let nanos = ticks.checked_mul(NANOS_PER_SEC)? / i128::from(timescale);
    let delta = SignedDuration::from_nanos(i64::try_from(nanos).ok()?);
    anchor.checked_add(delta).ok()
}

#[cfg(test)]
mod tests;
