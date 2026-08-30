//! Collection runner for danmu collection sessions.
//!
//! This module provides a state machine for running danmu collection with:
//! - Message buffering and sorting
//! - Segment-based file writing
//! - Reconnection when the provider's transport gives up
//! - Periodic buffer flushing

use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use platforms_parser::danmaku::{
    ConnectionConfig, DanmuConnection, DanmuControlEvent, DanmuItem, DanmuProvider,
    message::DanmuMessage,
};

use crate::danmu::XmlDanmuWriter;
use crate::error::{Error, Result};

use super::events::{CollectionCommand, DanmuEvent, DanmuEventPublisher};
use super::lifecycle::{CollectionExitReason, CollectionOutcome};
use super::statistics_session::StatisticsSession;

/// Configuration constants for the collection runner.
mod config {
    /// Buffer flush interval in milliseconds.
    pub const BUFFER_FLUSH_INTERVAL_MS: u64 = 500;
    /// Maximum number of messages to buffer before forcing a flush.
    pub const MAX_BUFFER_SIZE: usize = 100;
    /// Interval between periodic statistics persists for in-progress sessions.
    pub const STATS_PERSIST_INTERVAL_SECS: u64 = 60;
    /// Interval between aggregator checkpoints. Slower than the statistics
    /// persist because the checkpoint is a much larger blob; a crash costs up to
    /// this much *resume fidelity* while the published statistics stay on the
    /// faster cadence.
    pub const CHECKPOINT_INTERVAL_SECS: u64 = 300;
    /// Delay before the first reconnect attempt, doubling per attempt.
    pub const RECONNECT_BASE_DELAY_MS: u64 = 5_000;
    /// Ceiling for the exponential reconnect back-off.
    pub const RECONNECT_MAX_DELAY_MS: u64 = 60_000;
    /// How long a link may stay down before `DanmuEvent::ReconnectFailed` is
    /// emitted once for the outage. Reconnecting continues regardless; this only
    /// marks the point where an outage is worth an operator's attention.
    pub const RECONNECT_ALERT_AFTER_SECS: u64 = 300;
}

/// Result of command handling - indicates whether to continue or stop.
#[derive(Debug, PartialEq)]
pub(crate) enum CommandResult {
    /// Continue running the collection loop.
    Continue,
    /// Stop the collection loop.
    Stop(CollectionExitReason),
}

/// Transport state for the item channel handed over by `DanmuProvider::connect`.
enum Link {
    /// Items are flowing.
    Up(mpsc::Receiver<DanmuItem>),
    /// The provider's transport finished and closed the channel. `retry_at`
    /// gates the next `DanmuProvider::connect`; the attempt budget itself lives
    /// in `CollectionRunner::reconnect_attempts`, which survives link swaps.
    Down { retry_at: Instant },
}

/// What the run loop woke up for.
///
/// `select!` only *produces* one of these; every handler runs after the
/// `select!` block has ended. That keeps the borrow of `Link` taken by
/// `next_link_event` from overlapping the `&mut self` the handlers need.
enum LoopEvent {
    Command(Option<CollectionCommand>),
    Cancelled,
    Flush,
    Persist,
    Checkpoint,
    Item(DanmuItem),
    /// The item channel closed: the provider's transport is finished.
    LinkClosed,
    /// The reconnect back-off elapsed.
    RetryDue,
}

/// State machine for running a danmu collection session.
///
/// Encapsulates all state and logic for collecting danmu messages,
/// handling segment transitions, and managing reconnections.
pub(crate) struct CollectionRunner {
    // Identity
    session_id: String,
    streamer_id: String,
    room_id: String,

    // Provider and connection
    provider: Arc<dyn DanmuProvider>,
    connection: DanmuConnection,
    /// Retained so `reconnect` can open a new stream with the same cookies and
    /// platform extras the initial `connect` used.
    conn_config: ConnectionConfig,

    // Current segment writer
    current_writer: Option<(String, XmlDanmuWriter)>,

    // Message buffer for sorting before writing
    message_buffer: Vec<DanmuMessage>,

    statistics: StatisticsSession,

    /// Reconnect attempts since the last item arrived, used for the back-off
    /// curve and reported on `DanmuEvent::Reconnecting`.
    reconnect_attempts: u32,
    /// When the current outage began, or `None` while items are flowing. An
    /// arriving item — not a successful `connect` — is what ends an outage, since
    /// `connect` returns before the handshake is known to have worked.
    link_down_since: Option<Instant>,
    /// Whether this outage already emitted `DanmuEvent::ReconnectFailed`, so the
    /// alert fires once per outage rather than once per attempt.
    outage_alerted: bool,

    events: DanmuEventPublisher,
}

/// Parameters for creating a new collection runner.
pub(crate) struct RunnerParams {
    pub session_id: String,
    pub streamer_id: String,
    pub room_id: String,
    pub provider: Arc<dyn DanmuProvider>,
    pub conn_config: ConnectionConfig,
    pub statistics: StatisticsSession,
    pub events: DanmuEventPublisher,
}

impl CollectionRunner {
    /// Create a new collection runner, returning it alongside the item channel
    /// its `run` loop should consume.
    pub async fn new(params: RunnerParams) -> Result<(Self, mpsc::Receiver<DanmuItem>)> {
        let RunnerParams {
            session_id,
            streamer_id,
            room_id,
            provider,
            conn_config,
            statistics,
            events,
        } = params;

        // Connect to danmu stream
        let stream = provider.connect(&room_id, conn_config.clone()).await?;

        Ok((
            Self {
                session_id,
                streamer_id,
                room_id,
                provider,
                connection: stream.connection,
                conn_config,
                current_writer: None,
                message_buffer: Vec::with_capacity(config::MAX_BUFFER_SIZE),
                statistics,
                reconnect_attempts: 0,
                link_down_since: None,
                outage_alerted: false,
                events,
            },
            stream.items,
        ))
    }

    /// Run the collection loop until stopped, cancelled, or unrecoverable.
    ///
    /// Always finalizes the active segment and disconnects before returning, so
    /// a failure cannot leave an XML file without its closing `</i>` or skip the
    /// `DanmuEvent::SegmentCompleted` that creates the segment's `media_outputs`
    /// row.
    pub async fn run(
        mut self,
        mut command_rx: mpsc::Receiver<CollectionCommand>,
        items: mpsc::Receiver<DanmuItem>,
        cancel_token: CancellationToken,
    ) -> CollectionOutcome {
        let (reason, error) = match self.run_loop(&mut command_rx, items, &cancel_token).await {
            Ok(reason) => (reason, None),
            Err(error) => (CollectionExitReason::Failed, Some(error)),
        };
        let cleanup_errors = self.shutdown().await;
        let statistics = self.statistics.finish(reason).await;

        CollectionOutcome {
            statistics,
            reason,
            error,
            cleanup_errors,
        }
    }

    async fn run_loop(
        &mut self,
        command_rx: &mut mpsc::Receiver<CollectionCommand>,
        items: mpsc::Receiver<DanmuItem>,
        cancel_token: &CancellationToken,
    ) -> Result<CollectionExitReason> {
        let mut flush_interval =
            tokio::time::interval(Duration::from_millis(config::BUFFER_FLUSH_INTERVAL_MS));
        flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut persist_interval =
            tokio::time::interval(Duration::from_secs(config::STATS_PERSIST_INTERVAL_SECS));
        persist_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut checkpoint_interval =
            tokio::time::interval(Duration::from_secs(config::CHECKPOINT_INTERVAL_SECS));
        checkpoint_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut link = Link::Up(items);

        loop {
            let event = tokio::select! {
                biased;

                cmd = command_rx.recv() => LoopEvent::Command(cmd),
                _ = cancel_token.cancelled() => LoopEvent::Cancelled,
                _ = flush_interval.tick() => LoopEvent::Flush,
                _ = persist_interval.tick() => LoopEvent::Persist,
                _ = checkpoint_interval.tick() => LoopEvent::Checkpoint,
                event = Self::next_link_event(&mut link) => event,
            };

            match event {
                LoopEvent::Command(cmd) => {
                    let boundary_stop = if self.command_closes_current_segment(cmd.as_ref()) {
                        self.drain_segment_boundary(&mut link).await?
                    } else {
                        None
                    };

                    // The boundary command runs even when the drain found a
                    // stopping control event: `start_segment` is what opens the
                    // next segment's XML and, via `finalize_current_segment`,
                    // emits the `DanmuEvent::SegmentCompleted` that gives it a
                    // `media_outputs` row. Skipping it would leave the video
                    // segment without the danmu output
                    // `PipelineCoordinator::try_trigger_paired` requires.
                    let result = self.handle_command(cmd).await?;

                    if let Some(reason) = boundary_stop {
                        self.drain_pending(&mut link).await?;
                        return Ok(reason);
                    }

                    match result {
                        CommandResult::Continue => {}
                        CommandResult::Stop(reason) => {
                            self.drain_pending(&mut link).await?;
                            return Ok(reason);
                        }
                    }
                }
                LoopEvent::Cancelled => {
                    self.drain_pending(&mut link).await?;
                    return Ok(CollectionExitReason::Cancelled);
                }
                LoopEvent::Flush => self.flush_buffer_if_needed().await?,
                LoopEvent::Persist => self.statistics.persist_if_changed().await,
                LoopEvent::Checkpoint => self.statistics.checkpoint_if_changed().await,
                LoopEvent::Item(item) => {
                    // An arriving item is the only proof the link works, so it —
                    // not a successful `connect` — is what ends an outage.
                    self.note_link_recovered();
                    match self.handle_item(item).await? {
                        CommandResult::Continue => {}
                        CommandResult::Stop(reason) => {
                            self.drain_pending(&mut link).await?;
                            return Ok(reason);
                        }
                    }
                }
                LoopEvent::LinkClosed => {
                    link = self.schedule_retry("provider transport closed the item channel");
                }
                LoopEvent::RetryDue => {
                    link = self.reconnect().await;
                }
            }
        }
    }

    /// Take whatever the provider has already queued before leaving the loop.
    ///
    /// Commands are polled before items (`biased`), so a stop request preempts
    /// messages still sitting in the channel — up to its capacity. Those messages
    /// are part of the recording, so they are collected here instead of being
    /// dropped, and the unconditional flush in `run` writes them to the segment.
    ///
    /// `try_recv` only: this must never wait on a provider that has gone quiet.
    async fn drain_pending(&mut self, link: &mut Link) -> Result<()> {
        let Link::Up(items) = link else {
            return Ok(());
        };

        let mut drained = 0usize;
        while let Ok(item) = items.try_recv() {
            // Control events are skipped rather than breaking the drain: the loop
            // is already ending — a `StreamClosed` is often why — but a control
            // event sitting mid-queue must not strand the messages behind it.
            if let DanmuItem::Message(message) = item {
                self.handle_message(message).await?;
                drained += 1;
            }
        }

        if drained > 0 {
            debug!(
                session_id = %self.session_id,
                drained,
                "danmu: collected queued messages before stopping"
            );
        }
        Ok(())
    }

    /// Consume the items that were already queued when a segment boundary was
    /// observed, before the active writer is closed or replaced.
    ///
    /// A returned reason means a drained control event ends the session. The
    /// caller must still run the boundary command before leaving the loop, so
    /// the drain stops at that item rather than exiting on the caller's behalf.
    async fn drain_segment_boundary(
        &mut self,
        link: &mut Link,
    ) -> Result<Option<CollectionExitReason>> {
        let Link::Up(items) = link else {
            return Ok(None);
        };

        // Snapshot the queue depth so a busy producer cannot postpone segment
        // rotation indefinitely by refilling the channel while it is drained.
        let queued = items.len();
        let mut stop = None;
        let mut drained = 0usize;
        for _ in 0..queued {
            let Ok(item) = items.try_recv() else {
                break;
            };

            self.note_link_recovered();
            if matches!(item, DanmuItem::Message(_)) {
                drained += 1;
            }
            if let CommandResult::Stop(reason) = self.handle_item(item).await? {
                stop = Some(reason);
                break;
            }
        }

        if drained > 0 {
            debug!(
                session_id = %self.session_id,
                drained,
                "danmu: kept queued messages in the segment being closed"
            );
        }
        Ok(stop)
    }

    /// Wait for the next transport event: an item while the link is up, or the
    /// back-off deadline while it is down.
    async fn next_link_event(link: &mut Link) -> LoopEvent {
        match link {
            Link::Up(items) => match items.recv().await {
                Some(item) => LoopEvent::Item(item),
                None => LoopEvent::LinkClosed,
            },
            Link::Down { retry_at, .. } => {
                tokio::time::sleep_until(*retry_at).await;
                LoopEvent::RetryDue
            }
        }
    }

    /// Count a transport failure and schedule the next attempt.
    ///
    /// Never gives up: the runner serves one recording session and its lifetime
    /// is bounded by the cancellation token `stop_collection` fires at session
    /// end. Staying alive keeps `DanmuService::get_handle` answering, so segment
    /// rotation keeps opening one XML file per video segment while the link is
    /// down.
    fn schedule_retry(&mut self, cause: &str) -> Link {
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
        let attempt = self.reconnect_attempts;
        let down_since = *self.link_down_since.get_or_insert_with(Instant::now);
        let downtime = down_since.elapsed();

        let delay = Self::reconnect_delay(attempt);
        self.events.publish(DanmuEvent::Reconnecting {
            session_id: self.session_id.clone(),
            attempt,
        });

        // One alert per outage, raised once it has lasted long enough to be worth
        // an operator's attention. Reconnecting continues either way.
        if !self.outage_alerted
            && downtime >= Duration::from_secs(config::RECONNECT_ALERT_AFTER_SECS)
        {
            self.outage_alerted = true;
            self.events.publish(DanmuEvent::ReconnectFailed {
                session_id: self.session_id.clone(),
                error: format!(
                    "danmu link down for {}s after {attempt} attempts: {cause}",
                    downtime.as_secs()
                ),
            });
        }

        warn!(
            session_id = %self.session_id,
            room_id = %self.room_id,
            attempt,
            downtime_secs = downtime.as_secs(),
            delay_ms = delay.as_millis() as u64,
            cause,
            "danmu: scheduling reconnect"
        );

        Link::Down {
            retry_at: Instant::now() + delay,
        }
    }

    /// Clear outage state after an item proves the link works, announcing the
    /// recovery when there was an outage to recover from.
    fn note_link_recovered(&mut self) {
        if let Some(down_since) = self.link_down_since.take() {
            let downtime_secs = down_since.elapsed().as_secs();
            info!(
                session_id = %self.session_id,
                room_id = %self.room_id,
                attempts = self.reconnect_attempts,
                downtime_secs,
                "danmu: link recovered"
            );
            self.events.publish(DanmuEvent::Reconnected {
                session_id: self.session_id.clone(),
                attempts: self.reconnect_attempts,
                downtime_secs,
            });
        }
        self.reconnect_attempts = 0;
        self.outage_alerted = false;
    }

    /// Replace the transport with a fresh stream for the same room.
    ///
    /// The aggregator and the open segment writer are untouched, so a reconnect
    /// leaves no seam in the XML or the statistics.
    async fn reconnect(&mut self) -> Link {
        // Release the finished connection first so its provider-side tasks and
        // its slot in the connection semaphore are returned before a new one is
        // acquired.
        if let Err(error) = self.provider.disconnect(&mut self.connection).await {
            debug!(
                session_id = %self.session_id,
                %error,
                "danmu: disconnect before reconnect failed"
            );
        }

        match self
            .provider
            .connect(&self.room_id, self.conn_config.clone())
            .await
        {
            // The link counts as up only once an item arrives: `connect` returns
            // before the upgrade and handshake are known to have worked, so
            // `note_link_recovered` — not this branch — ends the outage.
            Ok(stream) => {
                debug!(
                    session_id = %self.session_id,
                    room_id = %self.room_id,
                    attempt = self.reconnect_attempts,
                    "danmu: reopened transport, awaiting first item"
                );
                self.connection = stream.connection;
                Link::Up(stream.items)
            }
            // `connect` only fails before the transport task starts; a failed
            // handshake surfaces later as a closed channel instead.
            Err(error) => {
                let cause = error.to_string();
                self.schedule_retry(&cause)
            }
        }
    }

    /// Exponential back-off, doubling from `RECONNECT_BASE_DELAY_MS` up to
    /// `RECONNECT_MAX_DELAY_MS`.
    fn reconnect_delay(attempt: u32) -> Duration {
        let shift = attempt.saturating_sub(1).min(5);
        let millis = config::RECONNECT_BASE_DELAY_MS
            .saturating_mul(1_u64 << shift)
            .min(config::RECONNECT_MAX_DELAY_MS);
        Duration::from_millis(millis)
    }

    /// Handle a command from the channel.
    async fn handle_command(&mut self, cmd: Option<CollectionCommand>) -> Result<CommandResult> {
        match cmd {
            Some(CollectionCommand::StartSegment {
                segment_id,
                output_path,
                start_time,
            }) => {
                self.start_segment(segment_id, output_path, start_time)
                    .await?;
                Ok(CommandResult::Continue)
            }
            Some(CollectionCommand::EndSegment { segment_id }) => {
                self.end_segment(&segment_id).await?;
                Ok(CommandResult::Continue)
            }
            // `run` finalizes the segment and disconnects after the loop ends, so
            // stopping only has to leave the loop.
            Some(CollectionCommand::Stop(reason)) => {
                Ok(CommandResult::Stop(CollectionExitReason::from(reason)))
            }
            None => Ok(CommandResult::Stop(
                CollectionExitReason::CommandChannelClosed,
            )),
        }
    }

    /// Whether `handle_command` would take `current_writer` away from the
    /// segment it currently holds: `start_segment` finalizes any open writer
    /// before opening the next one, while `end_segment` only acts when the id
    /// matches. `Stop` and a closed channel leave the writer to `shutdown`.
    fn command_closes_current_segment(&self, cmd: Option<&CollectionCommand>) -> bool {
        let Some((current_segment_id, _)) = &self.current_writer else {
            return false;
        };

        match cmd {
            Some(CollectionCommand::StartSegment { .. }) => true,
            Some(CollectionCommand::EndSegment { segment_id }) => segment_id == current_segment_id,
            Some(CollectionCommand::Stop(_)) | None => false,
        }
    }

    /// Start a new segment, flushing and finalizing the old one if present.
    async fn start_segment(
        &mut self,
        segment_id: String,
        output_path: PathBuf,
        start_time: DateTime<Utc>,
    ) -> Result<()> {
        // Flush buffer to old segment before switching
        self.flush_buffer().await?;

        // Finalize previous segment if any
        self.finalize_current_segment().await?;

        // Clear buffer for new segment
        self.message_buffer.clear();

        // Create output directory if needed
        crate::utils::fs::ensure_parent_dir(&output_path).await?;

        // Start new segment with the provided start time and metadata comments
        let comments = vec![
            format!("Rust-Srec version: {}", env!("CARGO_PKG_VERSION")),
            format!("Platform: {}", self.provider.platform()),
            format!("Room ID: {}", self.room_id),
            format!("Session ID: {}", self.session_id),
            format!("Segment ID: {}", segment_id),
            format!("Start Time: {}", start_time),
        ];
        let writer =
            XmlDanmuWriter::with_start_time_and_comments(&output_path, start_time, comments)
                .await?;
        self.events.publish(DanmuEvent::SegmentStarted {
            session_id: self.session_id.clone(),
            streamer_id: self.streamer_id.clone(),
            segment_id: segment_id.clone(),
            output_path,
            start_time,
        });
        self.current_writer = Some((segment_id, writer));

        Ok(())
    }

    /// End a specific segment by ID.
    async fn end_segment(&mut self, target_segment_id: &str) -> Result<()> {
        if let Some((current_id, _)) = &self.current_writer
            && current_id == target_segment_id
        {
            // Flush buffer before finalizing
            self.flush_buffer().await?;
            self.finalize_current_segment().await?;
        }
        Ok(())
    }

    /// Shutdown the runner, flushing and finalizing any active segment.
    ///
    /// Idempotent: `finalize_current_segment` takes the writer, `flush_buffer`
    /// no-ops on an empty buffer, and `disconnect` no-ops once the connection is
    /// gone from the provider's map.
    async fn shutdown(&mut self) -> Vec<Error> {
        let mut errors = Vec::new();

        if let Err(error) = self.flush_buffer().await {
            warn!(
                session_id = %self.session_id,
                %error,
                "danmu: failed to flush the final message buffer"
            );
            errors.push(error);
        }

        if let Err(error) = self.finalize_current_segment().await {
            warn!(
                session_id = %self.session_id,
                %error,
                "danmu: failed to finalize the active segment"
            );
            errors.push(error);
        }

        if let Err(error) = self.provider.disconnect(&mut self.connection).await {
            warn!(
                session_id = %self.session_id,
                %error,
                "danmu: failed to disconnect the collection transport"
            );
            errors.push(error.into());
        }

        errors
    }

    /// Finalize the current segment if one is active.
    async fn finalize_current_segment(&mut self) -> Result<()> {
        if let Some((segment_id, mut writer)) = self.current_writer.take() {
            let count = writer.message_count();
            let path = writer.output_path().to_path_buf();
            writer.finalize().await?;
            self.events.publish(DanmuEvent::SegmentCompleted {
                session_id: self.session_id.clone(),
                streamer_id: self.streamer_id.clone(),
                segment_id,
                output_path: path,
                message_count: count,
            });
        }
        Ok(())
    }

    /// Flush the message buffer if there are messages and a writer is active.
    async fn flush_buffer_if_needed(&mut self) -> Result<()> {
        if self.current_writer.is_some() && !self.message_buffer.is_empty() {
            self.flush_buffer().await?;
        }
        Ok(())
    }

    /// Flush the message buffer to the current writer, sorted by timestamp.
    async fn flush_buffer(&mut self) -> Result<()> {
        if self.message_buffer.is_empty() {
            return Ok(());
        }

        if let Some((_, ref mut writer)) = self.current_writer {
            // Sort messages by timestamp
            self.message_buffer.sort_by_key(|m| m.timestamp);

            // Write all messages
            for message in self.message_buffer.drain(..) {
                writer.write_message(&message).await?;
            }
        }

        Ok(())
    }

    async fn handle_item(&mut self, item: DanmuItem) -> Result<CommandResult> {
        match item {
            DanmuItem::Message(message) => self.handle_message(message).await,
            DanmuItem::Control(control) => self.handle_control(control).await,
        }
    }

    async fn handle_control(&mut self, control: DanmuControlEvent) -> Result<CommandResult> {
        // Control events are not written to XML.
        //
        // The event is emitted before acting on it so the application can react
        // immediately. For `StreamClosed`, `run` finalizes the active segment
        // after the loop ends (emitting `DanmuEvent::SegmentCompleted` if a
        // segment is open); `DanmuEvent::CollectionStopped` is emitted by the
        // service once the runner returns.
        self.events.publish(DanmuEvent::Control {
            session_id: self.session_id.clone(),
            streamer_id: self.streamer_id.clone(),
            platform: self.provider.platform().to_string(),
            control: control.clone(),
        });

        match control {
            DanmuControlEvent::StreamClosed { .. } => {
                Ok(CommandResult::Stop(CollectionExitReason::StreamClosed))
            }
            DanmuControlEvent::RoomInfoChanged { .. } | DanmuControlEvent::Other { .. } => {
                Ok(CommandResult::Continue)
            }
        }
    }

    /// Handle a received danmu message.
    async fn handle_message(&mut self, message: DanmuMessage) -> Result<CommandResult> {
        // Update session-level statistics.
        self.statistics.record_message(&message);

        // Buffer the message (will be written on flush)
        if self.current_writer.is_some() {
            self.message_buffer.push(message);

            // Flush if buffer is full
            if self.message_buffer.len() >= config::MAX_BUFFER_SIZE {
                self.flush_buffer().await?;
            }
        }

        Ok(CommandResult::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::danmu::test_support::{FakeProvider, temp_xml_path};
    use crate::domain::DanmuStatisticsConfig;

    use super::super::lifecycle::CollectionStopReason;

    fn chat(content: &str) -> DanmuItem {
        DanmuItem::Message(DanmuMessage::chat("id", "user-1", "User", content))
    }

    /// Build a runner over `provider`, consuming its first queued stream.
    async fn runner_for(
        provider: Arc<FakeProvider>,
        events: DanmuEventPublisher,
    ) -> (CollectionRunner, mpsc::Receiver<DanmuItem>) {
        CollectionRunner::new(RunnerParams {
            session_id: "session-1".to_string(),
            streamer_id: "streamer-1".to_string(),
            room_id: "room-1".to_string(),
            provider,
            conn_config: ConnectionConfig::default(),
            statistics: StatisticsSession::load(
                "session-1".to_string(),
                None,
                DanmuStatisticsConfig::default(),
            )
            .await,
            events,
        })
        .await
        .expect("first connect succeeds")
    }

    /// While the chat link is down the runner must keep serving segment rotation,
    /// so every video segment still gets its own XML file, closed and announced.
    /// That 1:1 mapping is what `PipelineCoordinator::try_trigger_paired` needs to
    /// pair a segment's video with its danmu.
    #[tokio::test(start_paused = true)]
    async fn link_outage_still_writes_one_xml_per_segment() {
        let (items_tx, items_rx) = mpsc::channel(8);
        // A single stream, so every reconnect fails and the link stays down.
        let provider = Arc::new(FakeProvider::new(vec![items_rx]));
        let event_publisher = DanmuEventPublisher::new(64);
        let mut events = event_publisher.subscribe();
        let (runner, items) = runner_for(provider.clone(), event_publisher).await;

        let (command_tx, command_rx) = mpsc::channel(8);
        let first = temp_xml_path("outage-seg-0");
        let second = temp_xml_path("outage-seg-1");

        command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: "0".to_string(),
                output_path: first.clone(),
                start_time: Utc::now(),
            })
            .await
            .expect("start first segment");
        items_tx.send(chat("hello")).await.expect("send message");
        // Closing the sender is what the provider does after exhausting its own
        // reconnect attempts.
        drop(items_tx);

        let handle = tokio::spawn(runner.run(command_rx, items, CancellationToken::new()));

        // Sit in the outage long enough for several reconnect attempts.
        tokio::time::sleep(Duration::from_secs(config::RECONNECT_ALERT_AFTER_SECS + 60)).await;

        // Rotate to a second segment while the link is still down.
        command_tx
            .send(CollectionCommand::EndSegment {
                segment_id: "0".to_string(),
            })
            .await
            .expect("end first segment");
        command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: "1".to_string(),
                output_path: second.clone(),
                start_time: Utc::now(),
            })
            .await
            .expect("start second segment");
        command_tx
            .send(CollectionCommand::Stop(CollectionStopReason::SessionEnded))
            .await
            .expect("send stop");

        let outcome = handle.await.expect("runner task");
        assert_eq!(outcome.reason, CollectionExitReason::SessionStopped);
        assert!(
            outcome.error.is_none(),
            "an outage is not a run failure: {:?}",
            outcome.error.map(|e| e.to_string())
        );
        assert!(
            provider.connects() > 2,
            "the runner must keep retrying rather than giving up (connects={})",
            provider.connects()
        );

        for (label, path) in [("first", &first), ("second", &second)] {
            let xml = tokio::fs::read_to_string(path)
                .await
                .unwrap_or_else(|e| panic!("{label} segment XML missing: {e}"));
            let _ = tokio::fs::remove_file(path).await;
            assert!(
                xml.trim_end().ends_with("</i>"),
                "{label} segment XML must be closed, got: {xml}"
            );
        }

        let mut completed_segments = Vec::new();
        let mut reconnect_alerts = 0;
        let mut saw_reconnecting = false;
        while let Ok(event) = events.try_recv() {
            match event {
                DanmuEvent::SegmentCompleted { segment_id, .. } => {
                    completed_segments.push(segment_id)
                }
                DanmuEvent::Reconnecting { .. } => saw_reconnecting = true,
                DanmuEvent::ReconnectFailed { .. } => reconnect_alerts += 1,
                _ => {}
            }
        }
        assert_eq!(
            completed_segments,
            vec!["0".to_string(), "1".to_string()],
            "each segment must be announced so it gets a media_outputs row"
        );
        assert!(saw_reconnecting, "reconnect attempts must be observable");
        assert_eq!(
            reconnect_alerts, 1,
            "a sustained outage should alert once, not once per attempt"
        );
    }

    /// Messages already queued when a video segment ends still belong to that
    /// segment. Segment commands have priority in the run loop, so the runner
    /// must drain the item queue before replacing the active writer.
    #[tokio::test(start_paused = true)]
    async fn queued_messages_do_not_cross_segment_boundary() {
        let (items_tx, items_rx) = mpsc::channel(8);
        let provider = Arc::new(FakeProvider::new(vec![items_rx]));
        let (runner, items) = runner_for(provider, DanmuEventPublisher::new(64)).await;

        let (command_tx, command_rx) = mpsc::channel(8);
        let first = temp_xml_path("queued-boundary-0");
        let second = temp_xml_path("queued-boundary-1");
        let first_start = Utc::now();
        let second_start = first_start + chrono::Duration::seconds(5);

        command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: "seg-0".to_string(),
                output_path: first.clone(),
                start_time: first_start,
            })
            .await
            .expect("start first segment");
        items_tx
            .send(chat("queued-before-boundary"))
            .await
            .expect("queue old-segment message");
        command_tx
            .send(CollectionCommand::EndSegment {
                segment_id: "seg-0".to_string(),
            })
            .await
            .expect("end first segment");
        command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: "seg-1".to_string(),
                output_path: second.clone(),
                start_time: second_start,
            })
            .await
            .expect("start second segment");
        command_tx
            .send(CollectionCommand::Stop(CollectionStopReason::SessionEnded))
            .await
            .expect("stop collection");

        let outcome = runner
            .run(command_rx, items, CancellationToken::new())
            .await;
        assert_eq!(outcome.reason, CollectionExitReason::SessionStopped);

        let first_xml = tokio::fs::read_to_string(&first)
            .await
            .expect("read first XML");
        let second_xml = tokio::fs::read_to_string(&second)
            .await
            .expect("read second XML");
        let _ = tokio::fs::remove_file(&first).await;
        let _ = tokio::fs::remove_file(&second).await;

        assert!(
            first_xml.contains("queued-before-boundary"),
            "queued message must remain in the first segment: {first_xml}"
        );
        assert!(
            !second_xml.contains("queued-before-boundary"),
            "queued message leaked into the second segment: {second_xml}"
        );
    }

    /// A `StreamClosed` sitting in the item queue at a segment boundary ends the
    /// session, but the boundary command still has to run: every video segment
    /// needs its own closed XML and `DanmuEvent::SegmentCompleted` for
    /// `PipelineCoordinator::try_trigger_paired` to pair it.
    #[tokio::test(start_paused = true)]
    async fn stream_closed_at_boundary_still_rotates_segment() {
        let (items_tx, items_rx) = mpsc::channel(8);
        let provider = Arc::new(FakeProvider::new(vec![items_rx]));
        let event_publisher = DanmuEventPublisher::new(64);
        let mut events = event_publisher.subscribe();
        let (runner, items) = runner_for(provider, event_publisher).await;

        let (command_tx, command_rx) = mpsc::channel(8);
        let first = temp_xml_path("boundary-closed-0");
        let second = temp_xml_path("boundary-closed-1");
        let first_start = Utc::now();

        command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: "seg-0".to_string(),
                output_path: first.clone(),
                start_time: first_start,
            })
            .await
            .expect("start first segment");
        items_tx
            .send(chat("queued-before-close"))
            .await
            .expect("queue old-segment message");
        items_tx
            .send(DanmuItem::Control(DanmuControlEvent::StreamClosed {
                message: None,
                action: None,
            }))
            .await
            .expect("queue stream close");
        command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: "seg-1".to_string(),
                output_path: second.clone(),
                start_time: first_start + chrono::Duration::seconds(5),
            })
            .await
            .expect("start second segment");

        let outcome = runner
            .run(command_rx, items, CancellationToken::new())
            .await;
        assert_eq!(outcome.reason, CollectionExitReason::StreamClosed);

        let first_xml = tokio::fs::read_to_string(&first)
            .await
            .expect("read first XML");
        let second_xml = tokio::fs::read_to_string(&second)
            .await
            .expect("second segment XML must exist even though the stream closed");
        let _ = tokio::fs::remove_file(&first).await;
        let _ = tokio::fs::remove_file(&second).await;

        assert!(
            first_xml.contains("queued-before-close"),
            "queued message must remain in the first segment: {first_xml}"
        );
        assert!(
            second_xml.trim_end().ends_with("</i>"),
            "second segment XML must be closed: {second_xml}"
        );

        let mut completed_segments = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let DanmuEvent::SegmentCompleted { segment_id, .. } = event {
                completed_segments.push(segment_id);
            }
        }
        assert_eq!(
            completed_segments,
            vec!["seg-0".to_string(), "seg-1".to_string()],
            "each segment must be announced so it gets a media_outputs row"
        );
    }

    /// The unconditional shutdown in `run`: even when the loop ends on an error,
    /// the transport is released and statistics survive for the service to
    /// persist.
    #[tokio::test(start_paused = true)]
    async fn loop_error_still_releases_the_transport() {
        let (items_tx, items_rx) = mpsc::channel(8);
        let provider = Arc::new(FakeProvider::new(vec![items_rx]));
        let (runner, items) = runner_for(provider.clone(), DanmuEventPublisher::new(64)).await;

        let (command_tx, command_rx) = mpsc::channel(8);
        items_tx.send(chat("counted")).await.expect("send message");

        let handle = tokio::spawn(runner.run(command_rx, items, CancellationToken::new()));
        tokio::time::sleep(Duration::from_millis(50)).await;

        // A segment path whose parent is a regular file cannot be created on any
        // platform, so `start_segment` fails and the loop returns the error.
        let blocker = temp_xml_path("loop-error-blocker");
        tokio::fs::write(&blocker, b"not a directory")
            .await
            .expect("write blocker file");
        command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: "0".to_string(),
                output_path: blocker.join("child").join("seg.xml"),
                start_time: Utc::now(),
            })
            .await
            .expect("start segment");

        let outcome = handle.await.expect("runner task");
        let _ = tokio::fs::remove_file(&blocker).await;

        assert!(
            outcome.error.is_some(),
            "the failed segment start must be reported"
        );
        assert_eq!(outcome.reason, CollectionExitReason::Failed);
        assert_eq!(
            outcome.statistics.total_count, 1,
            "statistics must survive so the service can persist them"
        );
        assert!(
            provider.disconnects() >= 1,
            "shutdown must run on the error path too"
        );
    }

    /// A transport that closes and then comes back must keep collecting into the
    /// same segment, with no seam in the statistics.
    #[tokio::test(start_paused = true)]
    async fn reconnect_resumes_collection_into_the_same_segment() {
        let (first_tx, first_rx) = mpsc::channel(8);
        let (second_tx, second_rx) = mpsc::channel(8);
        let provider = Arc::new(FakeProvider::new(vec![first_rx, second_rx]));
        let (runner, items) = runner_for(provider.clone(), DanmuEventPublisher::new(64)).await;

        let (command_tx, command_rx) = mpsc::channel(8);
        let path = temp_xml_path("reconnect");
        command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: "seg-0".to_string(),
                output_path: path.clone(),
                start_time: Utc::now(),
            })
            .await
            .expect("send start segment");

        first_tx.send(chat("before")).await.expect("send before");
        drop(first_tx);

        let handle = tokio::spawn(runner.run(command_rx, items, CancellationToken::new()));

        // Let the runner notice the closed channel, back off, and reconnect onto
        // the second stream. Paused time auto-advances through the back-off.
        tokio::time::sleep(Duration::from_millis(config::RECONNECT_BASE_DELAY_MS * 2)).await;

        second_tx.send(chat("after")).await.expect("send after");
        // Commands are polled before items (`biased`), so `Stop` would preempt an
        // item still sitting in the channel. Yield first so the runner drains it.
        tokio::time::sleep(Duration::from_millis(50)).await;
        command_tx
            .send(CollectionCommand::Stop(CollectionStopReason::SessionEnded))
            .await
            .expect("send stop");

        let outcome = handle.await.expect("runner task");
        assert_eq!(outcome.reason, CollectionExitReason::SessionStopped);
        assert!(
            outcome.error.is_none(),
            "a recovered transport is not a failure: {:?}",
            outcome.error.map(|e| e.to_string())
        );
        assert_eq!(
            outcome.statistics.total_count, 2,
            "messages from both connections must be counted"
        );
        assert_eq!(provider.connects(), 2, "exactly one reconnect expected");
        assert!(
            provider.disconnects() >= 1,
            "the dead connection must be released before reconnecting"
        );

        let xml = tokio::fs::read_to_string(&path).await.expect("read xml");
        let _ = tokio::fs::remove_file(&path).await;
        assert!(xml.contains("before") && xml.contains("after"));
        assert!(xml.trim_end().ends_with("</i>"));
    }

    /// A stop request must not discard messages already queued by the provider.
    ///
    /// Commands are polled before items, so without the drain a `Stop` preempted
    /// everything sitting in the channel — the final messages of the stream, and
    /// they were missing from the last segment's XML too.
    #[tokio::test(start_paused = true)]
    async fn stop_collects_queued_messages_before_finishing() {
        let (items_tx, items_rx) = mpsc::channel(8);
        let provider = Arc::new(FakeProvider::new(vec![items_rx]));
        let (runner, items) = runner_for(provider, DanmuEventPublisher::new(64)).await;

        let (command_tx, command_rx) = mpsc::channel(8);
        let path = temp_xml_path("stop-drain");
        command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: "0".to_string(),
                output_path: path.clone(),
                start_time: Utc::now(),
            })
            .await
            .expect("start segment");

        // Queue messages and the stop together, so the stop is polled first.
        for i in 0..5 {
            items_tx
                .send(chat(&format!("queued-{i}")))
                .await
                .expect("send message");
        }
        command_tx
            .send(CollectionCommand::Stop(CollectionStopReason::SessionEnded))
            .await
            .expect("send stop");

        let outcome = runner
            .run(command_rx, items, CancellationToken::new())
            .await;

        assert!(outcome.error.is_none());
        assert_eq!(
            outcome.statistics.total_count, 5,
            "queued messages must be counted, not dropped"
        );

        let xml = tokio::fs::read_to_string(&path).await.expect("read xml");
        let _ = tokio::fs::remove_file(&path).await;
        for i in 0..5 {
            assert!(
                xml.contains(&format!("queued-{i}")),
                "queued-{i} must reach the segment XML, got: {xml}"
            );
        }
    }

    #[tokio::test]
    async fn cancellation_has_an_explicit_interrupted_outcome() {
        let (_items_tx, items_rx) = mpsc::channel(8);
        let provider = Arc::new(FakeProvider::new(vec![items_rx]));
        let (runner, items) = runner_for(provider, DanmuEventPublisher::new(8)).await;
        let (_command_tx, command_rx) = mpsc::channel(8);
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();

        let outcome = runner.run(command_rx, items, cancel_token).await;

        assert_eq!(outcome.reason, CollectionExitReason::Cancelled);
        assert!(outcome.error.is_none());
    }

    /// A plain stop leaves no error behind and still closes the file.
    #[tokio::test(start_paused = true)]
    async fn stop_command_finalizes_without_error() {
        let (items_tx, items_rx) = mpsc::channel(8);
        let provider = Arc::new(FakeProvider::new(vec![items_rx]));
        let (runner, items) = runner_for(provider, DanmuEventPublisher::new(64)).await;

        let (command_tx, command_rx) = mpsc::channel(8);
        let path = temp_xml_path("stop");
        command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: "seg-0".to_string(),
                output_path: path.clone(),
                start_time: Utc::now(),
            })
            .await
            .expect("send start segment");
        items_tx.send(chat("hi")).await.expect("send message");

        let handle = tokio::spawn(runner.run(command_rx, items, CancellationToken::new()));
        // Commands are polled before items (`biased`), so let the runner drain the
        // message before asking it to stop.
        tokio::time::sleep(Duration::from_millis(50)).await;
        command_tx
            .send(CollectionCommand::Stop(CollectionStopReason::SessionEnded))
            .await
            .expect("send stop");

        let outcome = handle.await.expect("runner task");

        assert_eq!(outcome.reason, CollectionExitReason::SessionStopped);
        assert!(outcome.error.is_none());
        assert_eq!(outcome.statistics.total_count, 1);
        assert!(outcome.statistics.end_time.is_some());

        let xml = tokio::fs::read_to_string(&path).await.expect("read xml");
        let _ = tokio::fs::remove_file(&path).await;
        assert!(xml.trim_end().ends_with("</i>"));
    }
}
