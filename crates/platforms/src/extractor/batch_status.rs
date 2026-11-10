//! Live/offline status for many streamers of one platform from a single request.
//!
//! A platform offers this beside its [`PlatformExtractor`](super::platform_extractor::PlatformExtractor)
//! when it has an anonymous endpoint that answers for a list of channels at once.
//! [`batch_probe_for_url`](super::factory::batch_probe_for_url) finds the probe for a streamer URL.

use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::Client;

use super::error::ExtractorError;

/// What a batch status call reports for one requested key.
///
/// A key the platform did not answer for is absent from the result instead, which means its status
/// is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchPresence {
    /// The platform lists the channel as live. The batch endpoints do not see everything the full
    /// extractor checks (a hidden or locked room, for example), so the extractor has the final say.
    Live,
    /// The platform lists the channel as not live.
    Offline,
}

/// A platform's batch live-status call.
///
/// Calls are anonymous: they carry no cookies or account tokens, so a failure is never evidence
/// against an account. HTTP failures map to the same [`ExtractorError`] variants as the platform's
/// extractor: an HTTP 429 is [`ExtractorError::RateLimited`], any other error status is
/// [`ExtractorError::HttpError`], and an error the platform reports in the response body is
/// [`ExtractorError::ValidationError`].
///
/// Probes set no timeout of their own, so the caller bounds each [`probe`](Self::probe) call.
/// The calls are anonymous only if the `client` passed in carries no cookie store and no default
/// authentication headers.
#[async_trait]
pub trait BatchStatusProbe: Send + Sync {
    /// The largest number of keys one [`probe`](Self::probe) call accepts.
    fn max_keys(&self) -> usize;

    /// The key that identifies the streamer at `url` in a batch call, or `None` when the URL
    /// cannot be batched.
    ///
    /// Two URLs for the same channel in the same form yield the same key.
    fn key_for_url(&self, url: &str) -> Option<String>;

    /// The status of each key in `keys`, which holds at most [`max_keys`](Self::max_keys) keys
    /// produced by [`key_for_url`](Self::key_for_url).
    ///
    /// The result is keyed by the requested keys. A key the platform did not answer for (an unknown
    /// room or login, for instance) is absent.
    async fn probe(
        &self,
        client: &Client,
        keys: &[String],
    ) -> Result<HashMap<String, BatchPresence>, ExtractorError>;
}
