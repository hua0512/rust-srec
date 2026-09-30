//! # FLV Downloader
//!
//! This module implements efficient streaming download functionality for FLV resources.
//! It uses reqwest to download data in chunks and pipes it directly to the FLV parser,
//! minimizing memory usage and providing a seamless integration with the processing pipeline.

use bytes::{Bytes, BytesMut};
use flv::{data::FlvData, parser_async::FlvDecoderStream};
use futures::StreamExt;
use futures::stream::BoxStream;
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
        // FlvDecoderStream reads straight into its own read buffer of this
        // size, and BytesStreamReader already hands out whole network chunks,
        // so a BufReader in between would only copy every byte a second time.
        let buffer_size = self.config.buffer_size.max(64 * 1024);

        let flv_stream = FlvDecoderStream::with_capacity(Box::pin(reader), buffer_size);
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
        // The request and the content probe both wait on the network, so both
        // race cancellation: a server that sends headers and then stalls must
        // not hold a cancel until the read timeout.
        let (mut byte_stream, first_chunk) = tokio::select! {
            _ = token.cancelled() => {
                info!(url = %url, "Download cancelled");
                return Err(DownloadError::Cancelled);
            }
            opened = self.open_stream(&url, &events) => opened?,
        };
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
                                // A body error ends the resource: it was not
                                // finished, so no ResourceFinished follows it.
                                let failed = item.is_err();
                                if tx.send(item).await.is_err() || failed {
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

    /// Send the request and read enough of the body to tell FLV from an
    /// error page. Returns the rest of the body and the bytes already read.
    async fn open_stream(
        &self,
        url: &Url,
        events: &Option<EventSink>,
    ) -> Result<(BoxStream<'static, reqwest::Result<Bytes>>, Bytes), DownloadError> {
        let response = self.start_download_request(url).await?;
        emit_event(
            events,
            DownloadEvent::ResourceStarted {
                resource: ResourceId::FlvStream {
                    url: Arc::from(url.as_str()),
                },
                display_url: Arc::from(url.as_str()),
                content_length: response.content_length(),
            },
        );
        let mut byte_stream = response.bytes_stream().boxed();

        // A server may flush the first bytes in tiny chunks, so collect a
        // whole tag header's worth before judging the content.
        let mut probe = BytesMut::new();
        while probe.len() < PROBE_LEN {
            match byte_stream.next().await {
                Some(Ok(chunk)) if probe.is_empty() && chunk.len() >= PROBE_LEN => {
                    // Common case: the first chunk suffices, so no copy.
                    probe_flv_content(url, &chunk)?;
                    return Ok((byte_stream, chunk));
                }
                Some(Ok(chunk)) => probe.extend_from_slice(&chunk),
                Some(Err(e)) => return Err(DownloadError::Network { source: e }),
                None => break,
            }
        }
        let probe = probe.freeze();
        probe_flv_content(url, &probe)?;
        Ok((byte_stream, probe))
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

/// Bytes read before judging the content: one FLV tag header, which also
/// covers the 9-byte file header plus the first PreviousTagSize.
const PROBE_LEN: usize = flv::framing::TAG_HEADER_SIZE;

/// Accept a body that starts with the FLV signature, or with a plausible tag
/// header (a CDN joining mid-stream). The tag check includes the reserved bits
/// and StreamID, so text such as "Invalid token" is not mistaken for a tag.
fn probe_flv_content(url: &Url, probe: &[u8]) -> Result<(), DownloadError> {
    const FLV_SIGNATURE: &[u8; 3] = b"FLV";
    if probe.is_empty() {
        warn!(url = %url, "Empty FLV response");
        return Err(DownloadError::InvalidContent {
            protocol: "flv",
            reason: "Empty response received".to_string(),
        });
    }
    let is_header = probe.starts_with(FLV_SIGNATURE);
    let is_tag = probe
        .get(..flv::framing::TAG_HEADER_SIZE)
        .and_then(|header| <&[u8; flv::framing::TAG_HEADER_SIZE]>::try_from(header).ok())
        .is_some_and(flv::framing::is_plausible_tag_header);
    if is_header || is_tag {
        debug!(url = %url, is_header, "FLV content validated, starting stream");
        return Ok(());
    }

    let is_text = probe
        .iter()
        .take(64)
        .all(|&b| b.is_ascii_alphanumeric() || b.is_ascii_whitespace() || b.is_ascii_punctuation());
    let preview = if is_text {
        String::from_utf8_lossy(&probe[..probe.len().min(128)]).to_string()
    } else {
        format!("{:02X?}", &probe[..probe.len().min(32)])
    };
    warn!(
        url = %url,
        preview = %preview,
        first_byte = format!("0x{:02X}", probe[0]),
        is_text,
        "Invalid FLV content: expected FLV signature or valid tag header"
    );
    Err(DownloadError::InvalidContent {
        protocol: "flv",
        reason: format!(
            "Invalid FLV content: expected FLV signature or valid tag header: 0x{:02X}",
            probe[0]
        ),
    })
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
