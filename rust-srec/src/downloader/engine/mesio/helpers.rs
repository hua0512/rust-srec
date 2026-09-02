//! Shared helpers for FLV and HLS download orchestrators.

use std::fmt::Display;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, Utc};
use futures::StreamExt;
use pipeline_common::{
    PipelineError, PipelineSender, SplitReason, WriterError, WriterProgress, WriterStats,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::downloader::engine::traits::{
    DownloadFailureKind, DownloadProgress, SegmentEvent, SegmentInfo,
};
use crate::downloader::engine::utils::observe_segment_event_send;

// ---------------------------------------------------------------------------
// DownloadStats (moved from hls_downloader)
// ---------------------------------------------------------------------------

/// Statistics returned after download completes.
#[derive(Debug, Clone, Default)]
pub struct DownloadStats {
    /// Total bytes written across all files.
    pub total_bytes: u64,
    /// Total items (segments/tags) written.
    pub total_items: usize,
    /// Total media duration in seconds.
    pub total_duration_secs: f64,
    /// Number of files created.
    pub files_created: u32,
}

// ---------------------------------------------------------------------------
// WriterWithCallbacks trait + forwarding impls
// ---------------------------------------------------------------------------

/// Trait that bridges the concrete callback-setter methods on `FlvWriter` and
/// `HlsWriter` so that `setup_writer_callbacks` can operate generically.
pub(super) trait WriterWithCallbacks {
    fn set_on_segment_start_callback<F>(&mut self, cb: F)
    where
        F: Fn(&Path, u32) + Send + Sync + 'static;

    fn set_on_segment_complete_callback<F>(&mut self, cb: F)
    where
        F: Fn(&Path, u32, f64, u64, Option<&SplitReason>) + Send + Sync + 'static;

    fn set_progress_callback<F>(&mut self, cb: F)
    where
        F: Fn(WriterProgress) + Send + Sync + 'static;
}

impl WriterWithCallbacks for flv_fix::FlvWriter {
    fn set_on_segment_start_callback<F>(&mut self, cb: F)
    where
        F: Fn(&Path, u32) + Send + Sync + 'static,
    {
        flv_fix::FlvWriter::set_on_segment_start_callback(self, cb);
    }

    fn set_on_segment_complete_callback<F>(&mut self, cb: F)
    where
        F: Fn(&Path, u32, f64, u64, Option<&SplitReason>) + Send + Sync + 'static,
    {
        flv_fix::FlvWriter::set_on_segment_complete_callback(self, cb);
    }

    fn set_progress_callback<F>(&mut self, cb: F)
    where
        F: Fn(WriterProgress) + Send + Sync + 'static,
    {
        flv_fix::FlvWriter::set_progress_callback(self, cb);
    }
}

impl WriterWithCallbacks for hls_fix::HlsWriter {
    fn set_on_segment_start_callback<F>(&mut self, cb: F)
    where
        F: Fn(&Path, u32) + Send + Sync + 'static,
    {
        hls_fix::HlsWriter::set_on_segment_start_callback(self, cb);
    }

    fn set_on_segment_complete_callback<F>(&mut self, cb: F)
    where
        F: Fn(&Path, u32, f64, u64, Option<&SplitReason>) + Send + Sync + 'static,
    {
        hls_fix::HlsWriter::set_on_segment_complete_callback(self, cb);
    }

    fn set_progress_callback<F>(&mut self, cb: F)
    where
        F: Fn(WriterProgress) + Send + Sync + 'static,
    {
        hls_fix::HlsWriter::set_progress_callback(self, cb);
    }
}

// ---------------------------------------------------------------------------
// setup_writer_callbacks
// ---------------------------------------------------------------------------

/// Wire up segment-start, segment-complete, and progress callbacks on the
/// writer.  Replaces 4 identical ~40-line blocks.
///
/// Callbacks run on a blocking thread; `blocking_send` applies backpressure
/// rather than unbounded buffering.
pub(super) fn setup_writer_callbacks(
    writer: &mut impl WriterWithCallbacks,
    event_tx: &mpsc::Sender<SegmentEvent>,
) {
    let event_tx_start = event_tx.clone();
    let event_tx_complete = event_tx.clone();
    let event_tx_progress = event_tx.clone();

    // Segments are strictly sequential (start N, complete N, start N+1, …),
    // so a single atomic timestamp is enough to pass started_at from the start
    // callback to the complete callback without locking.
    let started_at_ms = Arc::new(AtomicI64::new(0));
    let started_at_writer = Arc::clone(&started_at_ms);
    let started_at_reader = started_at_ms;

    writer.set_on_segment_start_callback(move |path, sequence| {
        let started_at = Utc::now();
        started_at_writer.store(started_at.timestamp_millis(), Ordering::Release);
        let event = SegmentEvent::SegmentStarted {
            path: path.to_path_buf(),
            sequence,
            started_at,
        };
        if let Err(error) = event_tx_start.blocking_send(event) {
            debug!(%error, "segment-start event receiver closed");
        }
    });

    writer.set_on_segment_complete_callback(
        move |path, sequence, duration_secs, size_bytes, split_reason| {
            let event_path = path.to_path_buf();

            let ms = started_at_reader.swap(0, Ordering::Acquire);
            let started_at = if ms != 0 {
                DateTime::from_timestamp_millis(ms)
            } else {
                None
            };

            let (split_reason_code, split_reason_details_json) = if let Some(reason) = split_reason
            {
                (
                    Some(split_reason_code(reason).to_string()),
                    split_reason_details_json(reason),
                )
            } else {
                (None, None)
            };
            let event = SegmentEvent::SegmentCompleted(SegmentInfo {
                path: event_path,
                duration_secs,
                size_bytes,
                index: sequence,
                started_at,
                completed_at: Utc::now(),
                split_reason_code,
                split_reason_details_json,
            });
            if let Err(error) = event_tx_complete.blocking_send(event) {
                debug!(%error, "segment-complete event receiver closed");
            }
        },
    );

    writer.set_progress_callback(move |progress| {
        let download_progress = DownloadProgress {
            bytes_downloaded: progress.bytes_written_total,
            duration_secs: progress.elapsed_secs,
            speed_bytes_per_sec: progress.speed_bytes_per_sec,
            segments_completed: progress.current_file_sequence,
            current_segment: None,
            media_duration_secs: progress.media_duration_secs_total,
            playback_ratio: progress.playback_ratio,
        };
        match event_tx_progress.try_send(SegmentEvent::Progress(download_progress)) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(error @ mpsc::error::TrySendError::Closed(_)) => {
                debug!(%error, "progress event receiver closed");
            }
        }
    });
}

fn split_reason_code(reason: &SplitReason) -> &'static str {
    match reason {
        SplitReason::VideoCodecChange { .. } => "video_codec_change",
        SplitReason::AudioCodecChange { .. } => "audio_codec_change",
        SplitReason::SizeLimit => "size_limit",
        SplitReason::DurationLimit => "duration_limit",
        SplitReason::HeaderReceived => "header_received",
        SplitReason::ResolutionChange { .. } => "resolution_change",
        SplitReason::StreamStructureChange { .. } => "stream_structure_change",
        SplitReason::Discontinuity => "discontinuity",
        SplitReason::EndOfStream => "end_of_stream",
    }
}

fn split_reason_details_json(reason: &SplitReason) -> Option<String> {
    let details = match reason {
        SplitReason::VideoCodecChange { from, to } => serde_json::json!({
            "from": {
                "codec": from.codec.clone(),
                "profile": from.profile,
                "level": from.level,
                "width": from.width,
                "height": from.height,
                "signature": from.signature,
            },
            "to": {
                "codec": to.codec.clone(),
                "profile": to.profile,
                "level": to.level,
                "width": to.width,
                "height": to.height,
                "signature": to.signature,
            },
        }),
        SplitReason::AudioCodecChange { from, to } => serde_json::json!({
            "from": {
                "codec": from.codec.clone(),
                "sample_rate": from.sample_rate,
                "channels": from.channels,
                "signature": from.signature,
            },
            "to": {
                "codec": to.codec.clone(),
                "sample_rate": to.sample_rate,
                "channels": to.channels,
                "signature": to.signature,
            },
        }),
        SplitReason::ResolutionChange { from, to } => serde_json::json!({
            "from": { "width": from.0, "height": from.1 },
            "to": { "width": to.0, "height": to.1 },
        }),
        SplitReason::StreamStructureChange { description } => {
            serde_json::json!({ "description": description })
        }
        SplitReason::SizeLimit
        | SplitReason::DurationLimit
        | SplitReason::HeaderReceived
        | SplitReason::Discontinuity
        | SplitReason::EndOfStream => return None,
    };

    serde_json::to_string(&details).ok()
}

// ---------------------------------------------------------------------------
// consume_stream
// ---------------------------------------------------------------------------

pub(super) trait StreamSender<T> {
    fn send_item(
        &self,
        item: std::result::Result<T, PipelineError>,
    ) -> impl Future<Output = Result<(), ()>> + Send;
}

impl<T: Send> StreamSender<T> for mpsc::Sender<std::result::Result<T, PipelineError>> {
    async fn send_item(&self, item: std::result::Result<T, PipelineError>) -> Result<(), ()> {
        self.send(item).await.map_err(|_| ())
    }
}

impl<T: Send> StreamSender<T> for PipelineSender<T> {
    async fn send_item(&self, item: std::result::Result<T, PipelineError>) -> Result<(), ()> {
        self.send(item).await.map_err(|_| ())
    }
}

pub(super) struct StreamConsumeContext<'a> {
    pub parent_token: &'a CancellationToken,
    pub child_token: &'a CancellationToken,
    pub streamer_id: &'a str,
    pub protocol: &'a str,
}

/// Consume a protocol stream, forwarding items to a channel.
///
/// Returns `Some((kind, message))` if the stream yielded an error, or `None`
/// if it completed cleanly (or was cancelled).
///
/// `inspect` is invoked on every successful item before forwarding so callers
/// can tap protocol-specific signals (e.g. HLS observing
/// `HlsData::EndMarker(Some(SplitReason::EndOfStream))` to surface
/// `EngineEndSignal::HlsEndlist`). FLV callers pass a no-op closure.
pub(super) async fn consume_stream<T: Send, E: Display>(
    stream: impl futures::Stream<Item = std::result::Result<T, E>> + Unpin,
    tx: &impl StreamSender<T>,
    context: StreamConsumeContext<'_>,
    classify: impl Fn(&E) -> DownloadFailureKind,
    mut inspect: impl FnMut(&T),
) -> Option<(DownloadFailureKind, String)> {
    let mut stream = std::pin::pin!(stream);
    let mut stream_error: Option<(DownloadFailureKind, String)> = None;
    // One wait-list registration per token for the whole stream. A fresh
    // `cancelled()` future inside each `select!` would register and
    // deregister on every item, once to receive it and once to forward it.
    let mut parent_cancelled = std::pin::pin!(context.parent_token.cancelled());
    let mut child_cancelled = std::pin::pin!(context.child_token.cancelled());

    loop {
        let result = tokio::select! {
            biased;
            _ = &mut parent_cancelled => {
                debug!(
                    protocol = context.protocol,
                    streamer_id = context.streamer_id,
                    "Download cancelled while waiting for stream data"
                );
                break;
            }
            _ = &mut child_cancelled => {
                debug!(
                    protocol = context.protocol,
                    streamer_id = context.streamer_id,
                    "Download cancelled while waiting for stream data"
                );
                break;
            }
            result = stream.next() => result,
        };
        let Some(result) = result else {
            break;
        };

        match result {
            Ok(item) => {
                inspect(&item);
                let send_result = tokio::select! {
                    biased;
                    _ = &mut parent_cancelled => {
                        debug!(
                            protocol = context.protocol,
                            streamer_id = context.streamer_id,
                            "Download cancelled while forwarding stream data"
                        );
                        break;
                    }
                    _ = &mut child_cancelled => {
                        debug!(
                            protocol = context.protocol,
                            streamer_id = context.streamer_id,
                            "Download cancelled while forwarding stream data"
                        );
                        break;
                    }
                    result = tx.send_item(Ok(item)) => result,
                };
                if send_result.is_err() {
                    warn!(
                        protocol = context.protocol,
                        "Channel closed, stopping download"
                    );
                    break;
                }
            }
            Err(e) => {
                error!(
                    protocol = context.protocol,
                    streamer_id = context.streamer_id,
                    error = %e,
                    "Stream failed"
                );
                let kind = classify(&e);
                let msg = e.to_string();
                stream_error = Some((kind, msg.clone()));
                let send_result = tokio::select! {
                    biased;
                    _ = &mut parent_cancelled => {
                        debug!(
                            protocol = context.protocol,
                            streamer_id = context.streamer_id,
                            "Download cancelled while forwarding a stream error"
                        );
                        break;
                    }
                    _ = &mut child_cancelled => {
                        debug!(
                            protocol = context.protocol,
                            streamer_id = context.streamer_id,
                            "Download cancelled while forwarding a stream error"
                        );
                        break;
                    }
                    result = tx.send_item(Err(PipelineError::Strategy(Box::new(
                        std::io::Error::other(msg.clone()),
                    )))) => result,
                };
                if send_result.is_err() {
                    debug!(
                        protocol = context.protocol,
                        streamer_id = context.streamer_id,
                        "Stream error receiver already closed"
                    );
                }
                break;
            }
        }
    }

    stream_error
}

// ---------------------------------------------------------------------------
// handle_writer_result
// ---------------------------------------------------------------------------

/// Await the writer and pipeline tasks, emit terminal events, and return
/// `DownloadStats`.
///
/// `processing_tasks` should be empty for raw-mode calls. `engine_signal`
/// describes how this download ended from the engine's POV — see
/// [`crate::downloader::EngineEndSignal`]. Caller passes `CleanDisconnect`
/// for mesio FLV's TCP-close path and `HlsEndlist` for HLS when the
/// playlist contained `#EXT-X-ENDLIST`.
///
/// Replaces 4 identical ~40-line match blocks (plus 2 pipeline-await blocks).
pub(super) async fn handle_writer_result(
    writer_task: tokio::task::JoinHandle<std::result::Result<WriterStats, WriterError>>,
    stream_error: Option<(DownloadFailureKind, String)>,
    processing_tasks: Vec<tokio::task::JoinHandle<std::result::Result<(), PipelineError>>>,
    event_tx: &mpsc::Sender<SegmentEvent>,
    streamer_id: &str,
    protocol: &str,
    engine_signal: crate::downloader::EngineEndSignal,
) -> crate::Result<DownloadStats> {
    let writer_result = match writer_task.await {
        Ok(result) => result,
        Err(join_error) => {
            let cleanup_errors = settle_processing_tasks(processing_tasks).await;
            let mut writer_message = format!(
                "{} writer task failed to join for {}: {}",
                protocol, streamer_id, join_error
            );
            if !cleanup_errors.is_empty() {
                writer_message.push_str("; pipeline cleanup errors: ");
                writer_message.push_str(&cleanup_errors.join("; "));
            }

            let (kind, message) = if let Some((kind, stream_message)) = stream_error {
                (
                    kind,
                    format!(
                        "{} stream error for {}: {}; {}",
                        protocol, streamer_id, stream_message, writer_message
                    ),
                )
            } else {
                (DownloadFailureKind::Processing, writer_message)
            };
            observe_segment_event_send(
                event_tx
                    .send(SegmentEvent::DownloadFailed {
                        kind,
                        message: message.clone(),
                    })
                    .await,
                streamer_id,
            );
            return Err(crate::Error::Other(message));
        }
    };

    let processing_errors = settle_processing_tasks(processing_tasks).await;
    match writer_result {
        Ok(stats) if processing_errors.is_empty() => {
            let download_stats = DownloadStats {
                total_bytes: stats.bytes_written,
                total_items: stats.items_written,
                total_duration_secs: stats.duration_secs,
                files_created: stats.files_created,
            };

            if let Some((kind, msg)) = stream_error {
                let message = format!("{} stream error for {}: {}", protocol, streamer_id, msg);
                observe_segment_event_send(
                    event_tx
                        .send(SegmentEvent::DownloadFailed {
                            kind,
                            message: message.clone(),
                        })
                        .await,
                    streamer_id,
                );
                return Err(crate::Error::Other(message));
            }

            observe_segment_event_send(
                event_tx
                    .send(SegmentEvent::DownloadCompleted {
                        total_bytes: download_stats.total_bytes,
                        total_duration_secs: download_stats.total_duration_secs,
                        total_segments: download_stats.files_created,
                        engine_signal,
                    })
                    .await,
                streamer_id,
            );

            info!(
                "{} download completed for {}: {} items, {} files",
                protocol, streamer_id, stats.items_written, download_stats.files_created
            );

            Ok(download_stats)
        }
        Ok(_) => {
            let pipeline_message = format!(
                "{} pipeline cleanup failed for {}: {}",
                protocol,
                streamer_id,
                processing_errors.join("; ")
            );
            let (kind, message) = if let Some((kind, stream_message)) = stream_error {
                (
                    kind,
                    format!(
                        "{} stream error for {}: {}; {}",
                        protocol, streamer_id, stream_message, pipeline_message
                    ),
                )
            } else {
                (DownloadFailureKind::Processing, pipeline_message)
            };
            warn!(%message, "Pipeline processing task failed");
            observe_segment_event_send(
                event_tx
                    .send(SegmentEvent::DownloadFailed {
                        kind,
                        message: message.clone(),
                    })
                    .await,
                streamer_id,
            );
            Err(crate::Error::Other(message))
        }
        Err(writer_error) => {
            let mut writer_message = format!(
                "{} writer error for {}: {}",
                protocol, streamer_id, writer_error
            );
            if !processing_errors.is_empty() {
                writer_message.push_str("; pipeline cleanup errors: ");
                writer_message.push_str(&processing_errors.join("; "));
            }

            let (kind, message) = if let Some((kind, stream_message)) = stream_error {
                (
                    kind,
                    format!(
                        "{} stream error for {}: {}; {}",
                        protocol, streamer_id, stream_message, writer_message
                    ),
                )
            } else {
                (DownloadFailureKind::Processing, writer_message)
            };
            observe_segment_event_send(
                event_tx
                    .send(SegmentEvent::DownloadFailed {
                        kind,
                        message: message.clone(),
                    })
                    .await,
                streamer_id,
            );
            Err(crate::Error::Other(message))
        }
    }
}

async fn settle_processing_tasks(
    processing_tasks: Vec<tokio::task::JoinHandle<std::result::Result<(), PipelineError>>>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for (index, task) in processing_tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(PipelineError::Cancelled)) => {}
            Ok(Err(error)) => {
                errors.push(format!("pipeline task {index} failed: {error}"));
            }
            Err(join_error) if join_error.is_cancelled() => {}
            Err(join_error) => {
                errors.push(format!(
                    "pipeline task {index} failed to join: {join_error}"
                ));
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stream_consumption_stops_when_cancelled_while_source_is_pending() {
        let parent_token = CancellationToken::new();
        let child_token = parent_token.child_token();
        let cancel = parent_token.clone();
        let (tx, _rx) = mpsc::channel::<std::result::Result<u8, PipelineError>>(1);

        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel.cancel();
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            consume_stream(
                futures::stream::pending::<std::result::Result<u8, std::io::Error>>(),
                &tx,
                StreamConsumeContext {
                    parent_token: &parent_token,
                    child_token: &child_token,
                    streamer_id: "streamer-1",
                    protocol: "FLV",
                },
                |_| DownloadFailureKind::Network,
                |_| {},
            ),
        )
        .await
        .expect("cancellation should unblock a pending source");

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn stream_consumption_stops_when_cancelled_while_sink_is_full() {
        let parent_token = CancellationToken::new();
        let child_token = parent_token.child_token();
        let cancel = parent_token.clone();
        let (tx, mut rx) = mpsc::channel::<std::result::Result<u8, PipelineError>>(1);
        tx.send(Ok(1)).await.expect("initial item should fill sink");

        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel.cancel();
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            consume_stream(
                futures::stream::iter([Ok::<u8, std::io::Error>(2)]),
                &tx,
                StreamConsumeContext {
                    parent_token: &parent_token,
                    child_token: &child_token,
                    streamer_id: "streamer-1",
                    protocol: "FLV",
                },
                |_| DownloadFailureKind::Network,
                |_| {},
            ),
        )
        .await
        .expect("cancellation should unblock a full sink");

        assert!(result.is_none());
        assert_eq!(
            rx.recv()
                .await
                .expect("initial item should remain")
                .unwrap(),
            1
        );
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn writer_join_failure_waits_for_processing_tasks_and_keeps_context() {
        let writer_task = tokio::spawn(std::future::pending::<
            std::result::Result<WriterStats, WriterError>,
        >());
        writer_task.abort();

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let processing_task = tokio::spawn(async move {
            let _ = started_tx.send(());
            let _ = release_rx.await;
            Err::<(), PipelineError>(PipelineError::Strategy(Box::new(std::io::Error::other(
                "cleanup failed",
            ))))
        });
        let (event_tx, mut event_rx) = mpsc::channel(1);

        let settlement = tokio::spawn(async move {
            handle_writer_result(
                writer_task,
                Some((
                    DownloadFailureKind::Network,
                    "upstream disconnected".to_string(),
                )),
                vec![processing_task],
                &event_tx,
                "streamer-1",
                "FLV",
                crate::downloader::EngineEndSignal::CleanDisconnect,
            )
            .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), started_rx)
            .await
            .expect("processing cleanup should start")
            .expect("processing cleanup start sender should remain available");
        assert!(
            !settlement.is_finished(),
            "writer failure must still await pipeline cleanup"
        );
        release_tx
            .send(())
            .expect("processing task should still be waiting");

        let error = tokio::time::timeout(std::time::Duration::from_secs(1), settlement)
            .await
            .expect("settlement should complete")
            .expect("settlement task should not panic")
            .expect_err("writer join failure should fail the run");
        let message = error.to_string();
        assert!(message.contains("upstream disconnected"));
        assert!(message.contains("writer task failed to join"));
        assert!(message.contains("cleanup failed"));

        let event = event_rx
            .recv()
            .await
            .expect("failure event should be emitted");
        let SegmentEvent::DownloadFailed {
            kind,
            message: event_message,
        } = event
        else {
            panic!("expected DownloadFailed");
        };
        assert_eq!(kind, DownloadFailureKind::Network);
        assert_eq!(event_message, message);
    }
}
