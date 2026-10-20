//! HLS-specific download orchestrator.
//!
//! This module provides the `HlsDownloader` struct which handles HLS stream
//! downloading, including stream consumption, writer management, and event emission.
//! It supports both pipeline-processed and raw download modes.
//!
//! DASH sessions deliver the same fMP4 `HlsData` items and are recorded
//! through [`HlsDownloader::record`] as well, labelled with their protocol.

use futures::StreamExt;
use futures::future::BoxFuture;
use hls::{HlsData, SplitReason};
use hls_fix::{HlsPipeline, HlsWriter, HlsWriterConfig};
use mesio::flv::FlvProtocolConfig;
use mesio::{
    BoxMediaStream, DownloadError, DownloadRequest, DownloadTerminal, MesioConfig, MesioDownloader,
    ProtocolSelection,
};
use pipeline_common::{
    PipelineError, PipelineProvider, ProtocolWriter, StreamerContext, spawn_pipeline,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::super::utils::TASK_SETTLEMENT_TIMEOUT;
use super::super::utils::observe_segment_event_send;
use super::super::utils::sanitize_engine_message;
use super::classify_download_error;
use super::config::build_hls_config;
use super::helpers::{self, DownloadStats};
use crate::database::models::engine::MesioEngineConfig;
use crate::downloader::EngineEndSignal;
use crate::downloader::engine::traits::{
    DownloadFailureKind, DownloadHandle, EngineStartError, SegmentEvent,
};

/// A protocol whose sessions deliver fMP4/TS `HlsData` and are written by
/// [`HlsDownloader::record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SegmentedProtocol {
    Hls,
    Dash,
}

impl SegmentedProtocol {
    /// Label used in log fields and in event and error messages.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Hls => "HLS",
            Self::Dash => "DASH",
        }
    }

    /// The engine signal of a stream that ended with an `EndOfStream` marker:
    /// HLS `#EXT-X-ENDLIST`, or a static or ended DASH presentation recorded
    /// to its last segment.
    fn authoritative_end(self) -> EngineEndSignal {
        match self {
            Self::Hls => EngineEndSignal::HlsEndlist,
            Self::Dash => EngineEndSignal::DashEnd,
        }
    }

    fn fix_enabled(self, engine_config: &MesioEngineConfig) -> bool {
        match self {
            Self::Hls => engine_config.fix_hls,
            Self::Dash => engine_config.fix_dash,
        }
    }
}

async fn next_initial_item<T>(
    next: impl std::future::Future<Output = T>,
    cancellation: &CancellationToken,
    session_cancellation: &CancellationToken,
    protocol: SegmentedProtocol,
) -> std::result::Result<T, EngineStartError> {
    let cancelled = || {
        EngineStartError::new(
            DownloadFailureKind::Cancelled,
            format!(
                "{} download cancelled before the first segment",
                protocol.label()
            ),
        )
    };
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(cancelled()),
        _ = session_cancellation.cancelled() => Err(cancelled()),
        item = next => Ok(item),
    }
}

/// The mesio session's own report of how it ended, joined after its items
/// were consumed. The items alone cannot tell a clean end from one where an
/// engine task failed after the last item was produced.
pub(super) struct SessionLifecycle {
    cancel: Box<dyn Fn() + Send + Sync>,
    join: BoxFuture<'static, Option<Result<DownloadTerminal, DownloadError>>>,
}

impl From<mesio::DownloadHandle> for SessionLifecycle {
    fn from(handle: mesio::DownloadHandle) -> Self {
        let cancel_handle = handle.clone();
        Self {
            cancel: Box::new(move || cancel_handle.cancel()),
            join: Box::pin(async move { handle.join().await }),
        }
    }
}

/// How a session ended, as far as the backend needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionEnd {
    /// The engine reported an authoritative end.
    AuthoritativeEnd,
    /// Cancelled, or the consumer went away.
    Stopped,
    /// An engine task failed or panicked.
    Failed(String),
    /// The session reported nothing (its lifecycle was already joined).
    Unreported,
    /// The session did not finish within the settlement window.
    TimedOut,
}

impl SessionLifecycle {
    /// Waits for the session to finish. A session whose items ran out is
    /// finishing on its own and gets a grace period before it is cancelled;
    /// one abandoned early (cancellation, writer failure, stream error) is
    /// cancelled at once. Cancelling cannot change an outcome the engine has
    /// already decided.
    async fn settle(self, items_exhausted: bool) -> SessionEnd {
        let Self { cancel, mut join } = self;
        if !items_exhausted {
            cancel();
        }
        let joined = match tokio::time::timeout(TASK_SETTLEMENT_TIMEOUT, &mut join).await {
            Ok(joined) => joined,
            Err(_) if items_exhausted => {
                cancel();
                match tokio::time::timeout(TASK_SETTLEMENT_TIMEOUT, join).await {
                    Ok(joined) => joined,
                    Err(_) => return SessionEnd::TimedOut,
                }
            }
            Err(_) => return SessionEnd::TimedOut,
        };
        match joined {
            None => SessionEnd::Unreported,
            Some(Ok(DownloadTerminal::AuthoritativeEnd)) => SessionEnd::AuthoritativeEnd,
            Some(Ok(DownloadTerminal::Cancelled | DownloadTerminal::DownstreamClosed)) => {
                SessionEnd::Stopped
            }
            Some(Ok(DownloadTerminal::PipelineError(reason))) => {
                SessionEnd::Failed(reason.to_string())
            }
            Some(Err(error)) => SessionEnd::Failed(error.to_string()),
        }
    }
}

/// What the consumed items and the session's own report say together.
struct SessionObservation {
    /// The items ran out rather than being abandoned.
    items_exhausted: bool,
    /// An `EndOfStream` marker was among the items.
    saw_end_of_stream: bool,
    /// The download was cancelled through either token.
    cancelled: bool,
}

/// The terminal stream error and engine signal of a recording.
///
/// A typed stream error is kept as is. A session that ran out of items
/// without one but reports a failed engine task is a processing failure,
/// whatever its items ended with. The protocol's authoritative end is only
/// reported when the items ended with `EndOfStream` and the session confirms
/// an authoritative end; cancellation keeps its existing outcome.
fn conclude_session(
    protocol: SegmentedProtocol,
    observation: &SessionObservation,
    stream_error: Option<(DownloadFailureKind, String)>,
    session_end: &SessionEnd,
) -> (Option<(DownloadFailureKind, String)>, EngineEndSignal) {
    let stream_error = match (stream_error, session_end) {
        (None, SessionEnd::Failed(reason))
            if observation.items_exhausted && !observation.cancelled =>
        {
            Some((
                DownloadFailureKind::Processing,
                format!("{} session failed: {reason}", protocol.label()),
            ))
        }
        (stream_error, _) => stream_error,
    };
    let engine_signal =
        if observation.saw_end_of_stream && matches!(session_end, SessionEnd::AuthoritativeEnd) {
            protocol.authoritative_end()
        } else {
            EngineEndSignal::CleanDisconnect
        };
    (stream_error, engine_signal)
}

/// HLS-specific download orchestrator.
///
/// Handles HLS stream downloading with support for both pipeline-processed
/// and raw download modes. Manages stream consumption, writer setup, and
/// event emission through the provided channel.
pub(super) struct HlsDownloader {
    /// The download attempt: shared configuration, event sender, cancellation
    /// token and manual-split control.
    handle: Arc<DownloadHandle>,
    /// Engine-specific configuration.
    engine_config: MesioEngineConfig,
    /// Base HLS configuration from the engine.
    hls_config: Option<mesio::hls::HlsConfig>,
}

impl HlsDownloader {
    /// Create a new HLS downloader.
    ///
    /// # Arguments
    /// * `handle` - The download attempt to record.
    /// * `engine_config` - Mesio engine-specific configuration.
    /// * `hls_config` - Optional base HLS configuration from the engine.
    pub(super) fn new(
        handle: Arc<DownloadHandle>,
        engine_config: MesioEngineConfig,
        hls_config: Option<mesio::hls::HlsConfig>,
    ) -> Self {
        Self {
            handle,
            engine_config,
            hls_config,
        }
    }

    /// Create a MesioDownloader with the configured settings.
    fn create_downloader(&self, token: CancellationToken) -> MesioDownloader {
        let config = self.handle.config_snapshot();
        let hls_config = build_hls_config(&config, self.hls_config.clone(), &self.engine_config);

        MesioDownloader::new(MesioConfig {
            hls: hls_config,
            flv: FlvProtocolConfig::default(),
            dash: Default::default(),
            token,
        })
    }

    /// Run the HLS download, consuming the stream and writing to files.
    ///
    /// Starts a typed HLS download session with the configured HLS settings
    /// and records it through [`Self::record`].
    ///
    /// Returns download statistics on success.
    pub(super) async fn run(self) -> std::result::Result<DownloadStats, EngineStartError> {
        let token = self.handle.cancellation_token.child_token();

        let downloader = self.create_downloader(token.clone());

        let url = self.handle.config_snapshot().url;

        let request = DownloadRequest::from_url(&url)
            .map_err(|e| {
                let kind = classify_download_error(&e);
                EngineStartError::new(kind, format!("Invalid HLS download URL: {}", e))
            })?
            .with_protocol(ProtocolSelection::Hls(Default::default()))
            .with_cancel(token.clone());

        let session = downloader.start_hls(request).await.map_err(|e| {
            let kind = classify_download_error(&e);
            EngineStartError::new(kind, format!("Failed to start HLS download: {}", e))
        })?;

        self.record(
            SegmentedProtocol::Hls,
            token,
            session.items,
            SessionLifecycle::from(session.handle),
        )
        .await
    }

    /// Record the items of a started session to files.
    ///
    /// This method:
    /// 1. Peeks the first segment to determine file extension (ts vs m4s)
    /// 2. If `enable_processing` and the protocol's fix flag are set, routes
    ///    the stream through HlsPipeline
    /// 3. Creates an HlsWriter with callbacks for segment events
    /// 4. Sends HlsData items to the writer via channel
    /// 5. Handles cancellation, progress tracking, and error reporting
    ///
    /// `token` is the session's cancellation token, a child of this
    /// downloader's token.
    pub(super) async fn record(
        self,
        protocol: SegmentedProtocol,
        token: CancellationToken,
        mut hls_stream: BoxMediaStream<HlsData, DownloadError>,
        lifecycle: SessionLifecycle,
    ) -> std::result::Result<DownloadStats, EngineStartError> {
        let label = protocol.label();
        let config = self.handle.config_snapshot();
        let streamer_id = config.streamer_id;

        // Peek at the first segment to determine file extension
        let first_segment = loop {
            let next = next_initial_item(
                hls_stream.next(),
                &self.handle.cancellation_token,
                &token,
                protocol,
            )
            .await?;
            match next {
                Some(Ok(HlsData::EndMarker(_))) => {
                    debug!(
                        protocol = label,
                        "Skipping leading EndMarker before first data segment"
                    );
                    continue;
                }
                Some(Ok(segment)) => break segment,
                Some(Err(e)) => {
                    let kind = classify_download_error(&e);
                    observe_segment_event_send(
                        self.handle
                            .event_tx
                            .send(SegmentEvent::DownloadFailed {
                                kind,
                                message: sanitize_engine_message(
                                    &format!("Failed to get first {label} segment: {e}"),
                                    config.managed_credentials,
                                ),
                            })
                            .await,
                        &streamer_id,
                    );
                    return Err(EngineStartError::new(
                        kind,
                        format!("Failed to get first {label} segment: {e}"),
                    ));
                }
                None => {
                    observe_segment_event_send(
                        self.handle
                            .event_tx
                            .send(SegmentEvent::DownloadFailed {
                                kind: DownloadFailureKind::SourceUnavailable,
                                message: format!("{label} stream is empty"),
                            })
                            .await,
                        &streamer_id,
                    );
                    return Err(EngineStartError::new(
                        DownloadFailureKind::SourceUnavailable,
                        format!("{label} stream is empty"),
                    ));
                }
            }
        };

        // Determine extension from first segment
        let extension = match &first_segment {
            HlsData::TsData(_) => "ts",
            HlsData::M4sData(_) => "m4s",
            HlsData::EndMarker(_) => unreachable!("filtered before extension detection"),
        };

        let config_snapshot = self.handle.config_snapshot();
        let processing =
            config_snapshot.enable_processing && protocol.fix_enabled(&self.engine_config);
        info!(
            protocol = label,
            "Detected {} stream type for {}: {}, processing_enabled: {}",
            label,
            config_snapshot.streamer_id,
            extension.to_uppercase(),
            config_snapshot.enable_processing
        );

        // Route based on enable_processing flag AND engine config
        if processing {
            self.download_with_pipeline(
                protocol,
                token,
                hls_stream,
                first_segment,
                extension,
                lifecycle,
            )
            .await
        } else {
            self.download_raw(
                protocol,
                token,
                hls_stream,
                first_segment,
                extension,
                lifecycle,
            )
            .await
        }
    }

    /// Download HLS stream with pipeline processing enabled.
    ///
    /// Routes the stream through HlsPipeline for defragmentation, segment splitting,
    /// and other processing before writing to HlsWriter.
    async fn download_with_pipeline(
        &self,
        protocol: SegmentedProtocol,
        token: CancellationToken,
        hls_stream: impl futures::Stream<
            Item = std::result::Result<HlsData, mesio::hls::HlsDownloaderError>,
        > + Send
        + Unpin,
        first_segment: HlsData,
        extension: &str,
        lifecycle: SessionLifecycle,
    ) -> std::result::Result<DownloadStats, EngineStartError> {
        let config = self.handle.config_snapshot();
        let streamer_id = config.streamer_id.clone();
        info!(
            "Starting {} download with pipeline processing for {}",
            protocol.label(),
            streamer_id
        );

        // Build pipeline and common configs
        let pipeline_config = config.build_pipeline_config();
        let hls_pipeline_config = config.build_hls_pipeline_config();

        // Create StreamerContext with cancellation token
        let context = Arc::new(
            StreamerContext::with_name(&streamer_id, token.clone())
                .with_manual_split(self.handle.manual_split.clone()),
        );
        self.handle.manual_split.enable();

        // Create HlsPipeline using PipelineProvider::with_config
        let pipeline_provider =
            HlsPipeline::with_config(context, &pipeline_config, hls_pipeline_config);

        let pipeline = pipeline_provider.build_pipeline();

        let pipeline_common::SpawnedPipeline {
            input_tx: pipeline_input_tx,
            output_rx: pipeline_output_rx,
            tasks: processing_tasks,
        } = spawn_pipeline(
            pipeline,
            HlsPipeline::channel_spec(pipeline_config.channel_size),
        );

        // Create HlsWriter with callbacks
        let max_file_size = if config.max_segment_size_bytes > 0 {
            Some(config.max_segment_size_bytes)
        } else {
            None
        };

        let mut writer = HlsWriter::new(HlsWriterConfig {
            output_dir: config.output_dir.clone(),
            base_name: config.filename_template.clone(),
            extension: extension.to_string(),
            max_file_size,
        });

        helpers::setup_writer_callbacks(&mut writer, &self.handle.event_tx);

        // Spawn blocking writer task that reads from pipeline output
        let writer_task = tokio::task::spawn_blocking(move || writer.run(pipeline_output_rx));

        let (stream_error, engine_signal) = self
            .consume_session(
                protocol,
                &token,
                hls_stream,
                first_segment,
                pipeline_input_tx,
                lifecycle,
            )
            .await;

        helpers::handle_writer_result(
            writer_task,
            stream_error,
            processing_tasks,
            helpers::WriterSettleContext {
                managed_credentials: config.managed_credentials,
                event_tx: &self.handle.event_tx,
                streamer_id: &streamer_id,
                protocol: protocol.label(),
                output_dir: &config.output_dir,
                engine_signal,
            },
        )
        .await
        .map_err(EngineStartError::from)
    }

    /// Download HLS stream without pipeline processing (raw mode).
    ///
    /// Sends stream data directly to HlsWriter without any processing.
    async fn download_raw(
        &self,
        protocol: SegmentedProtocol,
        token: CancellationToken,
        hls_stream: impl futures::Stream<
            Item = std::result::Result<HlsData, mesio::hls::HlsDownloaderError>,
        > + Send
        + Unpin,
        first_segment: HlsData,
        extension: &str,
        lifecycle: SessionLifecycle,
    ) -> std::result::Result<DownloadStats, EngineStartError> {
        let config = self.handle.config_snapshot();
        let streamer_id = config.streamer_id.clone();
        info!(
            "Starting {} download without pipeline processing for {}",
            protocol.label(),
            streamer_id
        );

        // Build pipeline config for channel size
        let pipeline_config = config.build_pipeline_config();
        let channel_size = pipeline_config.channel_size;

        // Create channel for sending data to writer
        let (tx, rx) =
            tokio::sync::mpsc::channel::<std::result::Result<HlsData, PipelineError>>(channel_size);

        // Create HlsWriter with callbacks
        let max_file_size = if config.max_segment_size_bytes > 0 {
            Some(config.max_segment_size_bytes)
        } else {
            None
        };

        let mut writer = HlsWriter::new(HlsWriterConfig {
            output_dir: config.output_dir.clone(),
            base_name: config.filename_template.clone(),
            extension: extension.to_string(),
            max_file_size,
        });

        helpers::setup_writer_callbacks(&mut writer, &self.handle.event_tx);

        // Spawn blocking writer task
        let writer_task = tokio::task::spawn_blocking(move || writer.run(rx.into()));

        let (stream_error, engine_signal) = self
            .consume_session(protocol, &token, hls_stream, first_segment, tx, lifecycle)
            .await;

        helpers::handle_writer_result(
            writer_task,
            stream_error,
            vec![],
            helpers::WriterSettleContext {
                managed_credentials: config.managed_credentials,
                event_tx: &self.handle.event_tx,
                streamer_id: &streamer_id,
                protocol: protocol.label(),
                output_dir: &config.output_dir,
                engine_signal,
            },
        )
        .await
        .map_err(EngineStartError::from)
    }

    /// Forwards the session's items, starting with the peeked one, to the
    /// writer side through `tx`, then settles the session and concludes how
    /// the recording ended (see [`conclude_session`]).
    async fn consume_session(
        &self,
        protocol: SegmentedProtocol,
        token: &CancellationToken,
        hls_stream: impl futures::Stream<
            Item = std::result::Result<HlsData, mesio::hls::HlsDownloaderError>,
        > + Send
        + Unpin,
        first_segment: HlsData,
        tx: impl helpers::StreamSender<HlsData>,
        lifecycle: SessionLifecycle,
    ) -> (Option<(DownloadFailureKind, String)>, EngineEndSignal) {
        let config = self.handle.config_snapshot();

        // Tap each item to detect the `EndOfStream` marker mesio emits only
        // when the platform ended the stream (HLS `#EXT-X-ENDLIST`, the end of
        // a static or ended DASH presentation).
        let saw_end_of_stream = Arc::new(AtomicBool::new(false));
        let saw_end_of_stream_for_inspect = Arc::clone(&saw_end_of_stream);
        let items_exhausted = Arc::new(AtomicBool::new(false));
        let items_exhausted_for_tail = Arc::clone(&items_exhausted);

        // Feed the peeked segment through the same cancellation-aware path as
        // the rest of the stream. This prevents a full pipeline input from
        // pinning shutdown before the normal settlement path can join writers.
        // The empty tail runs only once the session's items have run out.
        let hls_stream = futures::stream::once(std::future::ready(Ok::<
            HlsData,
            mesio::hls::HlsDownloaderError,
        >(first_segment)))
        .chain(hls_stream)
        .chain(
            futures::stream::once(std::future::ready(())).filter_map(move |()| {
                items_exhausted_for_tail.store(true, Ordering::Relaxed);
                std::future::ready(None)
            }),
        );
        let stream_error = helpers::consume_stream(
            hls_stream,
            &tx,
            helpers::StreamConsumeContext {
                managed_credentials: config.managed_credentials,
                parent_token: &self.handle.cancellation_token,
                child_token: token,
                streamer_id: &config.streamer_id,
                protocol: protocol.label(),
            },
            classify_download_error,
            move |item: &HlsData| {
                if matches!(item, HlsData::EndMarker(Some(SplitReason::EndOfStream))) {
                    saw_end_of_stream_for_inspect.store(true, Ordering::Relaxed);
                }
            },
        )
        .await;

        // Closing the input lets the writer finish while the session settles.
        drop(tx);

        settle_session(
            SettleContext {
                protocol,
                parent_token: &self.handle.cancellation_token,
                child_token: token,
                streamer_id: &config.streamer_id,
                managed_credentials: config.managed_credentials,
            },
            lifecycle,
            items_exhausted.load(Ordering::Relaxed),
            saw_end_of_stream.load(Ordering::Relaxed),
            stream_error,
        )
        .await
    }
}

/// Per-download context for [`settle_session`].
struct SettleContext<'a> {
    protocol: SegmentedProtocol,
    parent_token: &'a CancellationToken,
    child_token: &'a CancellationToken,
    streamer_id: &'a str,
    managed_credentials: bool,
}

/// Settles the session once its items were consumed and concludes how the
/// recording ended (see [`conclude_session`]). Cancellation is read after
/// settling, so a download cancelled while its session finishes keeps the
/// cancellation outcome even if a task fails as it stops.
async fn settle_session(
    context: SettleContext<'_>,
    lifecycle: SessionLifecycle,
    items_exhausted: bool,
    saw_end_of_stream: bool,
    stream_error: Option<(DownloadFailureKind, String)>,
) -> (Option<(DownloadFailureKind, String)>, EngineEndSignal) {
    let protocol = context.protocol;
    let session_end = lifecycle.settle(items_exhausted).await;
    match &session_end {
        SessionEnd::Failed(reason) => warn!(
            protocol = protocol.label(),
            streamer_id = %context.streamer_id,
            reason = %sanitize_engine_message(reason, context.managed_credentials),
            "Download session reported a failed engine task"
        ),
        SessionEnd::TimedOut => warn!(
            protocol = protocol.label(),
            streamer_id = %context.streamer_id,
            "Download session did not finish within the settlement window"
        ),
        _ => {}
    }
    let observation = SessionObservation {
        items_exhausted,
        saw_end_of_stream,
        cancelled: context.parent_token.is_cancelled() || context.child_token.is_cancelled(),
    };
    conclude_session(protocol, &observation, stream_error, &session_end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::engine::traits::{DownloadConfig, EngineType};
    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn initial_segment_cancellation_is_typed_for_both_tokens() {
        for cancel_parent in [true, false] {
            let parent = CancellationToken::new();
            let session = parent.child_token();
            let stopped = if cancel_parent {
                parent.clone()
            } else {
                session.clone()
            };
            let (result, ()) = timeout(Duration::from_secs(2), async {
                tokio::join!(
                    next_initial_item(
                        std::future::pending::<()>(),
                        &parent,
                        &session,
                        SegmentedProtocol::Hls,
                    ),
                    async {
                        tokio::task::yield_now().await;
                        stopped.cancel();
                    },
                )
            })
            .await
            .expect("early cancellation remains bounded");
            let error = result.unwrap_err();
            assert_eq!(error.kind, DownloadFailureKind::Cancelled);
            assert!(!error.kind.affects_circuit_breaker());
        }
    }

    #[tokio::test]
    async fn download_raw_emits_segment_completed_before_download_failed_on_stream_error() {
        let temp = tempfile::tempdir().expect("tempdir");

        let config = DownloadConfig::new(
            "http://example.invalid/stream.m3u8",
            temp.path().to_path_buf(),
            "streamer",
            "streamer",
            "session",
        )
        .with_filename_template("test-hls");

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<SegmentEvent>(32);
        let handle = DownloadHandle::new("download", EngineType::Mesio, config, event_tx);
        let downloader = HlsDownloader::new(Arc::new(handle), MesioEngineConfig::default(), None);

        let first_segment = HlsData::ts(
            MediaSegment {
                uri: "seg0.ts".to_string(),
                duration: 1.0,
                ..Default::default()
            },
            Bytes::from_static(&[0_u8; 188]),
        );

        let hls_stream = futures::stream::iter([
            Ok(HlsData::ts(
                MediaSegment {
                    uri: "seg1.ts".to_string(),
                    duration: 1.0,
                    ..Default::default()
                },
                Bytes::from_static(&[1_u8; 188]),
            )),
            Err(mesio::hls::HlsDownloaderError::Playlist {
                reason: "simulated stream error".to_string(),
            }),
        ]);

        let events_task = tokio::spawn(async move {
            let mut events = Vec::new();
            loop {
                let next = timeout(Duration::from_secs(5), event_rx.recv())
                    .await
                    .expect("event recv timeout");
                let Some(ev) = next else {
                    break;
                };
                events.push(ev.clone());
                if matches!(ev, SegmentEvent::DownloadFailed { .. }) {
                    break;
                }
            }
            events
        });

        let result = downloader
            .download_raw(
                SegmentedProtocol::Hls,
                CancellationToken::new(),
                hls_stream,
                first_segment,
                "ts",
                scripted_lifecycle(None).0,
            )
            .await;
        assert!(result.is_err(), "expected stream error");

        let events = events_task.await.expect("events task join");

        let completed_idx = events
            .iter()
            .position(|e| matches!(e, SegmentEvent::SegmentCompleted(_)))
            .expect("expected SegmentCompleted");
        let failed_idx = events
            .iter()
            .position(|e| matches!(e, SegmentEvent::DownloadFailed { .. }))
            .expect("expected DownloadFailed");

        assert!(
            completed_idx < failed_idx,
            "expected SegmentCompleted before DownloadFailed, got: {:?}",
            events
                .iter()
                .map(|e| match e {
                    SegmentEvent::SegmentStarted { .. } => "SegmentStarted",
                    SegmentEvent::SegmentCompleted(_) => "SegmentCompleted",
                    SegmentEvent::Progress(_) => "Progress",
                    SegmentEvent::DownloadCompleted { .. } => "DownloadCompleted",
                    SegmentEvent::DownloadFailed { .. } => "DownloadFailed",
                    SegmentEvent::OutputIoError { .. } => "OutputIoError",
                })
                .collect::<Vec<_>>()
        );
    }

    /// A session lifecycle that reports `joined` and records whether it was
    /// cancelled.
    fn scripted_lifecycle(
        joined: Option<Result<DownloadTerminal, DownloadError>>,
    ) -> (SessionLifecycle, Arc<AtomicBool>) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_by = Arc::clone(&cancelled);
        (
            SessionLifecycle {
                cancel: Box::new(move || cancelled_by.store(true, Ordering::Relaxed)),
                join: Box::pin(async move { joined }),
            },
            cancelled,
        )
    }

    fn ts_segment(uri: &str) -> HlsData {
        HlsData::ts(
            MediaSegment {
                uri: uri.to_string(),
                duration: 1.0,
                ..Default::default()
            },
            Bytes::from_static(&[0_u8; 188]),
        )
    }

    /// Records `rest` after a first TS segment in raw mode and returns the
    /// result with the terminal segment event.
    async fn record_raw(
        protocol: SegmentedProtocol,
        rest: impl futures::Stream<Item = Result<HlsData, DownloadError>> + Send + Unpin,
        lifecycle: SessionLifecycle,
        parent: CancellationToken,
    ) -> (
        std::result::Result<DownloadStats, EngineStartError>,
        SegmentEvent,
    ) {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = DownloadConfig::new(
            "http://example.invalid/stream",
            temp.path().to_path_buf(),
            "streamer",
            "streamer",
            "session",
        )
        .with_filename_template("test-session-end");
        let (event_tx, mut event_rx) = mpsc::channel::<SegmentEvent>(64);
        let mut handle = DownloadHandle::new("download", EngineType::Mesio, config, event_tx);
        handle.cancellation_token = parent.clone();
        let downloader = HlsDownloader::new(Arc::new(handle), MesioEngineConfig::default(), None);
        let result = timeout(
            Duration::from_secs(10),
            downloader.download_raw(
                protocol,
                parent.child_token(),
                rest,
                ts_segment("seg0.ts"),
                "ts",
                lifecycle,
            ),
        )
        .await
        .expect("recording settles");
        let mut terminal = None;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(
                event,
                SegmentEvent::DownloadCompleted { .. } | SegmentEvent::DownloadFailed { .. }
            ) {
                terminal = Some(event);
            }
        }
        (result, terminal.expect("a terminal segment event"))
    }

    fn ended_items() -> impl futures::Stream<Item = Result<HlsData, DownloadError>> + Send + Unpin {
        futures::stream::iter([
            Ok(ts_segment("seg1.ts")),
            Ok(HlsData::end_marker_with_reason(SplitReason::EndOfStream)),
        ])
    }

    #[tokio::test]
    async fn end_of_stream_with_a_failed_session_is_a_processing_failure() {
        let (lifecycle, cancelled) = scripted_lifecycle(Some(Ok(DownloadTerminal::PipelineError(
            Arc::from("DASH assembler task failed"),
        ))));
        let (result, terminal) = record_raw(
            SegmentedProtocol::Dash,
            ended_items(),
            lifecycle,
            CancellationToken::new(),
        )
        .await;

        // The terminal event carries the classification, as for stream
        // errors; it is queued before the engine returns, so it wins.
        assert!(result.is_err());
        match terminal {
            SegmentEvent::DownloadFailed { kind, message } => {
                assert_eq!(kind, DownloadFailureKind::Processing);
                assert!(message.contains("DASH session failed"), "{message}");
            }
            other => panic!("expected DownloadFailed, got {other:?}"),
        }
        assert!(
            !cancelled.load(Ordering::Relaxed),
            "a session that ran out of items is joined, not cancelled"
        );

        // A panicked lifecycle task is a failure too.
        let (lifecycle, _) = scripted_lifecycle(Some(Err(DownloadError::Internal {
            reason: "download lifecycle task failed: panicked".to_string(),
        })));
        let (result, terminal) = record_raw(
            SegmentedProtocol::Dash,
            ended_items(),
            lifecycle,
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err());
        assert!(
            matches!(
                terminal,
                SegmentEvent::DownloadFailed {
                    kind: DownloadFailureKind::Processing,
                    ..
                }
            ),
            "{terminal:?}"
        );
    }

    #[tokio::test]
    async fn cancellation_keeps_its_outcome_whatever_the_session_reports() {
        let parent = CancellationToken::new();
        let (lifecycle, cancelled) = scripted_lifecycle(Some(Ok(DownloadTerminal::PipelineError(
            Arc::from("task failed while stopping"),
        ))));
        let stop = parent.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            stop.cancel();
        });
        let (result, terminal) = record_raw(
            SegmentedProtocol::Dash,
            futures::stream::pending(),
            lifecycle,
            parent,
        )
        .await;

        assert!(result.is_ok());
        assert!(
            matches!(
                terminal,
                SegmentEvent::DownloadCompleted {
                    engine_signal: EngineEndSignal::CleanDisconnect,
                    ..
                }
            ),
            "{terminal:?}"
        );
        assert!(
            cancelled.load(Ordering::Relaxed),
            "an abandoned session is cancelled"
        );
    }

    #[test]
    fn session_conclusions() {
        let ended = SessionObservation {
            items_exhausted: true,
            saw_end_of_stream: true,
            cancelled: false,
        };
        let failed = SessionEnd::Failed("task failed".to_string());
        let conclude = |observation: &SessionObservation, error, end: &SessionEnd| {
            conclude_session(SegmentedProtocol::Dash, observation, error, end)
        };

        // A typed stream error is kept over the session's report.
        let typed = Some((DownloadFailureKind::Network, "playlist".to_string()));
        assert_eq!(conclude(&ended, typed.clone(), &failed).0, typed);

        // An end the session cannot confirm is not authoritative.
        for end in [
            SessionEnd::Stopped,
            SessionEnd::TimedOut,
            SessionEnd::Unreported,
            failed.clone(),
        ] {
            assert_eq!(
                conclude(&ended, None, &end).1,
                EngineEndSignal::CleanDisconnect,
                "{end:?}"
            );
        }
        assert_eq!(
            conclude(&ended, None, &SessionEnd::AuthoritativeEnd).1,
            EngineEndSignal::DashEnd
        );

        // Without the marker a confirmed end stays a clean disconnect.
        let no_marker = SessionObservation {
            saw_end_of_stream: false,
            ..ended
        };
        assert_eq!(
            conclude(&no_marker, None, &SessionEnd::AuthoritativeEnd),
            (None, EngineEndSignal::CleanDisconnect)
        );

        // Items abandoned early (writer failure) leave the error to the writer.
        let abandoned = SessionObservation {
            items_exhausted: false,
            saw_end_of_stream: false,
            cancelled: false,
        };
        assert_eq!(conclude(&abandoned, None, &failed).0, None);
    }

    /// A lifecycle whose join is `join` and whose cancel records when it was
    /// called and cancels `session`.
    fn timed_lifecycle(
        join: impl std::future::Future<Output = Option<Result<DownloadTerminal, DownloadError>>>
        + Send
        + 'static,
        session: CancellationToken,
    ) -> (
        SessionLifecycle,
        Arc<parking_lot::Mutex<Option<tokio::time::Instant>>>,
    ) {
        let cancelled_at = Arc::new(parking_lot::Mutex::new(None));
        let record = Arc::clone(&cancelled_at);
        (
            SessionLifecycle {
                cancel: Box::new(move || {
                    record.lock().get_or_insert_with(tokio::time::Instant::now);
                    session.cancel();
                }),
                join: Box::pin(join),
            },
            cancelled_at,
        )
    }

    fn settle_context<'a>(
        protocol: SegmentedProtocol,
        parent: &'a CancellationToken,
        child: &'a CancellationToken,
    ) -> SettleContext<'a> {
        SettleContext {
            protocol,
            parent_token: parent,
            child_token: child,
            streamer_id: "streamer",
            managed_credentials: false,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_finishing_within_the_window_decides_the_outcome() {
        for (protocol, signal) in [
            (SegmentedProtocol::Dash, EngineEndSignal::DashEnd),
            (SegmentedProtocol::Hls, EngineEndSignal::HlsEndlist),
        ] {
            let (parent, child) = (CancellationToken::new(), CancellationToken::new());
            let (lifecycle, cancelled_at) = timed_lifecycle(
                async {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    Some(Ok(DownloadTerminal::AuthoritativeEnd))
                },
                CancellationToken::new(),
            );
            let started = tokio::time::Instant::now();

            let concluded = settle_session(
                settle_context(protocol, &parent, &child),
                lifecycle,
                true,
                true,
                None,
            )
            .await;

            assert_eq!(concluded, (None, signal), "{protocol:?}");
            assert_eq!(started.elapsed(), Duration::from_millis(1500));
            assert!(cancelled_at.lock().is_none(), "not cancelled");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_that_never_finishes_is_cancelled_then_abandoned() {
        let (parent, child) = (CancellationToken::new(), CancellationToken::new());
        let (lifecycle, cancelled_at) =
            timed_lifecycle(std::future::pending(), CancellationToken::new());
        let started = tokio::time::Instant::now();

        let concluded = settle_session(
            settle_context(SegmentedProtocol::Dash, &parent, &child),
            lifecycle,
            true,
            true,
            None,
        )
        .await;

        assert_eq!(concluded, (None, EngineEndSignal::CleanDisconnect));
        let cancelled_at = cancelled_at.lock().expect("cancelled after the window");
        assert_eq!(cancelled_at - started, TASK_SETTLEMENT_TIMEOUT);
        assert_eq!(started.elapsed(), TASK_SETTLEMENT_TIMEOUT * 2);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_the_download_while_the_session_settles_keeps_cancellation() {
        let parent = CancellationToken::new();
        let child = parent.child_token();
        // Like a mesio session: its engine token is a child of the download's,
        // and a task fails as it stops.
        let engine = child.child_token();
        let stopped = engine.clone();
        let (lifecycle, _) = timed_lifecycle(
            async move {
                stopped.cancelled().await;
                Some(Ok(DownloadTerminal::PipelineError(Arc::from(
                    "DASH assembler task failed",
                ))))
            },
            engine,
        );
        let stop = parent.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            stop.cancel();
        });
        let started = tokio::time::Instant::now();

        let concluded = settle_session(
            settle_context(SegmentedProtocol::Dash, &parent, &child),
            lifecycle,
            true,
            true,
            None,
        )
        .await;

        assert_eq!(concluded, (None, EngineEndSignal::CleanDisconnect));
        assert_eq!(started.elapsed(), Duration::from_millis(500));
    }
}
