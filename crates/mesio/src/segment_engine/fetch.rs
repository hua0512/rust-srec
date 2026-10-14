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
use crate::DownloadError;
use crate::cache::{CacheKey, CacheMetadata, CacheResourceType};
use crate::downloader::ClientPool;
use crate::redact::Redacted;
use crate::session::{DownloadEvent, EventSink};

use super::budget::{ByteBudget, ByteReservation};
use super::config::SegmentCoreConfig;
use super::container::{ContainerKind, is_m4s_segment};
use super::descriptor::{
    EncryptionDescriptor, EncryptionMethod, KeyFormat, SegmentDescriptor, SegmentSource,
};
use super::identity::{ByteRangeKey, SegmentKey, SegmentKind};
use super::metrics::PerformanceMetrics;
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
    /// See `SegmentOutcome::Failed::error`.
    pub error: Option<DownloadError>,
}

impl Failure {
    pub fn new(class: FailureClass, reason: impl Into<String>) -> Self {
        Self {
            class,
            reason: reason.into(),
            error: None,
        }
    }

    pub fn with_error(mut self, error: DownloadError) -> Self {
        self.error = Some(error);
        self
    }

    fn into_outcome(self, key: SegmentKey, msn: u64) -> SegmentOutcome {
        SegmentOutcome::Failed {
            key,
            msn,
            class: self.class,
            reason: Arc::from(self.reason),
            error: self.error,
        }
    }
}

pub async fn fetch_and_process(job: ReadyJob, ctx: Arc<FetchContext>) -> SegmentOutcome {
    let descriptor = job.descriptor;
    let key = descriptor.key.clone();
    let msn = descriptor.msn;
    let mut reservation = job.reservation;

    // Unsupported crypto is terminal before any byte moves.
    let decryptor = match &descriptor.encryption {
        Some(enc) => {
            let unsupported = |reason: String| {
                Failure::new(FailureClass::UnsupportedCrypto, reason).into_outcome(key.clone(), msn)
            };
            if let EncryptionMethod::Unsupported(token) = &enc.method {
                return unsupported(format!("unsupported encryption method: {token}"));
            }
            if let KeyFormat::Unsupported(format) = &enc.key_format {
                return unsupported(format!("unsupported key format: {format}"));
            }
            match &ctx.decryptor {
                Some(decryptor) => Some(decryptor),
                None => return unsupported(NO_DECRYPTOR.to_owned()),
            }
        }
        None => None,
    };

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
            if !matches!(
                failure.class,
                FailureClass::RateLimited {
                    held_back: true,
                    ..
                }
            ) && let Some(metrics) = &ctx.metrics
            {
                metrics.record_download_error();
            }
            return failure.into_outcome(key, msn);
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
    let (final_bytes, output_reservation): (Bytes, ByteReservation) =
        match (&descriptor.encryption, decryptor) {
            (Some(enc), Some(decryptor)) if enc.method == EncryptionMethod::Aes128Cbc => {
                match decryptor
                    .decrypt(&ctx, enc, msn, raw, &mut reservation)
                    .await
                {
                    Ok((bytes, processing)) => {
                        // The encrypted input was consumed by decrypt; its download
                        // reservation is released here and the output is now held by
                        // the processing reservation alone.
                        drop(reservation);
                        (bytes, processing)
                    }
                    Err(failure) => return failure.into_outcome(key, msn),
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
    let mut identifier = match key.byte_range {
        Some(range) => format!("{kind}:br={}@{}", range.length, range.offset),
        None => kind.to_string(),
    };
    if let Some(scope) = &key.scope {
        identifier.push_str(":scope=");
        identifier.push_str(scope);
    }
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

    let operation = match descriptor.key.kind {
        SegmentKind::Init => "init segment download",
        SegmentKind::Media => "segment download",
    };
    // A segment does not wait for a cooling origin here: it goes back to the
    // store, which retries it when the cool-down ends, so its download slot
    // and byte reservation serve other requests meanwhile. The admission is
    // held until the answer has been observed, so a probe stays out until
    // then.
    let admission = ctx
        .clients
        .rate_limiter()
        .try_admit(url)
        .map_err(|throttled| (held_back_failure(throttled.error(url, operation)), false))?;
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
    let rate_limited = admission.observe(response.url(), status, response.headers());
    if !status.is_success() {
        if let Some(retry_in) = rate_limited {
            let error =
                DownloadError::rate_limited(status, response.url().as_str(), operation, retry_in);
            return Err((rate_limited_failure(error), false));
        }
        let failure = Failure::new(
            FailureClass::Http(status.as_u16()),
            format!("HTTP {status} for {}", Redacted(url)),
        )
        .with_error(DownloadError::http_status(status, url.as_str(), operation));
        // A 5xx gets one quick attempt-level retry; anything else goes
        // straight to the store (it owns rate-limit and 404 pacing and the
        // 401/403 freshness rule).
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
        metrics.record_response(http_version, bytes.len() as u64);
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

/// Request settings of [`fetch_range`].
#[derive(Clone, Copy)]
pub(crate) struct RangeRequest<'a> {
    /// Query parameters added to the URL.
    pub params: &'a [(String, String)],
    pub timeout: Duration,
    pub cancel: &'a CancellationToken,
    /// Names the request in errors.
    pub operation: &'static str,
    /// Until when the request waits for a cool-down after rate limiting.
    pub throttle_deadline: tokio::time::Instant,
}

/// Fetches `range` of `url` outside the segment pipeline (no byte budget,
/// cache or events), for small metadata such as a DASH Segment Index. The
/// response is checked as for segments: a `206` must cover exactly the
/// range, and of a `200` (a server ignoring Range) only the range is kept.
pub(crate) async fn fetch_range(
    clients: &ClientPool,
    url: &Url,
    range: ByteRangeKey,
    settings: &RangeRequest<'_>,
) -> Result<Bytes, DownloadError> {
    let RangeRequest {
        params,
        timeout,
        cancel,
        operation,
        throttle_deadline,
    } = *settings;
    let invalid = |reason: String| DownloadError::InvalidContent {
        protocol: "http",
        reason,
    };
    let end = range_end(range).ok_or_else(|| invalid("empty byte range".to_owned()))?;
    let request = clients
        .client_for_url(url)
        .get(url.clone())
        .query(params)
        .header(RANGE, format!("bytes={}-{end}", range.offset))
        .timeout(timeout);
    let admission = clients
        .rate_limiter()
        .admit(url, cancel, throttle_deadline, operation)
        .await?;
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
        response = request.send() => response?,
    };
    let status = response.status();
    if let Some(retry_in) = admission.observe(response.url(), status, response.headers()) {
        return Err(DownloadError::rate_limited(
            status,
            response.url().as_str(),
            operation,
            retry_in,
        ));
    }
    drop(admission);
    if !status.is_success() {
        return Err(DownloadError::http_status(status, url.as_str(), operation));
    }
    let mode = validate_range_response(status, response.headers(), Some(range))
        .map_err(|failure| invalid(failure.reason))?;
    let mut window = match mode {
        RangeMode::Full(range) => Some(RangeWindow::new(range)),
        RangeMode::None | RangeMode::Partial(_) => None,
    };
    let mut buffer = BytesMut::new();
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else { break };
        let chunk = match window.as_mut() {
            Some(window) => window.keep(chunk?),
            None => chunk?,
        };
        if buffer.len() as u64 + chunk.len() as u64 > range.length {
            return Err(invalid(format!(
                "response is longer than the requested {} bytes",
                range.length
            )));
        }
        buffer.extend_from_slice(&chunk);
        if window.as_ref().is_some_and(RangeWindow::is_complete) {
            break;
        }
    }
    materialize_range(buffer.freeze(), mode).map_err(|failure| invalid(failure.reason))
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

/// A request held back by a cooling server before it was sent.
fn held_back_failure(error: DownloadError) -> Failure {
    let mut failure = rate_limited_failure(error);
    if let FailureClass::RateLimited { held_back, .. } = &mut failure.class {
        *held_back = true;
    }
    failure
}

/// The store's failure for a [`DownloadError::RateLimited`].
pub(crate) fn rate_limited_failure(error: DownloadError) -> Failure {
    let class = match &error {
        DownloadError::RateLimited {
            status, retry_in, ..
        } => FailureClass::RateLimited {
            status: status.as_u16(),
            delay: *retry_in,
            held_back: false,
        },
        _ => FailureClass::Network,
    };
    Failure::new(class, error.to_string()).with_error(error)
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
                scope: None,
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

    #[test]
    fn scoped_resources_at_the_same_uri_use_distinct_cache_entries() {
        let key = |scope: &str| SegmentKey {
            kind: SegmentKind::Media,
            uri: Arc::from("https://e.com/seg-1.m4s"),
            byte_range: None,
            scope: Some(Arc::from(scope)),
        };

        assert_ne!(
            segment_cache_key(&key("p0/video")),
            segment_cache_key(&key("p1/video"))
        );
    }

    fn context(clients: Arc<ClientPool>) -> Arc<FetchContext> {
        use super::super::config::{GapSkipStrategy, OutputConfig};
        Arc::new(FetchContext {
            clients,
            config: Arc::new(SegmentCoreConfig {
                fetch: super::super::config::FetchConfig {
                    params: Vec::new(),
                    segment_download_timeout: Duration::from_secs(5),
                    max_segment_retries: 3,
                    segment_retry_delay_base: Duration::from_millis(10),
                    progress_emit_min_bytes: 0,
                    progress_emit_min_interval: Duration::ZERO,
                    processed_segment_ttl: Duration::from_secs(1),
                    max_segment_size_bytes: 0,
                },
                output: OutputConfig {
                    gap_evaluation_interval: Duration::from_millis(10),
                    max_pending_init_segments: 0,
                    live_max_overall_stall_duration: None,
                    live_gap_strategy: GapSkipStrategy::default(),
                    vod_gap_strategy: GapSkipStrategy::WaitIndefinitely,
                    vod_segment_timeout: None,
                    buffer_limits: Default::default(),
                },
            }),
            budget: Arc::new(ByteBudget::new(1024 * 1024, 0)),
            decryptor: None,
            cache_manager: None,
            metrics: Some(Arc::new(PerformanceMetrics::new())),
            cancel: CancellationToken::new(),
            events: None,
        })
    }

    fn job(ctx: &FetchContext, uri: &str) -> ReadyJob {
        ReadyJob {
            descriptor: descriptor(uri, SegmentKind::Media, None),
            attempt: 1,
            reservation: ctx.budget.download.try_reserve(1024).expect("budget"),
        }
    }

    fn rate_limited(clients: &ClientPool, url: &str, retry_after: &str) {
        let url = Url::parse(url).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            HeaderValue::from_str(retry_after).unwrap(),
        );
        let admission = clients.rate_limiter().try_admit(&url).unwrap();
        admission.observe(&url, StatusCode::TOO_MANY_REQUESTS, &headers);
    }

    #[tokio::test]
    async fn a_segment_of_a_cooling_origin_goes_back_to_the_store_at_once() {
        let clients = Arc::new(ClientPool::new(&Default::default()).unwrap());
        // Nothing listens on the discard port: a request would fail as a
        // network error rather than as rate limiting.
        let uri = "http://127.0.0.1:9/live/1.ts";
        rate_limited(&clients, uri, "30");
        let ctx = context(Arc::clone(&clients));

        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            fetch_and_process(job(&ctx, uri), Arc::clone(&ctx)),
        )
        .await
        .expect("does not wait for the cool-down");

        let SegmentOutcome::Failed { class, error, .. } = outcome else {
            panic!("{outcome:?}");
        };
        let FailureClass::RateLimited {
            status: 429,
            delay,
            held_back: true,
        } = class
        else {
            panic!("{class:?}");
        };
        assert!(delay > Duration::from_secs(29) && delay <= Duration::from_secs(30));
        assert!(matches!(error, Some(DownloadError::RateLimited { .. })));
        assert_eq!(
            ctx.budget.download.available(),
            ctx.budget.download.capacity(),
            "the byte reservation is given back"
        );
        let metrics = ctx.metrics.as_ref().unwrap();
        assert_eq!(
            metrics
                .download_errors
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "nothing was downloaded, so nothing failed to download"
        );
    }

    /// Serves `answer` at every path and counts the requests.
    async fn serve(
        answer: impl Fn() -> axum::response::Response + Clone + Send + Sync + 'static,
    ) -> (String, Arc<std::sync::atomic::AtomicU32>) {
        let hits = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = Arc::clone(&hits);
        let app = axum::Router::new().fallback(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let response = answer();
            async move { response }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), hits)
    }

    #[tokio::test]
    async fn redirected_refusals_pause_the_entry_point_and_the_target() {
        let (cdn, cdn_hits) = serve(|| {
            axum::response::Response::builder()
                .status(429)
                .header("retry-after", "30")
                .body(axum::body::Body::empty())
                .unwrap()
        })
        .await;
        let target = format!("{cdn}/index.mp4");
        let (origin, origin_hits) = serve(move || {
            axum::response::Response::builder()
                .status(302)
                .header("location", target.as_str())
                .body(axum::body::Body::empty())
                .unwrap()
        })
        .await;
        let clients = ClientPool::new(&Default::default()).unwrap();
        let cancel = CancellationToken::new();
        let origin_url = Url::parse(&format!("{origin}/index.mp4")).unwrap();
        let cdn_url = Url::parse(&format!("{cdn}/index.mp4")).unwrap();
        let fetch = || async {
            fetch_range(
                &clients,
                &origin_url,
                RANGE_8,
                &RangeRequest {
                    params: &[],
                    timeout: Duration::from_secs(5),
                    cancel: &cancel,
                    operation: "test",
                    throttle_deadline: tokio::time::Instant::now(),
                },
            )
            .await
        };

        let refused = tokio::time::timeout(Duration::from_secs(5), fetch())
            .await
            .unwrap();
        assert!(
            matches!(&refused, Err(DownloadError::RateLimited { url, .. }) if url.starts_with(&cdn)),
            "{refused:?}"
        );
        assert!(clients.rate_limiter().try_admit(&cdn_url).is_err());
        assert!(
            clients.rate_limiter().try_admit(&origin_url).is_err(),
            "requests that lead to the refusing server pause too"
        );

        // The next request is held back at the entry point: neither server
        // is asked again during the pause.
        let held = tokio::time::timeout(Duration::from_secs(5), fetch())
            .await
            .unwrap();
        assert!(
            matches!(&held, Err(DownloadError::RateLimited { retry_in, .. })
                if *retry_in > Duration::from_secs(25)),
            "{held:?}"
        );
        assert_eq!(origin_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(cdn_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    const RANGE_8: ByteRangeKey = ByteRangeKey {
        offset: 0,
        length: 8,
    };

    #[tokio::test]
    async fn a_probe_redirected_within_its_origin_succeeds_once_the_server_recovers() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let refusals = Arc::new(AtomicU32::new(1));
        let app = axum::Router::new()
            .route(
                "/a",
                axum::routing::get(|| async {
                    axum::response::Response::builder()
                        .status(302)
                        .header("location", "/b")
                        .body(axum::body::Body::empty())
                        .unwrap()
                }),
            )
            .route(
                "/b",
                axum::routing::get(move || {
                    let refuse = refusals
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                        .is_ok();
                    async move {
                        let builder = axum::response::Response::builder();
                        if refuse {
                            builder.status(429).header("retry-after", "1")
                        } else {
                            builder.status(200)
                        }
                        .body(axum::body::Body::from(vec![7_u8; 16]))
                        .unwrap()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let clients = ClientPool::new(&Default::default()).unwrap();
        let cancel = CancellationToken::new();
        let url = Url::parse(&format!("http://{addr}/a")).unwrap();
        let fetch = |deadline: tokio::time::Instant| {
            let request = RangeRequest {
                params: &[],
                timeout: Duration::from_secs(5),
                cancel: &cancel,
                operation: "test",
                throttle_deadline: deadline,
            };
            let clients = &clients;
            let url = &url;
            async move { fetch_range(clients, url, RANGE_8, &request).await }
        };

        let refused = fetch(tokio::time::Instant::now()).await;
        assert!(
            matches!(refused, Err(DownloadError::RateLimited { .. })),
            "{refused:?}"
        );
        // The next request waits out the pause, goes first as the probe,
        // follows the redirect within the origin and is answered.
        let started = tokio::time::Instant::now();
        let recovered = tokio::time::timeout(
            Duration::from_secs(5),
            fetch(started + Duration::from_secs(5)),
        )
        .await
        .expect("bounded");
        assert_eq!(recovered.unwrap(), Bytes::from(vec![7_u8; 8]));
        assert!(started.elapsed() >= Duration::from_millis(900));
        let limiter = clients.rate_limiter();
        let first = limiter.try_admit(&url);
        assert!(
            first.is_ok() && limiter.try_admit(&url).is_ok(),
            "no cool-down left, so no probe"
        );
    }
}
