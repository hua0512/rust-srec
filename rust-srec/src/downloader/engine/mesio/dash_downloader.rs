//! MPEG-DASH download orchestrator.
//!
//! This module provides the `DashDownloader` struct which starts mesio DASH
//! sessions. Their output is one fragmented MP4 stream of `HlsData` items,
//! so it is processed and written by the HLS path
//! ([`HlsDownloader::record`]) under the DASH label.

use futures::StreamExt;
use mesio::flv::FlvProtocolConfig;
use mesio::{
    DashConfig, DownloadEvent, DownloadEventStream, DownloadRequest, MesioConfig, MesioDownloader,
    ProtocolSelection,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::super::utils::sanitize_engine_message;
use super::classify_download_error;
use super::config::build_dash_config;
use super::helpers::DownloadStats;
use super::hls_downloader::{HlsDownloader, SegmentedProtocol, SessionLifecycle};
use crate::database::models::engine::MesioEngineConfig;
use crate::downloader::engine::traits::{DownloadHandle, EngineStartError};

/// MPEG-DASH download orchestrator.
///
/// Starts a DASH session with the engine's DASH settings and records it
/// through the shared fMP4 writer path, with or without pipeline processing.
pub(super) struct DashDownloader {
    /// The download attempt: shared configuration, event sender, cancellation
    /// token and manual-split control.
    handle: Arc<DownloadHandle>,
    /// Engine-specific configuration.
    engine_config: MesioEngineConfig,
    /// Base DASH configuration from the engine.
    dash_config: Option<DashConfig>,
}

impl DashDownloader {
    /// Create a new DASH downloader.
    ///
    /// # Arguments
    /// * `handle` - The download attempt to record.
    /// * `engine_config` - Mesio engine-specific configuration.
    /// * `dash_config` - Optional base DASH configuration from the engine.
    pub(super) fn new(
        handle: Arc<DownloadHandle>,
        engine_config: MesioEngineConfig,
        dash_config: Option<DashConfig>,
    ) -> Self {
        Self {
            handle,
            engine_config,
            dash_config,
        }
    }

    /// Run the DASH download, consuming the stream and writing to files.
    ///
    /// Returns download statistics on success.
    pub(super) async fn run(self) -> std::result::Result<DownloadStats, EngineStartError> {
        let token = self.handle.cancellation_token.child_token();
        let config = self.handle.config_snapshot();

        let dash_config = build_dash_config(&config, self.dash_config.clone(), &self.engine_config)
            .map_err(|e| {
                EngineStartError::new(
                    classify_download_error(&e),
                    format!("Invalid DASH configuration: {e}"),
                )
            })?;
        let downloader = MesioDownloader::new(MesioConfig {
            hls: Default::default(),
            flv: FlvProtocolConfig::default(),
            dash: dash_config,
            token: token.clone(),
        });

        let request = DownloadRequest::from_url(&config.url)
            .map_err(|e| {
                EngineStartError::new(
                    classify_download_error(&e),
                    format!("Invalid DASH download URL: {e}"),
                )
            })?
            .with_protocol(ProtocolSelection::Dash(Default::default()))
            .with_cancel(token.clone());

        let session = downloader.start_dash(request).await.map_err(|e| {
            EngineStartError::new(
                classify_download_error(&e),
                format!("Failed to start DASH download: {e}"),
            )
        })?;

        // The observer ends with the session's event stream, or when this
        // download returns, whichever comes first.
        let observer_stop = CancellationToken::new();
        let _observer_guard = observer_stop.clone().drop_guard();
        tokio::spawn(observe_session_events(
            session.events,
            observer_stop,
            config.streamer_id.clone(),
            config.managed_credentials,
        ));

        HlsDownloader::new(self.handle, self.engine_config, None)
            .record(
                SegmentedProtocol::Dash,
                token,
                session.items,
                SessionLifecycle::from(session.handle),
            )
            .await
    }
}

/// Logs the DASH session events that change what is recorded: a track that
/// fell behind (its late data is dropped) or stopped contributing for the
/// rest of the session. Other events are download telemetry; recording
/// progress is reported by the writer.
async fn observe_session_events(
    mut events: DownloadEventStream,
    stop: CancellationToken,
    streamer_id: String,
    managed_credentials: bool,
) {
    loop {
        let event = tokio::select! {
            biased;
            event = events.next() => event,
            _ = stop.cancelled() => break,
        };
        let Some(event) = event else {
            break;
        };
        match event {
            DownloadEvent::TrackStalled { track, waited } => {
                warn!(
                    streamer_id = %streamer_id,
                    track = track.as_str(),
                    waited_ms = u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
                    "DASH track fell behind; recording continues without its late data"
                );
            }
            DownloadEvent::TrackDropped { track, reason } => {
                warn!(
                    streamer_id = %streamer_id,
                    track = track.as_str(),
                    reason = %sanitize_engine_message(&reason, managed_credentials),
                    "DASH track dropped; recording continues with the remaining track"
                );
            }
            DownloadEvent::Lagged { dropped } => {
                debug!(
                    streamer_id = %streamer_id,
                    dropped,
                    "DASH session events dropped before they were observed"
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use axum::Router;
    use axum::http::{StatusCode, Uri};
    use mesio::{EventSink, MediaTrack};
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    use super::*;
    use crate::downloader::EngineEndSignal;
    use crate::downloader::engine::traits::{DownloadConfig, EngineType, SegmentEvent};

    const FIXTURES: &[(&str, &[u8])] = &[
        (
            "v/init.mp4",
            include_bytes!("../../../../../crates/mp4/tests/fixtures/dash/video-init.mp4"),
        ),
        (
            "v/t0.m4s",
            include_bytes!("../../../../../crates/mp4/tests/fixtures/dash/video-1.m4s"),
        ),
        (
            "v/t12288.m4s",
            include_bytes!("../../../../../crates/mp4/tests/fixtures/dash/video-2.m4s"),
        ),
        (
            "a/init.mp4",
            include_bytes!("../../../../../crates/mp4/tests/fixtures/dash/audio-init.mp4"),
        ),
        (
            "a/t0.m4s",
            include_bytes!("../../../../../crates/mp4/tests/fixtures/dash/audio-1.m4s"),
        ),
        (
            "a/t45056.m4s",
            include_bytes!("../../../../../crates/mp4/tests/fixtures/dash/audio-2.m4s"),
        ),
        (
            "a/t96256.m4s",
            include_bytes!("../../../../../crates/mp4/tests/fixtures/dash/audio-3.m4s"),
        ),
    ];

    /// The fixtures as a static presentation: two video segments and three
    /// audio segments addressed by `$Time$` timelines.
    const STATIC_MPD: &str = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2.1S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/t$Time$.m4s" initialization="v/init.mp4" timescale="12288">
        <SegmentTimeline><S t="0" d="12288" r="1"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v" bandwidth="40000" codecs="avc1.64000a"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="a/t$Time$.m4s" initialization="a/init.mp4" timescale="48000">
        <SegmentTimeline><S t="0" d="45056"/><S d="51200"/><S d="4096"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="a" bandwidth="24000" codecs="mp4a.40.2"/>
    </AdaptationSet>
  </Period>
</MPD>"#;

    /// Serves the static presentation at `/stream` and returns its URL. The
    /// URL has no `.mpd` extension: the DASH downloader records whatever it is
    /// given as DASH.
    async fn serve_static_presentation() -> String {
        let mut files: HashMap<String, Vec<u8>> = FIXTURES
            .iter()
            .map(|(path, body)| ((*path).to_owned(), body.to_vec()))
            .collect();
        files.insert("stream".to_owned(), STATIC_MPD.as_bytes().to_vec());
        let files = Arc::new(files);
        let app = Router::new().fallback(move |uri: Uri| {
            let files = Arc::clone(&files);
            async move {
                match files.get(uri.path().trim_start_matches('/')) {
                    Some(body) => (StatusCode::OK, body.clone()),
                    None => (StatusCode::NOT_FOUND, Vec::new()),
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test origin");
        let address = listener.local_addr().expect("origin address");
        tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                panic!("test origin failed: {error}");
            }
        });
        format!("http://{address}/stream")
    }

    async fn record_static_presentation(processing: bool) {
        let url = serve_static_presentation().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let config = DownloadConfig::new(
            url,
            temp.path().to_path_buf(),
            "streamer",
            "streamer",
            "session",
        )
        .with_filename_template("dash-test")
        .with_protocol(crate::downloader::DownloadProtocol::Dash)
        .with_processing_enabled(processing);
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let handle = Arc::new(DownloadHandle::new(
            "download",
            EngineType::Mesio,
            config,
            event_tx,
        ));
        let downloader = DashDownloader::new(handle.clone(), MesioEngineConfig::default(), None);

        let stats = timeout(Duration::from_secs(30), downloader.run())
            .await
            .expect("a static presentation ends")
            .expect("recorded");
        assert!(stats.total_bytes > 0);

        let mut completed = None;
        while let Ok(event) = event_rx.try_recv() {
            match event {
                SegmentEvent::DownloadCompleted { engine_signal, .. } => {
                    completed = Some(engine_signal)
                }
                SegmentEvent::DownloadFailed { message, .. } => panic!("failed: {message}"),
                _ => {}
            }
        }
        assert_eq!(completed, Some(EngineEndSignal::DashEnd));
        if processing {
            assert_ne!(
                handle.manual_split.snapshot().revision,
                0,
                "the pipeline drives the attempt's own manual-split control"
            );
        }

        let written: Vec<_> = std::fs::read_dir(temp.path())
            .expect("output directory")
            .map(|entry| entry.expect("entry").path())
            .collect();
        assert!(
            written
                .iter()
                .any(|path| path.extension().is_some_and(|ext| ext == "m4s")),
            "{written:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn static_presentation_is_recorded_raw_and_ends_authoritatively() {
        record_static_presentation(false).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn static_presentation_is_recorded_through_the_pipeline_and_ends_authoritatively() {
        record_static_presentation(true).await;
    }

    #[tokio::test]
    async fn event_observer_ends_with_the_session_or_when_stopped() {
        let (sink, events) = EventSink::channel(8);
        sink.emit(DownloadEvent::TrackStalled {
            track: MediaTrack::Audio,
            waited: Duration::from_secs(10),
        });
        sink.emit(DownloadEvent::TrackDropped {
            track: MediaTrack::Audio,
            reason: Arc::from("unsupported codec"),
        });
        drop(sink);
        timeout(
            Duration::from_secs(5),
            observe_session_events(events, CancellationToken::new(), "s".into(), false),
        )
        .await
        .expect("observer ends with the event stream");

        let (sink, events) = EventSink::channel(8);
        let stop = CancellationToken::new();
        let observer = tokio::spawn(observe_session_events(
            events,
            stop.clone(),
            "s".into(),
            false,
        ));
        stop.cancel();
        timeout(Duration::from_secs(5), observer)
            .await
            .expect("observer ends when stopped")
            .expect("observer does not panic");
        drop(sink);
    }

    #[tokio::test]
    async fn invalid_dash_settings_fail_as_configuration_before_any_request() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = DownloadConfig::new(
            "http://127.0.0.1:9/live.mpd",
            temp.path().to_path_buf(),
            "streamer",
            "streamer",
            "session",
        );
        let engine_config: MesioEngineConfig = serde_json::from_value(serde_json::json!({
            "dash": { "include_video": false, "include_audio": false }
        }))
        .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(8);
        let handle = DownloadHandle::new("download", EngineType::Mesio, config, event_tx);
        let downloader = DashDownloader::new(Arc::new(handle), engine_config, None);

        let error = timeout(Duration::from_secs(5), downloader.run())
            .await
            .expect("fails without network access")
            .expect_err("both tracks excluded");
        assert_eq!(
            error.kind,
            crate::downloader::DownloadFailureKind::Configuration
        );
        assert!(error.message.contains("DASH"), "{}", error.message);
    }
}
