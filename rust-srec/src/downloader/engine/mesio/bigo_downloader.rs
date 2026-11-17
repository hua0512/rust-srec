//! Native Bigo recording: a room's own stream from BIGO's app servers,
//! written through the same FLV processing and writer as HTTP FLV.

use std::sync::Arc;
use std::time::Duration;

use futures::TryStreamExt;
use mesio::DownloadError;
use platforms_parser::extractor::platforms::bigo::native::stream::{self, NativeRoom};
use platforms_parser::extractor::platforms::bigo::native::{NativeError, Route};

use super::flv_downloader::FlvDownloader;
use super::helpers::DownloadStats;
use crate::database::models::engine::MesioEngineConfig;
use crate::downloader::engine::traits::{DownloadFailureKind, DownloadHandle, EngineStartError};
use crate::proxies::{ProxyTarget, SystemProxy};

pub(super) struct BigoDownloader {
    handle: Arc<DownloadHandle>,
    engine_config: MesioEngineConfig,
    room: NativeRoom,
}

impl BigoDownloader {
    pub(super) fn new(
        handle: Arc<DownloadHandle>,
        engine_config: MesioEngineConfig,
        room: NativeRoom,
    ) -> Self {
        Self {
            handle,
            engine_config,
            room,
        }
    }

    /// Records the room until it ends or its stream fails. The end of a
    /// broadcast, including a stream that stops sending media after it ran
    /// stably, is a clean disconnect, which leaves the session's end to the
    /// next status check.
    pub(super) async fn run(self) -> Result<DownloadStats, EngineStartError> {
        let route = route(&self.handle.config_snapshot().proxy)?;
        let token = self.handle.cancellation_token.child_token();
        let flv =
            stream::open(self.room.room_id, self.room.owner_uid, route).map_err(download_error);
        FlvDownloader::new(self.handle, self.engine_config)
            .download(token, Box::pin(flv))
            .await
    }
}

/// The download's route for native connections. A proxy the tunnel cannot
/// use fails the download rather than connecting directly.
fn route(proxy: &ProxyTarget) -> Result<Route, EngineStartError> {
    let unusable = |error: &dyn std::fmt::Display| {
        EngineStartError::new(
            DownloadFailureKind::Configuration,
            format!("The proxy cannot be used for the native Bigo stream: {error}"),
        )
    };
    let client = crate::utils::http_client::build_platforms_client(proxy, Duration::ZERO, 0)
        .map_err(|error| unusable(&error))?;
    let proxy = SystemProxy::current()
        .danmu_proxy_for(proxy)
        .map_err(|error| unusable(&error))?;
    Ok(Route { client, proxy })
}

/// Network failures stay retryable, and so does a stream this recorder cannot
/// decode: the session re-checks the room, and the web stream is offered
/// while the native one keeps failing.
fn download_error(error: NativeError) -> DownloadError {
    match error {
        NativeError::Unsupported(what) => {
            DownloadError::source_exhausted(format!("unsupported native Bigo stream: {what}"))
        }
        NativeError::Timeout(what) => DownloadError::Timeout {
            reason: format!("native Bigo stream: timed out waiting for {what}"),
        },
        error => DownloadError::StreamNetwork {
            reason: format!("native Bigo stream: {error}"),
        },
    }
}
