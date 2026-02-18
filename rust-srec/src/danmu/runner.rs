//! Collection runner for danmu collection sessions.
//!
//! This module provides a state machine for running danmu collection with:
//! - Message buffering and sorting
//! - Segment-based file writing
//! - Automatic reconnection
//! - Periodic buffer flushing

use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use platforms_parser::danmaku::{
    ConnectionConfig, DanmuConnection, DanmuControlEvent, DanmuItem, DanmuProvider,
    message::{DanmuMessage, DanmuType},
};

use crate::danmu::{DanmuSampler, DanmuStatistics, StatisticsAggregator, XmlDanmuWriter};
use crate::error::{Error, Result};

use super::events::{CollectionCommand, DanmuEvent};

/// Configuration constants for the collection runner.
mod config {
    /// Buffer flush interval in milliseconds.
    pub const BUFFER_FLUSH_INTERVAL_MS: u64 = 500;
    /// Maximum number of messages to buffer before forcing a flush.
    pub const MAX_BUFFER_SIZE: usize = 100;
}

/// Result of command handling - indicates whether to continue or stop.
#[derive(Debug, PartialEq)]
pub(crate) enum CommandResult {
    /// Continue running the collection loop.
    Continue,
    /// Stop the collection loop.
    Stop,
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

    // Current segment writer
    current_writer: Option<(String, XmlDanmuWriter)>,

    // Message buffer for sorting before writing
    message_buffer: Vec<DanmuMessage>,

    // Stats state
    stats: StatisticsAggregator,
    sampler: Box<dyn DanmuSampler>,
    sampling_enabled: bool,

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
    pub sampler: Box<dyn DanmuSampler>,
    pub sampling_enabled: bool,
    pub event_tx: broadcast::Sender<DanmuEvent>,
}

impl CollectionRunner {
    /// Create a new collection runner.
    pub async fn new(params: RunnerParams) -> Result<Self> {
        let RunnerParams {
            session_id,
            streamer_id,
            room_id,
            provider,
            conn_config,
            stats,
            sampler,
            sampling_enabled,
            event_tx,
        } = params;
        // Connect to danmu stream
        let connection = provider.connect(&room_id, conn_config.clone()).await?;

        Ok(Self {
            session_id,
            streamer_id,
            room_id,
            provider,
            connection,
            current_writer: None,
            message_buffer: Vec::with_capacity(config::MAX_BUFFER_SIZE),
            stats,
            sampler,
            sampling_enabled,
            event_tx,
        })
    }

    /// Run the collection loop until stopped or cancelled.
    pub async fn run(
        mut self,
        mut command_rx: mpsc::Receiver<CollectionCommand>,
        cancel_token: CancellationToken,
    ) -> Result<DanmuStatistics> {
        let mut flush_interval = tokio::time::interval(tokio::time::Duration::from_millis(
            config::BUFFER_FLUSH_INTERVAL_MS,
        ));
        flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;

                // Handle commands (highest priority)
                cmd = command_rx.recv() => {
                    match self.handle_command(cmd).await? {
                        CommandResult::Continue => {}
                        CommandResult::Stop => break,
                    }
                }

                // Handle cancellation
                _ = cancel_token.cancelled() => {
                    self.shutdown().await?;
                    break;
                }

                // Periodic buffer flush
                _ = flush_interval.tick() => {
                    self.flush_buffer_if_needed().await?;
                }

                // Receive danmu messages
                result = self.provider.receive(&self.connection) => {
                    match self.handle_receive_result(result).await? {
                        CommandResult::Continue => {}
                        CommandResult::Stop => break,
                    }
                }
            }
        }

        Ok(self.stats.current_stats())
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
            Some(CollectionCommand::Stop) | None => {
                self.shutdown().await?;
                Ok(CommandResult::Stop)
            }
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
    async fn shutdown(&mut self) -> Result<()> {
        self.flush_buffer().await?;
        self.finalize_current_segment().await?;
        let _ = self.provider.disconnect(&mut self.connection).await;
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

    /// Handle the result of receiving a message from the provider.
    async fn handle_receive_result(
        &mut self,
        result: platforms_parser::danmaku::error::Result<Option<DanmuItem>>,
    ) -> Result<CommandResult> {
        match result {
            Ok(Some(item)) => return self.handle_item(item).await,
            Ok(None) => {
                // No message available, wait a bit
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
            Err(e) => {
                // Log the error - reconnection is handled by the transport layer
                let _ = self.event_tx.send(DanmuEvent::Error {
                    session_id: self.session_id.clone(),
                    error: e.to_string(),
                });
                // Propagate the error to stop collection if transport layer can't recover
                return Err(Error::DanmakuError(e));
            }
        }
        Ok(CommandResult::Continue)
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
        // For `StreamClosed`, we emit the control event first (so the application can react
        // immediately), then shut down gracefully. Shutdown finalizes the active segment (emitting
        // `DanmuEvent::SegmentCompleted` if a segment is open); `DanmuEvent::CollectionStopped` is
        // emitted by the service after the runner exits.
        let _ = self.event_tx.send(DanmuEvent::Control {
            session_id: self.session_id.clone(),
            streamer_id: self.streamer_id.clone(),
            platform: self.provider.platform().to_string(),
            control: control.clone(),
        });

        match control {
            DanmuControlEvent::StreamClosed { .. } => {
                self.shutdown().await?;
                Ok(CommandResult::Stop)
            }
            DanmuControlEvent::RoomInfoChanged { .. } | DanmuControlEvent::Other { .. } => {
                Ok(CommandResult::Continue)
            }
        }
    }

    /// Handle a received danmu message.
    async fn handle_message(&mut self, message: DanmuMessage) -> Result<CommandResult> {
        // Update session-level statistics.
        let is_gift = matches!(message.message_type, DanmuType::Gift | DanmuType::SuperChat);
        self.stats.record_message(
            &message.user_id,
            &message.username,
            &message.content,
            is_gift,
            message.timestamp,
        );

        if self.sampling_enabled {
            // Update sampler (best-effort; used only when sampling is enabled)
            self.sampler.record_message(message.timestamp);
        }

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
