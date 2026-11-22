//! CPU-bound crypto execution and decryption-key caching.
//!
//! AES work must never run on async I/O worker threads. `CryptoExecutor`
//! dispatches on a closed backend enum (a trait with `async fn` would not be
//! dyn-compatible, defeating runtime backend selection). `TokioBlocking` is
//! the default; a rayon/dedicated-pool backend stays out until profiling
//! proves the blocking pool is contended.

use std::sync::Arc;
use std::time::Duration;

use aes::Aes128;
use bytes::{Bytes, BytesMut};
use cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};

use crate::DownloadError;
use crate::redact::redact_url_str;

type Aes128CbcDec = cbc::Decryptor<Aes128>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoBackend {
    /// Decrypt inline on the calling task. Only sane for tests or callers that
    /// already run on a blocking-tolerant thread.
    Inline,
    /// `tokio::task::spawn_blocking` — integrates with the existing runtime
    /// and avoids a second CPU pool until profiling justifies one.
    TokioBlocking,
}

#[derive(Debug, Clone)]
pub struct CryptoExecutor {
    backend: CryptoBackend,
}

impl CryptoExecutor {
    pub fn new(backend: CryptoBackend) -> Self {
        Self { backend }
    }

    /// AES-128-CBC with PKCS#7 padding. Reuses the owned `Bytes` buffer in
    /// place when uniquely held (`try_into_mut`), falling back to a single
    /// copy only when another handle shares the allocation. Output is never
    /// larger than the input, which is why the processing budget reserves the
    /// input length as an upper bound before dispatch.
    pub async fn decrypt_aes128_cbc(
        &self,
        data: Bytes,
        key: [u8; 16],
        iv: [u8; 16],
    ) -> Result<Bytes, DownloadError> {
        match self.backend {
            CryptoBackend::Inline => decrypt_aes128_cbc_sync(data, &key, &iv),
            CryptoBackend::TokioBlocking => {
                tokio::task::spawn_blocking(move || decrypt_aes128_cbc_sync(data, &key, &iv))
                    .await
                    .map_err(|e| DownloadError::Decryption {
                        reason: format!("decryption offload task failed: {e}"),
                    })?
            }
        }
    }
}

pub fn decrypt_aes128_cbc_sync(
    data: Bytes,
    key: &[u8; 16],
    iv: &[u8; 16],
) -> Result<Bytes, DownloadError> {
    let mut buffer = data
        .try_into_mut()
        .unwrap_or_else(|shared| BytesMut::from(shared.as_ref()));

    let cipher = Aes128CbcDec::new_from_slices(key, iv).map_err(|e| DownloadError::Decryption {
        reason: format!("failed to initialize AES decryptor: {e}"),
    })?;

    let decrypted_len = cipher
        .decrypt_padded::<Pkcs7>(&mut buffer)
        .map_err(|e| DownloadError::Decryption {
            reason: format!("decryption failed: {e}"),
        })?
        .len();

    buffer.truncate(decrypted_len);
    Ok(buffer.freeze())
}

/// TTL key cache with single-flight loading.
///
/// Keys are cached by `EncryptionDescriptor::key_identity_uri` (normalized,
/// stable across refreshes) — never by the full fetch URL, whose rotating auth
/// params would defeat every hit. `moka`'s `try_get_with` coalesces concurrent
/// loads for the same identity, so a key rotation cannot thundering-herd the
/// key server with `max_concurrency` simultaneous fetches.
#[derive(Debug, Clone)]
pub struct KeyCache {
    cache: moka::future::Cache<Arc<str>, [u8; 16]>,
}

impl KeyCache {
    pub fn new(ttl: Duration, max_entries: u64) -> Self {
        Self {
            cache: moka::future::Cache::builder()
                .max_capacity(max_entries)
                .time_to_live(ttl)
                .build(),
        }
    }

    /// Get the key for `identity`, loading it via `load` on a miss. Concurrent
    /// callers for the same identity share one in-flight load. `load` receives
    /// the *latest* fetch URL from its captor, so a TTL-expired entry re-fetches
    /// with the freshest signed URL, not the one the key was first fetched with.
    ///
    /// The flag is true only for the caller whose `load` ran; cache hits and
    /// callers that waited on another's load get false.
    pub async fn get_with<F>(
        &self,
        identity: Arc<str>,
        load: F,
    ) -> Result<([u8; 16], bool), Arc<DownloadError>>
    where
        F: Future<Output = Result<[u8; 16], DownloadError>>,
    {
        let entry = self.cache.entry(identity).or_try_insert_with(load).await?;
        let loaded = entry.is_fresh();
        Ok((entry.into_value(), loaded))
    }
}

/// Validate fetched key material: AES-128 keys are exactly 16 bytes.
pub fn validate_key_bytes(raw: &[u8], identity: &str) -> Result<[u8; 16], DownloadError> {
    <[u8; 16]>::try_from(raw).map_err(|_| DownloadError::Decryption {
        reason: format!(
            "decryption key for {} has invalid length {} (expected 16)",
            redact_url_str(identity),
            raw.len()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    type Aes128CbcEnc = cbc::Encryptor<Aes128>;

    fn encrypt(plaintext: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Vec<u8> {
        use cipher::BlockModeEncrypt;
        let cipher = Aes128CbcEnc::new_from_slices(key, iv).unwrap();
        let padded_len = ((plaintext.len() / 16) + 1) * 16;
        let mut buffer = vec![0u8; padded_len];
        buffer[..plaintext.len()].copy_from_slice(plaintext);
        cipher
            .encrypt_padded::<Pkcs7>(&mut buffer, plaintext.len())
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn decrypt_roundtrip_on_unique_and_shared_buffers() {
        let key = [0x42u8; 16];
        let iv = [0x24u8; 16];
        let plaintext = b"hello hls engine";
        let shared = Bytes::from(encrypt(plaintext, &key, &iv));

        // Unique buffer exercises in-place `try_into_mut` without copying.
        let unique = BytesMut::from(shared.as_ref()).freeze();
        let unique_ptr = unique.as_ptr();
        let out_unique = decrypt_aes128_cbc_sync(unique, &key, &iv).unwrap();
        assert_eq!(out_unique.as_ref(), plaintext);
        assert_eq!(out_unique.as_ptr(), unique_ptr);

        // Shared buffer falls back cleanly when another handle holds a ref.
        for backend in [CryptoBackend::Inline, CryptoBackend::TokioBlocking] {
            let exec = CryptoExecutor::new(backend);
            let out = exec
                .decrypt_aes128_cbc(shared.clone(), key, iv)
                .await
                .unwrap();
            assert_eq!(out.as_ref(), plaintext);
        }
    }

    #[tokio::test]
    async fn key_cache_coalesces_concurrent_loads() {
        let cache = KeyCache::new(Duration::from_secs(60), 16);
        let loads = Arc::new(AtomicU32::new(0));
        let identity: Arc<str> = Arc::from("https://e.com/key?id=1");

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let cache = cache.clone();
            let identity = Arc::clone(&identity);
            let loads = Arc::clone(&loads);
            tasks.push(tokio::spawn(async move {
                cache
                    .get_with(identity, async move {
                        loads.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        Ok([7u8; 16])
                    })
                    .await
            }));
        }
        let mut loaders = 0;
        for t in tasks {
            let (key, loaded) = t.await.unwrap().unwrap();
            assert_eq!(key, [7u8; 16]);
            loaders += usize::from(loaded);
        }
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "concurrent gets for one identity must share a single load"
        );
        assert_eq!(loaders, 1, "only the caller whose load ran reports it");
    }

    #[tokio::test]
    async fn key_cache_failed_load_is_not_cached() {
        let cache = KeyCache::new(Duration::from_secs(60), 16);
        let identity: Arc<str> = Arc::from("https://e.com/key?id=2");

        let err = cache
            .get_with(Arc::clone(&identity), async {
                Err(DownloadError::Decryption {
                    reason: "boom".into(),
                })
            })
            .await;
        assert!(err.is_err());

        // A later load must run again and can succeed.
        let ok = cache
            .get_with(Arc::clone(&identity), async { Ok([1u8; 16]) })
            .await;
        assert_eq!(ok.unwrap(), ([1u8; 16], true));
        // Served from the cache now.
        let hit = cache.get_with(identity, async { Ok([2u8; 16]) }).await;
        assert_eq!(hit.unwrap(), ([1u8; 16], false));
    }

    #[test]
    fn key_validation_rejects_wrong_length_and_redacts_secrets() {
        let err = validate_key_bytes(
            &[0u8; 15],
            "https://user:secret_pw@e.com/key.bin?token=super_secret#frag",
        )
        .unwrap_err()
        .to_string();
        assert!(!err.contains("secret_pw"), "{err}");
        assert!(!err.contains("super_secret"), "{err}");
        assert!(!err.contains("frag"), "{err}");
        assert!(validate_key_bytes(&[0u8; 17], "k").is_err());
        assert_eq!(validate_key_bytes(&[3u8; 16], "k").unwrap(), [3u8; 16]);
    }
}
