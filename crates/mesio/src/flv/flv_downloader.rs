//! # FLV Downloader
//!
//! This module implements efficient streaming download functionality for FLV resources.
//! It uses reqwest to download data in chunks and pipes it directly to the FLV parser,
//! minimizing memory usage and providing a seamless integration with the processing pipeline.

use flv::{data::FlvData, parser_async::FlvDecoderStream};
use futures::StreamExt;
use reqwest::{Response, StatusCode, Url};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, info, warn};

use super::error::FlvDownloadError;
use super::flv_config::FlvProtocolConfig;
use crate::bytes_stream::BytesStreamReader;
use crate::{BoxMediaStream, DownloadError, downloader::create_client_pool};
use crate::{
    DownloadEvent, DownloadRequest, DownloadSession, EventSink, MediaEngine, ProtocolSelection,
    ProtocolType, ResourceId,
};
use tokio_util::sync::CancellationToken;

/// FLV Downloader for streaming FLV content from URLs
pub struct FlvDownloader {
    clients: Arc<crate::downloader::ClientPool>,
    config: FlvProtocolConfig,
}

struct CancelOnDropStream {
    inner: BoxMediaStream<FlvData, DownloadError>,
    token: CancellationToken,
}

impl CancelOnDropStream {
    fn new(inner: BoxMediaStream<FlvData, DownloadError>, token: CancellationToken) -> Self {
        Self { inner, token }
    }
}

impl futures::Stream for CancelOnDropStream {
    type Item = Result<FlvData, DownloadError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for CancelOnDropStream {
    fn drop(&mut self) {
        self.token.cancel();
    }
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

        if let Some(content_length) = response.content_length() {
            info!(
                url = %url,
                content_length,
                "FLV download started"
            );
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

    async fn download_url_with_events(
        &self,
        url: Url,
        token: CancellationToken,
        events: Option<EventSink>,
    ) -> Result<BoxMediaStream<FlvData, FlvDownloadError>, DownloadError> {
        tokio::select! {
            _ = token.cancelled() => {
                info!(url = %url, "Download cancelled");
                Err(DownloadError::Cancelled)
            }
            response = self.start_download_request(&url) => {
                let response = response?;
                let content_length = response.content_length();
                emit_event(
                    &events,
                    DownloadEvent::ResourceStarted {
                        resource: ResourceId::FlvStream {
                            url: Arc::from(url.as_str()),
                        },
                        display_url: Arc::from(url.as_str()),
                        content_length,
                    },
                );
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
                let forward_events = events.clone();
                let resource_url: Arc<str> = Arc::from(url.as_str());
                let progress_emit_min_bytes = self.config.progress_emit_min_bytes;
                let progress_emit_min_interval = self.config.progress_emit_min_interval;
                tokio::spawn(async move {
                    // First, send the chunk we already validated
                    let mut bytes_total = first_chunk_for_send.len() as u64;
                    let mut progress_since_last = first_chunk_for_send.len() as u64;
                    let mut last_progress_emit = Instant::now();
                    if progress_emit_min_bytes == 0 || progress_emit_min_interval.is_zero() {
                        emit_event(
                            &forward_events,
                            DownloadEvent::Progress {
                                resource: ResourceId::FlvStream {
                                    url: Arc::clone(&resource_url),
                                },
                                bytes_delta: progress_since_last,
                                bytes_total,
                            },
                        );
                        progress_since_last = 0;
                    }
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
                                        if let Ok(bytes) = &item {
                                            bytes_total += bytes.len() as u64;
                                            progress_since_last += bytes.len() as u64;
                                            let elapsed = last_progress_emit.elapsed();
                                            if progress_emit_min_bytes == 0
                                                || progress_emit_min_interval.is_zero()
                                                || progress_since_last >= progress_emit_min_bytes
                                                || elapsed >= progress_emit_min_interval
                                            {
                                                emit_event(
                                                    &forward_events,
                                                    DownloadEvent::Progress {
                                                        resource: ResourceId::FlvStream {
                                                            url: Arc::clone(&resource_url),
                                                        },
                                                        bytes_delta: progress_since_last,
                                                        bytes_total,
                                                    },
                                                );
                                                progress_since_last = 0;
                                                last_progress_emit = Instant::now();
                                            }
                                        }
                                        if tx.send(item).await.is_err() {
                                            break;
                                        }
                                    }
                                    None => {
                                        if progress_since_last > 0 {
                                            emit_event(
                                                &forward_events,
                                                DownloadEvent::Progress {
                                                    resource: ResourceId::FlvStream {
                                                        url: Arc::clone(&resource_url),
                                                    },
                                                    bytes_delta: progress_since_last,
                                                    bytes_total,
                                                },
                                            );
                                        }
                                        emit_event(
                                            &forward_events,
                                            DownloadEvent::ResourceFinished {
                                                resource: ResourceId::FlvStream {
                                                    url: Arc::clone(&resource_url),
                                                },
                                                bytes: bytes_total,
                                                from_cache: false,
                                            },
                                        );
                                        break;
                                    }
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

    pub async fn start_session(
        &self,
        request: DownloadRequest,
    ) -> Result<DownloadSession<FlvData>, DownloadError> {
        let token = request.cancel.unwrap_or_default();
        let stream_token = token.child_token();
        let (events, event_stream) = EventSink::channel(256);
        events.emit(DownloadEvent::Started {
            protocol: ProtocolType::Flv,
            url: Arc::from(request.url.as_str()),
        });

        let stream = self
            .download_url_with_events(request.url, stream_token.clone(), Some(events.clone()))
            .await?;
        let stream = stream.map(|item| item.map_err(DownloadError::from)).boxed();
        let stream: BoxMediaStream<FlvData, DownloadError> =
            Box::pin(CancelOnDropStream::new(stream, stream_token.clone()));

        Ok(DownloadSession {
            items: stream,
            events: event_stream,
            handle: crate::DownloadHandle::new(stream_token, None, events.dropped_counter(), None),
        })
    }
}

fn emit_event(events: &Option<EventSink>, event: DownloadEvent) {
    if let Some(events) = events {
        events.emit(event);
    }
}

impl MediaEngine for FlvDownloader {
    type Item = FlvData;

    async fn start(
        &self,
        mut request: DownloadRequest,
    ) -> Result<DownloadSession<Self::Item>, DownloadError> {
        if matches!(request.protocol, ProtocolSelection::Auto) {
            request.protocol = ProtocolSelection::Flv(Default::default());
        }
        self.start_session(request).await
    }
}
