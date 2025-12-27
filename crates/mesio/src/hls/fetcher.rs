// HLS Segment Fetcher: Handles the raw download of individual media segments with retry logic.

use crate::cache::{CacheMetadata, CacheResourceType};
use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use crate::{CacheManager, cache::CacheKey};
use async_trait::async_trait;
use bytes::Bytes;
use reqwest::Client;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{Span, debug, error, info, instrument, trace};
use tracing_indicatif::span_ext::IndicatifSpanExt;
use url::Url;

use crate::hls::scheduler::ScheduledSegmentJob;

/// Tracks HTTP/2 connection statistics for observability
#[derive(Debug, Default)]
pub struct Http2Stats {
    /// Number of requests using HTTP/2
    pub http2_requests: AtomicU64,
    /// Number of requests using HTTP/1.x
    pub http1_requests: AtomicU64,
    /// Total bytes downloaded via HTTP/2
    pub http2_bytes: AtomicU64,
    /// Total bytes downloaded via HTTP/1.x
    pub http1_bytes: AtomicU64,
    /// Number of connections reused (estimated via HTTP/2 multiplexing)
    /// When multiple HTTP/2 requests go to the same host, they share a connection
    pub connections_reused: AtomicU64,
    /// Number of new connections established
    pub connections_new: AtomicU64,
    /// Track hosts seen for connection reuse estimation
    hosts_seen: std::sync::Mutex<BoundedHostSet>,
}

/// Bounded set to prevent unbounded memory growth from tracking distinct hosts.
///
/// This is used only for observability heuristics (connection reuse estimation), so it is
/// acceptable for it to evict older entries.
#[derive(Debug, Default)]
struct BoundedHostSet {
    order: VecDeque<String>,
    set: HashSet<String>,
}

impl BoundedHostSet {
    // Keep this modest: HLS/CDN hostnames are usually low-cardinality.
    const MAX_TRACKED_HOSTS: usize = 256;

    fn contains(&self, host: &str) -> bool {
        self.set.contains(host)
    }

    fn insert(&mut self, host: &str) -> bool {
        if self.set.contains(host) {
            return false;
        }

        let host_string = host.to_string();
        self.set.insert(host_string.clone());
        self.order.push_back(host_string);

        while self.order.len() > Self::MAX_TRACKED_HOSTS {
            if let Some(oldest) = self.order.pop_front() {
                self.set.remove(&oldest);
            }
        }
        true
    }
}

impl Http2Stats {
    pub fn new() -> Self {
        Self {
            http2_requests: AtomicU64::new(0),
            http1_requests: AtomicU64::new(0),
            http2_bytes: AtomicU64::new(0),
            http1_bytes: AtomicU64::new(0),
            connections_reused: AtomicU64::new(0),
            connections_new: AtomicU64::new(0),
            hosts_seen: std::sync::Mutex::new(BoundedHostSet::default()),
        }
    }

    /// Record a request with HTTP version and bytes downloaded
    pub fn record_request(&self, version: reqwest::Version, bytes: u64) {
        match version {
            reqwest::Version::HTTP_2 => {
                self.http2_requests.fetch_add(1, Ordering::Relaxed);
                self.http2_bytes.fetch_add(bytes, Ordering::Relaxed);
            }
            _ => {
                self.http1_requests.fetch_add(1, Ordering::Relaxed);
                self.http1_bytes.fetch_add(bytes, Ordering::Relaxed);
            }
        }
    }

    /// Record a request with connection reuse tracking
    ///
    /// For HTTP/2, multiple requests to the same host share a connection.
    /// This method tracks whether a host has been seen before to estimate
    /// connection reuse.
    pub fn record_request_with_host(&self, version: reqwest::Version, bytes: u64, host: &str) {
        self.record_request(version, bytes);

        // Track connection reuse based on host
        // For HTTP/2, requests to the same host reuse the connection
        if version == reqwest::Version::HTTP_2 {
            let mut hosts = self.hosts_seen.lock().unwrap();
            if hosts.contains(host) {
                self.connections_reused.fetch_add(1, Ordering::Relaxed);
            } else {
                self.connections_new.fetch_add(1, Ordering::Relaxed);
                hosts.insert(host);
            }
        } else {
            // HTTP/1.x may or may not reuse connections, but we count each as potentially new
            // since HTTP/1.1 keep-alive is less predictable
            self.connections_new.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn http2_percentage(&self) -> f64 {
        let h2 = self.http2_requests.load(Ordering::Relaxed);
        let h1 = self.http1_requests.load(Ordering::Relaxed);
        let total = h2 + h1;
        if total == 0 {
            0.0
        } else {
            (h2 as f64 / total as f64) * 100.0
        }
    }

    /// Get connection reuse rate as a percentage
    pub fn connection_reuse_rate(&self) -> f64 {
        let reused = self.connections_reused.load(Ordering::Relaxed);
        let new_conns = self.connections_new.load(Ordering::Relaxed);
        let total = reused + new_conns;
        if total == 0 {
            0.0
        } else {
            (reused as f64 / total as f64) * 100.0
        }
    }

    pub fn log_summary(&self) {
        let h2_reqs = self.http2_requests.load(Ordering::Relaxed);
        let h1_reqs = self.http1_requests.load(Ordering::Relaxed);
        let h2_bytes = self.http2_bytes.load(Ordering::Relaxed);
        let h1_bytes = self.http1_bytes.load(Ordering::Relaxed);
        let conn_reused = self.connections_reused.load(Ordering::Relaxed);
        let conn_new = self.connections_new.load(Ordering::Relaxed);

        if h2_reqs + h1_reqs > 0 {
            info!(
                http2_requests = h2_reqs,
                http1_requests = h1_reqs,
                http2_bytes = h2_bytes,
                http1_bytes = h1_bytes,
                http2_percentage = format!("{:.1}%", self.http2_percentage()),
                connections_reused = conn_reused,
                connections_new = conn_new,
                connection_reuse_rate = format!("{:.1}%", self.connection_reuse_rate()),
                "HTTP connection statistics"
            );
        }
    }
}

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
    http2_stats: Arc<Http2Stats>,
    /// Optional shared performance metrics for recording HTTP version usage
    performance_metrics: Option<Arc<super::metrics::PerformanceMetrics>>,
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
            http2_stats: Arc::new(Http2Stats::new()),
            performance_metrics: None,
        }
    }

    /// Create a new fetcher with a shared HTTP/2 stats tracker
    pub fn with_stats(
        http_client: Client,
        config: Arc<HlsConfig>,
        cache_service: Option<Arc<CacheManager>>,
        http2_stats: Arc<Http2Stats>,
    ) -> Self {
        Self {
            http_client,
            config,
            cache_service,
            http2_stats,
            performance_metrics: None,
        }
    }

    /// Create a new fetcher with shared HTTP/2 stats and performance metrics
    pub fn with_metrics(
        http_client: Client,
        config: Arc<HlsConfig>,
        cache_service: Option<Arc<CacheManager>>,
        http2_stats: Arc<Http2Stats>,
        performance_metrics: Arc<super::metrics::PerformanceMetrics>,
    ) -> Self {
        Self {
            http_client,
            config,
            cache_service,
            http2_stats,
            performance_metrics: Some(performance_metrics),
        }
    }

    /// Get the HTTP/2 statistics tracker
    pub fn http2_stats(&self) -> &Http2Stats {
        &self.http2_stats
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
        let mut attempts = 0;
        let streaming_threshold = self.config.fetcher_config.streaming_threshold_bytes;

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

            // Start timing the download for latency metrics
            let download_start = std::time::Instant::now();

            match request_builder
                .timeout(self.config.fetcher_config.segment_download_timeout)
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        let http_version = response.version();

                        // Log HTTP version for observability
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
                        let bytes = if content_length
                            .is_some_and(|len| len as usize > streaming_threshold)
                        {
                            self.stream_response(response, segment_span).await?
                        } else {
                            // Small segments: use simple bytes() for efficiency
                            let bytes = response.bytes().await.map_err(HlsDownloaderError::from)?;
                            segment_span.pb_set_position(bytes.len() as u64);
                            bytes
                        };

                        // Calculate download latency
                        let download_latency_ms = download_start.elapsed().as_millis() as u64;

                        // Record HTTP/2 statistics with connection reuse tracking
                        let host = segment_url.host_str().unwrap_or("unknown");
                        self.http2_stats.record_request_with_host(
                            http_version,
                            bytes.len() as u64,
                            host,
                        );

                        // Record metrics in performance metrics if available

                        if let Some(metrics) = &self.performance_metrics {
                            let is_http2 = http_version == reqwest::Version::HTTP_2;
                            metrics.record_http_version(is_http2);
                            metrics.record_download(bytes.len() as u64, download_latency_ms);

                            trace!(
                                url = %segment_url,
                                bytes = bytes.len(),
                                latency_ms = download_latency_ms,
                                http2 = is_http2,
                                "Recorded download metrics"
                            );
                        }

                        return Ok(bytes);
                    } else if response.status().is_client_error() {
                        // Record download error in metrics
                        if let Some(metrics) = &self.performance_metrics {
                            metrics.record_download_error();
                        }
                        return Err(HlsDownloaderError::SegmentFetchError(format!(
                            "Client error {} for segment {}",
                            response.status(),
                            segment_url
                        )));
                    }
                    if attempts > self.config.fetcher_config.max_segment_retries {
                        // Record download error in metrics
                        if let Some(metrics) = &self.performance_metrics {
                            metrics.record_download_error();
                        }
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
                        // Record download error in metrics
                        if let Some(metrics) = &self.performance_metrics {
                            metrics.record_download_error();
                        }
                        return Err(HlsDownloaderError::from(e));
                    }
                    if attempts > self.config.fetcher_config.max_segment_retries {
                        // Record download error in metrics
                        if let Some(metrics) = &self.performance_metrics {
                            metrics.record_download_error();
                        }
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

        while let Some(chunk_result) = stream.next().await {
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

        use indicatif::ProgressStyle;
        let style = ProgressStyle::default_bar()
            .template("{span_child_prefix}{spinner:.yellow} [{bar:20.yellow/white}] {bytes}/{total_bytes} {msg}")
            .unwrap()
            .progress_chars("=> ");
        current_span.pb_set_style(&style);
        current_span.pb_set_message(&segment_label);

        let segment_url = Url::parse(&job.media_segment.uri).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!(
                "Invalid segment URL {}: {}",
                job.media_segment.uri, e
            ))
        })?;

        let cache_key = CacheKey::new(
            CacheResourceType::Segment,
            job.media_segment.uri.clone(),
            None,
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
                    error!(
                        "Warning: Failed to read segment {} from cache: {}",
                        segment_url, e
                    );
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
                    &segment_url,
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

        result
    }
}
