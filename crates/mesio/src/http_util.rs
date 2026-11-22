//! Bounded HTTP body reading and rate-limited control-plane fetches.
//!
//! Playlists, MPDs, timing probes, and decryption keys are buffered in memory
//! before parsing. Every control-plane read enforces an explicit byte ceiling
//! and caps initial `BytesMut` preallocation so a hostile or misconfigured
//! `Content-Length` cannot exhaust memory.

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::DownloadError;
use crate::downloader::ClientPool;

/// Maximum HLS master or media playlist size (16 MiB).
pub(crate) const MAX_HLS_PLAYLIST_BYTES: usize = 16 * 1024 * 1024;

/// Maximum HLS decryption key response size (4 KiB; AES-128 keys are 16 bytes).
pub(crate) const MAX_HLS_KEY_BYTES: usize = 4 * 1024;

/// Upper bound on initial `BytesMut::with_capacity` from `Content-Length` hints.
const MAX_INITIAL_BUFFER_BYTES: usize = 256 * 1024;

/// Outcome when a bounded HTTP read fails.
#[derive(Debug)]
pub(crate) enum CappedReadError {
    TooLarge(usize),
    Network(DownloadError),
    Cancelled,
}

/// How [`fetch_bytes_capped`] acquires permission from the origin's [`crate::rate_limit::RateLimiter`].
pub(crate) enum AdmissionMode<'a> {
    /// Fail immediately with [`DownloadError::RateLimited`] if the origin is cooling down.
    Immediate,
    /// Wait until the cool-down expires, `cancel` fires, or `deadline` passes.
    WaitUntil {
        cancel: &'a CancellationToken,
        deadline: Instant,
    },
}

/// Successful bounded HTTP response body and its post-redirect URL.
pub(crate) struct FetchedBytes {
    pub bytes: Bytes,
    pub final_url: Url,
}

/// Streams a response body up to `limit` bytes, aborting immediately once
/// `Content-Length` or accumulated chunks exceed `limit`.
pub(crate) async fn read_capped(
    response: reqwest::Response,
    limit: usize,
    cancel: Option<&CancellationToken>,
) -> Result<Bytes, CappedReadError> {
    let content_length = response.content_length();
    if let Some(length) = content_length
        && length > limit as u64
    {
        return Err(CappedReadError::TooLarge(
            usize::try_from(length).unwrap_or(usize::MAX),
        ));
    }

    let initial_cap = content_length
        .and_then(|len| usize::try_from(len).ok())
        .map_or(0, |len| len.min(limit).min(MAX_INITIAL_BUFFER_BYTES));
    let mut body = BytesMut::with_capacity(initial_cap);
    let mut stream = response.bytes_stream();

    loop {
        let next = match cancel {
            Some(token) => {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => return Err(CappedReadError::Cancelled),
                    chunk = stream.next() => chunk,
                }
            }
            None => stream.next().await,
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|err| CappedReadError::Network(DownloadError::from(err)))?;
        let size = body.len().saturating_add(chunk.len());
        if size > limit {
            return Err(CappedReadError::TooLarge(size));
        }
        body.extend_from_slice(&chunk);
    }

    Ok(body.freeze())
}

/// Performs a rate-limited, status-checked, size-capped `GET` request,
/// redacting URLs in any surfaced [`DownloadError`].
#[allow(clippy::too_many_arguments)]
pub(crate) async fn fetch_bytes_capped(
    clients: &ClientPool,
    url: &Url,
    params: &[(String, String)],
    timeout: Duration,
    limit: usize,
    operation: &'static str,
    admission_mode: AdmissionMode<'_>,
    cancel: Option<&CancellationToken>,
    oversize_error: impl FnOnce(usize) -> DownloadError,
) -> Result<FetchedBytes, DownloadError> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Err(DownloadError::Cancelled);
    }

    let admission = match admission_mode {
        AdmissionMode::Immediate => clients
            .rate_limiter()
            .try_admit(url)
            .map_err(|throttled| throttled.error(url, operation))?,
        AdmissionMode::WaitUntil { cancel, deadline } => {
            clients
                .rate_limiter()
                .admit(url, cancel, deadline, operation)
                .await?
        }
    };

    let request = clients
        .client_for_url(url)
        .get(url.clone())
        .timeout(timeout)
        .query(params);

    let response = match cancel {
        Some(token) => {
            tokio::select! {
                biased;
                _ = token.cancelled() => return Err(DownloadError::Cancelled),
                res = request.send() => res.map_err(DownloadError::from)?,
            }
        }
        None => request.send().await.map_err(DownloadError::from)?,
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

    let final_url = response.url().clone();
    let bytes = match read_capped(response, limit, cancel).await {
        Ok(bytes) => bytes,
        Err(CappedReadError::TooLarge(actual)) => return Err(oversize_error(actual)),
        Err(CappedReadError::Network(err)) => return Err(err),
        Err(CappedReadError::Cancelled) => return Err(DownloadError::Cancelled),
    };

    Ok(FetchedBytes { bytes, final_url })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn serve_raw(raw_response: Vec<u8>) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let _ = socket.write_all(&raw_response).await;
                let _ = socket.shutdown().await;
            }
        });
        Url::parse(&format!("http://{addr}/resource?token=secret")).unwrap()
    }

    /// The server is local, so proxy environment variables must not apply.
    async fn get(url: Url) -> reqwest::Response {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        client.get(url).send().await.unwrap()
    }

    #[tokio::test]
    async fn read_capped_rejects_oversize_content_length_and_chunked_stream() {
        let header_url = serve_raw(
            b"HTTP/1.1 200 OK\r\nContent-Length: 99999999\r\nConnection: close\r\n\r\nhi".to_vec(),
        )
        .await;
        let resp = get(header_url).await;
        assert!(matches!(
            read_capped(resp, 64, None).await,
            Err(CappedReadError::TooLarge(99_999_999))
        ));

        let body = vec![b'x'; 128];
        let mut raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        raw.extend_from_slice(&body);
        let stream_url = serve_raw(raw).await;
        let resp = get(stream_url).await;
        assert!(matches!(
            read_capped(resp, 64, None).await,
            Err(CappedReadError::TooLarge(size)) if size > 64
        ));
    }
}
