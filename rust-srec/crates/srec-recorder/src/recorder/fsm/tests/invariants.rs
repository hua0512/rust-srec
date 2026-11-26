//! Randomized invariant tests for [`StreamerFsm`].
//!
//! Each scenario drives one state machine with plausible random inputs: a
//! small database model answers commits, a download model answers attempts,
//! and the clock advances to the next deadline or by a random step. After
//! every step the harness checks the invariants below and records the first
//! violating seed with the last steps that led to it. Seeds are fixed, so a
//! reported seed replays the identical sequence.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use jiff::SignedDuration;

use super::{TestClock, base_config, base_snapshot, live_payload};
use crate::downloader::engine::{DownloadProtocol, EngineType};
use crate::downloader::{
    DownloadFailureKind, DownloadStopCause, DownloadTerminalEvent, EngineEndSignal,
};
use crate::monitor::FilterReason;
use crate::recorder::fsm::schedule::jittered_delay;
use crate::recorder::fsm::state::{FsmEffect, FsmInput, MonitorPhase, SessionSlot, StreamerFsm};
use crate::recorder::ports::{
    AttemptStartFailure, DanmuStopResult, DbCommitFailure, DbCommitOp, DbCommitOutcome,
    INTERACTIVE_RETIREMENT, PipelineNotification, ProbeFailure, ProbeStatus,
    SessionStartDisposition,
};
use srec_core::domain::StreamerState;
use srec_core::domain::streamer::FatalErrorType;

/// Scenarios per test run; enough to reach every violation the harness has
/// found so far within a few seconds of a debug build.
const RUNS: u64 = 1_000;
/// Steps per scenario before it ends without a stop.
const MAX_STEPS: usize = 6_000;
/// Steps a stop may take to converge before it counts as stuck.
const STOP_CONVERGENCE_STEPS: usize = 3_000;
/// Steps of history kept for a violation report.
const TRACE_LEN: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Invariant {
    /// At most one probe and one commit are in flight, and a step dispatches
    /// at most one commit.
    SingleFlight,
    /// Attempts start only for the session open in the database, and every
    /// session the database ends gets exactly one `SessionEnded`
    /// notification, never while it is still open.
    SessionOwnership,
    /// A stop reaches `ActorStopped` or its follow-up phase.
    StopConverges,
    /// After `TimerFired` at `t`, the next deadline is absent or after `t`.
    TimerProgress,
    /// Queued database writes stay bounded.
    BoundedQueue,
    /// `ActorStopped` is emitted only once database work has settled.
    StopAfterCommits,
    /// A new attempt never starts while an earlier attempt is unfinished.
    SingleAttempt,
    /// A probe that follows an attempt-start failure, a failed session-start
    /// commit, or a failed or filtered probe result, with no external trigger
    /// in between, waits at least the shortest configured poll interval.
    ProbeRateFloor,
}

struct Violation {
    count: usize,
    first_seed: u64,
    detail: String,
    trace: String,
}

/// Run [`RUNS`] seeded scenarios and return the violations of `checked`.
fn violations_of(checked: &[Invariant]) -> Vec<(Invariant, Violation)> {
    let mut found: HashMap<Invariant, Violation> = HashMap::new();
    for seed in 1..=RUNS {
        let mut harness = Harness::new(seed);
        harness.run();
        for (invariant, detail, trace) in harness.violations {
            if !checked.contains(&invariant) {
                continue;
            }
            found
                .entry(invariant)
                .and_modify(|v| v.count += 1)
                .or_insert(Violation {
                    count: 1,
                    first_seed: seed,
                    detail,
                    trace,
                });
        }
    }
    let mut found: Vec<_> = found.into_iter().collect();
    found.sort_by_key(|(_, v)| std::cmp::Reverse(v.count));
    found
}

fn assert_holds(checked: &[Invariant]) {
    let found = violations_of(checked);
    let report: Vec<String> = found
        .iter()
        .map(|(invariant, v)| {
            format!(
                "{invariant:?}: {} violation(s), first seed {}: {}\n    {}",
                v.count, v.first_seed, v.detail, v.trace
            )
        })
        .collect();
    assert!(found.is_empty(), "{}", report.join("\n\n"));
}

#[test]
fn random_inputs_keep_single_flight_session_ownership_and_stop_convergence() {
    assert_holds(&[
        Invariant::SingleFlight,
        Invariant::SessionOwnership,
        Invariant::StopConverges,
    ]);
}

#[test]
#[ignore = "bug: the quiet-period backstop stays due while a commit is pending or the monitor \
            is stopping, so TimerFired spins and queues duplicate EndSession writes"]
fn known_bug_timer_fired_always_moves_the_next_deadline_forward() {
    assert_holds(&[Invariant::TimerProgress, Invariant::BoundedQueue]);
}

#[test]
#[ignore = "bug: Shutdown and non-quiescent Retire emit ActorStopped with a commit in flight, \
            losing the SessionEnded notification of a committed EndSession"]
fn known_bug_actor_stops_only_after_database_work_settles() {
    assert_holds(&[Invariant::StopAfterCommits]);
}

#[test]
#[ignore = "bug: a startup refused by the duplicate-download guard reports Cancelled, and a \
            later attempt starts while the earlier download is still running"]
fn known_bug_attempts_never_overlap() {
    assert_holds(&[Invariant::SingleAttempt]);
}

#[test]
#[ignore = "bug: attempt-start failures, failed session-start commits and metadata filter \
            results on an open session re-probe the platform immediately"]
fn known_bug_failures_and_filter_results_never_reprobe_immediately() {
    assert_holds(&[Invariant::ProbeRateFloor]);
}

/// Small deterministic xorshift generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn pct(&mut self, p: u64) -> bool {
        self.below(100) < p
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

struct Attempt {
    session_id: String,
    download_id: Option<String>,
    stop_requested: Option<DownloadStopCause>,
    next_segment: u32,
    open_segment: Option<u32>,
}

#[derive(Clone, Copy)]
enum Stop {
    Disable,
    Shutdown,
    Retire,
}

struct Harness {
    rng: Rng,
    clock: TestClock,
    started_at: Instant,
    fsm: StreamerFsm,
    record_danmu: bool,
    revision: u64,
    /// Shortest poll interval of the current configuration, in milliseconds.
    min_poll_interval_ms: u64,

    probe: Option<u64>,
    commit: Option<(u64, DbCommitOp)>,
    attempts: BTreeMap<u64, Attempt>,
    danmu_running: HashSet<String>,
    danmu_stops: VecDeque<String>,

    db_open: Option<String>,
    db_active: bool,
    db_errors: i32,
    next_session: u64,
    db_closed: Vec<String>,
    ended_notified: HashSet<String>,

    /// Instant of the last failure or filter result not yet followed by a
    /// probe or an external trigger.
    reprobe_floor_from: Option<Instant>,
    stop: Option<(Stop, usize)>,
    stopped: bool,
    step_no: usize,
    trace: VecDeque<String>,
    violations: Vec<(Invariant, String, String)>,
}

impl Harness {
    fn new(seed: u64) -> Self {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let clock = TestClock::new();
        let record_danmu = rng.pct(50);
        let mut config = base_config();
        config.record_danmu = record_danmu;
        config.offline_check_count = 1 + rng.below(3) as u32;
        config.offline_check_interval_ms = rng.pick(&[5_000, 15_000, 20_000]);
        config.check_interval_ms = rng.pick(&[30_000, 60_000]);
        let min_poll_interval_ms = config
            .offline_check_interval_ms
            .min(config.check_interval_ms);
        let state = rng.pick(&[StreamerState::NotLive, StreamerState::Live]);
        let db_open = (state == StreamerState::Live).then(|| "sess-0".to_owned());
        let fsm = StreamerFsm::new(base_snapshot(state, config), clock.now, clock.wall);
        Self {
            rng,
            started_at: clock.now,
            clock,
            fsm,
            record_danmu,
            revision: 1,
            min_poll_interval_ms,
            probe: None,
            commit: None,
            attempts: BTreeMap::new(),
            danmu_running: HashSet::new(),
            danmu_stops: VecDeque::new(),
            db_open,
            db_active: true,
            db_errors: 0,
            next_session: 1,
            db_closed: Vec::new(),
            ended_notified: HashSet::new(),
            reprobe_floor_from: None,
            stop: None,
            stopped: false,
            step_no: 0,
            trace: VecDeque::new(),
            violations: Vec::new(),
        }
    }

    fn violate(&mut self, invariant: Invariant, detail: String) {
        let trace: Vec<_> = self.trace.iter().cloned().collect();
        self.violations
            .push((invariant, detail, trace.join("\n    ")));
    }

    fn step(&mut self, input: FsmInput) {
        self.step_no += 1;
        let label: String = format!("{input:?}").chars().take(160).collect();
        self.trace.push_back(format!(
            "#{} t+{:?} {label} :: {}",
            self.step_no,
            self.clock.now.duration_since(self.started_at),
            phase_name(&self.fsm)
        ));
        if self.trace.len() > TRACE_LEN {
            self.trace.pop_front();
        }

        if matches!(
            input,
            FsmInput::CheckNow
                | FsmInput::ErrorCleared
                | FsmInput::RetryAdmission
                | FsmInput::BatchLiveHint
                | FsmInput::CredentialMaterialChanged
                | FsmInput::ConfigUpdated { .. }
        ) {
            self.reprobe_floor_from = None;
        }
        let arms_floor = match &input {
            FsmInput::AttemptStartFailed { reason, .. } => {
                !matches!(reason, AttemptStartFailure::Cancelled)
            }
            FsmInput::ProbeCompleted { result, .. } => {
                matches!(result, Err(_) | Ok(ProbeStatus::Filtered { .. }))
            }
            FsmInput::DbCommitted { result, op_id } => {
                result.is_err()
                    && matches!(
                        &self.fsm.session,
                        SessionSlot::CommittingStart { op_id: start, .. } if start == op_id
                    )
            }
            _ => false,
        };
        let is_timer = matches!(input, FsmInput::TimerFired);

        let effects = self.fsm.step(self.clock.ctx(), input);
        if arms_floor {
            self.reprobe_floor_from = Some(self.clock.now);
        }

        let mut commits = 0;
        for effect in effects {
            match effect {
                FsmEffect::SpawnProbe(request) => {
                    if self.probe.is_some() {
                        self.violate(
                            Invariant::SingleFlight,
                            "second probe spawned while one is in flight".into(),
                        );
                    }
                    if let Some(from) = self.reprobe_floor_from.take() {
                        let waited = self.clock.now.duration_since(from);
                        let floor = jittered_delay(self.min_poll_interval_ms, 9_000);
                        if waited < floor {
                            self.violate(
                                Invariant::ProbeRateFloor,
                                format!("re-probed after {waited:?}, floor {floor:?}"),
                            );
                        }
                    }
                    self.probe = Some(request.probe_id);
                }
                FsmEffect::CancelProbe { probe_id } => {
                    if self.probe == Some(probe_id) {
                        self.probe = None;
                    }
                }
                FsmEffect::CommitDb { op_id, op } => {
                    commits += 1;
                    if self.commit.is_some() {
                        self.violate(
                            Invariant::SingleFlight,
                            "CommitDb dispatched while another commit is in flight".into(),
                        );
                    }
                    self.commit = Some((op_id, op));
                }
                FsmEffect::StartAttempt(spec) => {
                    if !self.attempts.is_empty() {
                        self.violate(
                            Invariant::SingleAttempt,
                            format!(
                                "StartAttempt while {} attempt(s) are unfinished",
                                self.attempts.len()
                            ),
                        );
                    }
                    if self.db_open.as_deref() != Some(spec.session_id.as_str()) {
                        self.violate(
                            Invariant::SessionOwnership,
                            "StartAttempt for a session the database does not have open".into(),
                        );
                    }
                    self.attempts.insert(
                        spec.attempt_id,
                        Attempt {
                            session_id: spec.session_id.clone(),
                            download_id: None,
                            stop_requested: None,
                            next_segment: 0,
                            open_segment: None,
                        },
                    );
                }
                FsmEffect::StopAttempt {
                    attempt_id, cause, ..
                } => {
                    if let Some(attempt) = self.attempts.get_mut(&attempt_id) {
                        attempt.stop_requested = Some(cause);
                    }
                }
                FsmEffect::StopDanmu { session_id, .. } => {
                    if self.danmu_running.contains(&session_id) {
                        self.danmu_stops.push_back(session_id);
                    }
                }
                FsmEffect::NotifyPipeline(PipelineNotification::SessionEnded {
                    session_id,
                    ..
                }) => {
                    if self.db_open.as_deref() == Some(session_id.as_str()) {
                        self.violate(
                            Invariant::SessionOwnership,
                            "SessionEnded while the database session is still open".into(),
                        );
                    }
                    if !self.ended_notified.insert(session_id) {
                        self.violate(
                            Invariant::SessionOwnership,
                            "duplicate SessionEnded notification".into(),
                        );
                    }
                }
                FsmEffect::ActorStopped { reason } => {
                    self.stopped = true;
                    if self.commit.is_some()
                        || self.fsm.pending_db.is_some()
                        || !self.fsm.queued_db.is_empty()
                    {
                        let pending = self.fsm.pending_db.as_ref().map(|p| kind_name(&p.kind));
                        let queued: Vec<_> = self
                            .fsm
                            .queued_db
                            .iter()
                            .map(|q| kind_name(&q.kind))
                            .collect();
                        self.violate(
                            Invariant::StopAfterCommits,
                            format!(
                                "ActorStopped({reason:?}) with pending={pending:?} queued={queued:?}"
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
        if commits > 1 {
            self.violate(
                Invariant::SingleFlight,
                format!("{commits} CommitDb effects in one step"),
            );
        }
        if is_timer
            && let Some(deadline) = self.fsm.next_deadline(self.clock.now, self.clock.wall)
            && deadline <= self.clock.now
        {
            let detail = format!(
                "deadline still due after TimerFired: {}",
                phase_name(&self.fsm)
            );
            self.violate(Invariant::TimerProgress, detail);
        }
        if self.fsm.queued_db.len() > 8 {
            let detail = format!("queued_db holds {} writes", self.fsm.queued_db.len());
            self.violate(Invariant::BoundedQueue, detail);
        }
    }

    /// Fire the next deadline, or let up to `max_ms` pass when none is due sooner.
    fn advance(&mut self, max_ms: u64) {
        let target = self.clock.now + Duration::from_millis(self.rng.below(max_ms + 1));
        match self.fsm.next_deadline(self.clock.now, self.clock.wall) {
            Some(deadline) if deadline <= target => {
                if deadline > self.clock.now {
                    self.clock.advance(deadline - self.clock.now);
                }
                self.step(FsmInput::TimerFired);
            }
            _ => {
                let delta = target - self.clock.now;
                self.clock.advance(delta);
            }
        }
    }

    fn fire_timer(&mut self) -> bool {
        let Some(deadline) = self.fsm.next_deadline(self.clock.now, self.clock.wall) else {
            return false;
        };
        if deadline > self.clock.now {
            let gap = deadline - self.clock.now;
            self.clock.advance(gap.min(Duration::from_secs(4 * 3600)));
        }
        self.step(FsmInput::TimerFired);
        true
    }

    fn complete_probe(&mut self) {
        let Some(probe_id) = self.probe.take() else {
            return;
        };
        let result = match self.rng.below(100) {
            0..=39 => Ok(ProbeStatus::Live(live_payload("Live"))),
            40..=69 => Ok(ProbeStatus::Offline {
                batch_live_hint_armed: self.rng.pct(20),
            }),
            70..=84 => Err(ProbeFailure::Transient {
                message: "502".into(),
            }),
            85..=89 => Ok(ProbeStatus::Filtered {
                reason: if self.rng.pct(50) {
                    FilterReason::TitleMismatch
                } else {
                    FilterReason::OutOfSchedule {
                        next_available: Some(self.clock.wall + SignedDuration::from_mins(30)),
                    }
                },
                title: "t".into(),
                category: None,
            }),
            90..=91 => Ok(ProbeStatus::Fatal {
                kind: FatalErrorType::NotFound,
                message: "gone".into(),
            }),
            92..=94 => Err(ProbeFailure::Permanent {
                message: "bad".into(),
            }),
            _ => Err(ProbeFailure::CredentialSourceChanged),
        };
        self.step(FsmInput::ProbeCompleted { probe_id, result });
    }

    fn complete_commit(&mut self) {
        let Some((op_id, op)) = self.commit.take() else {
            return;
        };
        let result = if self.rng.pct(8) {
            Err(DbCommitFailure::Transient {
                message: "busy".into(),
            })
        } else {
            match self.apply_commit(op) {
                Some(outcome) => Ok(outcome),
                None => Err(DbCommitFailure::CredentialBindingConflict {
                    message: "conflict".into(),
                }),
            }
        };
        self.step(FsmInput::DbCommitted { op_id, result });
    }

    /// Apply `op` to the database model; `None` reports a binding conflict.
    fn apply_commit(&mut self, op: DbCommitOp) -> Option<DbCommitOutcome> {
        let wall = self.clock.wall;
        Some(match op {
            DbCommitOp::StartOrResumeSession { .. } => {
                if !self.db_active {
                    DbCommitOutcome::SessionStartSuppressed {
                        state: StreamerState::Disabled,
                        retry_after: None,
                    }
                } else if let Some(open) = &self.db_open {
                    DbCommitOutcome::SessionStarted {
                        session_id: open.clone(),
                        started_at: wall,
                        disposition: SessionStartDisposition::ReusedActive,
                        credential_binding: None,
                    }
                } else {
                    let session_id = format!("sess-{}", self.next_session);
                    self.next_session += 1;
                    self.db_open = Some(session_id.clone());
                    DbCommitOutcome::SessionStarted {
                        session_id,
                        started_at: wall,
                        disposition: SessionStartDisposition::Created,
                        credential_binding: None,
                    }
                }
            }
            DbCommitOp::ResumeHysteresis { session_id, .. } => {
                if self.db_active && self.db_open.as_deref() == Some(session_id.as_str()) {
                    DbCommitOutcome::HysteresisResumed {
                        session_id,
                        started_at: wall,
                    }
                } else {
                    DbCommitOutcome::HysteresisResumeSuppressed {
                        state: if self.db_active {
                            StreamerState::NotLive
                        } else {
                            StreamerState::Disabled
                        },
                        retry_after: None,
                    }
                }
            }
            DbCommitOp::EndSession { session_id, .. } => {
                let ends_open =
                    self.db_open.is_some() && (session_id.is_none() || session_id == self.db_open);
                if ends_open {
                    let ended = self.db_open.take();
                    self.db_closed.extend(ended.clone());
                    DbCommitOutcome::SessionEnded {
                        session_id: ended,
                        started_at: Some(wall),
                        ended_at: wall,
                        transitioned_from_open: true,
                    }
                } else {
                    DbCommitOutcome::SessionEnded {
                        session_id: None,
                        started_at: None,
                        ended_at: wall,
                        transitioned_from_open: false,
                    }
                }
            }
            DbCommitOp::EndForCredentialRetirement { session_id, .. } => {
                let ended = self.db_open.as_deref() == Some(session_id.as_str());
                if ended {
                    self.db_open = None;
                    self.db_closed.push(session_id.clone());
                }
                DbCommitOutcome::CredentialRetirementEnded { session_id, ended }
            }
            DbCommitOp::RewriteEndedCause { session_id, .. } => {
                DbCommitOutcome::EndedCauseRewritten {
                    session_id,
                    updated: true,
                }
            }
            DbCommitOp::CommitCredentialBinding { .. } => return None,
            DbCommitOp::RecordFilteredState { .. } => DbCommitOutcome::FilteredStateRecorded,
            DbCommitOp::RecordFatalError { new_state, .. } => {
                let ended = self.db_open.take();
                self.db_closed.extend(ended.clone());
                self.db_active = false;
                DbCommitOutcome::FatalErrorRecorded {
                    session_ended: ended,
                    new_state,
                }
            }
            DbCommitOp::RecordTransientError {
                offline_check_count,
                ..
            } => {
                self.db_errors += 1;
                let threshold = offline_check_count.max(2) as i32;
                let disabled_until = (self.db_errors >= threshold).then(|| {
                    let exponent = (self.db_errors - threshold).min(6) as u32;
                    wall + SignedDuration::from_secs((60 * 2_i64.pow(exponent)).min(3600))
                });
                DbCommitOutcome::TransientErrorRecorded {
                    consecutive_error_count: self.db_errors,
                    disabled_until,
                }
            }
            DbCommitOp::ClearErrors { .. } => {
                self.db_errors = 0;
                DbCommitOutcome::ErrorsCleared
            }
            DbCommitOp::ClearLastError { .. } => DbCommitOutcome::LastErrorCleared,
            DbCommitOp::AddSessionTitle { .. } => {
                DbCommitOutcome::SessionTitleAdded { changed: true }
            }
            DbCommitOp::RecordHysteresisEntered { .. } => {
                DbCommitOutcome::HysteresisEnteredRecorded
            }
        })
    }

    fn drive_attempt(&mut self) {
        let index = self.rng.below(self.attempts.len() as u64) as usize;
        let Some((&attempt_id, attempt)) = self.attempts.iter().nth(index) else {
            return;
        };
        let session_id = attempt.session_id.clone();
        let roll = self.rng.below(100);
        let Some(download_id) = attempt.download_id.clone() else {
            self.drive_startup(attempt_id, session_id, roll);
            return;
        };
        let stop = attempt.stop_requested.clone();
        let open_segment = attempt.open_segment;
        let next_segment = attempt.next_segment;

        if (stop.is_some() && roll < 50) || roll >= 85 {
            if let Some(index) = open_segment {
                let input =
                    segment_completed(attempt_id, &session_id, &download_id, index, &self.clock);
                self.step(input);
            }
            self.attempts.remove(&attempt_id);
            let event = self.terminal_event(download_id, session_id, stop, next_segment);
            self.step(FsmInput::DownloadEnded { attempt_id, event });
            return;
        }
        match (open_segment, roll) {
            (None, 0..=49) => {
                if let Some(attempt) = self.attempts.get_mut(&attempt_id) {
                    attempt.open_segment = Some(next_segment);
                    attempt.next_segment += 1;
                }
                self.step(FsmInput::SegmentStarted {
                    attempt_id,
                    session_id,
                    download_id,
                    segment_index: next_segment,
                    segment_path: format!("/rec/{attempt_id}_{next_segment}.flv"),
                    started_at: self.clock.wall,
                });
            }
            (Some(index), 0..=49) => {
                if let Some(attempt) = self.attempts.get_mut(&attempt_id) {
                    attempt.open_segment = None;
                }
                let input =
                    segment_completed(attempt_id, &session_id, &download_id, index, &self.clock);
                self.step(input);
            }
            _ => self.step(FsmInput::DownloadHeartbeat {
                session_id,
                download_id,
                progress: None,
            }),
        }
    }

    fn drive_startup(&mut self, attempt_id: u64, session_id: String, roll: u64) {
        let stop_requested = self
            .attempts
            .get(&attempt_id)
            .is_some_and(|attempt| attempt.stop_requested.is_some());
        if stop_requested && roll < 60 {
            self.attempts.remove(&attempt_id);
            self.step(FsmInput::AttemptStartFailed {
                attempt_id,
                session_id,
                reason: AttemptStartFailure::Cancelled,
            });
            return;
        }
        match roll {
            0..=9 => self.step(FsmInput::AttemptQueued {
                attempt_id,
                session_id,
            }),
            10..=69 => {
                let download_id = format!("dl-{attempt_id}");
                if let Some(attempt) = self.attempts.get_mut(&attempt_id) {
                    attempt.download_id = Some(download_id.clone());
                }
                self.step(FsmInput::AttemptStarted {
                    attempt_id,
                    session_id: session_id.clone(),
                    download_id,
                    engine_type: EngineType::Mesio,
                    protocol: DownloadProtocol::Flv,
                });
                if self.record_danmu && self.danmu_running.insert(session_id.clone()) {
                    self.step(FsmInput::DanmuCollectionStarted { session_id });
                }
            }
            _ => {
                self.attempts.remove(&attempt_id);
                let reason = match self.rng.below(5) {
                    0 => AttemptStartFailure::NotLiveAfterQueue,
                    1 => AttemptStartFailure::Other {
                        message: "engine unavailable".into(),
                    },
                    2 => AttemptStartFailure::RecorderFeedbackBusy,
                    3 => AttemptStartFailure::StreamerBackoff {
                        reason: "backoff".into(),
                        retry_after_secs: 30,
                    },
                    _ => AttemptStartFailure::Cancelled,
                };
                self.step(FsmInput::AttemptStartFailed {
                    attempt_id,
                    session_id,
                    reason,
                });
            }
        }
    }

    fn terminal_event(
        &mut self,
        download_id: String,
        session_id: String,
        stop: Option<DownloadStopCause>,
        total_segments: u32,
    ) -> DownloadTerminalEvent {
        let streamer_id = "streamer-1".to_owned();
        let streamer_name = "Streamer".to_owned();
        let completed = |engine_signal| DownloadTerminalEvent::Completed {
            download_id: download_id.clone(),
            streamer_id: streamer_id.clone(),
            streamer_name: streamer_name.clone(),
            session_id: session_id.clone(),
            total_bytes: 1,
            total_duration_secs: 1.0,
            total_segments,
            file_path: None,
            engine_signal,
            stop_cause: None,
        };
        match (stop, self.rng.below(4)) {
            (Some(cause), _) => DownloadTerminalEvent::Cancelled {
                download_id,
                streamer_id,
                streamer_name,
                session_id,
                cause,
            },
            (None, 0) => completed(EngineEndSignal::CleanDisconnect),
            (None, 1) => completed(EngineEndSignal::HlsEndlist),
            (None, _) => DownloadTerminalEvent::Failed {
                download_id,
                streamer_id,
                streamer_name,
                session_id,
                engine_type: EngineType::Mesio,
                protocol: DownloadProtocol::Flv,
                kind: self.rng.pick(&[
                    DownloadFailureKind::Network,
                    DownloadFailureKind::Io,
                    DownloadFailureKind::SourceUnavailable,
                ]),
                error: "boom".into(),
                recoverable: true,
            },
        }
    }

    fn external(&mut self) {
        match self.rng.below(100) {
            0..=24 => self.step(FsmInput::CheckNow),
            25..=39 => {
                self.revision += 1;
                let mut config = base_config();
                config.record_danmu = self.record_danmu;
                config.check_interval_ms = self.rng.pick(&[30_000, 60_000, 120_000]);
                config.offline_check_count = 1 + self.rng.below(3) as u32;
                self.min_poll_interval_ms = config
                    .offline_check_interval_ms
                    .min(config.check_interval_ms);
                self.step(FsmInput::ConfigUpdated {
                    revision: self.revision,
                    config: Box::new(config),
                });
            }
            40..=54 => self.step(FsmInput::ErrorCleared),
            55..=64 => self.step(FsmInput::RetryAdmission),
            65..=74 => self.step(FsmInput::BatchLiveHint),
            75..=84 => self.step(FsmInput::CredentialMaterialChanged),
            85..=89 => self.request_stop(Stop::Disable),
            90..=93 => self.request_stop(Stop::Shutdown),
            _ => self.request_stop(Stop::Retire),
        }
    }

    fn request_stop(&mut self, stop: Stop) {
        if matches!(stop, Stop::Disable | Stop::Retire) {
            self.db_active = false;
        }
        self.stop.get_or_insert((stop, self.step_no));
        self.step(match stop {
            Stop::Disable => FsmInput::Disable,
            Stop::Shutdown => FsmInput::Shutdown,
            Stop::Retire => FsmInput::Retire {
                bounds: INTERACTIVE_RETIREMENT,
            },
        });
    }

    fn run(&mut self) {
        while !self.stopped && self.step_no < MAX_STEPS {
            let mut choices = vec![0_u8];
            if self.probe.is_some() {
                choices.extend([1, 1, 1]);
            }
            if self.commit.is_some() {
                choices.extend([2, 2, 2]);
            }
            if !self.attempts.is_empty() {
                choices.extend([3, 3, 3]);
            }
            if !self.danmu_stops.is_empty() {
                choices.extend([4, 4]);
            }
            if self.stop.is_none() && self.rng.pct(10) {
                choices.push(5);
            }
            match self.rng.pick(&choices) {
                0 => {
                    if !self.fire_timer() {
                        self.advance(10_000);
                    }
                }
                1 => {
                    self.advance(2_000);
                    self.complete_probe();
                }
                2 => {
                    self.advance(200);
                    self.complete_commit();
                }
                3 => {
                    self.advance(3_000);
                    self.drive_attempt();
                }
                4 => {
                    if let Some(session_id) = self.danmu_stops.pop_front() {
                        self.danmu_running.remove(&session_id);
                        self.step(FsmInput::DanmuCollectionStopped {
                            session_id,
                            total_count: 0,
                            result: DanmuStopResult::Terminated,
                        });
                    }
                }
                _ => self.external(),
            }
            if let Some((_, requested_at)) = self.stop
                && !self.stopped
                && self.step_no > requested_at + STOP_CONVERGENCE_STEPS
            {
                let detail = format!("stop did not converge: {}", phase_name(&self.fsm));
                self.violate(Invariant::StopConverges, detail);
                break;
            }
        }
        if !self.stopped {
            for session_id in self.db_closed.clone() {
                if !self.ended_notified.contains(&session_id) {
                    self.violate(
                        Invariant::SessionOwnership,
                        format!("database ended {session_id} without a SessionEnded notification"),
                    );
                }
            }
        }
    }
}

fn segment_completed(
    attempt_id: u64,
    session_id: &str,
    download_id: &str,
    segment_index: u32,
    clock: &TestClock,
) -> FsmInput {
    FsmInput::SegmentCompleted {
        attempt_id,
        session_id: session_id.into(),
        download_id: download_id.into(),
        segment_index,
        segment_path: format!("/rec/{attempt_id}_{segment_index}.flv"),
        started_at: Some(clock.wall),
        completed_at: clock.wall,
        duration_secs: 30.0,
        size_bytes: 10_000_000,
        effective_size_bytes: 10_000_000,
        split_reason_code: None,
        split_reason_details_json: None,
        split_by_format_change: false,
    }
}

fn kind_name(kind: &impl std::fmt::Debug) -> String {
    let debug = format!("{kind:?}");
    debug
        .split([' ', '{'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn phase_name(fsm: &StreamerFsm) -> String {
    let monitor = match &fsm.monitor {
        MonitorPhase::Idle { .. } => "Idle",
        MonitorPhase::Filtered { .. } => "Filtered",
        MonitorPhase::RefusedBroadcast { .. } => "RefusedBroadcast",
        MonitorPhase::Blocked { .. } => "Blocked",
        MonitorPhase::LiveWatchdog { .. } => "LiveWatchdog",
        MonitorPhase::Stopping(_) => "Stopping",
        MonitorPhase::Terminated => "Terminated",
    };
    let session = match &fsm.session {
        SessionSlot::None => "None".to_owned(),
        SessionSlot::CommittingStart { .. } => "CommittingStart".to_owned(),
        SessionSlot::Open(open) => format!(
            "Open(attempt={}, danmu={:?}, quiet_period={})",
            kind_name(&open.attempt),
            open.danmu,
            open.quiet_period
                .as_ref()
                .map_or("none".to_owned(), |qp| format!(
                    "retrying={} grace={:?}",
                    qp.retrying_commit, qp.grace_remaining_checks
                ))
        ),
        SessionSlot::RecentlyEnded { .. } => "RecentlyEnded".to_owned(),
    };
    let pending = fsm.pending_db.as_ref().map(|p| kind_name(&p.kind));
    format!("monitor={monitor} session={session} pending={pending:?}")
}
