//! Shared FFmpeg recording state. Process owners decide when the writer is settled;
//! stderr EOF alone never authorizes publication of the final segment.

use std::future::Future;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use tokio::io::AsyncRead;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use super::{
    OutputRecordReader, is_segment_start, observe_segment_event_send, output_io_error_kind,
    parse_opened_path, parse_progress,
};
use crate::downloader::EngineEndSignal;
use crate::downloader::engine::{DownloadFailureKind, IoErrorKindSer, SegmentEvent, SegmentInfo};

#[derive(Clone, Copy)]
pub(crate) enum FfmpegSource {
    Direct,
    Streamlink,
}

impl FfmpegSource {
    fn process_name(self) -> &'static str {
        match self {
            Self::Direct => "FFmpeg",
            Self::Streamlink => "Streamlink/FFmpeg",
        }
    }

    fn output_name(self) -> &'static str {
        match self {
            Self::Direct => "ffmpeg",
            Self::Streamlink => "streamlink+ffmpeg",
        }
    }

    fn waiter_name(self) -> &'static str {
        match self {
            Self::Direct => "FFmpeg",
            Self::Streamlink => "Streamlink",
        }
    }
}

pub(crate) enum RecordingExit {
    Status(Option<i32>),
    Failed {
        kind: DownloadFailureKind,
        message: String,
    },
}

struct ActiveSegment {
    index: u32,
    path: PathBuf,
    started_media_at: f64,
    started_at: DateTime<Utc>,
    // A sample belongs to this path only. A new segment must fall back to parsed
    // progress until its own metadata lookup has succeeded.
    filesystem_bytes: Option<u64>,
    last_stat_at: Instant,
}

struct SegmentTracker {
    segment_mode: bool,
    started_instant: Instant,
    active: Option<ActiveSegment>,
    next_index: u32,
    segments_completed: u32,
    bytes_completed: u64,
    total_bytes: u64,
    total_duration: f64,
    media_offset: f64,
    media_total: f64,
    last_progress: Option<(u64, f64, f64)>,
}

#[derive(Default)]
struct RecordEvents {
    completed: Option<SegmentEvent>,
    started: Option<SegmentEvent>,
    progress: Option<SegmentEvent>,
}

impl RecordEvents {
    fn into_events(self) -> impl Iterator<Item = SegmentEvent> {
        self.completed
            .into_iter()
            .chain(self.started)
            .chain(self.progress)
    }
}

impl SegmentTracker {
    fn new(segment_mode: bool, started_instant: Instant) -> Self {
        Self {
            segment_mode,
            started_instant,
            active: None,
            next_index: 0,
            segments_completed: 0,
            bytes_completed: 0,
            total_bytes: 0,
            total_duration: 0.0,
            media_offset: 0.0,
            media_total: 0.0,
            last_progress: None,
        }
    }

    fn start(&mut self, path: PathBuf, now: Instant, wall: DateTime<Utc>) -> SegmentEvent {
        let index = self.next_index;
        self.next_index = self.next_index.saturating_add(1);
        self.active = Some(ActiveSegment {
            index,
            path: path.clone(),
            started_media_at: self.media_total,
            started_at: wall,
            filesystem_bytes: None,
            last_stat_at: now,
        });
        SegmentEvent::SegmentStarted {
            path,
            sequence: index,
            started_at: wall,
        }
    }

    async fn complete(&mut self, wall: DateTime<Utc>) -> Option<SegmentEvent> {
        let active = self.active.take()?;
        let size_bytes = tokio::fs::metadata(&active.path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let duration_secs = (self.media_total - active.started_media_at).max(0.0);
        self.segments_completed = self.segments_completed.saturating_add(1);
        self.bytes_completed = self.bytes_completed.saturating_add(size_bytes);
        self.total_bytes = self.bytes_completed;
        if self.segment_mode {
            self.media_offset += duration_secs;
            self.media_total = self.media_offset;
            self.total_duration = self.media_offset;
        } else {
            self.total_duration = self.media_total;
        }
        Some(SegmentEvent::SegmentCompleted(SegmentInfo {
            path: active.path,
            duration_secs,
            size_bytes,
            index: active.index,
            started_at: Some(active.started_at),
            completed_at: wall,
            split_reason_code: None,
            split_reason_details_json: None,
        }))
    }

    async fn observe(&mut self, line: &str, now: Instant, wall: DateTime<Utc>) -> RecordEvents {
        let mut events = RecordEvents::default();
        if self.segment_mode
            && is_segment_start(line)
            && let Some(path) = parse_opened_path(line)
        {
            events.completed = self.complete(wall).await;
            events.started = Some(self.start(path, now, wall));
        }
        if let Some(mut progress) = parse_progress(line) {
            let elapsed = now.duration_since(self.started_instant).as_secs_f64();
            self.media_total = if self.segment_mode {
                self.media_offset + progress.media_duration_secs
            } else {
                progress.media_duration_secs
            };
            let mut bytes = progress.bytes_downloaded;
            if let Some(active) = &mut self.active {
                if now.duration_since(active.last_stat_at) >= Duration::from_millis(500) {
                    if let Ok(metadata) = tokio::fs::metadata(&active.path).await {
                        active.filesystem_bytes = Some(metadata.len());
                    }
                    active.last_stat_at = now;
                }
                bytes = active.filesystem_bytes.unwrap_or(bytes);
            }
            if self.segment_mode {
                bytes = self.bytes_completed.saturating_add(bytes);
            }
            self.total_bytes = bytes;
            self.total_duration = self.media_total;
            progress.bytes_downloaded = bytes;
            progress.duration_secs = elapsed;
            progress.media_duration_secs = self.media_total;
            progress.segments_completed = self.segments_completed;
            progress.current_segment = self
                .active
                .as_ref()
                .map(|active| active.path.to_string_lossy().to_string());
            progress.speed_bytes_per_sec = self
                .last_progress
                .and_then(|(previous_bytes, previous_elapsed, _)| {
                    let dt = elapsed - previous_elapsed;
                    (dt > 0.0).then_some((bytes.saturating_sub(previous_bytes) as f64 / dt) as u64)
                })
                .unwrap_or(0);
            progress.playback_ratio = self
                .last_progress
                .and_then(|(_, previous_elapsed, previous_media)| {
                    let dt = elapsed - previous_elapsed;
                    (dt > 0.0).then_some((self.media_total - previous_media) / dt)
                })
                .unwrap_or(0.0);
            self.last_progress = Some((bytes, elapsed, self.media_total));
            events.progress = Some(SegmentEvent::Progress(progress));
        }
        events
    }
}

pub(crate) struct FfmpegEvents {
    pub source: FfmpegSource,
    pub segment_mode: bool,
    pub single_output_path: Option<PathBuf>,
    pub started_instant: Instant,
    pub streamer_id: String,
    pub output_dir: PathBuf,
    pub event_tx: mpsc::Sender<SegmentEvent>,
    pub forced_settlement: CancellationToken,
}

impl FfmpegEvents {
    async fn send(&self, event: SegmentEvent) {
        observe_segment_event_send(self.event_tx.send(event).await, &self.streamer_id);
    }

    pub(crate) async fn run<R: AsyncRead + Unpin, E: Into<RecordingExit>>(
        mut self,
        stderr: R,
        exit_rx: impl Future<Output = Result<E, oneshot::error::RecvError>>,
    ) {
        let mut tracker = SegmentTracker::new(self.segment_mode, self.started_instant);
        let mut reader = OutputRecordReader::new(stderr);
        let mut output_io_kind = None;
        let mut cleanup_unconfirmed = false;
        if let Some(path) = self.single_output_path.take() {
            self.send(tracker.start(path, Instant::now(), Utc::now()))
                .await;
        }
        loop {
            tokio::select! {
                biased;
                _ = self.forced_settlement.cancelled() => {
                    cleanup_unconfirmed = true;
                    warn!(streamer_id = %self.streamer_id, "Stopping FFmpeg stderr processing after unconfirmed process cleanup");
                    break;
                }
                record = reader.next_record() => match record {
                    Ok(Some(line)) => {
                        for event in tracker.observe(&line, Instant::now(), Utc::now()).await.into_events() { self.send(event).await; }
                        if !line.starts_with("frame=") { debug!(streamer_id = %self.streamer_id, %line, "FFmpeg stderr"); }
                        if matches!(self.source, FfmpegSource::Direct) && (line.contains("Error") || line.contains("error")) {
                            warn!(streamer_id = %self.streamer_id, %line, "FFmpeg error");
                        }
                        if output_io_kind.is_none() && let Some(io_kind) = output_io_error_kind(&line) {
                            output_io_kind = Some(io_kind);
                            self.send(SegmentEvent::OutputIoError { output_dir: self.output_dir.clone(), io_kind, detail: format!("{}: {line}", self.source.output_name()) }).await;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => { error!(streamer_id = %self.streamer_id, %error, "Error reading FFmpeg output"); break; }
                }
            }
        }
        let exit = match exit_rx.await {
            Ok(exit) => exit.into(),
            Err(_) => {
                cleanup_unconfirmed = true;
                RecordingExit::Failed {
                    kind: DownloadFailureKind::ProcessExit { code: None },
                    message: format!(
                        "{} process waiter stopped without an exit result",
                        self.source.waiter_name()
                    ),
                }
            }
        };
        cleanup_unconfirmed |= self.forced_settlement.is_cancelled();
        if !cleanup_unconfirmed && let Some(event) = tracker.complete(Utc::now()).await {
            self.send(event).await;
        }
        if matches!(exit, RecordingExit::Status(Some(228))) && output_io_kind.is_none() {
            output_io_kind = Some(IoErrorKindSer::StorageFull);
            self.send(SegmentEvent::OutputIoError {
                output_dir: self.output_dir.clone(),
                io_kind: IoErrorKindSer::StorageFull,
                detail: format!(
                    "{} exit 228 (I/O error, likely ENOSPC)",
                    self.source.output_name()
                ),
            })
            .await;
        }
        let event = match exit {
            RecordingExit::Status(Some(0)) if output_io_kind.is_none() => {
                SegmentEvent::DownloadCompleted {
                    total_bytes: tracker.total_bytes,
                    total_duration_secs: tracker.total_duration,
                    total_segments: tracker.segments_completed,
                    engine_signal: EngineEndSignal::SubprocessExitZero,
                }
            }
            exit => {
                let (kind, message) = match exit {
                    RecordingExit::Status(code) => (
                        DownloadFailureKind::ProcessExit { code },
                        match code {
                            Some(code) => {
                                format!("{} exited with code {code}", self.source.process_name())
                            }
                            None => format!(
                                "{} exited without an exit code",
                                self.source.process_name()
                            ),
                        },
                    ),
                    RecordingExit::Failed { kind, message } => (kind, message),
                };
                SegmentEvent::DownloadFailed {
                    kind: output_io_kind.map_or(kind, |io_kind| {
                        DownloadFailureKind::OutputRootUnavailable { io_kind }
                    }),
                    message,
                }
            }
        };
        self.send(event).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    #[tokio::test]
    async fn stderr_eof_waits_for_confirmed_exit_before_reading_the_final_file() {
        struct ObservedEof(Arc<AtomicBool>);
        impl AsyncRead for ObservedEof {
            fn poll_read(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
                _: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                self.0.store(true, Ordering::SeqCst);
                Poll::Ready(Ok(()))
            }
        }

        for source in [FfmpegSource::Direct, FfmpegSource::Streamlink] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("final.ts");
            std::fs::write(&path, b"prefix").unwrap();
            let (event_tx, mut event_rx) = mpsc::channel(16);
            let (exit_tx, mut exit_rx) = oneshot::channel::<RecordingExit>();
            let eof_polled = Arc::new(AtomicBool::new(false));
            let exit_pending = Arc::new(AtomicBool::new(false));
            let observed_exit = exit_pending.clone();
            let exit = std::future::poll_fn(move |cx| {
                let result = Pin::new(&mut exit_rx).poll(cx);
                if result.is_pending() {
                    observed_exit.store(true, Ordering::SeqCst);
                }
                result
            });
            let events = FfmpegEvents {
                source,
                segment_mode: false,
                single_output_path: Some(path.clone()),
                started_instant: Instant::now(),
                streamer_id: "test".into(),
                output_dir: directory.path().to_owned(),
                event_tx,
                forced_settlement: CancellationToken::new(),
            };
            let mut reader = Box::pin(events.run(ObservedEof(eof_polled.clone()), exit));
            // Drive the actual reader through EOF and into its pending exit wait.
            // This handshake does not depend on a native child's scheduling or on
            // a wall-clock window without events.
            tokio::time::timeout(
                Duration::from_secs(2),
                std::future::poll_fn(|cx| {
                    assert!(reader.as_mut().poll(cx).is_pending());
                    if exit_pending.load(Ordering::SeqCst) {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                }),
            )
            .await
            .unwrap();
            assert!(eof_polled.load(Ordering::SeqCst));
            assert!(matches!(
                event_rx.try_recv().unwrap(),
                SegmentEvent::SegmentStarted { .. }
            ));
            assert!(
                event_rx.try_recv().is_err(),
                "EOF must not publish completion or a terminal event before exit"
            );

            std::fs::write(&path, b"prefix-late-trailer").unwrap();
            assert!(exit_tx.send(RecordingExit::Status(Some(0))).is_ok());
            tokio::time::timeout(Duration::from_secs(2), reader)
                .await
                .unwrap();
            assert!(
                matches!(event_rx.try_recv().unwrap(), SegmentEvent::SegmentCompleted(segment) if segment.path == path && segment.size_bytes == 19)
            );
            assert!(matches!(
                event_rx.try_recv().unwrap(),
                SegmentEvent::DownloadCompleted {
                    total_bytes: 19,
                    total_segments: 1,
                    ..
                }
            ));
            assert!(event_rx.try_recv().is_err());
        }
    }

    async fn terminal_events(
        source: FfmpegSource,
        exit: Option<RecordingExit>,
    ) -> Vec<SegmentEvent> {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("final.ts");
        tokio::fs::write(&path, b"recording").await.unwrap();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let (exit_tx, exit_rx) = oneshot::channel::<RecordingExit>();
        if let Some(exit) = exit {
            assert!(exit_tx.send(exit).is_ok());
        } else {
            drop(exit_tx);
        }
        FfmpegEvents {
            source,
            segment_mode: false,
            single_output_path: Some(path),
            started_instant: Instant::now(),
            streamer_id: "test".into(),
            output_dir: directory.path().to_owned(),
            event_tx,
            forced_settlement: CancellationToken::new(),
        }
        .run(&b""[..], exit_rx)
        .await;
        let mut events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn lost_waiter_does_not_publish_a_final_segment() {
        for source in [FfmpegSource::Direct, FfmpegSource::Streamlink] {
            let events = terminal_events(source, None).await;
            assert!(
                matches!(events.as_slice(), [SegmentEvent::SegmentStarted { .. }, SegmentEvent::DownloadFailed { kind: DownloadFailureKind::ProcessExit { code: None }, message }] if message.contains("waiter stopped without an exit result"))
            );
        }
    }

    #[tokio::test]
    async fn classified_pipeline_exit_228_retains_its_origin_instead_of_gating_storage() {
        let events = terminal_events(
            FfmpegSource::Streamlink,
            Some(RecordingExit::Failed {
                kind: DownloadFailureKind::ProcessExit { code: Some(228) },
                message: "Streamlink exited with status 228".into(),
            }),
        )
        .await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SegmentEvent::OutputIoError { .. }))
        );
        assert!(
            matches!(events.last(), Some(SegmentEvent::DownloadFailed { kind: DownloadFailureKind::ProcessExit { code: Some(228) }, message }) if message == "Streamlink exited with status 228")
        );
        let events = terminal_events(
            FfmpegSource::Streamlink,
            Some(RecordingExit::Failed {
                kind: DownloadFailureKind::Network,
                message: "pipe failed; FFmpeg exited with code 228".into(),
            }),
        )
        .await;
        assert!(matches!(
            events.last(),
            Some(SegmentEvent::DownloadFailed {
                kind: DownloadFailureKind::Network,
                ..
            })
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SegmentEvent::OutputIoError { .. }))
        );
    }

    #[tokio::test]
    async fn rotation_discards_previous_segments_filesystem_sample() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.ts");
        let second = directory.path().join("second.ts");
        tokio::fs::write(&first, vec![0; 100]).await.unwrap();
        tokio::fs::write(&second, vec![0; 7]).await.unwrap();
        let now = Instant::now();
        let wall = Utc::now();
        let mut tracker = SegmentTracker::new(true, now);
        tracker
            .observe(
                &format!("[segment @ test] Opening '{}' for writing", first.display()),
                now,
                wall,
            )
            .await;
        tracker
            .observe(
                "frame=1 size=1kB time=00:00:01.00",
                now + Duration::from_millis(501),
                wall,
            )
            .await;
        tracker
            .observe(
                &format!(
                    "[segment @ test] Opening '{}' for writing",
                    second.display()
                ),
                now + Duration::from_millis(502),
                wall,
            )
            .await;
        let events = tracker
            .observe(
                "frame=2 size=2kB time=00:00:00.50",
                now + Duration::from_millis(503),
                wall,
            )
            .await;
        let Some(SegmentEvent::Progress(progress)) = &events.progress else {
            panic!("expected progress")
        };
        // Until this segment has its own successful stat, use its parsed bytes.
        assert_eq!(progress.bytes_downloaded, 100 + 2 * 1024);
        assert_eq!(progress.segments_completed, 1);
        assert_eq!(progress.media_duration_secs, 1.5);
        let events = tracker
            .observe(
                "frame=3 size=3kB time=00:00:01.00",
                now + Duration::from_millis(1003),
                wall,
            )
            .await;
        let Some(SegmentEvent::Progress(progress)) = &events.progress else {
            panic!("expected progress")
        };
        assert_eq!(progress.bytes_downloaded, 107);
    }
}
