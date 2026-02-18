//! Shared helpers for FLV and HLS download orchestrators.

use std::fmt::Display;
use std::path::Path;

use chrono::Utc;
use futures::StreamExt;
use pipeline_common::{
    PipelineError, RunCompletionError, WriterError, WriterProgress, WriterStats, settle_run,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::downloader::engine::traits::{
    DownloadFailureKind, DownloadProgress, SegmentEvent, SegmentInfo,
};

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
        F: Fn(&Path, u32, f64, u64) + Send + Sync + 'static;

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
        F: Fn(&Path, u32, f64, u64) + Send + Sync + 'static,
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
        F: Fn(&Path, u32, f64, u64) + Send + Sync + 'static,
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

    writer.set_on_segment_start_callback(move |path, sequence| {
        let event = SegmentEvent::SegmentStarted {
            path: path.to_path_buf(),
            sequence,
        };
        let _ = event_tx_start.blocking_send(event);
    });

    writer.set_on_segment_complete_callback(move |path, sequence, duration_secs, size_bytes| {
        let event_path = path.to_path_buf();
        let event = SegmentEvent::SegmentCompleted(SegmentInfo {
            path: event_path,
            duration_secs,
            size_bytes,
            index: sequence,
            completed_at: Utc::now(),
        });
        let _ = event_tx_complete.blocking_send(event);
    });

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
        let _ = event_tx_progress.try_send(SegmentEvent::Progress(download_progress));
    });
}

// ---------------------------------------------------------------------------
// consume_stream
// ---------------------------------------------------------------------------

/// Consume a protocol stream, forwarding items to a channel.
///
/// Returns `Some((kind, message))` if the stream yielded an error, or `None`
/// if it completed cleanly (or was cancelled).
///
/// Replaces 4 identical ~25-line `while let Some(result) = stream.next()` blocks.
pub(super) async fn consume_stream<T, E: Display>(
    stream: impl futures::Stream<Item = std::result::Result<T, E>> + Unpin,
    tx: &mpsc::Sender<std::result::Result<T, PipelineError>>,
    parent_token: &CancellationToken,
    child_token: &CancellationToken,
    streamer_id: &str,
    protocol: &str,
    classify: impl Fn(&E) -> DownloadFailureKind,
) -> Option<(DownloadFailureKind, String)> {
    let mut stream = std::pin::pin!(stream);
    let mut stream_error: Option<(DownloadFailureKind, String)> = None;

    while let Some(result) = stream.next().await {
        if parent_token.is_cancelled() || child_token.is_cancelled() {
            debug!("{} download cancelled for {}", protocol, streamer_id);
            break;
        }

        match result {
            Ok(item) => {
                if tx.send(Ok(item)).await.is_err() {
                    warn!("Channel closed, stopping {} download", protocol);
                    break;
                }
            }
            Err(e) => {
                error!("{} stream error for {}: {}", protocol, streamer_id, e);
                let kind = classify(&e);
                let msg = e.to_string();
                stream_error = Some((kind, msg.clone()));
                let _ = tx
                    .send(Err(PipelineError::Strategy(Box::new(
                        std::io::Error::other(msg.clone()),
                    ))))
                    .await;
                break;
            }
        }
    }

    stream_error
}

// ---------------------------------------------------------------------------
// handle_writer_result
// ---------------------------------------------------------------------------

/// Handle the writer result, await pipeline tasks, emit events, and return
/// `DownloadStats`.
///
/// `processing_tasks` should be empty for raw-mode calls.
///
/// Replaces 4 identical ~40-line match blocks (plus 2 pipeline-await blocks).
pub(super) async fn handle_writer_result(
    writer_result: std::result::Result<WriterStats, WriterError>,
    stream_error: Option<(DownloadFailureKind, String)>,
    processing_tasks: Vec<tokio::task::JoinHandle<std::result::Result<(), PipelineError>>>,
    event_tx: &mpsc::Sender<SegmentEvent>,
    streamer_id: &str,
    protocol: &str,
) -> crate::Result<DownloadStats> {
    match settle_run(writer_result, processing_tasks).await {
        Ok(stats) => {
            let download_stats = DownloadStats {
                total_bytes: stats.bytes_written,
                total_items: stats.items_written,
                total_duration_secs: stats.duration_secs,
                files_created: stats.files_created,
            };

            if let Some((kind, msg)) = stream_error {
                let _ = event_tx
                    .send(SegmentEvent::DownloadFailed {
                        kind,
                        message: msg.clone(),
                    })
                    .await;
                return Err(crate::Error::Other(format!(
                    "{} stream error: {}",
                    protocol, msg
                )));
            }

            let _ = event_tx
                .send(SegmentEvent::DownloadCompleted {
                    total_bytes: download_stats.total_bytes,
                    total_duration_secs: download_stats.total_duration_secs,
                    total_segments: download_stats.files_created,
                })
                .await;

            info!(
                "{} download completed for {}: {} items, {} files",
                protocol, streamer_id, stats.items_written, download_stats.files_created
            );

            Ok(download_stats)
        }
        Err(RunCompletionError::Writer(e)) => {
            if let Some((kind, msg)) = stream_error {
                let _ = event_tx
                    .send(SegmentEvent::DownloadFailed {
                        kind,
                        message: msg.clone(),
                    })
                    .await;
                return Err(crate::Error::Other(format!(
                    "{} stream error: {}",
                    protocol, msg
                )));
            }
            let _ = event_tx
                .send(SegmentEvent::DownloadFailed {
                    kind: DownloadFailureKind::Processing,
                    message: e.to_string(),
                })
                .await;
            Err(crate::Error::Other(format!(
                "{} writer error: {}",
                protocol, e
            )))
        }
        Err(RunCompletionError::Pipeline(e)) => {
            if let Some((kind, msg)) = stream_error {
                let _ = event_tx
                    .send(SegmentEvent::DownloadFailed {
                        kind,
                        message: msg.clone(),
                    })
                    .await;
                return Err(crate::Error::Other(format!(
                    "{} stream error: {}",
                    protocol, msg
                )));
            }
            warn!("Pipeline processing task error: {}", e);
            let _ = event_tx
                .send(SegmentEvent::DownloadFailed {
                    kind: DownloadFailureKind::Processing,
                    message: e.to_string(),
                })
                .await;
            Err(crate::Error::Other(format!(
                "{} pipeline error: {}",
                protocol, e
            )))
        }
    }
}
