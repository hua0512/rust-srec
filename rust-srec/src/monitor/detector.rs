//! Individual stream detection.
//!
//! This module handles checking the live status of individual streamers.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use platforms_parser::extractor::error::ExtractorError;
use platforms_parser::extractor::factory::ExtractorFactory;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, trace, warn};

use crate::Result;
use crate::domain::ProxyConfig;
use crate::domain::filter::{Filter, FilterType};
use crate::downloader::{StreamSelectionConfig, StreamSelector};
use crate::streamer::StreamerMetadata;

/// Re-export StreamInfo from platforms_parser for convenience.
pub use platforms_parser::media::StreamInfo;

/// Live status of a streamer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LiveStatus {
    /// Streamer is currently live.
    Live {
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
        media_extras: Option<HashMap<String, String>>,
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
}

/// Stream detector for checking live status.
pub struct StreamDetector {
    request_timeout: std::time::Duration,
    pool_max_idle_per_host: usize,
    client_cache: DashMap<ProxyKey, reqwest::Client>,
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
            client_cache: DashMap::new(),
        }
    }

    fn client_for_proxy_config(&self, proxy_config: &ProxyConfig) -> reqwest::Client {
        let key = ProxyKey::from(proxy_config);
        if let Some(existing) = self.client_cache.get(&key) {
            return existing.clone();
        }

        let client = crate::utils::http_client::build_platforms_client(
            proxy_config,
            self.request_timeout,
            self.pool_max_idle_per_host,
        );
        self.client_cache.insert(key, client.clone());
        client
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

    /// Check the live status of a streamer.
    ///
    /// Uses the platforms crate to extract media information from the streamer's URL.
    pub async fn check_status(
        &self,
        streamer: &StreamerMetadata,
        selection_config: Option<&StreamSelectionConfig>,
    ) -> Result<LiveStatus> {
        let proxy_config = ProxyConfig::disabled();
        self.check_status_with_cookies(streamer, None, selection_config, None, &proxy_config)
            .await
    }

    /// Check the live status of a streamer with optional cookies, selection config, and platform extras.
    ///
    /// # Arguments
    /// * `streamer` - The streamer to check
    /// * `cookies` - Optional cookies to use for the request
    /// * `selection_config` - Optional stream selection configuration
    /// * `platform_extras` - Optional platform-specific extractor configuration (merged from all config layers)
    pub async fn check_status_with_cookies(
        &self,
        streamer: &StreamerMetadata,
        cookies: Option<String>,
        selection_config: Option<&StreamSelectionConfig>,
        platform_extras: Option<serde_json::Value>,
        proxy_config: &ProxyConfig,
    ) -> Result<LiveStatus> {
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

        let extractor_factory = ExtractorFactory::new(self.client_for_proxy_config(proxy_config));

        // Create platform extractor for this streamer's URL
        let extractor =
            match extractor_factory.create_extractor(&streamer.url, cookies, merged_extras) {
                Ok(ext) => ext,
                Err(ExtractorError::UnsupportedExtractor) => {
                    warn!("Unsupported platform for URL: {}", streamer.url);
                    return Ok(LiveStatus::UnsupportedPlatform);
                }
                Err(e) => {
                    return Err(crate::Error::Monitor(format!(
                        "Failed to create extractor: {}",
                        e
                    )));
                }
            };

        // Extract media information
        let media_info = match extractor.extract().await {
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
            Err(ExtractorError::JsError(msg)) => {
                warn!(
                    "JavaScript-based extraction failed for {} ({}): {}",
                    streamer.name, streamer.url, msg
                );
                return Err(crate::Error::Monitor(format!(
                    "Failed to extract media info for {} ({}): js error: {}",
                    streamer.name, streamer.url, msg
                )));
            }
            // Transient errors - should be retried
            Err(e) => {
                return Err(crate::Error::Monitor(format!(
                    "Failed to extract media info for {} ({}): {}",
                    streamer.name, streamer.url, e
                )));
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

        // debug!("Media info: {:#?}", media_info);

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
                debug!(quality = %stream.quality, url = %stream.url, "selected stream candidate");
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

            let mut resolved_stream = None;
            for candidate in resolution_slice {
                let mut stream = (*candidate).clone();
                trace!(quality = %stream.quality, url = %stream.url, "resolving stream url");

                match extractor.get_url(&mut stream).await {
                    Ok(_) => {
                        trace!(url = %stream.url, "resolved stream url");
                        resolved_stream = Some(stream);
                        break;
                    }
                    Err(e) => {
                        error!(
                            streamer_name = %streamer.name,
                            quality = %candidate.quality,
                            error = %e,
                            "failed to resolve stream url for candidate"
                        );
                        // Continue to next candidate
                    }
                }
            }

            let selected_stream = match resolved_stream {
                Some(stream) => stream,
                None => {
                    warn!(
                        "All stream candidates failed resolution for {}. Treating as OFFLINE.",
                        streamer.name
                    );
                    return Ok(LiveStatus::Offline);
                }
            };

            let streams = vec![selected_stream];

            debug!(
                streamer_name = %streamer.name,
                streamer_url = %streamer.url,
                title = %media_info.title,
                category = ?category,
                viewers = ?viewer_count,
                streams = streams.len(),
                media_headers = media_headers.as_ref().map(|h| h.len()).unwrap_or(0),
                extras = media_extras.as_ref().map(|e| e.len()).unwrap_or(0),
                "status=LIVE"
            );

            Ok(LiveStatus::Live {
                title: media_info.title,
                category,
                avatar: media_info.artist_url.clone(),
                started_at: None, // TODO: platforms crate doesn't provide start time
                viewer_count,
                streams,
                media_headers,
                media_extras,
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
    /// * `platform_extras` - Optional platform-specific extractor configuration
    pub async fn check_status_with_filters(
        &self,
        streamer: &StreamerMetadata,
        filters: &[Filter],
        cookies: Option<String>,
        selection_config: Option<&StreamSelectionConfig>,
        platform_extras: Option<serde_json::Value>,
        proxy_config: &ProxyConfig,
    ) -> Result<LiveStatus> {
        let status = self
            .check_status_with_cookies(
                streamer,
                cookies,
                selection_config,
                platform_extras,
                proxy_config,
            )
            .await?;

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProxyKey {
    enabled: bool,
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
    use_system_proxy: bool,
}

impl From<&ProxyConfig> for ProxyKey {
    fn from(value: &ProxyConfig) -> Self {
        Self {
            enabled: value.enabled,
            url: value.url.clone(),
            username: value.username.clone(),
            password: value.password.clone(),
            use_system_proxy: value.use_system_proxy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use platforms_parser::media::{StreamFormat, formats::MediaFormat};

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
            title: "Test Stream".to_string(),
            category: Some("Gaming".to_string()),
            started_at: None,
            viewer_count: None,
            avatar: None,
            streams: vec![create_test_stream()],
            media_headers: None,
            media_extras: None,
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
            title: "Live Title".to_string(),
            category: None,
            started_at: None,
            viewer_count: None,
            avatar: None,
            streams: vec![create_test_stream()],
            media_headers: None,
            media_extras: None,
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
                title: "Test".to_string(),
                category: None,
                started_at: None,
                avatar: None,
                viewer_count: None,
                streams: vec![create_test_stream()],
                media_headers: None,
                media_extras: None,
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
