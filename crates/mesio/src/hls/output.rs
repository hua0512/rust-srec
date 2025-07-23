// HLS Output Manager: Manages the final stream of HLSStreamEvents provided to the client.
// For live streams, it handles buffering and reordering of segments.

use crate::hls::config::HlsConfig;
use crate::hls::events::HlsStreamEvent;
use crate::hls::scheduler::ProcessedSegmentOutput;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, mpsc};
use tokio::time::sleep;
use tracing::{debug, error, warn};

use super::HlsDownloaderError;

pub struct OutputManager {
    config: Arc<HlsConfig>,
    input_rx: mpsc::Receiver<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
    event_tx: mpsc::Sender<Result<HlsStreamEvent, HlsDownloaderError>>,
    reorder_buffer: BTreeMap<u64, ProcessedSegmentOutput>,
    is_live_stream: bool,
    expected_next_media_sequence: u64,
    playlist_ended: bool,
    shutdown_rx: broadcast::Receiver<()>,

    gap_detected_waiting_for_sequence: Option<u64>,
    segments_received_since_gap_detected: u64,

    last_input_received_time: Option<Instant>,
}

impl OutputManager {
    pub fn new(
        config: Arc<HlsConfig>,
        input_rx: mpsc::Receiver<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
        event_tx: mpsc::Sender<Result<HlsStreamEvent, HlsDownloaderError>>,
        is_live_stream: bool,
        initial_media_sequence: u64,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Self {
        Self {
            config,
            input_rx,
            event_tx,
            reorder_buffer: BTreeMap::new(),
            is_live_stream,
            expected_next_media_sequence: initial_media_sequence,
            playlist_ended: false,
            shutdown_rx,
            gap_detected_waiting_for_sequence: None,
            segments_received_since_gap_detected: 0,
            last_input_received_time: if is_live_stream {
                Some(Instant::now())
            } else {
                None
            },
        }
    }

    /// Main loop for the OutputManager.
    pub async fn run(&mut self) {
        debug!("is_live_stream: {}", self.is_live_stream);
        let mut global_shutdown_received = false; // Flag to acknowledge shutdown for VOD

        loop {
            // Determine timeout for select! based on live_max_overall_stall_duration
            let overall_stall_timeout = if self.is_live_stream
                && self
                    .config
                    .output_config
                    .live_max_overall_stall_duration
                    .is_some()
            {
                self.config
                    .output_config
                    .live_max_overall_stall_duration
                    .unwrap_or_else(|| std::time::Duration::from_secs(u64::MAX / 2))
            } else {
                // Effectively infinite timeout if not live or not configured
                std::time::Duration::from_secs(u64::MAX / 2) // A very long duration for select!
            };

            tokio::select! {
                biased;

                // Branch 1: Shutdown Signal
                // This arm is active if it's a live stream, OR if it's VOD and shutdown hasn't been acknowledged yet.
                // The `recv()` call consumes the signal. If the arm isn't taken due to `global_shutdown_received == true` for VOD,
                // the signal is effectively ignored for that iteration, allowing input_rx to be processed.
                _ = self.shutdown_rx.recv(), if self.is_live_stream || !global_shutdown_received => {
                    if self.is_live_stream {
                        debug!("Live stream, received shutdown signal. Preparing to exit.");
                        // exit inmediately
                        break;
                    } else {
                        // This is a VOD stream, and `global_shutdown_received` was false to enter this arm.
                        debug!("VOD stream, acknowledging shutdown. Will continue processing input queue until it's empty.");
                        global_shutdown_received = true;
                        // For VOD, we DO NOT break here. The loop continues, prioritizing input_rx.
                        // The primary exit for VOD is when input_rx closes.
                    }
                }

                // Branch 2: Max Overall Stall Timeout (Live streams only)
                _ = sleep(overall_stall_timeout), if self.is_live_stream && self.config.output_config.live_max_overall_stall_duration.is_some() => {
                    if let Some(last_input_time) = self.last_input_received_time {
                        if let Some(max_stall_duration) = self.config.output_config.live_max_overall_stall_duration {
                            if last_input_time.elapsed() >= max_stall_duration {
                                error!(
                                    "Live stream stalled for more than configured max duration ({:?}). No new segments or events received.",
                                    max_stall_duration
                                );
                                let _ = self.event_tx.send(Err(HlsDownloaderError::TimeoutError(
                                    "Stalled: No input received for max duration.".to_string()
                                ))).await;
                                break; // Exit loop for live stream stall
                            }
                        }
                    }
                }

                // Branch 3: Input from SegmentScheduler
                // This is the main processing path. For VOD, this continues even if global_shutdown_received is true.
                processed_result = self.input_rx.recv() => {

                    // Update last_input_received_time for live streams
                    if self.is_live_stream {
                        self.last_input_received_time = Some(Instant::now());
                    }

                    match processed_result {
                        Some(Ok(processed_output)) => {
                            let current_segment_sequence = processed_output.media_sequence_number;


                            debug!(
                                "Adding segment {} (live: {}) to reorder buffer.",
                                current_segment_sequence, self.is_live_stream
                            );

                            // For both live and VOD, add to reorder buffer.
                            // If it's a live stream and we are waiting for a gap, update counter.
                            if self.is_live_stream {
                                if let Some(missing_seq) = self.gap_detected_waiting_for_sequence {
                                    if current_segment_sequence > missing_seq {
                                        self.segments_received_since_gap_detected += 1;
                                        debug!(
                                            "Live stream: Received segment {} while waiting for {}. Segments since gap: {}.",
                                            current_segment_sequence, missing_seq, self.segments_received_since_gap_detected
                                        );
                                    }
                                }
                            }
                            self.reorder_buffer.insert(current_segment_sequence, processed_output);

                            // Attempt to emit segments from the buffer.
                            if self.try_emit_segments().await.is_err() {
                                error!("Error emitting segments from buffer. Exiting.");
                                break;
                            }
                        }
                        Some(Err(e)) => {
                            error!("Error received from input channel: {:?}. Forwarding and exiting.", e);
                            if self.event_tx.send(Err(e)).await.is_err() {
                                error!("Failed to send error event after receiving input error. Exiting.");
                            }
                            break; // Critical error, always break
                        }
                        None => { // input_rx channel closed by SegmentScheduler
                            debug!("input_rx channel closed. Natural end of stream or scheduler termination.");
                            // This is the primary condition for VOD to exit the loop gracefully after processing all segments.
                            // For live streams, this also indicates the end of input.
                            break;
                        }
                    }
                }
            }
        }

        // Post-loop operations: Flush any remaining segments for both live and VOD.
        // For VOD, this ensures all segments are emitted if the input channel closed.
        // For Live, this handles segments remaining after a shutdown signal.
        debug!(
            "Flushing reorder buffer (if any segments remain) for stream (live: {}).",
            self.is_live_stream
        );
        if !self.reorder_buffer.is_empty() {
            if self.flush_reorder_buffer().await.is_err() {
                error!(
                    "Failed to flush reorder buffer post-loop (live: {}). Event sender likely closed.",
                    self.is_live_stream
                );
            }
        } else {
            debug!("Reorder buffer already empty post-loop.");
        }

        debug!("Sending StreamEnded event.");
        if self
            .event_tx
            .send(Ok(HlsStreamEvent::StreamEnded))
            .await
            .is_err()
        {
            error!("Failed to send StreamEnded event after loop completion.");
        }
        debug!("Finished.");
    }

    /// Attempts to emit segments from the reorder buffer.
    /// Handles ordering, discontinuities, and gap skipping (for live streams).
    /// Returns Ok(()) if successful, Err(()) if event_tx is closed.
    async fn try_emit_segments(&mut self) -> Result<(), ()> {
        while let Some(entry) = self.reorder_buffer.first_entry() {
            let segment_sequence = *entry.key();

            if segment_sequence == self.expected_next_media_sequence {
                // Expected segment found
                if let Some((_seq, segment_output)) =
                    self.reorder_buffer.remove_entry(&segment_sequence)
                {
                    if segment_output.discontinuity {
                        debug!("sending discontinuity tag encountered");
                        if self
                            .event_tx
                            .send(Ok(HlsStreamEvent::DiscontinuityTagEncountered {}))
                            .await
                            .is_err()
                        {
                            return Err(());
                        }
                    }
                    let event = HlsStreamEvent::Data(Box::new(segment_output.data));
                    if self.event_tx.send(Ok(event)).await.is_err() {
                        return Err(());
                    }
                    self.expected_next_media_sequence += 1;

                    // Reset gap state as we've successfully emitted the expected segment
                    self.gap_detected_waiting_for_sequence = None;
                    self.segments_received_since_gap_detected = 0;
                } else {
                    // Should not happen if first_entry returned Some
                    break;
                }
            } else if segment_sequence < self.expected_next_media_sequence {
                // Stale segment
                debug!(
                    "Discarding stale segment from reorder buffer: sequence {}",
                    segment_sequence
                );
                self.reorder_buffer.remove(&segment_sequence); // Remove stale entry
            } else {
                // Gap detected (segment_sequence > self.expected_next_media_sequence)
                // If a new gap is identified or the gap we are waiting for has changed:
                if self.gap_detected_waiting_for_sequence.is_none()
                    || self.gap_detected_waiting_for_sequence
                        != Some(self.expected_next_media_sequence)
                {
                    debug!(
                        "New gap detected. Expected: {}, Found: {}. Resetting segments_since_gap count.",
                        self.expected_next_media_sequence, segment_sequence
                    );
                    self.gap_detected_waiting_for_sequence =
                        Some(self.expected_next_media_sequence);
                    self.segments_received_since_gap_detected = 0;
                    // Count already buffered segments that are newer than the missing one
                    for (&seq_in_buffer, _) in self.reorder_buffer.iter() {
                        if seq_in_buffer > self.expected_next_media_sequence {
                            self.segments_received_since_gap_detected += 1;
                        }
                    }
                    debug!(
                        "After re-counting buffered segments, segments_since_gap for expected {}: {}.",
                        self.expected_next_media_sequence,
                        self.segments_received_since_gap_detected
                    );
                }

                // --- Gap Skipping Logic (Live Streams Only) ---
                if self.is_live_stream && self.config.output_config.live_gap_skip_enabled {
                    if self.gap_detected_waiting_for_sequence == Some(self.expected_next_media_sequence) // Ensure we are addressing the current gap
                        && self.segments_received_since_gap_detected >= self.config.output_config.live_gap_skip_threshold_segments
                    {
                        warn!(
                            "Gap detected(Live) for expected segment {}. Received {} subsequent segments (threshold: {}). Skipping to next available segment {}.",
                            self.expected_next_media_sequence,
                            self.segments_received_since_gap_detected,
                            self.config.output_config.live_gap_skip_threshold_segments,
                            segment_sequence
                        );
                        // we don't send discontinuity tag here, because it's not a discontinuity
                        // if self
                        //     .event_tx
                        //     .send(Ok(HlsStreamEvent::DiscontinuityTagEncountered {}))
                        //     .await
                        //     .is_err()
                        // {
                        //     return Err(());
                        // }
                        self.expected_next_media_sequence = segment_sequence; // Jump to the available segment

                        // Reset gap state as we are skipping
                        self.gap_detected_waiting_for_sequence = None;
                        self.segments_received_since_gap_detected = 0;
                        continue; // Attempt to emit the new expected_next_media_sequence
                    } else {
                        // Skip condition not met for live stream, stall and wait
                        debug!(
                            "Gap detected(Live). Expected: {}, Found: {}. Waiting for {} more segments (current: {}, threshold: {}) or the expected segment. Gap skipping enabled but threshold not met or not current gap.",
                            self.expected_next_media_sequence,
                            segment_sequence,
                            self.config
                                .output_config
                                .live_gap_skip_threshold_segments
                                .saturating_sub(self.segments_received_since_gap_detected),
                            self.segments_received_since_gap_detected,
                            self.config.output_config.live_gap_skip_threshold_segments
                        );
                        break; // Stall and wait for the missing segment or more segments to arrive
                    }
                } else {
                    // --- No Gap Skipping (VOD or Live with skipping disabled) ---
                    // If there's a gap (segment_sequence > self.expected_next_media_sequence),
                    // we simply wait for the expected segment.
                    // If live_gap_skip_enabled is false, we also log that.
                    if self.is_live_stream && !self.config.output_config.live_gap_skip_enabled {
                        debug!(
                            // Changed from error to debug as it's a configured state
                            "Gap detected(Live). Expected: {}, Found: {}. Gap skipping disabled. Waiting.",
                            self.expected_next_media_sequence, segment_sequence
                        );
                    } else if !self.is_live_stream {
                        debug!(
                            "Gap detected(VOD). Expected: {}, Found: {}. Waiting for expected segment.",
                            self.expected_next_media_sequence, segment_sequence
                        );
                    }
                    // For VOD, or if live and gap skipping is disabled, we must wait.
                    break;
                }
            }
        }
        // Pruning is primarily for live streams to manage buffer size.
        if self.is_live_stream {
            self.prune_reorder_buffer();
        }
        Ok(())
    }

    // Prunes the reorder buffer based on configuration (duration/max_segments).
    fn prune_reorder_buffer(&mut self) {
        if !self.is_live_stream {
            return;
        }

        let max_segments = self.config.output_config.live_reorder_buffer_max_segments;
        if self.reorder_buffer.len() > max_segments {
            let num_to_remove = self.reorder_buffer.len() - max_segments;

            let keys_to_remove: Vec<u64> = self
                .reorder_buffer
                .keys()
                .filter(|&&key| key < self.expected_next_media_sequence) // Only consider segments older than expected
                .take(num_to_remove) // Take the oldest ones
                .cloned()
                .collect();

            for key_to_remove in keys_to_remove {
                if self.reorder_buffer.remove(&key_to_remove).is_some() {
                    debug!(
                        "Pruning segment {} by count (max_segments: {})",
                        key_to_remove, max_segments
                    );
                }
            }
        }

        let max_buffer_duration_secs = self
            .config
            .output_config
            .live_reorder_buffer_duration
            .as_secs_f32();

        // Only proceed with duration pruning if a positive max duration is set.
        if max_buffer_duration_secs > 0.0_f32 {
            let mut old_segments_info: Vec<(u64, f32)> = Vec::new();
            let mut current_total_old_duration = 0.0_f32;

            // Collect information about segments older than the expected next one.
            // BTreeMap iterates keys in ascending order, so this processes from oldest.
            for (&sequence_number, segment_output) in &self.reorder_buffer {
                if sequence_number < self.expected_next_media_sequence {
                    let duration = segment_output
                        .data
                        .media_segment()
                        .map_or(0.0, |ms| ms.duration); // Use 0.0 for non-media or no duration
                    old_segments_info.push((sequence_number, duration));
                    current_total_old_duration += duration;
                } else {
                    // Since BTreeMap is sorted, no further segments will be older.
                    break;
                }
            }

            if current_total_old_duration > max_buffer_duration_secs {
                let mut duration_to_shed = current_total_old_duration - max_buffer_duration_secs;

                // Iterate through the collected old segments (already sorted oldest first)
                // to prune those that fit the criteria.
                for (key_to_prune, seg_duration) in old_segments_info {
                    if duration_to_shed <= 0.0 {
                        // We've shed enough duration or gone below the target.
                        break;
                    }

                    // Only consider pruning segments that have a positive duration.
                    if seg_duration > 0.0 {
                        // Adhere to the rule: prune if segment's duration is less than or equal to
                        // the remaining duration we need to shed.
                        if seg_duration <= duration_to_shed {
                            // Check if the segment still exists in the reorder_buffer,
                            // as count-based pruning might have already removed it.
                            // If remove is successful, log and update duration_to_shed.
                            let initial_shed_needed_for_this_segment_check = duration_to_shed;
                            if self.reorder_buffer.remove(&key_to_prune).is_some() {
                                debug!(
                                    "Pruning segment {} ({}s) by duration. Need to shed {:.2}s, segment fits. Max buffer duration: {:.2}s.",
                                    key_to_prune,
                                    seg_duration,
                                    initial_shed_needed_for_this_segment_check,
                                    max_buffer_duration_secs
                                );
                                duration_to_shed -= seg_duration;
                            }
                        }
                        // If seg_duration > duration_to_shed, this segment is "too large" to remove
                        // under the current rule. We skip it and check the next segment in old_segments_info.
                    }
                    // Segments with 0.0 duration are not pruned by this duration-based logic,
                    // as per the instruction "The original proposal implies pruning segments with duration."
                }
            }
        }
        // If max_buffer_duration_secs is not positive, duration pruning is skipped.
    }

    /// Flushes remaining segments from the reorder buffer.
    /// Returns Ok(()) if successful, Err(()) if event_tx is closed.
    async fn flush_reorder_buffer(&mut self) -> Result<(), ()> {
        // Removes and returns the first element (smallest key),
        while let Some((_key, segment_output)) = self.reorder_buffer.pop_first() {
            if segment_output.discontinuity {
                debug!("sending discontinuity tag encountered in flush_reorder_buffer");
                if self
                    .event_tx
                    .send(Ok(HlsStreamEvent::DiscontinuityTagEncountered {}))
                    .await
                    .is_err()
                {
                    // If sending fails, return the error.
                    // The event_tx channel is likely closed.
                    return Err(());
                }
            }
            let event = HlsStreamEvent::Data(Box::new(segment_output.data));
            if self.event_tx.send(Ok(event)).await.is_err() {
                // If sending fails, return the error.
                return Err(());
            }
        }
        Ok(())
    }

    /// Called when the playlist is known to have ended (e.g., ENDLIST tag or VOD completion).
    /// This is also called by the run loop's shutdown path.
    pub async fn signal_stream_end_and_flush(&mut self) {
        debug!(
            "start to signal end, is_live_stream: {}",
            self.is_live_stream
        );
        if self.is_live_stream || !self.reorder_buffer.is_empty() {
            debug!("Flushing reorder buffer.");
            // Also flush for VOD if somehow buffered
            if self.flush_reorder_buffer().await.is_err() {
                error!("Failed to flush reorder buffer, event_tx likely closed.");
                // event_tx closed, can't send StreamEnded either
                return;
            }
            debug!("Reorder buffer flushed.");
        } else {
            debug!("No flush needed (not live or buffer empty).");
        }
        self.playlist_ended = true; // Mark as ended
        // The main run loop will send StreamEnded upon exiting.
        debug!("Stream end signaled, buffer flushed (if applicable).");
    }

    // Method to update live status and expected sequence, perhaps called by coordinator during init
    pub fn update_stream_state(&mut self, is_live: bool, initial_sequence: u64) {
        self.is_live_stream = is_live;
        self.expected_next_media_sequence = initial_sequence;
        self.playlist_ended = false; // Reset if re-initializing
        self.reorder_buffer.clear();
        // Reset new state fields as well
        self.gap_detected_waiting_for_sequence = None;
        self.segments_received_since_gap_detected = 0;
        self.last_input_received_time = if is_live { Some(Instant::now()) } else { None };
    }
}
