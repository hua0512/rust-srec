//! Native Bigo stream opener implementing [`NativeFlvOpener`] via
//! `platforms-parser`.

use std::time::Duration;

use futures::TryStreamExt;
use platforms_parser::extractor::platforms::bigo::native::stream::{self, NativeRoom};
use platforms_parser::extractor::platforms::bigo::native::{NativeError, Route};

use crate::downloader::engine::{
    DownloadFailureKind, EngineStartError, NativeFlvOpener, NativeFlvStream, NativeStreamError,
};
use crate::proxies::{build_platforms_client, danmu_proxy_for};
use srec_core::domain::proxy::{ProxyTarget, SystemProxy};

/// Opens `bigo://room/...` URLs over BIGO's native media protocol and yields
/// raw FLV packets for `MesioEngine`'s FLV writer pipeline.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct BigoNativeFlvOpener;

impl NativeFlvOpener for BigoNativeFlvOpener {
    fn can_open(&self, url: &str) -> bool {
        NativeRoom::parse(url).is_some()
    }

    fn open(
        &self,
        url: &str,
        proxy: &ProxyTarget,
    ) -> std::result::Result<NativeFlvStream, EngineStartError> {
        let Some(room) = NativeRoom::parse(url) else {
            return Err(EngineStartError::new(
                DownloadFailureKind::Configuration,
                format!("Invalid native Bigo room URL: {url}"),
            ));
        };
        let route = native_route(proxy)?;
        let flv = stream::open(room.room_id, room.owner_uid, route).map_err(map_native_error);
        Ok(Box::pin(flv))
    }
}

/// The download's route for native connections. A proxy the tunnel cannot
/// use fails the download rather than connecting directly.
fn native_route(proxy: &ProxyTarget) -> Result<Route, EngineStartError> {
    let unusable = |error: &dyn std::fmt::Display| {
        EngineStartError::new(
            DownloadFailureKind::Configuration,
            format!("The proxy cannot be used for the native Bigo stream: {error}"),
        )
    };
    let client =
        build_platforms_client(proxy, Duration::ZERO, 0).map_err(|error| unusable(&error))?;
    let proxy = danmu_proxy_for(SystemProxy::current(), proxy).map_err(|error| unusable(&error))?;
    Ok(Route { client, proxy })
}

/// Network failures stay retryable, and so does a stream this recorder cannot
/// decode: the session re-checks the room, and the web stream is offered
/// while the native one keeps failing.
fn map_native_error(error: NativeError) -> NativeStreamError {
    match error {
        NativeError::Unsupported(what) => NativeStreamError::Unsupported(what.to_owned()),
        NativeError::Timeout(what) => NativeStreamError::Timeout(what.to_owned()),
        error => NativeStreamError::Network(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::downloader::engine::{
        DownloadConfig, DownloadEngine, DownloadHandle, DownloadProtocol, EngineType, MesioEngine,
    };
    use srec_core::domain::proxy::{ProxyEndpoint, ProxyTarget};

    /// A native Bigo room goes to its own opener, which here refuses a
    /// proxy its connections cannot take before connecting anywhere. The
    /// HTTP FLV downloader, which the room's `Flv` label would pick, refuses
    /// the URL instead.
    #[tokio::test]
    async fn native_bigo_rooms_are_recorded_over_their_own_protocol() {
        let proxy = "ftp://127.0.0.1:9";
        let output = tempfile::tempdir().unwrap();
        let config = DownloadConfig::new(
            "bigo://room/1?uid=2",
            output.path(),
            "streamer",
            "streamer",
            "session",
        )
        .with_protocol(DownloadProtocol::Flv)
        .with_proxy(ProxyTarget::Explicit(ProxyEndpoint::from_url(proxy)));
        let (event_tx, _events) = tokio::sync::mpsc::channel(32);
        let handle = DownloadHandle::new("download", EngineType::Mesio, config, event_tx);

        let error = MesioEngine::new()
            .with_native_flv_opener(Arc::new(BigoNativeFlvOpener))
            .run(Arc::new(handle))
            .await
            .unwrap_err();
        assert_eq!(error.kind, DownloadFailureKind::Configuration);
        assert!(
            error.message.contains("native Bigo stream"),
            "{}",
            error.message
        );
    }
}
