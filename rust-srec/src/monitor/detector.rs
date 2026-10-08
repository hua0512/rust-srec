//! Individual stream detection.
//!
//! This module handles checking the live status of individual streamers.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use platforms_parser::extractor::error::ExtractorError;
use platforms_parser::extractor::factory::{ExtractorFactory, ExtractorSelection};
use platforms_parser::extractor::platform_extractor::PlatformExtractor;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace, warn};

use crate::Result;
use crate::domain::filter::{Filter, FilterType};
use crate::downloader::{StreamSelectionConfig, StreamSelector};
use crate::proxies::ProxyTarget;
use crate::streamer::StreamerMetadata;
use crate::utils::http_client::ClientCache;

/// Re-export StreamInfo from platforms_parser for convenience.
pub use platforms_parser::media::StreamInfo;

/// Live status of a streamer.
#[derive(Clone)]
pub enum LiveStatus {
    /// Streamer is currently live.
    Live {
        credential_binding: Option<Box<crate::credentials::CredentialBinding>>,
        credential_snapshot: Option<std::sync::Arc<crate::credentials::CredentialSnapshot>>,
        /// Stream title.
        title: String,
        /// Stream category (if available).
        category: Option<String>,
        /// Stream start time (if available).
        started_at: Option<DateTime<Utc>>,
        /// Viewer count (if available).
        viewer_count: Option<u64>,
        // Avatar url (if available)
        avatar: Option<String>,
        /// Stream information from platform parser (URLs, format, quality, headers).
        /// Note: Some platforms require calling get_url() to resolve the final URL.
        streams: Vec<StreamInfo>,
        /// HTTP headers extracted from MediaInfo.headers (user-agent, referer, etc.).
        /// These should be passed to download engines for platforms that require specific headers.
        media_headers: Option<HashMap<String, String>>,
        /// Additional platform-specific metadata extracted from MediaInfo.extras.
        media_extras: Option<Box<HashMap<String, String>>>,

        /// Hint for when to check next (used for boundary wakes).
        ///
        /// This is currently used to stop recording exactly at the end of a time-based
        /// schedule window while a download is active.
        next_check_hint: Option<DateTime<Utc>>,

        /// Total number of stream candidates the platform extractor returned
        /// for this poll, before selection narrowed the list to one. The
        /// `streams` field above carries only the chosen stream; this
        /// carries every candidate descriptor the extractor returned, used
        /// by the check-history strip's tooltip to show all available
        /// qualities/formats with the selected one marked.
        ///
        /// URLs in this list are unresolved (some platforms require a
        /// per-candidate `get_url()` call). The check-history writer
        /// projects each candidate to a `SelectedStreamSummary` that
        /// strips the URL before persistence — operator-facing diagnostic
        /// surfaces never see signed query params or CDN tokens.
        candidates: Vec<StreamInfo>,
    },
    /// Streamer is offline.
    Offline,
    /// Streamer is live but filtered out (e.g., out of schedule).
    Filtered {
        /// Reason for filtering.
        reason: FilterReason,
        /// Original live status.
        title: String,
        category: Option<String>,
    },
    /// Fatal error - streamer not found on platform.
    NotFound,
    /// Fatal error - streamer is banned on platform.
    Banned,
    /// Fatal error - content is age-restricted.
    AgeRestricted,
    /// Fatal error - content is region-locked.
    RegionLocked,
    /// Fatal error - content is private.
    Private,
    /// Fatal error - unsupported platform.
    UnsupportedPlatform,
}

impl std::fmt::Debug for LiveStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Live {
                credential_binding,
                streams,
                ..
            } => formatter
                .debug_struct("Live")
                .field("credential_binding", credential_binding)
                .field("stream_count", &streams.len())
                .finish_non_exhaustive(),
            Self::Filtered { reason, .. } => {
                formatter.debug_tuple("Filtered").field(reason).finish()
            }
            Self::Offline => formatter.write_str("Offline"),
            Self::NotFound => formatter.write_str("NotFound"),
            Self::Banned => formatter.write_str("Banned"),
            Self::AgeRestricted => formatter.write_str("AgeRestricted"),
            Self::RegionLocked => formatter.write_str("RegionLocked"),
            Self::Private => formatter.write_str("Private"),
            Self::UnsupportedPlatform => formatter.write_str("UnsupportedPlatform"),
        }
    }
}

/// Reason why a stream was filtered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterReason {
    /// Outside scheduled time window.
    OutOfSchedule {
        /// Next time the schedule window opens.
        next_available: Option<DateTime<Utc>>,
    },
    /// Title doesn't match keyword filter.
    TitleMismatch,
    /// Category doesn't match filter.
    CategoryMismatch,
}

impl LiveStatus {
    /// Check if the status indicates the streamer is live.
    pub fn is_live(&self) -> bool {
        matches!(self, LiveStatus::Live { .. })
    }

    /// Check if the status indicates the streamer is offline.
    pub fn is_offline(&self) -> bool {
        matches!(self, LiveStatus::Offline)
    }

    /// Check if the status was filtered.
    pub fn is_filtered(&self) -> bool {
        matches!(self, LiveStatus::Filtered { .. })
    }

    /// Check if the status indicates a fatal error.
    pub fn is_fatal_error(&self) -> bool {
        matches!(
            self,
            LiveStatus::NotFound
                | LiveStatus::Banned
                | LiveStatus::AgeRestricted
                | LiveStatus::RegionLocked
                | LiveStatus::Private
                | LiveStatus::UnsupportedPlatform
        )
    }

    /// Get the stream title if live.
    pub fn title(&self) -> Option<&str> {
        match self {
            LiveStatus::Live { title, .. } => Some(title),
            LiveStatus::Filtered { title, .. } => Some(title),
            _ => None,
        }
    }

    /// Get the stream category if available.
    pub fn category(&self) -> Option<&str> {
        match self {
            LiveStatus::Live { category, .. } => category.as_deref(),
            LiveStatus::Filtered { category, .. } => category.as_deref(),
            _ => None,
        }
    }

    /// Get a description of the fatal error, if any.
    pub fn fatal_error_description(&self) -> Option<&'static str> {
        match self {
            LiveStatus::NotFound => Some("Streamer not found on platform"),
            LiveStatus::Banned => Some("Streamer is banned on platform"),
            LiveStatus::AgeRestricted => Some("Content is age-restricted"),
            LiveStatus::RegionLocked => Some("Content is region-locked"),
            LiveStatus::Private => Some("Content is private"),
            LiveStatus::UnsupportedPlatform => Some("Platform is not supported"),
            _ => None,
        }
    }

    /// Map a fatal-error variant to its domain discriminator.
    /// Returns `None` for non-fatal variants (Live, Offline, Filtered).
    /// Used by the check-history writer to populate `fatal_kind` without
    /// hand-spelling the variant strings.
    pub fn fatal_kind(&self) -> Option<crate::domain::streamer::FatalErrorType> {
        use crate::domain::streamer::FatalErrorType;
        match self {
            LiveStatus::NotFound => Some(FatalErrorType::NotFound),
            LiveStatus::Banned => Some(FatalErrorType::Banned),
            LiveStatus::AgeRestricted => Some(FatalErrorType::AgeRestricted),
            LiveStatus::RegionLocked => Some(FatalErrorType::RegionLocked),
            LiveStatus::Private => Some(FatalErrorType::Private),
            LiveStatus::UnsupportedPlatform => Some(FatalErrorType::UnsupportedPlatform),
            _ => None,
        }
    }
}

impl FilterReason {
    /// Stable string discriminator for `streamer_check_history.filter_reason`.
    /// Returned as `&'static str` so callers don't pay an allocation per row.
    pub fn as_str(&self) -> &'static str {
        match self {
            FilterReason::OutOfSchedule { .. } => "OutOfSchedule",
            FilterReason::TitleMismatch => "TitleMismatch",
            FilterReason::CategoryMismatch => "CategoryMismatch",
        }
    }
}

/// Resolved configuration a liveness check needs beyond the streamer itself.
///
/// Grouped rather than passed as loose parameters because every field is read from the same
/// `MergedConfig` and they are always supplied together.
pub struct CheckContext<'a> {
    /// Cookies for the extractor request.
    pub cookies: Option<String>,
    /// Stream preferences, merged into `platform_extras` before extraction.
    pub selection_config: Option<&'a StreamSelectionConfig>,
    /// Platform-specific extractor options, merged from all config layers.
    pub platform_extras: Option<serde_json::Value>,
    /// How the extractor connects.
    pub proxy: &'a ProxyTarget,
    /// Which extractor resolves the stream URL.
    pub extractor: ExtractorSelection,
}

/// Stands in for extraction in tests; receives the cookies and the proxy the
/// extractor would have used.
#[cfg(test)]
pub(crate) type TestExtraction = std::sync::Arc<
    dyn Fn(Option<String>, ProxyTarget) -> futures::future::BoxFuture<'static, Result<LiveStatus>>
        + Send
        + Sync,
>;

/// Stream detector for checking live status.
pub struct StreamDetector {
    request_timeout: std::time::Duration,
    pool_max_idle_per_host: usize,
    /// Clients kept for distinct proxies at once.
    client_cache: ClientCache<ProxyTarget>,
    #[cfg(test)]
    pub(crate) test_extraction: Option<TestExtraction>,
}

impl StreamDetector {
    /// Create a new stream detector.
    pub fn new() -> Self {
        Self::with_http_config(std::time::Duration::ZERO, 0)
    }

    pub fn with_http_config(
        request_timeout: std::time::Duration,
        pool_max_idle_per_host: usize,
    ) -> Self {
        Self {
            request_timeout,
            pool_max_idle_per_host,
            client_cache: ClientCache::new(64),
            #[cfg(test)]
            test_extraction: None,
        }
    }

    fn client_for_proxy(&self, proxy: &ProxyTarget) -> Result<reqwest::Client> {
        Ok(self.client_cache.get_or_try_build(proxy, || {
            crate::utils::http_client::build_platforms_client(
                proxy,
                self.request_timeout,
                self.pool_max_idle_per_host,
            )
        })?)
    }

    /// Merge stream selection preferences into platform_extras.
    ///
    /// This allows platforms like Douyu to use the user's preferred CDN during extraction.
    /// Only values that aren't already set in platform_extras will be added.
    fn merge_selection_config_into_extras(
        platform_extras: Option<serde_json::Value>,
        selection_config: Option<&StreamSelectionConfig>,
    ) -> Option<serde_json::Value> {
        let Some(config) = selection_config else {
            return platform_extras;
        };

        // Start with existing extras or create an empty object
        let mut extras = match platform_extras {
            Some(serde_json::Value::Object(map)) => map,
            Some(other) => return Some(other), // Non-object, can't merge
            None => serde_json::Map::new(),
        };

        // Inject preferred CDN if configured and not already set
        // This is used by platforms like Douyu that need CDN during extraction
        if !config.preferred_cdns.is_empty() && !extras.contains_key("cdn") {
            extras.insert(
                "cdn".to_string(),
                serde_json::Value::String(config.preferred_cdns[0].clone()),
            );
            trace!(cdn = %config.preferred_cdns[0], "injecting cdn into platform extras");
        }

        // Inject preferred quality if configured and not already set
        // This is used by platforms like Bilibili that need quality during extraction
        if !config.preferred_qualities.is_empty() && !extras.contains_key("quality") {
            extras.insert(
                "quality".to_string(),
                serde_json::Value::String(config.preferred_qualities[0].clone()),
            );
        }

        if extras.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(extras))
        }
    }

    /// Check the live status of a streamer using resolved configuration.
    pub async fn check_status_with_cookies(
        &self,
        streamer: &StreamerMetadata,
        context: CheckContext<'_>,
    ) -> Result<LiveStatus> {
        #[cfg(test)]
        if let Some(extract) = &self.test_extraction {
            return extract(context.cookies, context.proxy.clone()).await;
        }
        let CheckContext {
            cookies,
            selection_config,
            platform_extras,
            proxy,
            extractor: selection,
        } = context;

        trace!(
            streamer_name = %streamer.name,
            streamer_url = %streamer.url,
            platform_extras = platform_extras.is_some(),
            "detector check"
        );

        // Merge CDN preference from selection_config into platform_extras
        // This allows platforms like Douyu to use the preferred CDN during extraction
        let merged_extras =
            Self::merge_selection_config_into_extras(platform_extras, selection_config);

        let extractor_factory =
            ExtractorFactory::new(self.client_for_proxy(proxy)?).with_proxy(proxy.clone());

        // Create platform extractor for this streamer's URL
        let extractor = match extractor_factory.create_extractor(
            &streamer.url,
            cookies,
            merged_extras,
            selection,
        ) {
            Ok(ext) => ext,
            Err(ExtractorError::UnsupportedExtractor) => {
                warn!("Unsupported platform for URL: {}", streamer.url);
                return Ok(LiveStatus::UnsupportedPlatform);
            }
            Err(e) => {
                return Err(e.into());
            }
        };

        self.check_status_with_extractor(streamer, selection_config, extractor.as_ref())
            .await
    }

    async fn check_status_with_extractor(
        &self,
        streamer: &StreamerMetadata,
        selection_config: Option<&StreamSelectionConfig>,
        extractor: &dyn PlatformExtractor,
    ) -> Result<LiveStatus> {
        // Extract media information
        let mut media_info = match extractor.extract().await {
            Ok(info) => info,
            // Fatal errors - these should stop monitoring
            Err(ExtractorError::StreamerNotFound) => {
                warn!("Streamer not found on platform: {}", streamer.name);
                return Ok(LiveStatus::NotFound);
            }
            Err(ExtractorError::StreamerBanned) => {
                warn!("Streamer is banned: {}", streamer.name);
                return Ok(LiveStatus::Banned);
            }
            Err(ExtractorError::AgeRestrictedContent) => {
                warn!("Age-restricted content: {}", streamer.name);
                return Ok(LiveStatus::AgeRestricted);
            }
            Err(ExtractorError::RegionLockedContent) => {
                warn!("Region-locked content: {}", streamer.name);
                return Ok(LiveStatus::RegionLocked);
            }
            Err(ExtractorError::PrivateContent) => {
                warn!("Private content: {}", streamer.name);
                return Ok(LiveStatus::Private);
            }
            // Non-fatal - streamer is just offline
            Err(ExtractorError::NoStreamsFound) => {
                trace!(
                    streamer_name = %streamer.name,
                    streamer_url = %streamer.url,
                    reason = "no_streams",
                    "status=OFFLINE"
                );
                return Ok(LiveStatus::Offline);
            }
            // Transient errors - should be retried
            Err(e) => {
                return Err(e.into());
            }
        };

        trace!(
            streamer_name = %streamer.name,
            streamer_url = %streamer.url,
            title = %media_info.title,
            is_live = media_info.is_live,
            streams = media_info.streams.len(),
            has_headers = media_info.headers.is_some(),
            "media info"
        );

        if !media_info.streams.is_empty() {
            for (idx, s) in media_info.streams.iter().enumerate() {
                debug!(
                    streamer_name = %streamer.name,
                    idx,
                    quality = %s.quality,
                    stream_format = ?s.stream_format,
                    media_format = ?s.media_format,
                    bitrate = s.bitrate,
                    priority = s.priority,
                    codec = %s.codec,
                    fps = s.fps,
                    "extracted stream candidate"
                );
            }
        }

        if media_info.is_live {
            let category = media_info
                .category
                .as_ref()
                .filter(|c| !c.is_empty())
                .map(|c| c.join(", "));

            let viewer_count = media_info
                .extras
                .as_ref()
                .and_then(|extras| extras.get("viewer_count"))
                .and_then(|v| v.parse::<u64>().ok());

            // Extract HTTP headers from MediaInfo.headers for download engines
            let media_headers = media_info.headers.as_ref().map(|h| {
                let mut out = HashMap::with_capacity(h.len());
                out.extend(h.iter().map(|(k, v)| (k.clone(), v.clone())));
                out
            });

            // Extract additional extras from MediaInfo.extras
            let media_extras = media_info.extras.as_ref().map(|e| {
                let mut out = HashMap::with_capacity(e.len());
                out.extend(e.iter().map(|(k, v)| (k.clone(), v.clone())));
                out
            });

            if let Some(headers) = &media_headers {
                trace!(
                    streamer_name = %streamer.name,
                    streamer_url = %streamer.url,
                    count = headers.len(),
                    keys = ?headers.keys().collect::<Vec<_>>(),
                    "media headers extracted"
                );
            }

            // Select the best stream - always emit exactly one stream
            // Use config-based selection if provided, otherwise use default selector
            let selector = match selection_config {
                Some(config) => {
                    debug!(config = ?config, "stream selection config");
                    StreamSelector::with_config(config.clone())
                }
                None => StreamSelector::new(),
            };

            let candidates = selector.sort_candidates(&media_info.streams);
            let selected_stream = if let Some(stream) = candidates.first() {
                debug!(quality = %stream.quality, "selected stream candidate");
                (*stream).clone()
            } else if let Some(stream) = media_info.streams.first() {
                // Fallback: if no candidates match selection criteria, take the first available stream
                debug!(
                    streams = media_info.streams.len(),
                    "stream selection fallback (no candidates matched criteria)"
                );
                stream.clone()
            } else {
                // No streams available at all - treat as offline
                warn!(
                    "Streamer {} is reported as live but has no streams available. Treating as OFFLINE.",
                    streamer.name
                );
                return Ok(LiveStatus::Offline);
            };

            // Resolve final URL for the selected stream
            // Some platforms (Huya, Douyu, Bilibili) require get_url() to get the real stream URL
            // We iterate through candidates until we successfully resolve one
            // Build a slice of references to iterate over
            let fallback_candidates;
            let resolution_slice: &[&StreamInfo] = if candidates.is_empty() {
                fallback_candidates = [&selected_stream];
                &fallback_candidates
            } else {
                &candidates
            };

            let selected_stream = match resolve_candidates(extractor, resolution_slice).await {
                Ok(stream) => stream,
                Err(error) => return terminal_status(&error).map_or_else(|| Err(error.into()), Ok),
            };

            let streams = vec![selected_stream];

            // Take ownership of the full candidate list before `media_info`
            // is consumed by the constructor below. The check-history strip
            // shows all candidates with the selected one marked, so we
            // forward the full Vec rather than just a count. URLs here are
            // unresolved — that's fine, the writer strips them anyway.
            let candidates = std::mem::take(&mut media_info.streams);

            debug!(
                streamer_name = %streamer.name,
                streamer_url = %streamer.url,
                title = %media_info.title,
                category = ?category,
                viewers = ?viewer_count,
                streams = streams.len(),
                candidates = candidates.len(),
                media_headers = media_headers.as_ref().map(|h| h.len()).unwrap_or(0),
                extras = media_extras.as_ref().map(|e| e.len()).unwrap_or(0),
                "status=LIVE"
            );

            Ok(LiveStatus::Live {
                credential_binding: None,
                credential_snapshot: None,
                title: media_info.title,
                category,
                avatar: media_info.artist_url.clone(),
                started_at: None, // TODO: platforms crate doesn't provide start time
                viewer_count,
                streams,
                media_headers,
                media_extras: media_extras.map(Box::new),
                next_check_hint: None,
                candidates,
            })
        } else {
            trace!(
                streamer_name = %streamer.name,
                streamer_url = %streamer.url,
                "status=OFFLINE"
            );
            Ok(LiveStatus::Offline)
        }
    }

    /// Check status and apply filters.
    ///
    /// # Arguments
    /// * `streamer` - The streamer to check
    /// * `filters` - Filters to apply to the live status
    /// * `cookies` - Optional cookies to use for the request
    /// * `selection_config` - Optional stream selection configuration
    /// * `context` - Resolved configuration for the check
    pub async fn check_status_with_filters(
        &self,
        streamer: &StreamerMetadata,
        filters: &[Filter],
        context: CheckContext<'_>,
    ) -> Result<LiveStatus> {
        let status = self.check_status_with_cookies(streamer, context).await?;

        // If offline, no need to filter
        if status.is_offline() {
            return Ok(status);
        }

        // Apply filters
        if let LiveStatus::Live {
            title, category, ..
        } = &status
        {
            let now = Utc::now();
            let mut next_check_hint: Option<DateTime<Utc>> = None;

            for filter in filters {
                let matches = filter.matches(title, category.as_deref().unwrap_or(""), now);

                if !matches {
                    let reason = match filter.filter_type() {
                        FilterType::TimeBased | FilterType::Cron => {
                            let next_available = filter.next_match_time(now);
                            FilterReason::OutOfSchedule { next_available }
                        }
                        FilterType::Keyword => FilterReason::TitleMismatch,
                        FilterType::Category => FilterReason::CategoryMismatch,
                        FilterType::Regex => FilterReason::TitleMismatch,
                    };

                    return Ok(LiveStatus::Filtered {
                        reason,
                        title: title.clone(),
                        category: category.clone(),
                    });
                }

                // If the filter is time-based and currently matches, compute the end boundary
                // so the scheduler can perform a boundary wake while Live.
                if (filter.filter_type() == FilterType::TimeBased
                    || filter.filter_type() == FilterType::Cron)
                    && let Some(end_at) = filter.next_unmatch_time(now)
                {
                    next_check_hint = match next_check_hint {
                        Some(existing) => Some(std::cmp::min(existing, end_at)),
                        None => Some(end_at),
                    };
                }
            }

            // Attach the hint to the live status.
            if let LiveStatus::Live {
                title,
                category,
                started_at,
                viewer_count,
                avatar,
                streams,
                media_headers,
                media_extras,
                candidates,
                ..
            } = status
            {
                return Ok(LiveStatus::Live {
                    credential_binding: None,
                    credential_snapshot: None,
                    title,
                    category,
                    started_at,
                    viewer_count,
                    avatar,
                    streams,
                    media_headers,
                    media_extras,
                    next_check_hint,
                    candidates,
                });
            }
        }

        Ok(status)
    }
}

impl Default for StreamDetector {
    fn default() -> Self {
        Self::new()
    }
}

fn terminal_status(error: &ExtractorError) -> Option<LiveStatus> {
    match error {
        ExtractorError::NoStreamsFound => Some(LiveStatus::Offline),
        ExtractorError::StreamerNotFound => Some(LiveStatus::NotFound),
        ExtractorError::StreamerBanned => Some(LiveStatus::Banned),
        ExtractorError::AgeRestrictedContent => Some(LiveStatus::AgeRestricted),
        ExtractorError::RegionLockedContent => Some(LiveStatus::RegionLocked),
        ExtractorError::PrivateContent => Some(LiveStatus::Private),
        ExtractorError::UnsupportedExtractor => Some(LiveStatus::UnsupportedPlatform),
        _ => None,
    }
}

async fn resolve_candidates(
    extractor: &dyn PlatformExtractor,
    candidates: &[&StreamInfo],
) -> std::result::Result<StreamInfo, ExtractorError> {
    let mut last_error = None;
    for candidate in candidates {
        let mut stream = (*candidate).clone();
        match extractor.get_url(&mut stream).await {
            Ok(()) => return Ok(stream),
            Err(error) => {
                // Authentication and shared throttling must reach the operation
                // owner before another candidate can hide the provider evidence.
                if matches!(
                    error,
                    ExtractorError::Authentication { .. } | ExtractorError::RateLimited { .. }
                ) || terminal_status(&error).is_some()
                {
                    return Err(error);
                }
                debug!(quality = %candidate.quality, category = error.category(), "stream URL candidate failed");
                last_error = Some(error);
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| ExtractorError::Other("No URL resolution candidates".to_owned())))
}

#[cfg(test)]
mod tests {
    use super::*;

    use platforms_parser::media::{StreamFormat, formats::MediaFormat};

    struct FakeExtractor {
        base: platforms_parser::extractor::platform_extractor::Extractor,
        media: std::sync::Mutex<
            Option<std::result::Result<platforms_parser::media::MediaInfo, ExtractorError>>,
        >,
        urls: std::sync::Mutex<std::collections::VecDeque<std::result::Result<(), ExtractorError>>>,
    }

    impl FakeExtractor {
        fn new(
            media: std::result::Result<platforms_parser::media::MediaInfo, ExtractorError>,
            urls: Vec<std::result::Result<(), ExtractorError>>,
        ) -> Self {
            crate::utils::http_client::install_rustls_provider();
            Self {
                base: platforms_parser::extractor::platform_extractor::Extractor::new(
                    "fixture",
                    "https://example.com/channel",
                    reqwest::Client::new(),
                ),
                media: std::sync::Mutex::new(Some(media)),
                urls: std::sync::Mutex::new(urls.into()),
            }
        }
    }

    #[async_trait::async_trait]
    impl PlatformExtractor for FakeExtractor {
        fn get_extractor(&self) -> &platforms_parser::extractor::platform_extractor::Extractor {
            &self.base
        }

        async fn extract(
            &self,
        ) -> std::result::Result<platforms_parser::media::MediaInfo, ExtractorError> {
            self.media.lock().unwrap().take().unwrap()
        }

        async fn get_url(&self, _: &mut StreamInfo) -> std::result::Result<(), ExtractorError> {
            self.urls.lock().unwrap().pop_front().unwrap()
        }
    }

    #[tokio::test]
    async fn detector_keeps_initial_and_url_authentication_errors_typed() {
        let streamer =
            StreamerMetadata::from_db_model(&crate::database::models::StreamerDbModel::new(
                "fixture",
                "https://example.com/channel",
                "fixture-platform",
            ));
        let detector = StreamDetector::new();
        for during_resolution in [false, true] {
            let authentication = ExtractorError::Authentication {
                code: "-101".into(),
            };
            let extractor = if during_resolution {
                FakeExtractor::new(
                    Ok(platforms_parser::media::MediaInfo::builder(
                        "https://example.com/channel",
                        "fixture",
                        "fixture",
                    )
                    .is_live(true)
                    .streams(vec![create_test_stream()])
                    .build()),
                    vec![Err(authentication)],
                )
            } else {
                FakeExtractor::new(Err(authentication), vec![])
            };
            let error = detector
                .check_status_with_extractor(&streamer, None, &extractor)
                .await
                .unwrap_err();
            assert!(
                matches!(error, crate::Error::Extractor(ExtractorError::Authentication { code }) if code == "-101")
            );
        }
        let extractor = FakeExtractor::new(
            Ok(platforms_parser::media::MediaInfo::builder(
                "https://example.com/channel",
                "fixture",
                "fixture",
            )
            .is_live(true)
            .streams(vec![create_test_stream()])
            .build()),
            vec![Err(ExtractorError::Other("signed-url-secret".into()))],
        );
        let error = detector
            .check_status_with_extractor(&streamer, None, &extractor)
            .await
            .unwrap_err();
        assert!(matches!(error, crate::Error::Extractor(_)));
        assert!(!error.to_string().contains("signed-url-secret"));
    }

    #[tokio::test]
    async fn candidate_resolution_preserves_auth_and_throttle_evidence() {
        let failures = [
            ExtractorError::Authentication {
                code: "-101".into(),
            },
            ExtractorError::RateLimited {
                code: Some("429".into()),
                retry_after: Some(std::time::Duration::from_secs(3600)),
            },
            ExtractorError::RateLimited {
                code: None,
                retry_after: None,
            },
        ];
        let stream = create_test_stream();
        for error in failures {
            let category = error.category();
            let extractor = FakeExtractor::new(
                Ok(platforms_parser::media::MediaInfo::empty()),
                vec![Err(error), Ok(())],
            );
            let result = resolve_candidates(&extractor, &[&stream, &stream])
                .await
                .unwrap_err();
            assert_eq!(result.category(), category);
            assert_eq!(
                extractor.urls.lock().unwrap().len(),
                1,
                "must stop before hiding account or throttle evidence"
            );
        }
    }

    #[tokio::test]
    async fn candidate_failures_are_errors_but_positive_offline_is_terminal() {
        let stream = create_test_stream();
        let extractor = FakeExtractor::new(
            Ok(platforms_parser::media::MediaInfo::empty()),
            vec![
                Err(ExtractorError::ValidationError("bad candidate".into())),
                Err(ExtractorError::Other("transport failure".into())),
            ],
        );
        let error = resolve_candidates(&extractor, &[&stream, &stream])
            .await
            .unwrap_err();
        assert!(terminal_status(&error).is_none());
        assert!(matches!(
            crate::Error::from(error),
            crate::Error::Extractor(_)
        ));
        let extractor = FakeExtractor::new(
            Ok(platforms_parser::media::MediaInfo::empty()),
            vec![Err(ExtractorError::NoStreamsFound), Ok(())],
        );
        let error = resolve_candidates(&extractor, &[&stream, &stream])
            .await
            .unwrap_err();
        assert!(matches!(terminal_status(&error), Some(LiveStatus::Offline)));
        assert_eq!(extractor.urls.lock().unwrap().len(), 1);
    }

    fn create_test_stream() -> StreamInfo {
        StreamInfo {
            url: "https://example.com/stream.flv".to_string(),
            stream_format: StreamFormat::Flv,
            media_format: MediaFormat::Flv,
            quality: "best".to_string(),
            bitrate: 5000000,
            priority: 1,
            extras: None,
            codec: "h264".to_string(),
            fps: 30.0,
            is_headers_needed: false,
            is_audio_only: false,
        }
    }

    #[test]
    fn test_live_status_is_live() {
        let status = LiveStatus::Live {
            credential_binding: None,
            credential_snapshot: None,
            title: "Test Stream".to_string(),
            category: Some("Gaming".to_string()),
            started_at: None,
            viewer_count: None,
            avatar: None,
            streams: vec![create_test_stream()],
            media_headers: None,
            media_extras: None,
            next_check_hint: None,
            candidates: vec![],
        };
        assert!(status.is_live());
        assert!(!status.is_offline());
        assert!(!status.is_filtered());
    }

    #[test]
    fn test_live_status_is_offline() {
        let status = LiveStatus::Offline;
        assert!(!status.is_live());
        assert!(status.is_offline());
        assert!(!status.is_filtered());
    }

    #[test]
    fn test_live_status_is_filtered() {
        let status = LiveStatus::Filtered {
            reason: FilterReason::OutOfSchedule {
                next_available: None,
            },
            title: "Test Stream".to_string(),
            category: None,
        };
        assert!(!status.is_live());
        assert!(!status.is_offline());
        assert!(status.is_filtered());
    }

    #[test]
    fn test_live_status_title() {
        let live = LiveStatus::Live {
            credential_binding: None,
            credential_snapshot: None,
            title: "Live Title".to_string(),
            category: None,
            started_at: None,
            viewer_count: None,
            avatar: None,
            streams: vec![create_test_stream()],
            media_headers: None,
            media_extras: None,
            next_check_hint: None,
            candidates: vec![],
        };
        assert_eq!(live.title(), Some("Live Title"));

        let filtered = LiveStatus::Filtered {
            reason: FilterReason::OutOfSchedule {
                next_available: None,
            },
            title: "Filtered Title".to_string(),
            category: None,
        };
        assert_eq!(filtered.title(), Some("Filtered Title"));

        let offline = LiveStatus::Offline;
        assert_eq!(offline.title(), None);
    }

    #[test]
    fn test_live_status_is_fatal_error() {
        assert!(LiveStatus::NotFound.is_fatal_error());
        assert!(LiveStatus::Banned.is_fatal_error());
        assert!(LiveStatus::AgeRestricted.is_fatal_error());
        assert!(LiveStatus::RegionLocked.is_fatal_error());
        assert!(LiveStatus::Private.is_fatal_error());
        assert!(LiveStatus::UnsupportedPlatform.is_fatal_error());

        // Non-fatal statuses
        assert!(!LiveStatus::Offline.is_fatal_error());
        assert!(
            !LiveStatus::Live {
                credential_binding: None,
                credential_snapshot: None,
                title: "Test".to_string(),
                category: None,
                started_at: None,
                avatar: None,
                viewer_count: None,
                streams: vec![create_test_stream()],
                media_headers: None,
                media_extras: None,
                next_check_hint: None,
                candidates: vec![],
            }
            .is_fatal_error()
        );
    }

    #[test]
    fn test_fatal_error_description() {
        assert_eq!(
            LiveStatus::NotFound.fatal_error_description(),
            Some("Streamer not found on platform")
        );
        assert_eq!(
            LiveStatus::Banned.fatal_error_description(),
            Some("Streamer is banned on platform")
        );
        assert_eq!(LiveStatus::Offline.fatal_error_description(), None);
    }
}
