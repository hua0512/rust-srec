// HLS Decryption Service: Manages fetching decryption keys and performing segment decryption.

use crate::CacheManager;
use crate::cache::{CacheKey, CacheMetadata, CacheResourceType};
use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use aes::Aes128;
use bytes::Bytes;
use cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7}; // Pkcs7 for padding
use hex;
use m3u8_rs::Key;
use reqwest::Client;
use std::sync::Arc;

// --- KeyFetcher Struct ---
// Responsible for fetching raw key data from a URI.
pub struct KeyFetcher {
    http_client: Client,
    config: Arc<HlsConfig>,
}

impl KeyFetcher {
    pub fn new(http_client: Client, config: Arc<HlsConfig>) -> Self {
        Self {
            http_client,
            config,
        }
    }

    pub async fn fetch_key(&self, key_uri: &str) -> Result<Bytes, HlsDownloaderError> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            match self
                .http_client
                .get(key_uri)
                .timeout(self.config.fetcher_config.key_download_timeout)
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        return response.bytes().await.map_err(HlsDownloaderError::from);
                    } else if response.status().is_client_error() {
                        return Err(HlsDownloaderError::DecryptionError(format!(
                            "Client error {} fetching key from {}",
                            response.status(),
                            key_uri
                        )));
                    }
                    // Server errors or other retryable issues
                    if attempts > self.config.fetcher_config.max_key_retries {
                        return Err(HlsDownloaderError::DecryptionError(format!(
                            "Max retries ({}) exceeded for key {}. Last status: {}",
                            self.config.fetcher_config.max_key_retries,
                            key_uri,
                            response.status()
                        )));
                    }
                }
                Err(e) => {
                    // Check if error is retryable (connect, timeout, etc.)
                    if !e.is_connect() && !e.is_timeout() && !e.is_request() {
                        return Err(HlsDownloaderError::from(e)); // Non-retryable network error
                    }
                    if attempts > self.config.fetcher_config.max_key_retries {
                        return Err(HlsDownloaderError::DecryptionError(format!(
                            "Max retries ({}) exceeded for key {} due to network error: {}",
                            self.config.fetcher_config.max_key_retries, key_uri, e
                        )));
                    }
                }
            }
            let delay = self.config.fetcher_config.key_retry_delay_base
                * (2_u32.pow(attempts.saturating_sub(1)));
            tokio::time::sleep(delay).await;
        }
    }
}

// --- DecryptionService Struct ---
pub struct DecryptionService {
    config: Arc<HlsConfig>,
    key_fetcher: Arc<KeyFetcher>,
    cache_manager: Option<Arc<CacheManager>>,
}

type Aes128CbcDec = cbc::Decryptor<Aes128>;

impl DecryptionService {
    pub fn new(
        config: Arc<HlsConfig>,
        key_fetcher: Arc<KeyFetcher>,
        cache_manager: Option<Arc<CacheManager>>,
    ) -> Self {
        Self {
            config,
            key_fetcher,
            cache_manager,
        }
    }

    async fn get_key_data(
        &self,
        key_info: &Key,
        base_url: &str,
    ) -> Result<Bytes, HlsDownloaderError> {
        let key_uri_str = match &key_info.uri {
            Some(uri) => {
                if uri.starts_with("http://") || uri.starts_with("https://") {
                    uri.clone()
                } else {
                    let base = url::Url::parse(base_url).map_err(|e| {
                        HlsDownloaderError::PlaylistError(format!(
                            "Invalid base URL {base_url}: {e}"
                        ))
                    })?;
                    base.join(uri)
                        .map_err(|e| {
                            HlsDownloaderError::PlaylistError(format!(
                                "Could not join base URL {base_url} with key URI {uri}: {e}"
                            ))
                        })?
                        .to_string()
                }
            }
            None => {
                return Err(HlsDownloaderError::DecryptionError(
                    "Key URI is missing".to_string(),
                ));
            }
        };

        // Check in-memory cache first

        let key = CacheKey::new(CacheResourceType::Key, key_uri_str.clone(), None);
        if let Some(cache_manager) = &self.cache_manager {
            if let Some(cached_key) = cache_manager
                .get(&key)
                .await
                .map_err(|e| HlsDownloaderError::CacheError(format!("Cache error: {e}")))?
            {
                return Ok(cached_key.0);
            }
        }

        let fetched_key_bytes = self.key_fetcher.fetch_key(&key_uri_str).await?;
        if fetched_key_bytes.len() != 16 {
            // AES-128 keys are 16 bytes
            return Err(HlsDownloaderError::DecryptionError(format!(
                "Fetched decryption key from {key_uri_str} has incorrect length: {} bytes (expected 16)",
                fetched_key_bytes.len()
            )));
        }
        let key_clone = fetched_key_bytes.clone();
        let len = key_clone.len();

        // Store in cache

        if let Some(cache_manager) = &self.cache_manager {
            let metadata = CacheMetadata::new(len as u64)
                .with_expiration(self.config.decryption_config.key_cache_ttl);
            cache_manager
                .put(key.clone(), key_clone, metadata)
                .await
                .map_err(|e| HlsDownloaderError::CacheError(format!("Cache error: {e}")))?;
        }

        Ok(fetched_key_bytes)
    }

    fn parse_iv(iv_hex_str: &str) -> Result<[u8; 16], HlsDownloaderError> {
        let iv_str = iv_hex_str.trim_start_matches("0x");
        let mut iv_bytes = [0u8; 16];
        hex::decode_to_slice(iv_str, &mut iv_bytes).map_err(|e| {
            HlsDownloaderError::DecryptionError(format!(
                "Failed to parse IV '{iv_hex_str}': {e}"
            ))
        })?;
        Ok(iv_bytes)
    }

    pub async fn decrypt(
        &self,
        data: Bytes,
        key_info: &Key,
        // The IV should ideally be derived by the caller (e.g., SegmentProcessor)
        // based on media_sequence number if not present in key_info.
        // For SAMPLE-AES, IV handling is more complex and per-sample.
        iv_override: Option<[u8; 16]>, // e.g. calculated from media sequence for AES-128 CBC
        base_url: &str,
    ) -> Result<Bytes, HlsDownloaderError> {
        if key_info.method != m3u8_rs::KeyMethod::AES128 {
            // Changed to AES128 (all caps)
            // For now, only support AES-128. SAMPLE-AES would need different handling.
            return Err(HlsDownloaderError::DecryptionError(format!(
                "Unsupported decryption method: {key_info:?}"
            )));
        }

        let key_data = self.get_key_data(key_info, base_url).await?;

        let iv_bytes: [u8; 16] = match (iv_override, &key_info.iv) {
            (Some(iv_val), _) => iv_val,
            (None, Some(iv_hex)) => Self::parse_iv(iv_hex)?,
            (None, None) => {
                // This case should ideally be handled by the caller by providing iv_override
                // based on media_sequence for AES-128 CBC if IV is not in playlist.
                return Err(HlsDownloaderError::DecryptionError(
                    "IV is missing and not overridden for AES-128 decryption".to_string(),
                ));
            }
        };

        // Decrypt
        // Note: For CPU-bound tasks like this, consider `tokio::task::spawn_blocking`
        // TODO: or a dedicated thread pool if `offload_decryption_to_cpu_pool` is true.
        let mut buffer = data.to_vec(); // Clone data for mutable operations
        let cipher = Aes128CbcDec::new_from_slices(&key_data, &iv_bytes).map_err(|e| {
            HlsDownloaderError::DecryptionError(format!(
                "Failed to initialize AES decryptor: {e}"
            ))
        })?;

        let decrypted_len = cipher
            .decrypt_padded_mut::<Pkcs7>(&mut buffer)
            .map_err(|e| HlsDownloaderError::DecryptionError(format!("Decryption failed: {e}")))?
            .len();

        Ok(Bytes::copy_from_slice(&buffer[..decrypted_len]))
    }
}
