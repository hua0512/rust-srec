// HLS Segment Fetcher: Handles the raw download of individual media segments with retry logic.

use crate::cache::{CacheMetadata, CacheResourceType};
use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use crate::{CacheManager, cache::CacheKey};
use async_trait::async_trait;
use bytes::Bytes;
use reqwest::Client;
use std::sync::Arc;
use tracing::{debug, error};
use url::Url;

use crate::hls::scheduler::ScheduledSegmentJob;

#[async_trait]
pub trait SegmentDownloader: Send + Sync {
    async fn download_segment_from_job(
        &self,
        job: &ScheduledSegmentJob,
    ) -> Result<Bytes, HlsDownloaderError>;
}

pub struct SegmentFetcher {
    http_client: Client,
    config: Arc<HlsConfig>,
    cache_service: Option<Arc<CacheManager>>,
}

impl SegmentFetcher {
    pub fn new(
        http_client: Client,
        config: Arc<HlsConfig>,
        cache_service: Option<Arc<CacheManager>>,
    ) -> Self {
        Self {
            http_client,
            config,
            cache_service,
        }
    }

    /// Fetches a segment with retry logic.
    /// Retries on network errors and server errors (5xx).
    async fn fetch_with_retries(
        &self,
        segment_url: &Url,
        byte_range: Option<&m3u8_rs::ByteRange>,
    ) -> Result<Bytes, HlsDownloaderError> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            let mut request_builder = self.http_client.get(segment_url.clone());
            if let Some(range) = byte_range {
                let range_str = if let Some(offset) = range.offset {
                    format!("bytes={}-{}", range.length, range.length + offset - 1)
                } else {
                    format!("bytes=0-{}", range.length - 1)
                };
                request_builder = request_builder.header(reqwest::header::RANGE, range_str);
            }

            match request_builder
                .timeout(self.config.fetcher_config.segment_download_timeout)
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        return response.bytes().await.map_err(HlsDownloaderError::from);
                    } else if response.status().is_client_error() {
                        // Non-retryable client errors (4xx)
                        return Err(HlsDownloaderError::SegmentFetchError(format!(
                            "Client error {} for segment {}",
                            response.status(),
                            segment_url
                        )));
                    }
                    // Server errors (5xx) or other retryable issues
                    if attempts > self.config.fetcher_config.max_segment_retries {
                        return Err(HlsDownloaderError::SegmentFetchError(format!(
                            "Max retries ({}) exceeded for segment {}. Last status: {}",
                            self.config.fetcher_config.max_segment_retries,
                            segment_url,
                            response.status()
                        )));
                    }
                }
                Err(e) => {
                    if !e.is_connect() && !e.is_timeout() && !e.is_request() {
                        // Non-retryable network errors
                        return Err(HlsDownloaderError::from(e));
                    }
                    if attempts > self.config.fetcher_config.max_segment_retries {
                        return Err(HlsDownloaderError::SegmentFetchError(format!(
                            "Max retries ({}) exceeded for segment {} due to network error: {}",
                            self.config.fetcher_config.max_segment_retries, segment_url, e
                        )));
                    }
                }
            }

            let delay = self.config.fetcher_config.segment_retry_delay_base
                * (2_u32.pow(attempts.saturating_sub(1)));
            tokio::time::sleep(delay).await;
        }
    }
}

#[async_trait]
impl SegmentDownloader for SegmentFetcher {
    /// Downloads a segment from the given job.
    /// If the segment is already cached, it retrieves it from the cache.
    /// If not, it downloads the segment and caches it.
    /// Returns the raw bytes of the segment.
    async fn download_segment_from_job(
        &self,
        job: &ScheduledSegmentJob,
    ) -> Result<Bytes, HlsDownloaderError> {
        // segment_uri is already absolute if resolved by PlaylistEngine, or needs resolving with job.base_url
        // Assuming job.segment_uri is the absolute URL for now.
        // If not, PlaylistEngine should resolve it before creating the job, or fetcher needs base_url.
        // For consistency, let's assume segment_uri in ScheduledSegmentJob is absolute.
        // If job.segment_uri can be relative, then:
        // let absolute_segment_url = Url::parse(&job.base_url)?
        //     .join(&job.segment_uri)
        //     .map_err(|e| HlsDownloaderError::PlaylistError(format!("Could not join base URL {} with segment URI {}: {}", job.base_url, job.segment_uri, e)))?;

        let segment_url = Url::parse(&job.segment_uri).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!(
                "Invalid segment URL {}: {}",
                job.segment_uri, e
            ))
        })?;

        // Check if the segment is already cached
        let cache_key = CacheKey::new(CacheResourceType::Segment, segment_url.to_string(), None);
        if let Some(cache) = &self.cache_service {
            if let Ok(Some(data)) = cache.get(&cache_key).await {
                return Ok(data.0);
            }
        }

        let downloaded_bytes = self
            .fetch_with_retries(&segment_url, job.byte_range.as_ref())
            .await?;

        if let Some(cache) = &self.cache_service {
            let metadata = CacheMetadata::new(downloaded_bytes.len() as u64)
                .with_expiration(self.config.fetcher_config.segment_raw_cache_ttl);

            if let Err(e) = cache
                .put(cache_key, downloaded_bytes.clone(), metadata)
                .await
            {
                error!(
                    "Warning: Failed to cache raw segment {}: {}",
                    segment_url, e
                );
            }
        }

        debug!(
            "Downloaded {} bytes from segment URL: {}",
            downloaded_bytes.len(),
            segment_url
        );
        Ok(downloaded_bytes)
    }
}
