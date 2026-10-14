//! HLS segment decryption: AES-128 keys fetched from the playlist's key URI
//! (single-flight per key identity, cached with a TTL) and IVs that default
//! to the segment's media sequence number.

use std::sync::Arc;

use bytes::Bytes;
use futures::future::BoxFuture;
use tracing::trace;

use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsFetcherConfig;
use crate::redact::Redacted;
use crate::segment_engine::budget::ByteReservation;
use crate::segment_engine::crypto::{CryptoExecutor, KeyCache, validate_key_bytes};
use crate::segment_engine::descriptor::{EffectiveIv, EncryptionDescriptor};
use crate::segment_engine::fetch::{Failure, FetchContext, SegmentDecryptor, rate_limited_failure};
use crate::segment_engine::store::FailureClass;
use crate::session::{DownloadEvent, ResourceId};

pub struct HlsDecryptor {
    crypto: CryptoExecutor,
    key_cache: KeyCache,
    /// Key download timeout and retry policy.
    fetcher: HlsFetcherConfig,
}

impl HlsDecryptor {
    pub fn new(crypto: CryptoExecutor, key_cache: KeyCache, fetcher: HlsFetcherConfig) -> Self {
        Self {
            crypto,
            key_cache,
            fetcher,
        }
    }
}

impl SegmentDecryptor for HlsDecryptor {
    fn decrypt<'a>(
        &'a self,
        ctx: &'a FetchContext,
        encryption: &'a EncryptionDescriptor,
        msn: u64,
        encrypted: Bytes,
        download_reservation: &'a mut ByteReservation,
    ) -> BoxFuture<'a, Result<(Bytes, ByteReservation), Failure>> {
        Box::pin(decrypt_segment(
            self,
            ctx,
            encryption,
            msn,
            encrypted,
            download_reservation,
        ))
    }
}

/// Decrypt the encrypted body, returning the plaintext together with the
/// processing reservation that keeps it charged. The caller drops the
/// download reservation once this returns (the input has been consumed) and
/// holds the returned processing reservation until the payload is wrapped.
async fn decrypt_segment(
    decryptor: &HlsDecryptor,
    ctx: &FetchContext,
    enc: &EncryptionDescriptor,
    msn: u64,
    encrypted: Bytes,
    download_reservation: &mut ByteReservation,
) -> Result<(Bytes, ByteReservation), Failure> {
    let key_bytes = fetch_key(decryptor, ctx, enc).await?;

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
    let decrypted = decryptor
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
async fn fetch_key(
    decryptor: &HlsDecryptor,
    ctx: &FetchContext,
    enc: &EncryptionDescriptor,
) -> Result<[u8; 16], Failure> {
    let identity = Arc::clone(&enc.key_identity_uri);
    let fetch_url = Arc::clone(&enc.key_fetch_url);
    // Key resource events describe the one real fetch, not every segment
    // that reads the cached key.
    let result = decryptor
        .key_cache
        .get_with(Arc::clone(&identity), {
            let events = ctx.events.clone();
            let ctx_clients = Arc::clone(&ctx.clients);
            let fetcher = decryptor.fetcher.clone();
            let params = ctx.config.fetch.params.clone();
            let cancel = ctx.cancel.clone();
            let identity = Arc::clone(&identity);
            async move {
                if let Some(events) = &events {
                    events.emit(DownloadEvent::ResourceStarted {
                        resource: ResourceId::HlsKey {
                            uri: Arc::clone(&identity),
                        },
                        display_url: Arc::from(Redacted(&fetch_url).to_string()),
                        content_length: None,
                    });
                }
                let client = ctx_clients.client_for_url(&fetch_url);
                let mut retries = 0;
                loop {
                    let attempt = async {
                        // Like a segment, the key does not wait for a
                        // cooling server here: the segment goes back to the
                        // store, which retries it when the cool-down ends.
                        let admission = ctx_clients
                            .rate_limiter()
                            .try_admit(&fetch_url)
                            .map_err(|throttled| throttled.error(&fetch_url, KEY_FETCH))?;
                        let response = client
                            .get(fetch_url.as_ref().clone())
                            .query(&params)
                            .timeout(fetcher.key_download_timeout)
                            .send()
                            .await
                            .map_err(|error| crate::rate_limit::request_error(error, KEY_FETCH))?;
                        let status = response.status();
                        if let Some(retry_in) =
                            admission.observe(response.url(), status, response.headers())
                        {
                            return Err(HlsDownloaderError::rate_limited(
                                status,
                                response.url().as_str(),
                                KEY_FETCH,
                                retry_in,
                            ));
                        }
                        drop(admission);
                        if !status.is_success() {
                            return Err(HlsDownloaderError::http_status(
                                status,
                                fetch_url.as_str(),
                                KEY_FETCH,
                            ));
                        }
                        let bytes = response.bytes().await.map_err(HlsDownloaderError::from)?;
                        validate_key_bytes(&bytes, &identity)
                    };
                    let error = match attempt.await {
                        Ok(key) => return Ok(key),
                        Err(error) => error,
                    };
                    if retries >= fetcher.max_key_retries || !is_retryable_key_error(&error) {
                        return Err(error);
                    }
                    let delay = fetcher
                        .key_retry_delay_base
                        .checked_mul(1u32 << retries.min(16))
                        .unwrap_or(fetcher.max_key_retry_delay)
                        .min(fetcher.max_key_retry_delay);
                    retries += 1;
                    trace!(retries, ?delay, "key fetch failed; retrying");
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(HlsDownloaderError::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
            }
        })
        .await;

    match result {
        Ok((key, fetched)) => {
            if fetched && let Some(events) = &ctx.events {
                events.emit(DownloadEvent::ResourceFinished {
                    resource: ResourceId::HlsKey { uri: identity },
                    bytes: key.len() as u64,
                    from_cache: false,
                });
            }
            Ok(key)
        }
        Err(e) => Err(match e.as_ref() {
            HlsDownloaderError::HttpStatus {
                status,
                url,
                operation,
            } => Failure::new(
                FailureClass::Http(status.as_u16()),
                format!("key fetch failed: {e}"),
            )
            .with_error(HlsDownloaderError::HttpStatus {
                status: *status,
                url: url.clone(),
                operation,
            }),
            HlsDownloaderError::RateLimited {
                status,
                url,
                operation,
                retry_in,
            } => rate_limited_failure(HlsDownloaderError::RateLimited {
                status: *status,
                url: url.clone(),
                operation,
                retry_in: *retry_in,
            }),
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

const KEY_FETCH: &str = "hls key fetch";

/// Key fetch failures worth retrying within one fetch: transport errors,
/// timeouts, and server-side statuses. A 4xx or an invalid key would fail
/// the same way again; rate limiting is left to the segment's lifecycle
/// retry, which waits for the cool-down without holding a download slot.
fn is_retryable_key_error(error: &HlsDownloaderError) -> bool {
    match error {
        HlsDownloaderError::Network { .. } => true,
        HlsDownloaderError::HttpStatus { status, .. } => {
            status.is_server_error() || *status == reqwest::StatusCode::REQUEST_TIMEOUT
        }
        _ => false,
    }
}
