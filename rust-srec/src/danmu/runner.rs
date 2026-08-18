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
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use platforms_parser::danmaku::{
    ConnectionConfig, DanmuConnection, DanmuControlEvent, DanmuItem, DanmuProvider,
    message::DanmuMessage,
};

use crate::danmu::{DanmuStatistics, StatisticsAggregator, XmlDanmuWriter};
use crate::database::repositories::SessionRepository;
use crate::error::Result;

use super::checkpoint;
use super::events::{CollectionCommand, DanmuEvent};
use super::service::persist_statistics;

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
    Stop,
}

/// Final state of a collection run.
///
/// `statistics` is always populated, including when `error` is `Some`: the
/// aggregator's contents are just as final after a transport failure as after a
/// clean stop, and `DanmuService` persists them either way.
pub(crate) struct CollectionOutcome {
    pub statistics: DanmuStatistics,
    /// `Some` when the loop stopped because of a failure rather than a stop
    /// request or cancellation.
    pub error: Option<crate::error::Error>,
    /// Whether the session itself is over, as opposed to collection being
    /// interrupted by a process shutdown. Decides whether the aggregator
    /// checkpoint is kept for a resume or discarded.
    pub session_ended: bool,
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

    // Stats state
    stats: StatisticsAggregator,
    /// Repository for periodic in-progress statistics persists.
    session_repo: Option<Arc<dyn SessionRepository>>,
    /// Total count at the last periodic persist; skips redundant writes when
    /// no message arrived during the persist interval.
    last_persisted_total: u64,
    /// Total count at the last aggregator checkpoint, for the same reason.
    last_checkpoint_total: u64,

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

    event_tx: broadcast::Sender<DanmuEvent>,
}

/// Parameters for creating a new collection runner.
pub(crate) struct RunnerParams {
    pub session_id: String,
    pub streamer_id: String,
    pub room_id: String,
    pub provider: Arc<dyn DanmuProvider>,
    pub conn_config: ConnectionConfig,
    pub stats: StatisticsAggregator,
    pub session_repo: Option<Arc<dyn SessionRepository>>,
    pub event_tx: broadcast::Sender<DanmuEvent>,
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
            stats,
            session_repo,
            event_tx,
        } = params;
        // Read before `stats` is moved into the runner.
        let resumed_total = stats.total_count();

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
                stats,
                session_repo,
                last_persisted_total: 0,
                // Seeded from the resumed count so an immediately-restarted
                // collector does not rewrite an identical checkpoint.
                last_checkpoint_total: resumed_total,
                reconnect_attempts: 0,
                link_down_since: None,
                outage_alerted: false,
                event_tx,
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
        let loop_result = self.run_loop(&mut command_rx, items, &cancel_token).await;

        if let Err(error) = self.shutdown().await {
            warn!(
                session_id = %self.session_id,
                %error,
                "danmu: failed to finalize collection cleanly"
            );
        }

        // A cancelled token means `DanmuService::shutdown` is tearing the process
        // down — it cancels before sending `Stop` — so the session is not over and
        // its checkpoint must survive for the next start to resume from. Anything
        // else (a stop request from session end, `StreamClosed`, or a failure)
        // means this session is finished.
        let session_ended = !cancel_token.is_cancelled();
        if !session_ended {
            // Take a final checkpoint so a shutdown costs nothing rather than up
            // to one checkpoint interval.
            checkpoint::save(
                self.session_repo.as_ref(),
                &self.session_id,
                &self.stats.export_state(),
            )
            .await;
        }

        CollectionOutcome {
            // Consume the aggregator so the statistics carry end_time and
            // duration_secs; the service persists this exact value.
            statistics: self.stats.finalize(Utc::now()),
            error: loop_result.err(),
            session_ended,
        }
    }

    async fn run_loop(
        &mut self,
        command_rx: &mut mpsc::Receiver<CollectionCommand>,
        items: mpsc::Receiver<DanmuItem>,
        cancel_token: &CancellationToken,
    ) -> Result<()> {
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
                LoopEvent::Command(cmd) => match self.handle_command(cmd).await? {
                    CommandResult::Continue => {}
                    CommandResult::Stop => return Ok(()),
                },
                LoopEvent::Cancelled => return Ok(()),
                LoopEvent::Flush => self.flush_buffer_if_needed().await?,
                LoopEvent::Persist => self.persist_stats_if_changed().await,
                LoopEvent::Checkpoint => self.checkpoint_if_changed().await,
                LoopEvent::Item(item) => {
                    // An arriving item is the only proof the link works, so it —
                    // not a successful `connect` — is what ends an outage.
                    self.note_link_recovered();
                    match self.handle_item(item).await? {
                        CommandResult::Continue => {}
                        CommandResult::Stop => return Ok(()),
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
    /// Returns `Err` once `config::MAX_RECONNECT_ATTEMPTS` consecutive attempts
    /// have passed without an item, which ends the run — `run` still finalizes
    /// the segment and the service still persists statistics and emits
    /// `DanmuEvent::CollectionStopped`.
    fn schedule_retry(&mut self, cause: &str) -> Link {
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
        let attempt = self.reconnect_attempts;
        let down_since = *self.link_down_since.get_or_insert_with(Instant::now);
        let downtime = down_since.elapsed();

        let delay = Self::reconnect_delay(attempt);
        let _ = self.event_tx.send(DanmuEvent::Reconnecting {
            session_id: self.session_id.clone(),
            attempt,
        });

        // One alert per outage, raised once it has lasted long enough to be worth
        // an operator's attention. Reconnecting continues either way.
        if !self.outage_alerted
            && downtime >= Duration::from_secs(config::RECONNECT_ALERT_AFTER_SECS)
        {
            self.outage_alerted = true;
            let _ = self.event_tx.send(DanmuEvent::ReconnectFailed {
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
            let _ = self.event_tx.send(DanmuEvent::Reconnected {
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

    /// Persist an in-progress statistics snapshot when new messages arrived
    /// since the last persist. Uses `persist_statistics`, the same upsert path
    /// the service uses for final statistics on stop.
    async fn persist_stats_if_changed(&mut self) {
        let Some(repo) = &self.session_repo else {
            return;
        };
        let snapshot = self.stats.current_stats();
        if snapshot.total_count == self.last_persisted_total {
            return;
        }
        persist_statistics(Some(repo.as_ref()), &self.session_id, &snapshot).await;
        self.last_persisted_total = snapshot.total_count;
    }

    /// Store an aggregator checkpoint when new messages arrived since the last
    /// one, so a restart resumes counting instead of starting over.
    async fn checkpoint_if_changed(&mut self) {
        if self.session_repo.is_none() || self.stats.total_count() == self.last_checkpoint_total {
            return;
        }
        let state = self.stats.export_state();
        checkpoint::save(self.session_repo.as_ref(), &self.session_id, &state).await;
        self.last_checkpoint_total = self.stats.total_count();
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
            Some(CollectionCommand::Stop) | None => Ok(CommandResult::Stop),
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
        let _ = self.event_tx.send(DanmuEvent::SegmentStarted {
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
    async fn shutdown(&mut self) -> Result<()> {
        self.flush_buffer().await?;
        self.finalize_current_segment().await?;
        self.provider.disconnect(&mut self.connection).await?;
        Ok(())
    }

    /// Finalize the current segment if one is active.
    async fn finalize_current_segment(&mut self) -> Result<()> {
        if let Some((segment_id, mut writer)) = self.current_writer.take() {
            let count = writer.message_count();
            let path = writer.output_path().to_path_buf();
            writer.finalize().await?;
            let _ = self.event_tx.send(DanmuEvent::SegmentCompleted {
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
        let _ = self.event_tx.send(DanmuEvent::Control {
            session_id: self.session_id.clone(),
            streamer_id: self.streamer_id.clone(),
            platform: self.provider.platform().to_string(),
            control: control.clone(),
        });

        match control {
            DanmuControlEvent::StreamClosed { .. } => Ok(CommandResult::Stop),
            DanmuControlEvent::RoomInfoChanged { .. } | DanmuControlEvent::Other { .. } => {
                Ok(CommandResult::Continue)
            }
        }
    }

    /// Handle a received danmu message.
    async fn handle_message(&mut self, message: DanmuMessage) -> Result<CommandResult> {
        // Update session-level statistics.
        self.stats.record_message(&message);

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
    use platforms_parser::danmaku::StatisticsAggregator;

    fn chat(content: &str) -> DanmuItem {
        DanmuItem::Message(DanmuMessage::chat("id", "user-1", "User", content))
    }

    /// Build a runner over `provider`, consuming its first queued stream.
    async fn runner_for(
        provider: Arc<FakeProvider>,
        event_tx: broadcast::Sender<DanmuEvent>,
    ) -> (CollectionRunner, mpsc::Receiver<DanmuItem>) {
        CollectionRunner::new(RunnerParams {
            session_id: "session-1".to_string(),
            streamer_id: "streamer-1".to_string(),
            room_id: "room-1".to_string(),
            provider,
            conn_config: ConnectionConfig::default(),
            stats: StatisticsAggregator::new(),
            session_repo: None,
            event_tx,
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
        let (event_tx, mut events) = broadcast::channel(64);
        let (runner, items) = runner_for(provider.clone(), event_tx).await;

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
            .send(CollectionCommand::Stop)
            .await
            .expect("send stop");

        let outcome = handle.await.expect("runner task");
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

    /// The unconditional shutdown in `run`: even when the loop ends on an error,
    /// the transport is released and statistics survive for the service to
    /// persist.
    #[tokio::test(start_paused = true)]
    async fn loop_error_still_releases_the_transport() {
        let (items_tx, items_rx) = mpsc::channel(8);
        let provider = Arc::new(FakeProvider::new(vec![items_rx]));
        let (event_tx, _events) = broadcast::channel(64);
        let (runner, items) = runner_for(provider.clone(), event_tx).await;

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
        let (event_tx, _events) = broadcast::channel(64);
        let (runner, items) = runner_for(provider.clone(), event_tx).await;

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
            .send(CollectionCommand::Stop)
            .await
            .expect("send stop");

        let outcome = handle.await.expect("runner task");
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

    /// A plain stop leaves no error behind and still closes the file.
    #[tokio::test(start_paused = true)]
    async fn stop_command_finalizes_without_error() {
        let (items_tx, items_rx) = mpsc::channel(8);
        let provider = Arc::new(FakeProvider::new(vec![items_rx]));
        let (event_tx, _events) = broadcast::channel(64);
        let (runner, items) = runner_for(provider, event_tx).await;

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
            .send(CollectionCommand::Stop)
            .await
            .expect("send stop");

        let outcome = handle.await.expect("runner task");

        assert!(outcome.error.is_none());
        assert_eq!(outcome.statistics.total_count, 1);
        assert!(outcome.statistics.end_time.is_some());

        let xml = tokio::fs::read_to_string(&path).await.expect("read xml");
        let _ = tokio::fs::remove_file(&path).await;
        assert!(xml.trim_end().ends_with("</i>"));
    }
}
