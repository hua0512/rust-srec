//! Segment discard policy, format-change exemption, paired video/danmu
//! synchronization, and FIFO `CollectionStopped` drain guards.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use jiff::Timestamp;

use crate::danmu::DanmuControlEvent;
use crate::downloader::DownloadStopCause;
use crate::downloader::engine::DownloadProgress;
use crate::session::{OfflineSignal, TerminalCause};
use crate::streamer::download_failure_threshold;
use srec_core::domain::StreamerState;

use super::schedule;
use super::state::{
    AttemptPhase, DanmuSessionState, FsmEffect, MonitorPhase, PendingDbKind, SessionSlot,
    StepContext, StopDanmu, StreamerFsm,
};
use super::stop::request_stop_open_attempt;
use crate::recorder::ports::{
    DanmuCloseSegmentSpec, DanmuSegmentRecord, DanmuStopKind, DanmuStopResult, DbCommitOp,
    PipelineNotification, VideoSegmentRecord,
};

/// Minimum sustained download bytes (8 MiB) required for a progress heartbeat
/// to clear transient streamer error state when no segment has completed yet.
pub const RECOVERY_PROGRESS_MIN_BYTES: u64 = 8 * 1024 * 1024;

/// Decide whether a completed video segment should be discarded for being
/// smaller than `min_segment_size_bytes`.
///
/// Segments closed due to a format/codec/resolution change are always exempt
/// because they contain live stream continuity rather than stray start/end clips.
pub fn should_discard_segment(
    effective_size_bytes: u64,
    min_segment_size_bytes: u64,
    split_by_format_change: bool,
) -> bool {
    min_segment_size_bytes > 0
        && effective_size_bytes < min_segment_size_bytes
        && !split_by_format_change
}

/// Derive the paired danmu `.xml` path from a video segment path.
pub fn paired_danmu_path(video_segment_path: &str) -> String {
    let mut path = PathBuf::from(video_segment_path);
    path.set_extension("xml");
    path.to_string_lossy().into_owned()
}

/// Whether a download progress heartbeat proves healthy recording recovery.
pub fn should_record_recovery_from_progress(progress: &DownloadProgress) -> bool {
    progress.segments_completed > 0
        || (progress.bytes_downloaded >= RECOVERY_PROGRESS_MIN_BYTES
            && progress.speed_bytes_per_sec > 0)
}

/// Per-session guard coordinating video/danmu segment discard ordering until
/// both the download has ended and danmu `CollectionStopped` has drained.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSegmentGuard {
    pub session_id: String,
    /// Video segment indices discarded because they were below `min_segment_size_bytes`,
    /// retained to suppress trailing `DanmuSegmentCompleted` events until `CollectionStopped`.
    pub discarded_segments: HashSet<u32>,
    /// Danmu XML segment paths that finished and were persisted *before* their
    /// paired video segment completed (e.g. on `StreamClosed` or transport error),
    /// enabling reverse cleanup (`RemoveDanmuSegmentOutput` + file deletion) if
    /// the video segment is later discarded.
    pub danmu_persisted_segments: HashMap<u32, String>,
    pub download_ended: bool,
    pub danmu_stopped: bool,
}

impl SessionSegmentGuard {
    /// Create a fresh segment guard for `session_id` starting idle (`download_ended`
    /// and `danmu_stopped` both `true` until an attempt or danmu collector starts).
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            discarded_segments: HashSet::new(),
            danmu_persisted_segments: HashMap::new(),
            download_ended: true,
            danmu_stopped: true,
        }
    }

    /// Record a video-side discard decision. Returns the previously persisted
    /// danmu path if danmu finished first and now needs reverse cleanup.
    pub fn mark_video_discarded(&mut self, segment_index: u32) -> Option<String> {
        self.discarded_segments.insert(segment_index);
        self.danmu_persisted_segments.remove(&segment_index)
    }

    /// Record a video-side accepted segment; no reverse cleanup will be needed.
    pub fn mark_video_kept(&mut self, segment_index: u32) {
        self.danmu_persisted_segments.remove(&segment_index);
    }

    /// Evaluate an incoming `DanmuSegmentCompleted`. Returns `true` when the
    /// paired video segment was already discarded and this XML must be deleted
    /// and suppressed from persistence/pipeline.
    pub fn on_danmu_segment_completed(&mut self, segment_index: u32, output_path: &str) -> bool {
        if self.discarded_segments.contains(&segment_index) {
            return true;
        }
        self.danmu_persisted_segments
            .insert(segment_index, output_path.to_owned());
        false
    }

    /// Whether the guard can be safely evicted (both download and danmu drained).
    pub fn is_drained(&self) -> bool {
        self.download_ended && self.danmu_stopped
    }
}

/// Input bundle for a newly started video segment passed to segment processing.
pub(super) struct SegmentStartedInput {
    pub attempt_id: u64,
    pub session_id: String,
    pub segment_index: u32,
    pub segment_path: String,
    pub started_at: Timestamp,
}

/// Input bundle for a completed video segment passed to segment processing.
pub(super) struct VideoSegmentInput {
    pub attempt_id: u64,
    pub session_id: String,
    pub segment_index: u32,
    pub segment_path: String,
    pub started_at: Option<Timestamp>,
    pub completed_at: Timestamp,
    pub duration_secs: f64,
    pub size_bytes: u64,
    pub effective_size_bytes: u64,
    pub split_reason_code: Option<String>,
    pub split_reason_details_json: Option<String>,
    pub split_by_format_change: bool,
}

impl StreamerFsm {
    /// Handle `SegmentStarted`: refresh activity for the current attempt,
    /// clear `last_error` if set, and start the paired danmu XML segment.
    pub(super) fn on_segment_started(
        &mut self,
        ctx: StepContext,
        input: SegmentStartedInput,
        effects: &mut Vec<FsmEffect>,
    ) {
        let SegmentStartedInput {
            attempt_id,
            session_id,
            segment_index,
            segment_path,
            started_at,
        } = input;
        let is_current = self.matches_current_attempt(&session_id, attempt_id);
        if is_current {
            if let MonitorPhase::LiveWatchdog {
                last_activity_at, ..
            } = &mut self.monitor
            {
                *last_activity_at = ctx.now;
            }
            if !self.errors.is_disabled(ctx.wall_now) && self.errors.has_last_error() {
                self.errors.clear_last_error();
                self.enqueue_or_dispatch_db(
                    PendingDbKind::ClearLastError,
                    DbCommitOp::ClearLastError {
                        streamer_id: self.streamer_id.clone(),
                    },
                    effects,
                );
            }
        }

        let danmu_active = matches!(
            &self.session,
            SessionSlot::Open(open)
                if open.session_id == session_id
                    && matches!(
                        open.danmu,
                        DanmuSessionState::Starting | DanmuSessionState::Running { .. }
                    )
        );
        if danmu_active {
            effects.push(FsmEffect::StartDanmuSegment {
                session_id,
                segment_index,
                video_segment_path: segment_path,
                started_at,
            });
        }
    }

    /// Handle `SegmentCompleted`.
    ///
    /// Session-level segment persistence is strictly decoupled from attempt
    /// generation control: even if `attempt_id` is stale or the actor is in
    /// `Stopping` / already ended the session, a non-discarded trailing segment
    /// is still persisted to `session_segments` and forwarded to the pipeline.
    pub(super) fn on_segment_completed(
        &mut self,
        ctx: StepContext,
        input: VideoSegmentInput,
        effects: &mut Vec<FsmEffect>,
    ) {
        let is_current = self.matches_current_attempt(&input.session_id, input.attempt_id);
        if is_current
            && let MonitorPhase::LiveWatchdog {
                last_activity_at, ..
            } = &mut self.monitor
        {
            *last_activity_at = ctx.now;
        }

        let discard = should_discard_segment(
            input.effective_size_bytes,
            self.config.min_segment_size_bytes,
            input.split_by_format_change,
        );

        let danmu_open_for_session = matches!(
            &self.session,
            SessionSlot::Open(open)
                if open.session_id == input.session_id
                    && !matches!(open.danmu, DanmuSessionState::Off)
        ) || self
            .draining_segment_guards
            .get(&input.session_id)
            .is_some_and(|guard| !guard.danmu_stopped);

        if discard {
            let mut guard_present = false;
            let reverse_cleanup_danmu = self
                .segment_guard_if_present_mut(&input.session_id)
                .and_then(|guard| {
                    guard_present = true;
                    guard.mark_video_discarded(input.segment_index)
                });

            if danmu_open_for_session {
                effects.push(FsmEffect::CloseDanmuSegment(DanmuCloseSegmentSpec {
                    session_id: input.session_id.clone(),
                    segment_index: input.segment_index,
                    discard: true,
                }));
            }

            let danmu_path = reverse_cleanup_danmu
                .clone()
                .unwrap_or_else(|| paired_danmu_path(&input.segment_path));

            effects.push(FsmEffect::DeleteDiscardedFiles {
                video_path: Some(input.segment_path),
                danmu_path: Some(danmu_path.clone()),
            });

            // When the segment guard was already evicted (or this is a late
            // discard after actor replacement), issue an idempotent DB removal
            // for the paired danmu path so an XML-first row is never orphaned.
            if let Some(persisted_danmu) = reverse_cleanup_danmu {
                effects.push(FsmEffect::RemoveDanmuSegmentOutput {
                    session_id: input.session_id,
                    danmu_path: persisted_danmu,
                });
            } else if !guard_present {
                effects.push(FsmEffect::RemoveDanmuSegmentOutput {
                    session_id: input.session_id,
                    danmu_path,
                });
            }
            return;
        }

        if is_current {
            self.network_failures.note_successful_segment();
        }

        if let Some(guard) = self.segment_guard_if_present_mut(&input.session_id) {
            guard.mark_video_kept(input.segment_index);
        }

        if danmu_open_for_session {
            effects.push(FsmEffect::CloseDanmuSegment(DanmuCloseSegmentSpec {
                session_id: input.session_id.clone(),
                segment_index: input.segment_index,
                discard: false,
            }));
        }

        effects.push(FsmEffect::PersistVideoSegment(VideoSegmentRecord {
            session_id: input.session_id.clone(),
            streamer_id: self.streamer_id.clone(),
            segment_index: input.segment_index,
            segment_path: input.segment_path.clone(),
            started_at: input.started_at,
            completed_at: input.completed_at,
            duration_secs: input.duration_secs,
            size_bytes: input.size_bytes,
            split_reason_code: input.split_reason_code,
            split_reason_details_json: input.split_reason_details_json,
        }));

        effects.push(FsmEffect::NotifyPipeline(
            PipelineNotification::VideoSegmentCompleted {
                session_id: input.session_id,
                streamer_id: self.streamer_id.clone(),
                segment_index: input.segment_index,
                path: PathBuf::from(input.segment_path),
                auto_thumbnail_candidate: input.segment_index == 0,
            },
        ));

        if is_current
            && self.errors.has_transient_error_state()
            && !matches!(
                self.monitor,
                MonitorPhase::Stopping(_) | MonitorPhase::Terminated
            )
        {
            self.schedule_clear_errors_live(ctx, effects);
        }
    }

    /// Handle a throttled `DownloadHeartbeat` for the active attempt.
    pub(super) fn on_download_heartbeat(
        &mut self,
        ctx: StepContext,
        session_id: &str,
        download_id: &str,
        progress: Option<&DownloadProgress>,
        effects: &mut Vec<FsmEffect>,
    ) {
        let matches = matches!(
            &self.session,
            SessionSlot::Open(open)
                if open.session_id == session_id
                    && matches!(
                        &open.attempt,
                        super::state::AttemptPhase::Running { download_id: current, .. }
                            if current == download_id
                    )
        );
        if !matches {
            return;
        }
        if let MonitorPhase::LiveWatchdog {
            last_activity_at, ..
        } = &mut self.monitor
        {
            *last_activity_at = ctx.now;
        }
        if progress.is_some_and(should_record_recovery_from_progress)
            && self.errors.has_transient_error_state()
            && !matches!(
                self.monitor,
                MonitorPhase::Stopping(_) | MonitorPhase::Terminated
            )
        {
            self.schedule_clear_errors_live(ctx, effects);
        }
    }

    /// Handle `DanmuCollectionStarted`.
    pub(super) fn on_danmu_collection_started(
        &mut self,
        session_id: String,
        effects: &mut Vec<FsmEffect>,
    ) {
        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
        {
            open.danmu = DanmuSessionState::Running {
                link_down_since: None,
            };
            open.segment_guard.danmu_stopped = false;
        }
        effects.push(FsmEffect::NotifyPipeline(
            PipelineNotification::DanmuCollectionStarted {
                session_id,
                streamer_id: self.streamer_id.clone(),
            },
        ));
    }

    /// Handle `DanmuSegmentCompleted` through the session segment discard guard.
    pub(super) fn on_danmu_segment_completed(
        &mut self,
        session_id: String,
        segment_index: u32,
        output_path: String,
        message_count: u64,
        size_bytes: u64,
        effects: &mut Vec<FsmEffect>,
    ) {
        let suppressed = self
            .segment_guard_if_present_mut(&session_id)
            .is_some_and(|guard| guard.on_danmu_segment_completed(segment_index, &output_path));

        if suppressed {
            effects.push(FsmEffect::DeleteDiscardedFiles {
                video_path: None,
                danmu_path: Some(output_path),
            });
            return;
        }

        effects.push(FsmEffect::PersistDanmuSegment(DanmuSegmentRecord {
            session_id: session_id.clone(),
            streamer_id: self.streamer_id.clone(),
            segment_index,
            output_path: output_path.clone(),
            message_count,
            size_bytes,
        }));

        effects.push(FsmEffect::NotifyPipeline(
            PipelineNotification::DanmuSegmentCompleted {
                session_id,
                streamer_id: self.streamer_id.clone(),
                segment_index,
                path: Path::new(&output_path).to_path_buf(),
                message_count,
            },
        ));
    }

    /// Handle `DanmuControl` (`RoomInfoChanged` title update or `StreamClosed`).
    pub(super) fn on_danmu_control(
        &mut self,
        ctx: StepContext,
        session_id: String,
        control: DanmuControlEvent,
        effects: &mut Vec<FsmEffect>,
    ) {
        match control {
            DanmuControlEvent::RoomInfoChanged {
                title: Some(new_title),
                ..
            } => {
                if let SessionSlot::Open(open) = &mut self.session
                    && open.session_id == session_id
                    && open.title != new_title
                {
                    open.title = new_title.clone();
                    self.enqueue_or_dispatch_db(
                        PendingDbKind::AddSessionTitle,
                        DbCommitOp::AddSessionTitle {
                            session_id,
                            title: new_title,
                            now: ctx.wall_now,
                        },
                        effects,
                    );
                }
            }
            DanmuControlEvent::StreamClosed { .. } => {
                if !self.config.end_stream_on_danmu_stream_closed {
                    return;
                }
                if matches!(
                    self.monitor,
                    MonitorPhase::Stopping(_) | MonitorPhase::Terminated
                ) {
                    return;
                }
                // A delayed `StreamClosed` from an older session must never
                // tear down a successor session.
                let was_in_hysteresis = match &mut self.session {
                    SessionSlot::Open(open) if open.session_id == session_id => {
                        let was_hysteresis = open.quiet_period.take().is_some();
                        request_stop_open_attempt(
                            open,
                            DownloadStopCause::DanmuStreamClosed,
                            effects,
                        );
                        was_hysteresis
                    }
                    _ => return,
                };

                self.offline_confirmation.reset();
                self.monitor = schedule::schedule_idle(
                    &self.config,
                    &self.offline_confirmation,
                    false,
                    self.errors.error_count_u32(),
                    None,
                    ctx,
                );

                let clear_errors = self.errors.has_errors();
                let threshold = download_failure_threshold(self.config.offline_check_count);
                let cause = TerminalCause::DefinitiveOffline {
                    signal: OfflineSignal::DanmuStreamClosed,
                };
                self.enqueue_or_dispatch_db(
                    PendingDbKind::EndSession {
                        session_id: Some(session_id.clone()),
                        cause: cause.clone(),
                        via_hysteresis: was_in_hysteresis,
                        from_quiet_period_timer_id: None,
                        sets_offline: true,
                        sets_out_of_schedule: false,
                        clears_errors: clear_errors,
                        for_stop: false,
                    },
                    DbCommitOp::EndSession {
                        streamer_id: self.streamer_id.clone(),
                        streamer_name: self.config.streamer_name.clone(),
                        session_id: Some(session_id),
                        cause,
                        via_hysteresis: was_in_hysteresis,
                        update_streamer_state: Some(StreamerState::NotLive),
                        clear_errors,
                        checks_restored_threshold: Some(threshold),
                        infra_block: None,
                        suppress_offline_notification: false,
                        now: ctx.wall_now,
                    },
                    effects,
                );
            }
            _ => {}
        }
    }

    /// Handle `DanmuReconnecting` while collection is still active.
    pub(super) fn on_danmu_reconnecting(&mut self, ctx: StepContext, session_id: &str) {
        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
            && let DanmuSessionState::Running { link_down_since } = &mut open.danmu
        {
            link_down_since.get_or_insert(ctx.now);
        }
    }

    /// Handle `DanmuReconnected`.
    pub(super) fn on_danmu_reconnected(&mut self, session_id: &str) {
        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
            && let DanmuSessionState::Running { link_down_since } = &mut open.danmu
        {
            *link_down_since = None;
        }
    }

    /// Handle `DanmuCollectionStopped`: release the FIFO segment discard guard
    /// when download has also ended, forward `DanmuCollectionStopped` to the
    /// pipeline, and advance any active stop state.
    pub(super) fn on_danmu_collection_stopped(
        &mut self,
        ctx: StepContext,
        session_id: String,
        total_count: u64,
        result: DanmuStopResult,
        effects: &mut Vec<FsmEffect>,
    ) {
        let unconfirmed = matches!(result, DanmuStopResult::Unconfirmed { .. });
        // Only the collector this stop waits for settles its danmu leg.
        let settles_stop = matches!(
            &self.monitor,
            MonitorPhase::Stopping(stopping)
                if matches!(&stopping.danmu, StopDanmu::Pending { session_id: awaited } if *awaited == session_id)
        );
        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
        {
            if !unconfirmed {
                open.danmu = DanmuSessionState::Off;
                open.segment_guard.danmu_stopped = true;
            }
        } else if let Some(guard) = self.draining_segment_guards.get_mut(&session_id)
            && !unconfirmed
        {
            guard.danmu_stopped = true;
            if guard.is_drained() {
                self.draining_segment_guards.remove(&session_id);
            }
        }

        if matches!(result, DanmuStopResult::Terminated) {
            effects.push(FsmEffect::NotifyPipeline(
                PipelineNotification::DanmuCollectionStopped {
                    session_id,
                    total_count,
                },
            ));
        }

        if settles_stop && let MonitorPhase::Stopping(stopping) = &mut self.monitor {
            if unconfirmed {
                stopping.danmu = StopDanmu::Unconfirmed;
                self.lingering_physical_lease = true;
            } else {
                stopping.danmu = StopDanmu::Confirmed;
            }
            self.advance_stopping(ctx, effects);
        }
    }

    /// Let go of an open session that left `self.session`: park its segment
    /// guard while trailing segments drain, and detach an attempt that has not
    /// reported its end yet, asking it to stop if nothing has.
    pub(super) fn release_open_session(
        &mut self,
        open: Box<super::state::OpenSession>,
        effects: &mut Vec<FsmEffect>,
    ) {
        let open = *open;
        let ended_session = DownloadStopCause::Other("session ended".to_owned());
        match open.attempt {
            AttemptPhase::Starting { attempt_id, .. } => {
                effects.push(FsmEffect::StopAttempt {
                    attempt_id,
                    download_id: None,
                    cause: ended_session.clone(),
                });
                self.detached_attempts.insert(attempt_id, ended_session);
            }
            AttemptPhase::Running {
                attempt_id,
                download_id,
                ..
            } => {
                effects.push(FsmEffect::StopAttempt {
                    attempt_id,
                    download_id: Some(download_id),
                    cause: ended_session.clone(),
                });
                self.detached_attempts.insert(attempt_id, ended_session);
            }
            AttemptPhase::StoppingAttempt {
                attempt_id, cause, ..
            } => {
                self.detached_attempts.insert(attempt_id, cause);
            }
            AttemptPhase::Idle
            | AttemptPhase::WaitingCredentials
            | AttemptPhase::DiagnosingCredentials { .. } => {}
        }
        self.park_segment_guard_if_draining(open.segment_guard);
    }

    /// Park an open session's segment guard into `draining_segment_guards` if
    /// trailing segments or danmu `CollectionStopped` have not finished draining.
    pub(super) fn park_segment_guard_if_draining(&mut self, mut guard: SessionSegmentGuard) {
        if !guard.is_drained() {
            let sid = guard.session_id.clone();
            if let Some(existing) = self.draining_segment_guards.remove(&sid) {
                guard.discarded_segments.extend(existing.discarded_segments);
                for (idx, path) in existing.danmu_persisted_segments {
                    guard.danmu_persisted_segments.entry(idx).or_insert(path);
                }
                guard.download_ended |= existing.download_ended;
                guard.danmu_stopped |= existing.danmu_stopped;
            }
            if !guard.is_drained() {
                self.draining_segment_guards.insert(sid, guard);
            }
        }
    }

    /// Mark download ended on a session's segment guard, evicting it if drained.
    pub(super) fn mark_guard_download_ended(&mut self, session_id: &str) {
        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
        {
            open.segment_guard.download_ended = true;
            return;
        }
        if let Some(guard) = self.draining_segment_guards.get_mut(session_id) {
            guard.download_ended = true;
            if guard.is_drained() {
                self.draining_segment_guards.remove(session_id);
            }
        }
    }

    fn segment_guard_if_present_mut(
        &mut self,
        session_id: &str,
    ) -> Option<&mut SessionSegmentGuard> {
        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
        {
            return Some(&mut open.segment_guard);
        }
        self.draining_segment_guards.get_mut(session_id)
    }

    fn schedule_clear_errors_live(&mut self, ctx: StepContext, effects: &mut Vec<FsmEffect>) {
        // Deduplicate against an already pending/queued `ClearErrors` instead of
        // clearing `self.errors` optimistically before SQLite commits; `commit.rs`
        // clears `self.errors` once `DbCommitOp::ClearErrors` succeeds.
        let already_scheduled = matches!(
            self.pending_db.as_ref().map(|p| &p.kind),
            Some(PendingDbKind::ClearErrors)
        ) || self
            .queued_db
            .iter()
            .any(|q| matches!(q.kind, PendingDbKind::ClearErrors));
        if already_scheduled {
            return;
        }
        let threshold = download_failure_threshold(self.config.offline_check_count);
        self.enqueue_or_dispatch_db(
            PendingDbKind::ClearErrors,
            DbCommitOp::ClearErrors {
                streamer_id: self.streamer_id.clone(),
                streamer_name: self.config.streamer_name.clone(),
                checks_restored_threshold: threshold,
                set_state: Some(StreamerState::Live),
                now: ctx.wall_now,
            },
            effects,
        );
    }

    /// Stop danmu collection for an open session if it is starting or running.
    pub(super) fn stop_open_danmu(
        open: &mut super::state::OpenSession,
        reason: DanmuStopKind,
        effects: &mut Vec<FsmEffect>,
    ) -> bool {
        match open.danmu {
            DanmuSessionState::Starting | DanmuSessionState::Running { .. } => {
                open.danmu = DanmuSessionState::Stopping;
                effects.push(FsmEffect::StopDanmu {
                    session_id: open.session_id.clone(),
                    reason,
                });
                true
            }
            DanmuSessionState::Stopping => true,
            DanmuSessionState::Off => false,
        }
    }
}
