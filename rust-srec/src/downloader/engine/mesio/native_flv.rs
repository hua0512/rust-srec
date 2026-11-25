//! Pluggable opener for native (non-HTTP) FLV streams written through
//! [`super::flv_downloader::FlvDownloader`].

use std::pin::Pin;

use futures::Stream;

use crate::domain::proxy::ProxyTarget;
use crate::downloader::engine::traits::EngineStartError;

/// Failure surfaced by a custom native FLV stream.
#[derive(Debug)]
pub enum NativeStreamError {
    /// Stream format or protocol feature not supported by the native decoder.
    Unsupported(String),
    /// Timed out waiting for connection or media frames.
    Timeout(String),
    /// Retryable network or transport failure.
    Network(String),
}

/// Boxed asynchronous stream of FLV packets supplied by a native opener.
pub type NativeFlvStream =
    Pin<Box<dyn Stream<Item = std::result::Result<flv::FlvData, NativeStreamError>> + Send>>;

/// Factory injected into [`super::MesioEngine`] for non-HTTP native FLV URLs
/// (such as `bigo://room/...`) so the engine layer remains independent of
/// platform extractors.
pub trait NativeFlvOpener: Send + Sync {
    /// Whether this opener handles `url`.
    fn can_open(&self, url: &str) -> bool;

    /// Open the native FLV stream for `url` using `proxy`.
    fn open(
        &self,
        url: &str,
        proxy: &ProxyTarget,
    ) -> std::result::Result<NativeFlvStream, EngineStartError>;
}
