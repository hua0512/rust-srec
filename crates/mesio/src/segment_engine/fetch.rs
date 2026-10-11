//! The per-segment data-plane unit the reactor spawns.
//!
//! One future = download + decrypt + wrap, so the payload is finished by the
//! time the reactor observes the outcome and no channel hop sits between
//! fetch and decrypt. The future reports a `FailureClass`; it never decides
//! lifecycle retry policy, never touches the store, and never dedups.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use futures::future::BoxFuture;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, HeaderMap, HeaderValue, RANGE};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};
use url::Url;

use crate::CacheManager;
use crate::cache::{CacheKey, CacheMetadata, CacheResourceType};
use crate::downloader::ClientPool;
use crate::hls::PerformanceMetrics;
use crate::redact::Redacted;
use crate::session::{DownloadEvent, EventSink};

use super::budget::{ByteBudget, ByteReservation};
use super::config::SegmentCoreConfig;
use super::container::{ContainerKind, is_m4s_segment};
use super::descriptor::{
    EncryptionDescriptor, EncryptionMethod, KeyFormat, SegmentDescriptor, SegmentSource,
};
use super::identity::{ByteRangeKey, SegmentKey, SegmentKind};
use super::payload::SegmentPayload;
use super::store::{FailureClass, ReadyJob, SegmentOutcome};

/// Everything a fetch-and-process task needs, shared across all tasks.
pub struct FetchContext {
    pub clients: Arc<ClientPool>,
    pub config: Arc<SegmentCoreConfig>,
    pub budget: Arc<ByteBudget>,
    /// Decrypts segments whose descriptor carries `encryption`. Protocols
    /// whose descriptors never carry it pass `None`; an encrypted segment then
    /// fails terminally as unsupported crypto.
    pub decryptor: Option<Arc<dyn SegmentDecryptor>>,
    pub cache_manager: Option<Arc<CacheManager>>,
    pub metrics: Option<Arc<PerformanceMetrics>>,
    pub cancel: CancellationToken,
    pub events: Option<EventSink>,
}

/// Whole-segment decryption for descriptors carrying an
/// [`EncryptionDescriptor`] with a supported method.
pub trait SegmentDecryptor: Send + Sync {
    /// Decrypt `encrypted`, returning the plaintext together with the
    /// processing reservation that keeps it charged. `download_reservation`
    /// keeps the encrypted input charged until the decrypt has consumed it;
    /// the caller releases it once this returns and holds the returned
    /// reservation until the payload is wrapped.
    fn decrypt<'a>(
        &'a self,
        ctx: &'a FetchContext,
        encryption: &'a EncryptionDescriptor,
        msn: u64,
        encrypted: Bytes,
        download_reservation: &'a mut ByteReservation,
    ) -> BoxFuture<'a, Result<(Bytes, ByteReservation), Failure>>;
}

const NO_DECRYPTOR: &str = "no decryptor is configured for encrypted segments";

/// A failed fetch or decrypt, classified for the store's retry policy.
pub struct Failure {
    pub class: FailureClass,
    pub reason: String,
}

impl Failure {
    pub fn new(class: FailureClass, reason: impl Into<String>) -> Self {
        Self {
            class,
            reason: reason.into(),
        }
    }
}

pub async fn fetch_and_process(job: ReadyJob, ctx: Arc<FetchContext>) -> SegmentOutcome {
    let descriptor = job.descriptor;
    let key = descriptor.key.clone();
    let msn = descriptor.msn;
    let mut reservation = job.reservation;

    // Unsupported crypto is terminal before any byte moves.
    if let Some(enc) = &descriptor.encryption {
        if let EncryptionMethod::Unsupported(token) = &enc.method {
            return SegmentOutcome::Failed {
                key,
                msn,
                class: FailureClass::UnsupportedCrypto,
                reason: Arc::from(format!("unsupported encryption method: {token}")),
            };
        }
        if let KeyFormat::Unsupported(format) = &enc.key_format {
            return SegmentOutcome::Failed {
                key,
                msn,
                class: FailureClass::UnsupportedCrypto,
                reason: Arc::from(format!("unsupported key format: {format}")),
            };
        }
        if ctx.decryptor.is_none() {
            return SegmentOutcome::Failed {
                key,
                msn,
                class: FailureClass::UnsupportedCrypto,
                reason: Arc::from(NO_DECRYPTOR),
            };
        }
    }

    // Cache lookup keys on the normalized identity URI (not the volatile
    // fetch URL), so a rotated auth token still hits. Cached bytes are the
    // final (decrypted) payload.
    let cache_key = segment_cache_key(&descriptor.key);
    if let Some(cache) = &ctx.cache_manager
        && let Ok(Some((bytes, _, _))) = cache.get(&cache_key).await
    {
        debug!(msn, "segment served from cache");
        if let Some(metrics) = &ctx.metrics {
            metrics.record_cache_hit();
        }
        emit_event(
            &ctx,
            DownloadEvent::ResourceFinished {
                resource: descriptor.resource_id(),
                bytes: bytes.len() as u64,
                from_cache: true,
            },
        );
        reservation.reconcile(bytes.len() as u64);
        let payload = wrap_payload(bytes, &descriptor);
        drop(reservation);
        return SegmentOutcome::Completed { key, msn, payload };
    }
    if ctx.cache_manager.is_some()
        && let Some(metrics) = &ctx.metrics
    {
        metrics.record_cache_miss();
    }

    let is_prefetch = descriptor.source == SegmentSource::PlaylistPrefetch;
    if is_prefetch && let Some(metrics) = &ctx.metrics {
        metrics.record_prefetch_initiated();
    }

    // --- Download (attempt-level retry, deliberately tight) ---
    let raw = match download_body(&ctx, &descriptor, &mut reservation).await {
        Ok(bytes) => bytes,
        Err(failure) => {
            if let Some(metrics) = &ctx.metrics {
                metrics.record_download_error();
            }
            return SegmentOutcome::Failed {
                key,
                msn,
                class: failure.class,
                reason: Arc::from(failure.reason),
            };
        }
    };

    // --- Decrypt (off-thread) when the descriptor carries encryption ---
    // `output_reservation` keeps the resident output bytes charged to a budget
    // until the payload is wrapped: the processing reservation on the
    // encrypted path, the download reservation on the clear path. It must
    // outlive the cache.put await below — releasing at decrypt return would
    // leave the decrypted bytes uncharged across that I/O (the spec's release
    // point is "when the payload is wrapped", not "after decrypt"). Held only
    // for its drop.
    let (final_bytes, output_reservation): (Bytes, ByteReservation) = match &descriptor.encryption {
        Some(enc) if enc.method == EncryptionMethod::Aes128Cbc => {
            let decrypted = match &ctx.decryptor {
                Some(decryptor) => {
                    decryptor
                        .decrypt(&ctx, enc, msn, raw, &mut reservation)
                        .await
                }
                None => Err(Failure::new(FailureClass::UnsupportedCrypto, NO_DECRYPTOR)),
            };
            match decrypted {
                Ok((bytes, processing)) => {
                    // The encrypted input was consumed by decrypt; its download
                    // reservation is released here and the output is now held by
                    // the processing reservation alone.
                    drop(reservation);
                    (bytes, processing)
                }
                Err(failure) => {
                    return SegmentOutcome::Failed {
                        key,
                        msn,
                        class: failure.class,
                        reason: Arc::from(failure.reason),
                    };
                }
            }
        }
        // Clear path: the handle moves unchanged (zero-copy); the download
        // reservation keeps the body charged through wrap.
        _ => (raw, reservation),
    };

    if let Some(cache) = &ctx.cache_manager {
        let metadata = CacheMetadata::new(final_bytes.len() as u64)
            .with_expiration(ctx.config.fetch.processed_segment_ttl);
        if let Err(e) = cache.put(cache_key, final_bytes.clone(), metadata).await {
            warn!(msn, error = %crate::redact::redact_diagnostic(&e), "failed to cache segment");
        }
    }

    let payload = wrap_payload(final_bytes, &descriptor);
    // Release the output reservation only now, after wrap: from here the
    // payload is accounted by the reactor's pending budget.
    drop(output_reservation);
    if is_prefetch && let Some(metrics) = &ctx.metrics {
        metrics.record_prefetch_used();
    }
    SegmentOutcome::Completed { key, msn, payload }
}

/// Keyed like `SegmentKey`: an init and a media segment at the same URI and
/// range are distinct identities, so they must not share a cache entry.
fn segment_cache_key(key: &SegmentKey) -> CacheKey {
    let kind = match key.kind {
        SegmentKind::Init => "init",
        SegmentKind::Media => "media",
    };
    let identifier = match key.byte_range {
        Some(range) => format!("{kind}:br={}@{}", range.length, range.offset),
        None => kind.to_string(),
    };
    CacheKey::new(
        CacheResourceType::Segment,
        key.uri.to_string(),
        Some(identifier),
    )
}

fn wrap_payload(data: Bytes, descriptor: &Arc<SegmentDescriptor>) -> SegmentPayload {
    let descriptor = Arc::clone(descriptor);
    let container = descriptor
        .meta
        .container()
        .unwrap_or_else(|| detect_container(&data, &descriptor));
    match (descriptor.key.kind, container) {
        (SegmentKind::Init, ContainerKind::Ts) => SegmentPayload::TsInit { data, descriptor },
        (SegmentKind::Init, ContainerKind::Fmp4) => SegmentPayload::Mp4Init { data, descriptor },
        (SegmentKind::Media, ContainerKind::Ts) => SegmentPayload::Ts { data, descriptor },
        (SegmentKind::Media, ContainerKind::Fmp4) => SegmentPayload::Mp4Media { data, descriptor },
    }
}

/// Container of a body whose manifest does not declare one. An init map says
/// nothing about the container (RFC 8216 allows `EXT-X-MAP` for MPEG-TS too);
/// the bytes do. Media that is not TS is fMP4 when it has an init or its URL
/// looks like a fragment.
fn detect_container(data: &[u8], descriptor: &SegmentDescriptor) -> ContainerKind {
    if is_mpeg_ts(data) {
        ContainerKind::Ts
    } else if descriptor.key.kind == SegmentKind::Init
        || descriptor.init_key.is_some()
        || is_m4s_segment(&descriptor.parsed_url)
    {
        ContainerKind::Fmp4
    } else {
        ContainerKind::Ts
    }
}

/// MPEG-TS packets are 188 bytes, each starting with the 0x47 sync byte. An
/// fMP4 box starts with a big-endian size, so a leading 0x47 would mean a box
/// over 1 GiB; checking the next packet's sync byte too rules out chance.
fn is_mpeg_ts(data: &[u8]) -> bool {
    const TS_PACKET_SIZE: usize = 188;
    const SYNC_BYTE: u8 = 0x47;
    data.first() == Some(&SYNC_BYTE)
        && data
            .get(TS_PACKET_SIZE)
            .is_none_or(|&byte| byte == SYNC_BYTE)
}

/// Download with attempt-level retry. The attempt budget is tight on purpose:
/// this future holds a concurrency slot and its download byte reservation the
/// whole time, and it can only ever use the URL captured at spawn —
/// re-discovery refreshes reach the store, not a running future. Anything
/// longer-lived returns a `FailureClass` and lets the lifecycle retry (which
/// frees the slot and re-engages the refreshed-URL path) handle it.
async fn download_body(
    ctx: &FetchContext,
    descriptor: &SegmentDescriptor,
    reservation: &mut ByteReservation,
) -> Result<Bytes, Failure> {
    const MAX_ATTEMPT_RETRIES: u32 = 2;
    let attempt_retries = ctx
        .config
        .fetch
        .max_segment_retries
        .min(MAX_ATTEMPT_RETRIES);
    let retry_delay = ctx
        .config
        .fetch
        .segment_retry_delay_base
        .min(Duration::from_secs(1));

    let mut last_failure: Option<Failure> = None;
    for attempt in 0..=attempt_retries {
        if attempt > 0 {
            tokio::select! {
                _ = ctx.cancel.cancelled() => {
                    return Err(Failure::new(FailureClass::Network, "cancelled"));
                }
                _ = tokio::time::sleep(retry_delay) => {}
            }
        }
        match download_once(ctx, descriptor, reservation).await {
            Ok(bytes) => return Ok(bytes),
            Err((failure, retryable)) => {
                if !retryable {
                    return Err(failure);
                }
                trace!(
                    attempt,
                    class = ?failure.class,
                    reason = %crate::redact::redact_diagnostic(&failure.reason),
                    "segment attempt failed; attempt-level retry"
                );
                last_failure = Some(failure);
            }
        }
    }
    Err(last_failure
        .unwrap_or_else(|| Failure::new(FailureClass::Network, "attempt retries exhausted")))
}

/// One download attempt. The bool in the error is attempt-level retryability
/// (network hiccups and 5xx); everything else goes straight back to the store.
async fn download_once(
    ctx: &FetchContext,
    descriptor: &SegmentDescriptor,
    reservation: &mut ByteReservation,
) -> Result<Bytes, (Failure, bool)> {
    let url: &Url = &descriptor.parsed_url;
    let byte_range = descriptor.key.byte_range;
    let resource = descriptor.resource_id();
    let client = ctx.clients.client_for_url(url);
    let mut request = client
        .get(url.clone())
        .query(&ctx.config.fetch.params)
        .timeout(ctx.config.fetch.segment_download_timeout);
    if let Some(range) = byte_range {
        let Some(end) = range_end(range) else {
            return Err((
                Failure::new(
                    FailureClass::InvalidFormat,
                    format!(
                        "invalid byte range length={} offset={}",
                        range.length, range.offset
                    ),
                ),
                false,
            ));
        };
        request = request.header(RANGE, format!("bytes={}-{end}", range.offset));
    }

    let started = std::time::Instant::now();
    let response = tokio::select! {
        _ = ctx.cancel.cancelled() => {
            return Err((Failure::new(FailureClass::Network, "cancelled"), false));
        }
        response = request.send() => response,
    }
    .map_err(|e| {
        let class = classify_reqwest(&e);
        (
            Failure::new(class, crate::redact::redact_reqwest(e).to_string()),
            matches!(class, FailureClass::Network | FailureClass::Timeout),
        )
    })?;

    let status = response.status();
    if !status.is_success() {
        let failure = Failure::new(
            FailureClass::Http(status.as_u16()),
            format!("HTTP {status} for {}", Redacted(url)),
        );
        // 5xx gets one quick attempt-level retry; 4xx goes straight to the
        // store (it owns 404/429 pacing and the 401/403 freshness rule).
        return Err((failure, status.is_server_error()));
    }
    let range_mode =
        validate_range_response(status, response.headers(), byte_range).map_err(|e| (e, false))?;
    let http_version = response.version();
    let content_length = response.content_length();
    emit_event(
        ctx,
        DownloadEvent::ResourceStarted {
            resource: resource.clone(),
            display_url: Arc::from(Redacted(url).to_string()),
            content_length,
        },
    );

    let max_segment_size = ctx.config.fetch.max_segment_size_bytes;

    // A size that can never fit the whole budget is terminal — retrying it
    // through the lifecycle budget would just burn reschedules.
    let budget_capacity = ctx.budget.download.capacity();
    let can_never_fit = |size: u64| -> bool { budget_capacity > 0 && size > budget_capacity };

    // A server that ignored Range sends the whole resource, but only the
    // requested window is kept (see `RangeWindow`), so size checks apply to it.
    let expected_len = match range_mode {
        RangeMode::Full(range) => Some(range.length),
        RangeMode::None | RangeMode::Partial(_) => content_length,
    };
    let mut window = match range_mode {
        RangeMode::Full(range) => Some(RangeWindow::new(range)),
        RangeMode::None | RangeMode::Partial(_) => None,
    };

    // First reconcile point: response headers. Content-Length was unknowable
    // at admission; grow (or shrink later) toward it now.
    if let Some(content_length) = expected_len {
        if (max_segment_size > 0 && content_length > max_segment_size)
            || can_never_fit(content_length)
        {
            return Err((
                Failure::new(
                    FailureClass::Oversize,
                    format!(
                        "Content-Length {content_length} exceeds the per-segment maximum or the download byte budget"
                    ),
                ),
                false,
            ));
        }
        let held = reservation.held_bytes();
        if content_length > held && reservation.grow(content_length - held).is_err() {
            return Err((
                Failure::new(
                    FailureClass::OverBudget,
                    "download byte budget cannot fit Content-Length",
                ),
                false,
            ));
        }
    }

    // Stream the body, enforcing the reservation at chunk granularity: a
    // chunked or lying response cannot blow the budget one chunk at a time.
    let mut buffer = BytesMut::with_capacity(
        usize::try_from(expected_len.unwrap_or(8 * 1024)).unwrap_or(8 * 1024),
    );
    let mut stream = response.bytes_stream();
    let mut progress_since_last = 0_u64;
    let mut last_progress_emit = Instant::now();
    loop {
        let chunk = tokio::select! {
            _ = ctx.cancel.cancelled() => {
                return Err((Failure::new(FailureClass::Network, "cancelled"), false));
            }
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| {
            let class = classify_reqwest(&e);
            (
                Failure::new(class, crate::redact::redact_reqwest(e).to_string()),
                true,
            )
        })?;
        let chunk = match window.as_mut() {
            Some(window) => window.keep(chunk),
            None => chunk,
        };

        let new_len = buffer.len() as u64 + chunk.len() as u64;
        progress_since_last += chunk.len() as u64;
        let elapsed = last_progress_emit.elapsed();
        let progress_min_bytes = ctx.config.fetch.progress_emit_min_bytes;
        let progress_min_interval = ctx.config.fetch.progress_emit_min_interval;
        if progress_min_bytes == 0
            || progress_min_interval.is_zero()
            || progress_since_last >= progress_min_bytes
            || elapsed >= progress_min_interval
        {
            emit_event(
                ctx,
                DownloadEvent::Progress {
                    resource: resource.clone(),
                    bytes_delta: progress_since_last,
                    bytes_total: new_len,
                },
            );
            progress_since_last = 0;
            last_progress_emit = Instant::now();
        }
        if (max_segment_size > 0 && new_len > max_segment_size) || can_never_fit(new_len) {
            return Err((
                Failure::new(
                    FailureClass::Oversize,
                    format!(
                        "body exceeds the per-segment maximum or the download byte budget at {new_len} bytes"
                    ),
                ),
                false,
            ));
        }
        if new_len > reservation.held_bytes() {
            let needed = new_len - reservation.held_bytes();
            if reservation.grow(needed).is_err() {
                // Abort rather than wait: blocking here while holding budget
                // would deadlock against other growers. The lifecycle retry
                // re-attempts when the budget is freer.
                return Err((
                    Failure::new(
                        FailureClass::OverBudget,
                        "download byte budget exhausted mid-body",
                    ),
                    false,
                ));
            }
        }
        buffer.extend_from_slice(&chunk);
        if window.as_ref().is_some_and(RangeWindow::is_complete) {
            // The rest of the ignored-Range resource is not needed.
            break;
        }
    }

    let bytes = materialize_range(buffer.freeze(), range_mode).map_err(|e| (e, false))?;
    if progress_since_last > 0 {
        emit_event(
            ctx,
            DownloadEvent::Progress {
                resource: resource.clone(),
                bytes_delta: progress_since_last,
                bytes_total: bytes.len() as u64,
            },
        );
    }
    reservation.reconcile(bytes.len() as u64);

    if let Some(metrics) = &ctx.metrics {
        metrics.record_download(bytes.len() as u64, started.elapsed().as_millis() as u64);
        metrics.record_request_with_host(
            http_version,
            bytes.len() as u64,
            url.host_str().unwrap_or_default(),
        );
    }
    emit_event(
        ctx,
        DownloadEvent::ResourceFinished {
            resource,
            bytes: bytes.len() as u64,
            from_cache: false,
        },
    );
    trace!(size = bytes.len(), url = %Redacted(url), "segment downloaded");
    Ok(bytes)
}

fn emit_event(ctx: &FetchContext, event: DownloadEvent) {
    if let Some(events) = &ctx.events {
        events.emit(event);
    }
}

#[derive(Debug, Clone, Copy)]
enum RangeMode {
    None,
    /// The server honored the Range request and returned exactly the requested
    /// sub-resource.
    Partial(ByteRangeKey),
    /// The server ignored Range and returned the full resource. Only the
    /// requested bytes are kept as the body streams in (see `RangeWindow`).
    Full(ByteRangeKey),
}

fn validate_range_response(
    status: StatusCode,
    headers: &HeaderMap,
    range: Option<ByteRangeKey>,
) -> Result<RangeMode, Failure> {
    let Some(range) = range else {
        return Ok(RangeMode::None);
    };

    match status {
        StatusCode::PARTIAL_CONTENT => {
            let Some((start, end)) = headers.get(CONTENT_RANGE).and_then(parse_content_range)
            else {
                return Err(Failure::new(
                    FailureClass::InvalidFormat,
                    "206 response for BYTERANGE is missing a valid Content-Range header",
                ));
            };
            let expected_end = range_end(range).ok_or_else(|| {
                Failure::new(
                    FailureClass::InvalidFormat,
                    format!(
                        "invalid byte range length={} offset={}",
                        range.length, range.offset
                    ),
                )
            })?;
            if start != range.offset || end != expected_end {
                return Err(Failure::new(
                    FailureClass::InvalidFormat,
                    format!(
                        "206 Content-Range bytes {start}-{end} does not match requested bytes {}-{expected_end}",
                        range.offset
                    ),
                ));
            }
            Ok(RangeMode::Partial(range))
        }
        StatusCode::OK => Ok(RangeMode::Full(range)),
        other => Err(Failure::new(
            FailureClass::InvalidFormat,
            format!("unexpected HTTP {other} for BYTERANGE request"),
        )),
    }
}

fn parse_content_range(value: &HeaderValue) -> Option<(u64, u64)> {
    let value = value.to_str().ok()?.trim();
    let rest = value.strip_prefix("bytes ")?;
    let (range, _) = rest.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.trim().parse::<u64>().ok()?;
    let end = end.trim().parse::<u64>().ok()?;
    (start <= end).then_some((start, end))
}

/// The requested byte range within a full response whose server ignored the
/// Range header. Bytes outside it are dropped as they stream in, so memory and
/// the download budget are charged only for the range.
struct RangeWindow {
    /// Position in the full resource of the next byte to arrive.
    position: u64,
    start: u64,
    end: u64,
}

impl RangeWindow {
    fn new(range: ByteRangeKey) -> Self {
        Self {
            position: 0,
            start: range.offset,
            end: range.offset.saturating_add(range.length),
        }
    }

    fn keep(&mut self, chunk: Bytes) -> Bytes {
        let chunk_start = self.position;
        self.position = self.position.saturating_add(chunk.len() as u64);
        let from = self
            .start
            .saturating_sub(chunk_start)
            .min(chunk.len() as u64);
        let to = self.end.saturating_sub(chunk_start).min(chunk.len() as u64);
        if from >= to {
            return Bytes::new();
        }
        // Both bounds are within chunk.len(), so they fit in usize.
        chunk.slice(from as usize..to as usize)
    }

    fn is_complete(&self) -> bool {
        self.position >= self.end
    }
}

fn materialize_range(bytes: Bytes, mode: RangeMode) -> Result<Bytes, Failure> {
    match mode {
        RangeMode::None => Ok(bytes),
        RangeMode::Partial(range) => {
            if bytes.len() as u64 == range.length {
                Ok(bytes)
            } else {
                Err(Failure::new(
                    FailureClass::InvalidFormat,
                    format!(
                        "partial BYTERANGE body length {} does not match requested length {}",
                        bytes.len(),
                        range.length
                    ),
                ))
            }
        }
        // The body was already narrowed to the window while streaming.
        RangeMode::Full(range) => {
            if bytes.len() as u64 == range.length {
                Ok(bytes)
            } else {
                Err(Failure::new(
                    FailureClass::InvalidFormat,
                    format!(
                        "full response does not contain requested byte range {}-{} (got {} bytes of it)",
                        range.offset,
                        range.offset.saturating_add(range.length),
                        bytes.len()
                    ),
                ))
            }
        }
    }
}

fn range_end(range: ByteRangeKey) -> Option<u64> {
    range
        .length
        .checked_sub(1)
        .and_then(|last| range.offset.checked_add(last))
}

fn classify_reqwest(e: &reqwest::Error) -> FailureClass {
    if e.is_timeout() {
        FailureClass::Timeout
    } else {
        FailureClass::Network
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment_engine::descriptor::{SegmentDescriptor, SegmentMeta};

    fn descriptor(
        uri: &str,
        kind: SegmentKind,
        init_key: Option<SegmentKey>,
    ) -> Arc<SegmentDescriptor> {
        Arc::new(SegmentDescriptor {
            key: SegmentKey {
                kind,
                uri: Arc::from(uri),
                byte_range: None,
            },
            msn: 1,
            source: SegmentSource::Playlist,
            parsed_url: Arc::new(Url::parse(uri).unwrap()),
            discontinuity: false,
            encryption: None,
            init_key,
            meta: SegmentMeta::Hls(Arc::new(m3u8_rs::MediaSegment {
                uri: uri.to_string(),
                ..Default::default()
            })),
        })
    }

    fn ts_packets(count: usize) -> Bytes {
        let mut data = vec![0u8; 188 * count];
        for packet in data.chunks_mut(188) {
            packet[0] = 0x47;
        }
        Bytes::from(data)
    }

    #[test]
    fn ts_stream_with_a_map_stays_ts() {
        let map = descriptor("https://e.com/map.ts", SegmentKind::Init, None);
        let media = descriptor(
            "https://e.com/seg.ts",
            SegmentKind::Media,
            Some(map.key.clone()),
        );

        assert!(matches!(
            wrap_payload(ts_packets(2), &map),
            SegmentPayload::TsInit { .. }
        ));
        assert!(matches!(
            wrap_payload(ts_packets(3), &media),
            SegmentPayload::Ts { .. }
        ));
    }

    #[test]
    fn fmp4_with_a_map_stays_fmp4() {
        let map = descriptor("https://e.com/init.mp4", SegmentKind::Init, None);
        let media = descriptor(
            "https://e.com/seg.m4s",
            SegmentKind::Media,
            Some(map.key.clone()),
        );
        let ftyp = Bytes::from_static(b"\x00\x00\x00\x18ftypiso6");
        let moof = Bytes::from_static(b"\x00\x00\x00\x10moof");

        assert!(matches!(
            wrap_payload(ftyp, &map),
            SegmentPayload::Mp4Init { .. }
        ));
        assert!(matches!(
            wrap_payload(moof, &media),
            SegmentPayload::Mp4Media { .. }
        ));
    }

    #[test]
    fn a_lone_sync_byte_followed_by_non_ts_data_is_not_ts() {
        let mut data = vec![0u8; 400];
        data[0] = 0x47;
        assert!(!is_mpeg_ts(&data));
        assert!(is_mpeg_ts(&ts_packets(1)));
    }

    #[test]
    fn init_and_media_at_the_same_uri_use_distinct_cache_entries() {
        let media = descriptor("https://e.com/same.mp4", SegmentKind::Media, None);
        let init = descriptor("https://e.com/same.mp4", SegmentKind::Init, None);

        assert_ne!(segment_cache_key(&media.key), segment_cache_key(&init.key));
    }
}
