// HLS Segment Fetcher: Handles the raw download of individual media segments with retry logic.

use crate::cache::{CacheMetadata, CacheResourceType};
use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use crate::{CacheManager, cache::CacheKey};
use async_trait::async_trait;
use bytes::Bytes;
use reqwest::Client;
use std::sync::Arc;
use tracing::{Span, debug, error, instrument};
use tracing_indicatif::span_ext::IndicatifSpanExt;
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
        segment_span: &Span,
    ) -> Result<Bytes, HlsDownloaderError> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            let mut request_builder = self
                .http_client
                .get(segment_url.clone())
                .query(&self.config.base.params);
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
                        if let Some(len) = response.content_length() {
                            segment_span.pb_set_length(len);
                        }

                        // Use bytes() instead of streaming to avoid memory accumulation
                        // reqwest internally handles chunked downloads efficiently
                        let bytes = response.bytes().await.map_err(HlsDownloaderError::from)?;
                        segment_span.pb_set_position(bytes.len() as u64);

                        return Ok(bytes);
                    } else if response.status().is_client_error() {
                        return Err(HlsDownloaderError::SegmentFetchError(format!(
                            "Client error {} for segment {}",
                            response.status(),
                            segment_url
                        )));
                    }
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
    #[instrument(skip(self, job), fields(msn = job.media_sequence_number))]
    async fn download_segment_from_job(
        &self,
        job: &ScheduledSegmentJob,
    ) -> Result<Bytes, HlsDownloaderError> {
        let segment_label = format!("Segment #{}", job.media_sequence_number);
        // current download span
        let current_span = Span::current();

        use indicatif::ProgressStyle;
        let style = ProgressStyle::default_bar()
            .template(&format!(
                "{{span_child_prefix}}{{spinner:.yellow}} [{{bar:20.yellow/white}}] {{bytes}}/{{total_bytes}} {}",
                segment_label
            ))
            .unwrap()
            .progress_chars("=> ");
        current_span.pb_set_style(&style);
        current_span.pb_set_message(&segment_label);

        let segment_url = Url::parse(&job.segment_uri).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!(
                "Invalid segment URL {}: {}",
                job.segment_uri, e
            ))
        })?;

        let cache_key = CacheKey::new(CacheResourceType::Segment, segment_url.to_string(), None);

        let mut cached_bytes: Option<Bytes> = None;
        if let Some(cache) = &self.cache_service {
            match cache.get(&cache_key).await {
                Ok(Some(data)) => {
                    debug!(msn = job.media_sequence_number, "Segment loaded from cache");
                    current_span.pb_set_length(data.0.len() as u64);
                    current_span.pb_set_position(data.0.len() as u64);
                    cached_bytes = Some(data.0);
                }
                Ok(None) => {}
                Err(e) => {
                    error!(
                        "Warning: Failed to read segment {} from cache: {}",
                        segment_url, e
                    );
                }
            }
        }

        let result = if let Some(bytes) = cached_bytes {
            Ok(bytes)
        } else {
            let downloaded_bytes = self
                .fetch_with_retries(&segment_url, job.byte_range.as_ref(), &current_span)
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
                msn = job.media_sequence_number,
                size = downloaded_bytes.len(),
                "Downloaded segment"
            );

            Ok(downloaded_bytes)
        };

        match &result {
            Ok(_) => current_span.pb_set_finish_message(&segment_label),
            Err(err) => current_span.pb_set_finish_message(&format!(
                "Segment #{} failed: {}",
                job.media_sequence_number, err
            )),
        }

        current_span.pb_reset();
        result
    }
}
