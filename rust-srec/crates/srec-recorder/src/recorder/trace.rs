//! Recent inputs of each streamer's state machine, for replaying incidents.
//!
//! [`StreamerFsm::step`](super::StreamerFsm::step) is pure: from the same
//! initial snapshot, the same `(StepContext, FsmInput)` pairs reach the same
//! decisions. Each actor therefore keeps its latest inputs, so that a problem
//! reported from the field can be rebuilt as a unit test. The trace is logged
//! when one of the checks below fails or the actor panics, and
//! `GET /api/streamers/{id}/trace` returns it while the actor runs.
//!
//! Inputs are kept as their `Debug` text, whose impls leave out signed stream
//! URLs, request headers and credential material. Streamer and avatar URLs in
//! the configuration are cut to their origin here. Error messages appear as
//! the logs show them.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use jiff::Timestamp;
use serde::Serialize;

use super::fsm::{FsmInput, StepContext};
use super::ports::{InitialStreamerSnapshot, ResolvedActorConfig};
use srec_core::utils::url::redact_url_for_log;

/// Inputs kept per actor. An idle streamer adds two per check (the timer and
/// the probe result), so this covers its last 32 checks.
const CAPACITY: usize = 64;
/// Longest `Debug` text kept for one input, in bytes.
const MAX_INPUT_LEN: usize = 1_024;
/// Probes an actor may start within [`PROBE_WINDOW`]. Polling starts a few a
/// minute at most, so more means a loop that re-probes without waiting.
const PROBE_BURST: usize = 20;
const PROBE_WINDOW: Duration = Duration::from_secs(60);
/// Database writes that may wait behind the one in flight.
const MAX_QUEUED_WRITES: usize = 8;

/// A check on each step's result that only a state machine bug fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceCheck {
    /// A timer step left a deadline that is already due, so the actor would
    /// fire it again at once.
    TimerProgress,
    /// More than 20 probes started within a minute.
    ProbeBurst,
    /// More than 8 database writes wait in the queue.
    QueuedWrites,
}

/// What a step left behind, as the checks need it.
pub struct StepResult {
    /// The step handled `TimerFired`.
    pub after_timer: bool,
    /// The state machine's next deadline after the step.
    pub next_deadline: Option<Instant>,
    /// Probes the step started.
    pub probes_started: usize,
    /// Database writes queued behind the one in flight.
    pub queued_writes: usize,
}

/// One state machine's recent inputs and the state of its checks.
pub struct InputTrace {
    generation: u64,
    started_at: Instant,
    started_wall: Timestamp,
    initial: String,
    entries: VecDeque<Entry>,
    next_seq: u64,
    probe_starts: VecDeque<Instant>,
    /// Checks that have failed; each is reported once per actor.
    failed: Vec<TraceCheck>,
}

struct Entry {
    seq: u64,
    offset: Duration,
    at: Timestamp,
    jitter_bps: u16,
    input: String,
    heartbeat: bool,
    repeats: u32,
}

/// A copy of an [`InputTrace`], oldest input first.
#[derive(Debug, Clone, Serialize)]
pub struct InputTraceDump {
    /// The actor generation that recorded the trace.
    pub generation: u64,
    /// When the actor started.
    pub started_at: Timestamp,
    /// The snapshot the state machine started from.
    pub initial: String,
    /// The latest inputs, oldest first.
    pub entries: Vec<InputTraceEntry>,
}

/// One input as the state machine received it.
#[derive(Debug, Clone, Serialize)]
pub struct InputTraceEntry {
    /// Position among all inputs of this actor, counting from 0.
    pub seq: u64,
    /// Time since the actor started, on the monotonic clock, in milliseconds.
    pub offset_ms: u64,
    /// Wall-clock time of the step.
    pub at: Timestamp,
    /// The step's jitter factor in basis points.
    pub jitter_bps: u16,
    /// The input's `Debug` text.
    pub input: String,
    /// Further download heartbeats folded into this entry; it shows the last.
    pub repeats: u32,
}

impl InputTrace {
    pub fn new(generation: u64, ctx: StepContext, initial: &InitialStreamerSnapshot) -> Self {
        let initial = InitialStreamerSnapshot {
            config: redacted_config(&initial.config),
            ..initial.clone()
        };
        Self {
            generation,
            started_at: ctx.now,
            started_wall: ctx.wall_now,
            initial: bounded(format!("{initial:?}")),
            entries: VecDeque::with_capacity(CAPACITY),
            next_seq: 0,
            probe_starts: VecDeque::new(),
            failed: Vec::new(),
        }
    }

    /// A trace with no inputs, for a handle registered without an actor.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn empty(generation: u64) -> Self {
        Self {
            generation,
            started_at: Instant::now(),
            started_wall: Timestamp::now(),
            initial: String::new(),
            entries: VecDeque::new(),
            next_seq: 0,
            probe_starts: VecDeque::new(),
            failed: Vec::new(),
        }
    }

    /// Record a stepped input, described by [`describe`], and run the checks
    /// on the step's result. Returns the checks that failed for the first
    /// time.
    pub fn observe(
        &mut self,
        ctx: StepContext,
        input: (String, bool),
        result: &StepResult,
    ) -> Vec<TraceCheck> {
        self.record(ctx, input);

        self.probe_starts
            .extend(std::iter::repeat_n(ctx.now, result.probes_started));
        while self
            .probe_starts
            .front()
            .is_some_and(|start| ctx.now.saturating_duration_since(*start) >= PROBE_WINDOW)
        {
            self.probe_starts.pop_front();
        }

        let failing = [
            (
                TraceCheck::TimerProgress,
                result.after_timer && result.next_deadline.is_some_and(|due| due <= ctx.now),
            ),
            (
                TraceCheck::ProbeBurst,
                self.probe_starts.len() > PROBE_BURST,
            ),
            (
                TraceCheck::QueuedWrites,
                result.queued_writes > MAX_QUEUED_WRITES,
            ),
        ];
        let mut newly = Vec::new();
        for (check, fails) in failing {
            if fails && !self.failed.contains(&check) {
                self.failed.push(check);
                newly.push(check);
            }
        }
        newly
    }

    fn record(&mut self, ctx: StepContext, (input, heartbeat): (String, bool)) {
        let offset = ctx.now.saturating_duration_since(self.started_at);
        if heartbeat
            && let Some(last) = self.entries.back_mut()
            && last.heartbeat
        {
            last.offset = offset;
            last.at = ctx.wall_now;
            last.jitter_bps = ctx.jitter_bps;
            last.input = input;
            last.repeats = last.repeats.saturating_add(1);
            return;
        }
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            seq: self.next_seq,
            offset,
            at: ctx.wall_now,
            jitter_bps: ctx.jitter_bps,
            input,
            heartbeat,
            repeats: 0,
        });
        self.next_seq += 1;
    }

    pub fn dump(&self) -> InputTraceDump {
        InputTraceDump {
            generation: self.generation,
            started_at: self.started_wall,
            initial: self.initial.clone(),
            entries: self
                .entries
                .iter()
                .map(|entry| InputTraceEntry {
                    seq: entry.seq,
                    offset_ms: u64::try_from(entry.offset.as_millis()).unwrap_or(u64::MAX),
                    at: entry.at,
                    jitter_bps: entry.jitter_bps,
                    input: entry.input.clone(),
                    repeats: entry.repeats,
                })
                .collect(),
        }
    }
}

impl fmt::Display for InputTraceDump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "generation {} started {}\ninitial: {}",
            self.generation, self.started_at, self.initial
        )?;
        for entry in &self.entries {
            write!(
                f,
                "\n#{} +{}.{:03}s jitter {} {}",
                entry.seq,
                entry.offset_ms / 1_000,
                entry.offset_ms % 1_000,
                entry.jitter_bps,
                entry.input
            )?;
            if entry.repeats > 0 {
                write!(f, " (after {} more like it)", entry.repeats)?;
            }
        }
        Ok(())
    }
}

/// Describe `input` for [`InputTrace::observe`]: its `Debug` text, and
/// whether it is a download heartbeat, which folds into the previous one.
pub fn describe(input: &FsmInput) -> (String, bool) {
    let text = match input {
        FsmInput::ConfigUpdated { revision, config } => format!(
            "ConfigUpdated {{ revision: {revision}, config: {:?} }}",
            redacted_config(config)
        ),
        input => format!("{input:?}"),
    };
    (
        bounded(text),
        matches!(input, FsmInput::DownloadHeartbeat { .. }),
    )
}

fn redacted_config(config: &ResolvedActorConfig) -> ResolvedActorConfig {
    ResolvedActorConfig {
        streamer_url: redact_url_for_log(&config.streamer_url),
        avatar_url: config.avatar_url.as_deref().map(redact_url_for_log),
        ..config.clone()
    }
}

fn bounded(mut text: String) -> String {
    if text.len() > MAX_INPUT_LEN {
        let mut end = MAX_INPUT_LEN;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use srec_core::domain::StreamerState;

    struct Clock {
        now: Instant,
    }

    impl Clock {
        fn ctx(&self) -> StepContext {
            StepContext::deterministic(self.now, Timestamp::UNIX_EPOCH)
        }

        fn advance(&mut self, by: Duration) {
            self.now += by;
        }
    }

    fn step(after_timer: bool, next_deadline: Option<Instant>, probes: usize) -> StepResult {
        StepResult {
            after_timer,
            next_deadline,
            probes_started: probes,
            queued_writes: 0,
        }
    }

    fn trace(clock: &Clock) -> InputTrace {
        let snapshot = InitialStreamerSnapshot {
            streamer_id: "streamer".into(),
            state: StreamerState::NotLive,
            deleted: false,
            consecutive_error_count: 0,
            disabled_until: None,
            last_error: None,
            config: ResolvedActorConfig {
                streamer_url: "https://example.com/room/1?password=hunter2".into(),
                ..ResolvedActorConfig::default()
            },
            config_revision: 1,
        };
        InputTrace::new(1, clock.ctx(), &snapshot)
    }

    #[test]
    fn heartbeats_fold_and_the_oldest_inputs_make_room() {
        let mut clock = Clock {
            now: Instant::now(),
        };
        let mut trace = trace(&clock);
        let heartbeat = FsmInput::DownloadHeartbeat {
            session_id: "session".into(),
            download_id: "download".into(),
            progress: None,
        };
        for _ in 0..3 {
            clock.advance(Duration::from_secs(1));
            trace.observe(clock.ctx(), describe(&heartbeat), &step(false, None, 0));
        }
        let dump = trace.dump();
        assert_eq!(dump.entries.len(), 1);
        assert_eq!(dump.entries[0].repeats, 2);
        assert_eq!(dump.entries[0].offset_ms, 3_000);

        for _ in 0..CAPACITY {
            trace.observe(
                clock.ctx(),
                describe(&FsmInput::CheckNow),
                &step(false, None, 0),
            );
        }
        let dump = trace.dump();
        assert_eq!(dump.entries.len(), CAPACITY);
        assert_eq!(dump.entries[0].seq, 1);
        assert!(!dump.initial.contains("hunter2"), "{}", dump.initial);
    }

    #[test]
    fn each_check_reports_its_first_failure_once() {
        let mut clock = Clock {
            now: Instant::now(),
        };
        let mut trace = trace(&clock);

        let due = Some(clock.now);
        let timer = describe(&FsmInput::TimerFired);
        assert!(
            trace
                .observe(clock.ctx(), timer.clone(), &step(false, due, 0))
                .is_empty()
        );
        assert_eq!(
            trace.observe(clock.ctx(), timer.clone(), &step(true, due, 0)),
            [TraceCheck::TimerProgress]
        );
        assert!(
            trace
                .observe(clock.ctx(), timer, &step(true, due, 0))
                .is_empty()
        );

        let check = describe(&FsmInput::CheckNow);
        for _ in 0..PROBE_BURST {
            clock.advance(Duration::from_secs(2));
            assert!(
                trace
                    .observe(clock.ctx(), check.clone(), &step(false, None, 1))
                    .is_empty()
            );
        }
        // The first probes have aged out of the window by now.
        clock.advance(Duration::from_secs(30));
        assert!(
            trace
                .observe(clock.ctx(), check.clone(), &step(false, None, 1))
                .is_empty()
        );
        assert_eq!(
            trace.observe(clock.ctx(), check, &step(false, None, 12)),
            [TraceCheck::ProbeBurst]
        );
    }
}
