//! # FLV Downloader
//!
//! This module implements efficient streaming download functionality for FLV resources.
//! It uses reqwest to download data in chunks and pipes it directly to the FLV parser,
//! minimizing memory usage and providing a seamless integration with the processing pipeline.

use bytes::Bytes;
use flv::{data::FlvData, parser_async::FlvDecoderStream};
use futures::StreamExt;
use reqwest::{Response, StatusCode, Url};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, info, instrument, warn};

use super::error::FlvDownloadError;
use super::flv_config::FlvProtocolConfig;
use crate::bytes_stream::BytesStreamReader;
use crate::{
    DownloadError,
    cache::{CacheKey, CacheManager, CacheMetadata, CacheResourceType, CacheStatus},
    downloader::create_client_pool,
    media_protocol::BoxMediaStream,
    source::{ContentSource, SourceManager},
};
use tokio_util::sync::CancellationToken;

// Import new capability-based traits
use crate::{Cacheable, Download, MultiSource, ProtocolBase, RawDownload, RawResumable, Resumable};

/// Format a byte count as a human-readable binary size (e.g. "1.5 MiB").
fn format_size_binary(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut size = bytes as f64;
    for &unit in UNITS {
        if size < 1024.0 {
            return if size.fract() < 0.05 {
                format!("{:.0} {}", size, unit)
            } else {
                format!("{:.2} {}", size, unit)
            };
        }
        size /= 1024.0;
    }
    format!("{:.2} PiB", size)
}

/// FLV Downloader for streaming FLV content from URLs
pub struct FlvDownloader {
    clients: Arc<crate::downloader::ClientPool>,
    config: FlvProtocolConfig,
}

impl FlvDownloader {
    fn log_unexpected_status(url: &Url, status: StatusCode, context: &'static str) {
        let reason = status.canonical_reason().unwrap_or("unknown");
        if status == StatusCode::NOT_FOUND {
            warn!(
                url = %url,
                status = %status,
                reason,
                context,
                "FLV request returned 404 Not Found; stream may be offline or URL may be expired"
            );
        } else {
            warn!(
                url = %url,
                status = %status,
                reason,
                context,
                "FLV request failed with non-success HTTP status"
            );
        }
    }

    /// Create a new FlvDownloader with default configuration
    pub fn new() -> Result<Self, DownloadError> {
        Self::with_config(FlvProtocolConfig::default())
    }

    /// Create a new FlvDownloader with custom configuration
    pub fn with_config(config: FlvProtocolConfig) -> Result<Self, DownloadError> {
        let clients = Arc::new(create_client_pool(&config.base)?);
        Ok(Self { clients, config })
    }

    /// Download a stream from a URL string and return an FLV data stream
    #[instrument(skip(self), level = "debug")]
    pub(crate) async fn download_flv(
        &self,
        url_str: &str,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<FlvData, FlvDownloadError>, DownloadError> {
        let url = url_str
            .parse::<Url>()
            .map_err(|e| DownloadError::invalid_url(url_str, e.to_string()))?;
        self.download_url(url, token).await
    }

    /// Download a stream from a URL string and return a raw byte stream without parsing
    #[instrument(skip(self), level = "debug")]
    pub(crate) async fn download_raw(
        &self,
        url_str: &str,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<Bytes, FlvDownloadError>, DownloadError> {
        let url = url_str
            .parse::<Url>()
            .map_err(|e| DownloadError::invalid_url(url_str, e.to_string()))?;
        self.download_url_raw(url, token).await
    }

    /// Core method to start a download request and return the response
    async fn start_download_request(&self, url: &Url) -> Result<Response, DownloadError> {
        info!(url = %url, "Starting FLV download request");
        debug!(url = %url, params = ?self.config.base.params, "Sending FLV download request");

        let client = self.clients.client_for_url(url);
        let response = client
            .get(url.clone())
            .query(&self.config.base.params)
            .send()
            .await?;

        // Check response status
        if !response.status().is_success() {
            Self::log_unexpected_status(url, response.status(), "initial_request");
            return Err(DownloadError::http_status(
                response.status(),
                url.to_string(),
                "initial_request",
            ));
        }

        // Fast path: Check Content-Type header if present
        // Reject obviously wrong content types early without reading body
        if let Some(content_type) = response.headers().get("content-type")
            && let Ok(ct_str) = content_type.to_str()
        {
            let ct_lower = ct_str.to_lowercase();

            // Accept: video/x-flv, video/flv, application/octet-stream, or no/unknown content type
            // Reject: text/html, text/plain, application/json (likely error responses)
            let is_text_response = ct_lower.starts_with("text/")
                || ct_lower.contains("html")
                || ct_lower.contains("json")
                || ct_lower.contains("xml");

            if is_text_response {
                warn!(
                    url = %url,
                    content_type = %ct_str,
                    "Response has text Content-Type, likely not FLV data"
                );
                return Err(DownloadError::InvalidContent {
                    protocol: "flv",
                    reason: format!(
                        "Invalid Content-Type: {}. Expected video/x-flv or binary content",
                        ct_str
                    ),
                });
            }

            debug!(url = %url, content_type = %ct_str, "Content-Type check passed");
        }

        // Log file size and update progress bar if available
        if let Some(content_length) = response.content_length() {
            info!(
                url = %url,
                size = %format_size_binary(content_length),
                "FLV download started"
            );

            // Update the current span's progress bar length
            use tracing::Span;
            use tracing_indicatif::span_ext::IndicatifSpanExt;
            let span = Span::current();
            span.pb_set_length(content_length);
        } else {
            debug!(url = %url, "FLV content length not available");
        }

        Ok(response)
    }

    /// Create an FLV decoder stream from any async reader
    #[inline]
    fn create_decoder_stream<R>(&self, reader: R) -> BoxMediaStream<FlvData, FlvDownloadError>
    where
        R: tokio::io::AsyncRead + Send + 'static,
    {
        // Determine optimal buffer size based on expected content
        // Use larger buffers (at least 64KB) for better throughput, as most modern networks
        // can easily saturate smaller buffers
        let buffer_size = self.config.buffer_size.max(64 * 1024);

        let buffered_reader = tokio::io::BufReader::with_capacity(buffer_size, reader);
        let pinned_reader = Box::pin(buffered_reader);
        let flv_stream = FlvDecoderStream::with_capacity(pinned_reader, buffer_size);
        flv_stream
            .map(|result| match result {
                Ok(data) => Ok(data),
                Err(err) => Err(FlvDownloadError::Decoder(err)),
            })
            .boxed()
    }

    /// Download a stream from a URL and return an FLV data stream
    #[instrument(skip(self), level = "debug")]
    pub(crate) async fn download_url(
        &self,
        url: Url,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<FlvData, FlvDownloadError>, DownloadError> {
        tokio::select! {
            _ = token.cancelled() => {
                info!(url = %url, "Download cancelled");
                return Err(DownloadError::Cancelled);
            }
            response = self.start_download_request(&url) => {
                let response = response?;
                let mut byte_stream = response.bytes_stream();

                // Read the first chunk to validate it's FLV binary data
                let first_chunk = match byte_stream.next().await {
                    Some(Ok(chunk)) => chunk,
                    Some(Err(e)) => return Err(DownloadError::Network { source: e }),
                    None => return Err(DownloadError::InvalidContent {
                        protocol: "flv",
                        reason: "Empty response received".to_string(),
                    }),
                };

                // Validate FLV signature (first 3 bytes should be "FLV" = 0x46 0x4C 0x56)
                // OR first byte is a valid FLV tag type (for mid-stream CDN joins)
                if first_chunk.is_empty() {
                    warn!(url = %url, "Empty first chunk received");
                    return Err(DownloadError::InvalidContent {
                        protocol: "flv",
                        reason: "Empty response received".to_string(),
                    });
                }

                // Check for FLV magic bytes OR valid FLV tag types
                const FLV_SIGNATURE: [u8; 3] = [0x46, 0x4C, 0x56]; // "FLV"
                const TAG_TYPE_AUDIO: u8 = 8;
                const TAG_TYPE_VIDEO: u8 = 9;
                const TAG_TYPE_SCRIPT: u8 = 18;

                let first_byte = first_chunk[0];
                let is_header = first_chunk.len() >= 3 && first_chunk[0..3] == FLV_SIGNATURE;
                let is_valid_flv = if is_header {
                    true
                } else {
                    // Check if first byte is a valid FLV tag type (for mid-stream CDN joins)
                    // The lower 5 bits contain the tag type (ignore filter bit)
                    let tag_type = first_byte & 0x1F;
                    tag_type == TAG_TYPE_AUDIO || tag_type == TAG_TYPE_VIDEO || tag_type == TAG_TYPE_SCRIPT
                };

                if !is_valid_flv {
                    // Check if it looks like text/HTML content
                    let is_text = first_chunk.iter().take(64).all(|&b| {
                        b.is_ascii_alphanumeric() || b.is_ascii_whitespace() || b.is_ascii_punctuation()
                    });

                    let preview = if is_text {
                        // Convert to string for readable error message
                        String::from_utf8_lossy(&first_chunk[..first_chunk.len().min(128)]).to_string()
                    } else {
                        format!("{:02X?}", &first_chunk[..first_chunk.len().min(32)])
                    };

                    warn!(
                        url = %url,
                        preview = %preview,
                        first_byte = format!("0x{:02X}", first_byte),
                        is_text = is_text,
                        "Invalid FLV content: expected FLV signature or valid tag type"
                    );
                    return Err(DownloadError::InvalidContent {
                        protocol: "flv",
                        reason: format!(
                            "Invalid FLV content: expected FLV signature or valid tag type: 0x{:02X}",
                            first_byte
                        ),
                    });
                }

                // Log validation result once (header vs mid-stream)
                debug!(
                    url = %url,
                    is_header = is_header,
                    "FLV content validated, starting stream"
                );

                let (tx, rx) = mpsc::channel(2);

                // Send the first chunk we already read
                let first_chunk_for_send = first_chunk.clone();
                let stream_token = token.clone();
                tokio::spawn(async move {
                    // First, send the chunk we already validated
                    if tx.send(Ok(first_chunk_for_send)).await.is_err() {
                        return;
                    }

                    // Then continue with the rest of the stream
                    loop {
                        tokio::select! {
                            _ = stream_token.cancelled() => {
                                debug!("FLV download stream cancelled");
                                break;
                            }
                            data = byte_stream.next() => {
                                match data {
                                    Some(item) => {
                                        if tx.send(item).await.is_err() {
                                            break;
                                        }
                                    }
                                    None => break,
                                }
                            }
                        }
                    }
                });

                let stream = ReceiverStream::new(rx);
                let reader = BytesStreamReader::new(stream.boxed());
                Ok(self.create_decoder_stream(reader))
            }
        }
    }

    /// Download a stream from a URL and return a raw byte stream without parsing
    #[instrument(skip(self), level = "debug")]
    pub(crate) async fn download_url_raw(
        &self,
        url: Url,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<Bytes, FlvDownloadError>, DownloadError> {
        info!(url = %url, "Starting raw download");

        tokio::select! {
            _ = token.cancelled() => {
                info!(url = %url, "Download cancelled");
                return Err(DownloadError::Cancelled);
            }
            response = self.start_download_request(&url) => {
                let response = response?;
                let mut byte_stream = response.bytes_stream();
                let (tx, rx) = mpsc::channel(2);

                let stream_token = token.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = stream_token.cancelled() => {
                                debug!("Raw download stream cancelled");
                                break;
                            }
                            data = byte_stream.next() => {
                                match data {
                                    Some(Ok(bytes)) => {
                                        if tx.send(Ok(bytes)).await.is_err() {
                                            break;
                                        }
                                    }
                                    Some(Err(e)) => {
                                        let _ = tx
                                            .send(Err(FlvDownloadError::Download(
                                                DownloadError::Network { source: e },
                                            )))
                                            .await;
                                        break;
                                    }
                                    None => break,
                                }
                            }
                        }
                    }
                });

                Ok(ReceiverStream::new(rx).boxed())
            }
        }
    }

    /// Try to validate cached content using conditional requests
    pub(crate) async fn try_revalidate_cache(
        &self,
        url: &Url,
        metadata: &CacheMetadata,
    ) -> Result<Option<Response>, DownloadError> {
        let client = self.clients.client_for_url(url);
        let mut req = client.get(url.clone());

        if let Some(etag) = &metadata.etag {
            req = req.header("If-None-Match", etag);
        }

        if let Some(last_modified) = &metadata.last_modified {
            req = req.header("If-Modified-Since", last_modified);
        }

        let response = req.send().await?;

        if response.status() == StatusCode::NOT_MODIFIED {
            debug!(url = %url, "Content not modified");
            return Ok(None);
        }

        // Content was modified
        if !response.status().is_success() {
            Self::log_unexpected_status(url, response.status(), "cache_revalidation");
            return Err(DownloadError::http_status(
                response.status(),
                url.to_string(),
                "cache_revalidation",
            ));
        }

        Ok(Some(response))
    }

    /// Download from a URL string using a cache if available
    #[instrument(skip(self, cache_manager), level = "debug")]
    pub(crate) async fn perform_download_with_cache(
        &self,
        url_str: &str,
        cache_manager: Arc<CacheManager>,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<FlvData, FlvDownloadError>, DownloadError> {
        // Validate URL
        let url = url_str
            .parse::<Url>()
            .map_err(|e| DownloadError::invalid_url(url_str, e.to_string()))?;

        // Check cache first
        let cache_key = CacheKey::new(CacheResourceType::Response, url_str.to_string(), None);

        if let Ok(Some((data, metadata, status))) = cache_manager.get(&cache_key).await {
            match status {
                CacheStatus::Hit => {
                    info!(url = %url, "Using cached FLV data");

                    // Create a cursor over the cached data
                    let cursor = std::io::Cursor::new(data);
                    return Ok(self.create_decoder_stream(cursor));
                }
                CacheStatus::Expired => {
                    debug!(url = %url, "Cache expired, revalidating");

                    // Try to revalidate
                    match self.try_revalidate_cache(&url, &metadata).await? {
                        None => {
                            // Not modified, use cache
                            info!(url = %url, "Content not modified, using cache");

                            // Update the cache entry with new expiration
                            let new_metadata = CacheMetadata::new(data.len() as u64)
                                .with_expiration(cache_manager.config().default_ttl);

                            // No need to await this, fire and forget
                            let _ = cache_manager
                                .put(cache_key, data.clone(), new_metadata)
                                .await;

                            // Create a cursor over the cached data
                            let cursor = std::io::Cursor::new(data);
                            return Ok(self.create_decoder_stream(cursor));
                        }
                        Some(_response) => {
                            // Content modified, proceed with download below
                        }
                    }
                }
                _ => {
                    // Proceed with download
                }
            }
        }

        // Cache miss or revalidation needed, download the content
        info!(url = %url, "Starting FLV download (not in cache)");

        // Start the request
        let client = self.clients.client_for_url(&url);
        let response = client
            .get(url.clone())
            .query(&self.config.base.params)
            .send()
            .await?;

        // Check response status
        if !response.status().is_success() {
            Self::log_unexpected_status(&url, response.status(), "cache_miss_download");
            return Err(DownloadError::http_status(
                response.status(),
                url.to_string(),
                "cache_miss_download",
            ));
        }

        // Extract caching headers
        // let (etag, last_modified, content_type) = extract_cache_headers(&response);

        // Get content as bytes stream
        let bytes_stream = response.bytes_stream();

        // TODO: I dont think caching catching the entire stream is a good idea
        // // Store in cache if smaller than 10MB
        // const MAX_CACHE_SIZE: usize = 10 * 1024 * 1024;
        // if content.len() < MAX_CACHE_SIZE {
        //     let _ = cache_manager
        //         .put_response(url_str, content.clone(), etag, last_modified, content_type)
        //         .await;
        // }

        // Create our bytes stream reader adapter
        let reader = BytesStreamReader::new(bytes_stream);
        Ok(self.create_decoder_stream(reader))
    }

    /// Attempt to download from a single source
    pub(crate) async fn try_download_from_source(
        &self,
        source: &ContentSource,
        source_manager: &mut SourceManager,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<FlvData, FlvDownloadError>, DownloadError> {
        let start_time = Instant::now();

        match self.download_flv(&source.url, token).await {
            Ok(stream) => {
                // Record success for this source
                let elapsed = start_time.elapsed();
                source_manager.record_success(&source.url, elapsed);
                Ok(stream)
            }
            Err(err) => {
                // Record failure for this source
                let elapsed = start_time.elapsed();
                source_manager.record_failure(&source.url, &err, elapsed);

                warn!(
                    url = %source.url,
                    error = %err,
                    "Failed to download from source"
                );
                Err(err)
            }
        }
    }

    /// Download a stream with support for range requests
    #[instrument(skip(self), level = "debug")]
    pub(crate) async fn download_range(
        &self,
        url_str: &str,
        range: (u64, Option<u64>),
        token: CancellationToken,
    ) -> Result<BoxMediaStream<FlvData, FlvDownloadError>, DownloadError> {
        let url = url_str
            .parse::<Url>()
            .map_err(|e| DownloadError::invalid_url(url_str, e.to_string()))?;

        info!(
            url = %url,
            range_start = range.0,
            range_end = ?range.1,
            "Starting ranged FLV download"
        );

        // Create range header
        let range_header = match range.1 {
            Some(end) => format!("bytes={}-{}", range.0, end),
            None => format!("bytes={}-", range.0),
        };

        // Start the request with range
        let client = self.clients.client_for_url(&url);
        let response = client
            .get(url.clone())
            .header("Range", range_header)
            .query(&self.config.base.params)
            .send()
            .await?;

        // Check response status - should be 206 Partial Content
        if response.status() != StatusCode::PARTIAL_CONTENT && response.status() != StatusCode::OK {
            Self::log_unexpected_status(&url, response.status(), "ranged_download");
            return Err(DownloadError::http_status(
                response.status(),
                url.to_string(),
                "ranged_download",
            ));
        }

        // Get the bytes stream from the response
        let bytes_stream = response.bytes_stream();

        // Wrap the bytes stream in our adapter
        let reader = BytesStreamReader::new(bytes_stream);

        // Create the decoder stream
        Ok(self.create_decoder_stream(reader))
    }

    /// Attempt to resume download from a single source
    #[allow(dead_code)]
    async fn try_resume_from_source(
        &self,
        source: &ContentSource,
        range: (u64, Option<u64>),
        source_manager: &mut SourceManager,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<FlvData, FlvDownloadError>, DownloadError> {
        let start_time = Instant::now();

        match self.download_range(&source.url, range, token).await {
            Ok(stream) => {
                // Record success
                let elapsed = start_time.elapsed();
                source_manager.record_success(&source.url, elapsed);
                Ok(stream)
            }
            Err(err) => {
                // Record failure
                let elapsed = start_time.elapsed();
                source_manager.record_failure(&source.url, &err, elapsed);
                Err(err)
            }
        }
    }

    /// Attempt to download raw data from a single source
    #[allow(dead_code)]
    async fn try_download_raw_from_source(
        &self,
        source: &ContentSource,
        source_manager: &mut SourceManager,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<Bytes, FlvDownloadError>, DownloadError> {
        let start_time = Instant::now();

        match self.download_raw(&source.url, token).await {
            Ok(stream) => {
                // Record success for this source
                let elapsed = start_time.elapsed();
                source_manager.record_success(&source.url, elapsed);
                Ok(stream)
            }
            Err(err) => {
                // Record failure for this source
                let elapsed = start_time.elapsed();
                source_manager.record_failure(&source.url, &err, elapsed);

                warn!(
                    url = %source.url,
                    error = %err,
                    "Failed to download raw data from source"
                );
                Err(err)
            }
        }
    }

    /// Download a raw byte stream with support for range requests
    #[instrument(skip(self), level = "debug")]
    pub(crate) async fn download_raw_range(
        &self,
        url_str: &str,
        range: (u64, Option<u64>),
        token: CancellationToken,
    ) -> Result<BoxMediaStream<Bytes, FlvDownloadError>, DownloadError> {
        let url = url_str
            .parse::<Url>()
            .map_err(|e| DownloadError::invalid_url(url_str, e.to_string()))?;

        info!(
            url = %url,
            range_start = range.0,
            range_end = ?range.1,
            "Starting ranged raw download"
        );

        // Create range header
        let range_header = match range.1 {
            Some(end) => format!("bytes={}-{}", range.0, end),
            None => format!("bytes={}-", range.0),
        };

        // Start the request with range
        let client = self.clients.client_for_url(&url);
        let response = client
            .get(url.clone())
            .header("Range", range_header)
            .query(&self.config.base.params)
            .send()
            .await?;

        // Check response status - should be 206 Partial Content
        if response.status() != StatusCode::PARTIAL_CONTENT && response.status() != StatusCode::OK {
            Self::log_unexpected_status(&url, response.status(), "ranged_raw_download");
            return Err(DownloadError::http_status(
                response.status(),
                url.to_string(),
                "ranged_raw_download",
            ));
        }

        // Transform the reqwest bytes stream into our raw byte stream
        let raw_stream = response
            .bytes_stream()
            .map(|result| {
                result.map_err(|e| FlvDownloadError::Download(DownloadError::Network { source: e }))
            })
            .boxed();

        Ok(raw_stream)
    }

    /// Attempt to resume a raw download from a single source
    #[allow(dead_code)]
    async fn try_resume_raw_from_source(
        &self,
        source: &ContentSource,
        range: (u64, Option<u64>),
        source_manager: &mut SourceManager,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<Bytes, FlvDownloadError>, DownloadError> {
        let start_time = Instant::now();

        match self.download_raw_range(&source.url, range, token).await {
            Ok(stream) => {
                // Record success
                let elapsed = start_time.elapsed();
                source_manager.record_success(&source.url, elapsed);
                Ok(stream)
            }
            Err(err) => {
                // Record failure
                let elapsed = start_time.elapsed();
                source_manager.record_failure(&source.url, &err, elapsed);
                Err(err)
            }
        }
    }
}

// Implement base protocol trait
impl ProtocolBase for FlvDownloader {
    type Config = FlvProtocolConfig;

    fn new(config: Self::Config) -> Result<Self, DownloadError> {
        Self::with_config(config)
    }
}

// Implement core download capability
impl Download for FlvDownloader {
    type Data = FlvData;
    type Error = FlvDownloadError;
    type Stream = BoxMediaStream<Self::Data, Self::Error>;

    async fn download(
        &self,
        url: &str,
        token: CancellationToken,
    ) -> Result<Self::Stream, DownloadError> {
        self.download_flv(url, token).await
    }
}

// Implement resumable download capability
impl Resumable for FlvDownloader {
    async fn resume(
        &self,
        url: &str,
        range: (u64, Option<u64>),
        token: CancellationToken,
    ) -> Result<Self::Stream, DownloadError> {
        self.download_range(url, range, token).await
    }
}

// Implement multi-source download capability
impl MultiSource for FlvDownloader {
    async fn download_with_sources(
        &self,
        url: &str,
        source_manager: &mut SourceManager,
        token: CancellationToken,
    ) -> Result<Self::Stream, DownloadError> {
        if !source_manager.has_sources() {
            source_manager.add_url(url, 0);
        }

        let mut last_error = None;

        // Try sources until one succeeds or all active sources are tried
        while let Some(source) = source_manager.select_source() {
            match self
                .try_download_from_source(&source, source_manager, token.clone())
                .await
            {
                Ok(stream) => return Ok(stream),
                Err(err) => {
                    last_error = Some(err);
                }
            }
        }

        // All sources failed
        Err(last_error.unwrap_or_else(|| DownloadError::source_exhausted("No source available")))
    }
}

// Implement cache capability
impl Cacheable for FlvDownloader {
    async fn download_with_cache(
        &self,
        url: &str,
        cache_manager: Arc<CacheManager>,
        token: CancellationToken,
    ) -> Result<Self::Stream, DownloadError> {
        self.perform_download_with_cache(url, cache_manager, token)
            .await
    }
}

// Implement raw download capability
impl RawDownload for FlvDownloader {
    type Error = FlvDownloadError;
    type RawStream = BoxMediaStream<bytes::Bytes, Self::Error>;

    async fn download_raw(
        &self,
        url: &str,
        token: CancellationToken,
    ) -> Result<Self::RawStream, DownloadError> {
        self.download_raw(url, token).await
    }
}

// Implement raw resumable download capability
impl RawResumable for FlvDownloader {
    async fn resume_raw(
        &self,
        url: &str,
        range: (u64, Option<u64>),
        token: CancellationToken,
    ) -> Result<Self::RawStream, DownloadError> {
        self.download_raw_range(url, range, token).await
    }
}
