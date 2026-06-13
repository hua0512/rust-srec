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
use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, HeaderMap, HeaderValue, RANGE};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};
use url::Url;

use crate::CacheManager;
use crate::cache::{CacheKey, CacheMetadata, CacheResourceType};
use crate::downloader::ClientPool;
use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use crate::hls::metrics::PerformanceMetrics;
use crate::hls::segment_utils::is_m4s_segment;
use crate::session::{DownloadEvent, EventSink, ResourceId};

use super::budget::{ByteBudget, ByteReservation};
use super::crypto::{CryptoExecutor, KeyCache, validate_key_bytes};
use super::descriptor::{EffectiveIv, EncryptionDescriptor, EncryptionMethod, KeyFormat};
use super::identity::{ByteRangeKey, SegmentKind};
use super::payload::SegmentPayload;
use super::store::{FailureClass, ReadyJob, SegmentOutcome};

/// Everything a fetch-and-process task needs, shared across all tasks.
pub struct FetchContext {
    pub clients: Arc<ClientPool>,
    pub config: Arc<HlsConfig>,
    pub budget: Arc<ByteBudget>,
    pub crypto: CryptoExecutor,
    pub key_cache: KeyCache,
    pub cache_manager: Option<Arc<CacheManager>>,
    pub metrics: Option<Arc<PerformanceMetrics>>,
    pub cancel: CancellationToken,
    pub events: Option<EventSink>,
}

struct Failure {
    class: FailureClass,
    reason: String,
}

impl Failure {
    fn new(class: FailureClass, reason: impl Into<String>) -> Self {
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
                resource: ResourceId::HlsSegment {
                    key: descriptor.key.clone(),
                },
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

    // --- Download (attempt-level retry, deliberately tight) ---
    let raw = match download_body(
        &ctx,
        &descriptor.parsed_url,
        &descriptor.key,
        &mut reservation,
    )
    .await
    {
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
            match decrypt_segment(&ctx, enc, msn, raw, &mut reservation).await {
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
            .with_expiration(ctx.config.processor_config.processed_segment_ttl);
        if let Err(e) = cache.put(cache_key, final_bytes.clone(), metadata).await {
            warn!(msn, "failed to cache segment: {e}");
        }
    }

    let payload = wrap_payload(final_bytes, &descriptor);
    // Release the output reservation only now, after wrap: from here the
    // payload is accounted by the reactor's pending budget.
    drop(output_reservation);
    SegmentOutcome::Completed { key, msn, payload }
}

fn segment_cache_key(key: &super::identity::SegmentKey) -> CacheKey {
    CacheKey::new(
        CacheResourceType::Segment,
        key.uri.to_string(),
        key.byte_range
            .map(|range| format!("br={}@{}", range.length, range.offset)),
    )
}

fn wrap_payload(
    data: Bytes,
    descriptor: &Arc<super::descriptor::SegmentDescriptor>,
) -> SegmentPayload {
    let descriptor = Arc::clone(descriptor);
    if descriptor.key.kind == SegmentKind::Init {
        SegmentPayload::Mp4Init { data, descriptor }
    } else if descriptor.init_key.is_some() || is_m4s_segment(&descriptor.parsed_url) {
        SegmentPayload::Mp4Media { data, descriptor }
    } else {
        SegmentPayload::Ts { data, descriptor }
    }
}

/// Download with attempt-level retry. The attempt budget is tight on purpose:
/// this future holds a concurrency slot and its download byte reservation the
/// whole time, and it can only ever use the URL captured at spawn —
/// re-discovery refreshes reach the store, not a running future. Anything
/// longer-lived returns a `FailureClass` and lets the lifecycle retry (which
/// frees the slot and re-engages the refreshed-URL path) handle it.
async fn download_body(
    ctx: &FetchContext,
    url: &Url,
    key: &super::identity::SegmentKey,
    reservation: &mut ByteReservation,
) -> Result<Bytes, Failure> {
    const MAX_ATTEMPT_RETRIES: u32 = 2;
    let attempt_retries = ctx
        .config
        .fetcher_config
        .max_segment_retries
        .min(MAX_ATTEMPT_RETRIES);
    let retry_delay = ctx
        .config
        .fetcher_config
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
        match download_once(ctx, url, key, reservation).await {
            Ok(bytes) => return Ok(bytes),
            Err((failure, retryable)) => {
                if !retryable {
                    return Err(failure);
                }
                trace!(
                    attempt,
                    class = ?failure.class,
                    reason = %failure.reason,
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
    url: &Url,
    key: &super::identity::SegmentKey,
    reservation: &mut ByteReservation,
) -> Result<Bytes, (Failure, bool)> {
    let client = ctx.clients.client_for_url(url);
    let mut request = client
        .get(url.clone())
        .query(&ctx.config.base.params)
        .timeout(ctx.config.fetcher_config.segment_download_timeout);
    if let Some(range) = key.byte_range {
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
            Failure::new(class, e.to_string()),
            matches!(class, FailureClass::Network | FailureClass::Timeout),
        )
    })?;

    let status = response.status();
    if !status.is_success() {
        let failure = Failure::new(
            FailureClass::Http(status.as_u16()),
            format!("HTTP {status} for {url}"),
        );
        // 5xx gets one quick attempt-level retry; 4xx goes straight to the
        // store (it owns 404/429 pacing and the 401/403 freshness rule).
        return Err((failure, status.is_server_error()));
    }
    let range_mode = validate_range_response(status, response.headers(), key.byte_range)
        .map_err(|e| (e, false))?;
    let content_length = response.content_length();
    emit_event(
        ctx,
        DownloadEvent::ResourceStarted {
            resource: ResourceId::HlsSegment { key: key.clone() },
            display_url: Arc::from(url.as_str()),
            content_length,
        },
    );

    let max_segment_size = ctx.config.engine_config.max_segment_size_bytes;

    // A size that can never fit the whole budget is terminal — retrying it
    // through the lifecycle budget would just burn reschedules.
    let budget_capacity = ctx.budget.download.capacity();
    let can_never_fit = |size: u64| -> bool { budget_capacity > 0 && size > budget_capacity };

    // First reconcile point: response headers. Content-Length was unknowable
    // at admission; grow (or shrink later) toward it now.
    if let Some(content_length) = content_length {
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
        usize::try_from(content_length.unwrap_or(8 * 1024)).unwrap_or(8 * 1024),
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
            (Failure::new(class, e.to_string()), true)
        })?;

        let new_len = buffer.len() as u64 + chunk.len() as u64;
        progress_since_last += chunk.len() as u64;
        let elapsed = last_progress_emit.elapsed();
        let progress_min_bytes = ctx.config.fetcher_config.progress_emit_min_bytes;
        let progress_min_interval = ctx.config.fetcher_config.progress_emit_min_interval;
        if progress_min_bytes == 0
            || progress_min_interval.is_zero()
            || progress_since_last >= progress_min_bytes
            || elapsed >= progress_min_interval
        {
            emit_event(
                ctx,
                DownloadEvent::Progress {
                    resource: ResourceId::HlsSegment { key: key.clone() },
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
    }

    let bytes = materialize_range(buffer.freeze(), range_mode).map_err(|e| (e, false))?;
    if progress_since_last > 0 {
        emit_event(
            ctx,
            DownloadEvent::Progress {
                resource: ResourceId::HlsSegment { key: key.clone() },
                bytes_delta: progress_since_last,
                bytes_total: bytes.len() as u64,
            },
        );
    }
    reservation.reconcile(bytes.len() as u64);

    if let Some(metrics) = &ctx.metrics {
        metrics.record_download(bytes.len() as u64, started.elapsed().as_millis() as u64);
    }
    emit_event(
        ctx,
        DownloadEvent::ResourceFinished {
            resource: ResourceId::HlsSegment { key: key.clone() },
            bytes: bytes.len() as u64,
            from_cache: false,
        },
    );
    trace!(size = bytes.len(), %url, "segment downloaded");
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
    /// The server ignored Range and returned the full resource. We must copy
    /// out the requested bytes so the retained `Bytes` allocation is the range,
    /// not the whole backing object.
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
        RangeMode::Full(range) => {
            let start = usize::try_from(range.offset).map_err(|_| {
                Failure::new(
                    FailureClass::InvalidFormat,
                    format!("byte range offset {} cannot be represented", range.offset),
                )
            })?;
            let length = usize::try_from(range.length).map_err(|_| {
                Failure::new(
                    FailureClass::InvalidFormat,
                    format!("byte range length {} cannot be represented", range.length),
                )
            })?;
            let end = start.checked_add(length).ok_or_else(|| {
                Failure::new(
                    FailureClass::InvalidFormat,
                    format!(
                        "byte range length={} offset={} overflows",
                        range.length, range.offset
                    ),
                )
            })?;
            let Some(slice) = bytes.get(start..end) else {
                return Err(Failure::new(
                    FailureClass::InvalidFormat,
                    format!(
                        "full response length {} does not contain requested byte range {}-{end}",
                        bytes.len(),
                        range.offset
                    ),
                ));
            };
            Ok(Bytes::copy_from_slice(slice))
        }
    }
}

fn range_end(range: ByteRangeKey) -> Option<u64> {
    range
        .length
        .checked_sub(1)
        .and_then(|last| range.offset.checked_add(last))
}

/// Decrypt the encrypted body, returning the plaintext together with the
/// processing reservation that keeps it charged. The caller drops the
/// download reservation once this returns (the input has been consumed) and
/// holds the returned processing reservation until the payload is wrapped.
async fn decrypt_segment(
    ctx: &FetchContext,
    enc: &EncryptionDescriptor,
    msn: u64,
    encrypted: Bytes,
    download_reservation: &mut ByteReservation,
) -> Result<(Bytes, ByteReservation), Failure> {
    let key_bytes = fetch_key(ctx, enc).await?;

    let iv = match enc.iv {
        EffectiveIv::Explicit(iv) => iv,
        EffectiveIv::MediaSequenceDerived(_) => {
            // Per RFC 8216: big-endian MSN of *this* segment in a 16-octet IV.
            let mut iv = [0u8; 16];
            iv[8..].copy_from_slice(&msn.to_be_bytes());
            iv
        }
    };

    // Reserve the decrypted-output upper bound (AES-CBC output is never
    // larger than its input) BEFORE dispatching crypto. The wait is FIFO; a
    // task parked here still holds its download reservation, so a decrypt
    // backlog throttles admission instead of piling up uncounted bytes.
    let encrypted_len = encrypted.len() as u64;
    let mut processing = match ctx.budget.processing.reserve(encrypted_len).await {
        Ok(reservation) => reservation,
        Err(_) => {
            return Err(Failure::new(
                FailureClass::Oversize,
                format!(
                    "encrypted input of {encrypted_len} bytes can never fit max_processing_bytes"
                ),
            ));
        }
    };

    let started = std::time::Instant::now();
    let decrypted = ctx
        .crypto
        .decrypt_aes128_cbc(encrypted, key_bytes, iv)
        .await
        .map_err(|e| Failure::new(FailureClass::Decode, e.to_string()))?;
    // The encrypted input has been consumed by the decrypt copy. The caller
    // drops the download reservation on success; reconcile it to 0 here too so
    // an error return (which the caller does not drain) cannot leave the input
    // charged. There is no release-then-recharge hole while waiting for the
    // crypto gate: the input stayed charged until decrypt consumed it.
    download_reservation.reconcile(0);
    processing.reconcile(decrypted.len() as u64);

    if let Some(metrics) = &ctx.metrics {
        metrics.record_decryption(decrypted.len() as u64, started.elapsed().as_millis() as u64);
    }

    // The processing reservation is returned to the caller, which holds it
    // across cache.put and releases it only after the payload is wrapped.
    Ok((decrypted, processing))
}

/// Fetch (or hit) the decryption key. Single-flight per identity URI via the
/// key cache; the load future uses the *latest* fetch URL.
async fn fetch_key(ctx: &FetchContext, enc: &EncryptionDescriptor) -> Result<[u8; 16], Failure> {
    let identity = Arc::clone(&enc.key_identity_uri);
    let fetch_url = Arc::clone(&enc.key_fetch_url);
    emit_event(
        ctx,
        DownloadEvent::ResourceStarted {
            resource: ResourceId::HlsKey {
                uri: Arc::clone(&identity),
            },
            display_url: Arc::from(fetch_url.as_str()),
            content_length: None,
        },
    );
    let result = ctx
        .key_cache
        .get_with(Arc::clone(&identity), {
            let ctx_clients = Arc::clone(&ctx.clients);
            let timeout = ctx.config.fetcher_config.key_download_timeout;
            let params = ctx.config.base.params.clone();
            let identity = Arc::clone(&identity);
            async move {
                let client = ctx_clients.client_for_url(&fetch_url);
                let response = client
                    .get(fetch_url.as_ref().clone())
                    .query(&params)
                    .timeout(timeout)
                    .send()
                    .await
                    .map_err(|e| HlsDownloaderError::Network { source: e })?;
                let status = response.status();
                if !status.is_success() {
                    return Err(HlsDownloaderError::http_status(
                        status,
                        fetch_url.as_str(),
                        "hls key fetch",
                    ));
                }
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|e| HlsDownloaderError::Network { source: e })?;
                validate_key_bytes(&bytes, &identity)
            }
        })
        .await;

    match result {
        Ok(key) => {
            emit_event(
                ctx,
                DownloadEvent::ResourceFinished {
                    resource: ResourceId::HlsKey { uri: identity },
                    bytes: key.len() as u64,
                    from_cache: false,
                },
            );
            Ok(key)
        }
        Err(e) => Err(match e.as_ref() {
            HlsDownloaderError::HttpStatus { status, .. } => Failure::new(
                FailureClass::Http(status.as_u16()),
                format!("key fetch failed: {e}"),
            ),
            HlsDownloaderError::Network { source } if source.is_timeout() => {
                Failure::new(FailureClass::Timeout, format!("key fetch timed out: {e}"))
            }
            HlsDownloaderError::Network { .. } => {
                Failure::new(FailureClass::Network, format!("key fetch failed: {e}"))
            }
            HlsDownloaderError::Decryption { .. } => Failure::new(
                FailureClass::InvalidFormat,
                format!("key validation failed: {e}"),
            ),
            other => Failure::new(FailureClass::Network, format!("key fetch failed: {other}")),
        }),
    }
}

fn classify_reqwest(e: &reqwest::Error) -> FailureClass {
    if e.is_timeout() {
        FailureClass::Timeout
    } else {
        FailureClass::Network
    }
}
