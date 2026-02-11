// HLS Segment Fetcher: Handles the raw download of individual media segments with retry logic.

use crate::cache::{CacheMetadata, CacheResourceType};
use crate::downloader::ClientPool;
use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use crate::hls::retry::{RetryAction, RetryPolicy, is_retryable_reqwest_error, retry_with_backoff};
use crate::{CacheManager, cache::CacheKey};
use async_trait::async_trait;
use bytes::Bytes;
use indicatif::ProgressStyle;
use std::sync::Arc;
use tracing::{Span, debug, instrument, trace, warn};
use tracing_indicatif::span_ext::IndicatifSpanExt;
use url::Url;

use tokio_util::sync::CancellationToken;

use crate::hls::scheduler::ScheduledSegmentJob;

#[async_trait]
pub trait SegmentDownloader: Send + Sync {
    async fn download_segment_from_job(
        &self,
        job: &ScheduledSegmentJob,
    ) -> Result<Bytes, HlsDownloaderError>;
}

pub struct SegmentFetcher {
    clients: Arc<ClientPool>,
    config: Arc<HlsConfig>,
    cache_service: Option<Arc<CacheManager>>,
    performance_metrics: Option<Arc<super::metrics::PerformanceMetrics>>,
    /// Pre-built progress bar style to avoid re-parsing the template on every segment
    progress_style: ProgressStyle,
    token: CancellationToken,
}

impl SegmentFetcher {
    pub fn new(
        clients: Arc<ClientPool>,
        config: Arc<HlsConfig>,
        cache_service: Option<Arc<CacheManager>>,
        token: CancellationToken,
    ) -> Self {
        let progress_style = match ProgressStyle::default_bar().template(
            "{span_child_prefix}{spinner:.yellow} [{bar:20.yellow/white}] {bytes}/{total_bytes} {msg}",
        ) {
            Ok(style) => style.progress_chars("=> "),
            Err(error) => {
                // Avoid panicking in production if indicatif changes template parsing.
                debug!(?error, "Failed to build progress bar template; falling back");
                ProgressStyle::default_bar()
            }
        };
        Self {
            clients,
            config,
            cache_service,
            performance_metrics: None,
            progress_style,
            token,
        }
    }

    /// Create a new fetcher with shared performance metrics
    pub fn with_metrics(
        clients: Arc<ClientPool>,
        config: Arc<HlsConfig>,
        cache_service: Option<Arc<CacheManager>>,
        performance_metrics: Arc<super::metrics::PerformanceMetrics>,
        token: CancellationToken,
    ) -> Self {
        let mut fetcher = Self::new(clients, config, cache_service, token);
        fetcher.performance_metrics = Some(performance_metrics);
        fetcher
    }

    /// Fetches a segment with retry logic.
    /// Retries on network errors and server errors (5xx).
    /// For large segments (above streaming_threshold_bytes), uses streaming to reduce memory spikes.
    async fn fetch_with_retries(
        &self,
        segment_url: &Url,
        byte_range: Option<&m3u8_rs::ByteRange>,
        segment_span: &Span,
    ) -> Result<Bytes, HlsDownloaderError> {
        let policy = RetryPolicy {
            max_retries: self.config.fetcher_config.max_segment_retries,
            base_delay: self.config.fetcher_config.segment_retry_delay_base,
            max_delay: self.config.fetcher_config.max_segment_retry_delay,
            jitter: true,
        };
        let streaming_threshold = self.config.fetcher_config.streaming_threshold_bytes;

        retry_with_backoff(&policy, &self.token, |_attempt| async {
            let client = self.clients.client_for_url(segment_url);
            let mut request_builder = client
                .get(segment_url.clone())
                .query(&self.config.base.params);
            if let Some(range) = byte_range {
                let start = range.offset.unwrap_or(0);
                let end = start.saturating_add(range.length).saturating_sub(1);
                let range_str = format!("bytes={start}-{end}");
                request_builder = request_builder.header(reqwest::header::RANGE, range_str);
            }

            let download_start = std::time::Instant::now();

            let response = tokio::select! {
                _ = self.token.cancelled() => {
                    return RetryAction::Fail(HlsDownloaderError::Cancelled);
                }
                response = request_builder
                    .timeout(self.config.fetcher_config.segment_download_timeout)
                    .send() => response,
            };

            match response {
                Ok(response) => {
                    if response.status().is_success() {
                        let http_version = response.version();

                        trace!(
                            url = %segment_url,
                            http_version = ?http_version,
                            "Segment download using HTTP version"
                        );

                        let content_length = response.content_length();
                        if let Some(len) = content_length {
                            segment_span.pb_set_length(len);
                        }

                        // Use streaming for large segments to reduce memory spikes
                        let bytes_result = if content_length
                            .is_some_and(|len| len as usize > streaming_threshold)
                        {
                            self.stream_response(response, segment_span).await
                        } else {
                            let bytes = tokio::select! {
                                _ = self.token.cancelled() => {
                                    return RetryAction::Fail(HlsDownloaderError::Cancelled);
                                }
                                bytes = response.bytes() => bytes,
                            };
                            match bytes {
                                Ok(b) => {
                                    segment_span.pb_set_position(b.len() as u64);
                                    Ok(b)
                                }
                                Err(e) => Err(HlsDownloaderError::from(e)),
                            }
                        };

                        match bytes_result {
                            Ok(bytes) => {
                                let download_latency_ms =
                                    download_start.elapsed().as_millis() as u64;

                                if let Some(metrics) = &self.performance_metrics {
                                    let host = segment_url.host_str().unwrap_or("unknown");
                                    metrics.record_request_with_host(
                                        http_version,
                                        bytes.len() as u64,
                                        host,
                                    );
                                    metrics
                                        .record_download(bytes.len() as u64, download_latency_ms);

                                    trace!(
                                        url = %segment_url,
                                        bytes = bytes.len(),
                                        latency_ms = download_latency_ms,
                                        http2 = (http_version == reqwest::Version::HTTP_2),
                                        "Recorded download metrics"
                                    );
                                }

                                RetryAction::Success(bytes)
                            }
                            Err(err) => {
                                if let Some(metrics) = &self.performance_metrics {
                                    metrics.record_download_error();
                                }
                                // Body read errors during streaming are retryable
                                RetryAction::Retry(err)
                            }
                        }
                    } else if response.status().is_client_error() {
                        if let Some(metrics) = &self.performance_metrics {
                            metrics.record_download_error();
                        }
                        RetryAction::Fail(HlsDownloaderError::SegmentFetchError(format!(
                            "Client error {} for segment {}",
                            response.status(),
                            segment_url
                        )))
                    } else {
                        // Server errors (5xx) are retryable
                        RetryAction::Retry(HlsDownloaderError::SegmentFetchError(format!(
                            "Server error {} for segment {}",
                            response.status(),
                            segment_url
                        )))
                    }
                }
                Err(e) => {
                    if is_retryable_reqwest_error(&e) {
                        RetryAction::Retry(HlsDownloaderError::from(e))
                    } else {
                        if let Some(metrics) = &self.performance_metrics {
                            metrics.record_download_error();
                        }
                        RetryAction::Fail(HlsDownloaderError::from(e))
                    }
                }
            }
        })
        .await
    }

    /// Streams a response body in chunks to reduce memory pressure for large segments.
    /// Updates progress as chunks are received.
    async fn stream_response(
        &self,
        response: reqwest::Response,
        segment_span: &Span,
    ) -> Result<Bytes, HlsDownloaderError> {
        use bytes::BytesMut;
        use futures::StreamExt;

        let content_length = response.content_length().unwrap_or(0) as usize;
        let mut buffer = BytesMut::with_capacity(content_length);
        let mut stream = response.bytes_stream();
        let mut downloaded: u64 = 0;

        while let Some(chunk_result) = tokio::select! {
            _ = self.token.cancelled() => {
                return Err(HlsDownloaderError::Cancelled);
            }
            next = stream.next() => next,
        } {
            let chunk = chunk_result.map_err(HlsDownloaderError::from)?;
            downloaded += chunk.len() as u64;
            buffer.extend_from_slice(&chunk);
            segment_span.pb_set_position(downloaded);
        }

        Ok(buffer.freeze())
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

        current_span.pb_set_style(&self.progress_style);
        current_span.pb_set_message(&segment_label);

        // Avoid cloning `Url` when we already pre-parsed it in the playlist engine.
        // We only allocate an owned `Url` if we need to parse the URI string.
        let segment_url_storage = if job.parsed_url.is_some() {
            None
        } else {
            Some(Url::parse(&job.media_segment.uri).map_err(|e| {
                HlsDownloaderError::PlaylistError(format!(
                    "Invalid segment URL {}: {}",
                    job.media_segment.uri, e
                ))
            })?)
        };
        let segment_url: &Url = job.parsed_url.as_deref().unwrap_or_else(|| {
            segment_url_storage
                .as_ref()
                .expect("segment_url_storage set")
        });

        let cache_key = CacheKey::new(
            CacheResourceType::Segment,
            job.media_segment.uri.clone(),
            job.media_segment.byte_range.as_ref().map(|range| {
                let offset = range
                    .offset
                    .map(|o| o.to_string())
                    .unwrap_or_else(|| "none".to_string());
                format!("br={}@{}", range.length, offset)
            }),
        );

        let mut cached_bytes: Option<Bytes> = None;
        if let Some(cache) = &self.cache_service {
            match cache.get(&cache_key).await {
                Ok(Some(data)) => {
                    debug!(msn = job.media_sequence_number, "Segment loaded from cache");
                    current_span.pb_set_length(data.0.len() as u64);
                    current_span.pb_set_position(data.0.len() as u64);
                    cached_bytes = Some(data.0);

                    // Record cache hit in performance metrics
                    if let Some(metrics) = &self.performance_metrics {
                        metrics.record_cache_hit();
                    }
                }
                Ok(None) => {
                    // Record cache miss in performance metrics
                    if let Some(metrics) = &self.performance_metrics {
                        metrics.record_cache_miss();
                    }
                }
                Err(e) => {
                    warn!("Failed to read segment {} from cache: {}", segment_url, e);
                    // Treat cache error as a miss
                    if let Some(metrics) = &self.performance_metrics {
                        metrics.record_cache_miss();
                    }
                }
            }
        }

        let result = if let Some(bytes) = cached_bytes {
            Ok(bytes)
        } else {
            let downloaded_bytes = self
                .fetch_with_retries(
                    segment_url,
                    job.media_segment.byte_range.as_ref(),
                    &current_span,
                )
                .await?;

            if let Some(cache) = &self.cache_service {
                let metadata = CacheMetadata::new(downloaded_bytes.len() as u64)
                    .with_expiration(self.config.fetcher_config.segment_raw_cache_ttl);
                if let Err(e) = cache
                    .put(cache_key, downloaded_bytes.clone(), metadata)
                    .await
                {
                    warn!("Failed to cache raw segment {}: {}", segment_url, e);
                }
            }

            trace!(
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

        result
    }
}
