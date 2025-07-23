// HLS Segment Processor: Processes raw downloaded segment data.
// It also handles caching of processed segments.

use crate::CacheManager;
use crate::cache::{CacheKey, CacheMetadata, CacheResourceType};
use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use crate::hls::decryption::DecryptionService;
use crate::hls::scheduler::ScheduledSegmentJob;
use crate::hls::segment_utils::create_hls_data;
use async_trait::async_trait;
use bytes::Bytes;
use hls::HlsData;
use std::sync::Arc;
use tracing::error;

#[async_trait]
pub trait SegmentTransformer: Send + Sync {
    async fn process_segment_from_job(
        &self,
        raw_data: Bytes,
        job: &ScheduledSegmentJob,
    ) -> Result<HlsData, HlsDownloaderError>;
}

pub struct SegmentProcessor {
    config: Arc<HlsConfig>,
    decryption_service: Arc<DecryptionService>,
    cache_service: Option<Arc<CacheManager>>,
}

impl SegmentProcessor {
    pub fn new(
        config: Arc<HlsConfig>,
        decryption_service: Arc<DecryptionService>,
        cache_service: Option<Arc<CacheManager>>,
    ) -> Self {
        Self {
            config,
            decryption_service,
            cache_service,
        }
    }

    fn u64_to_iv_bytes(val: u64) -> [u8; 16] {
        let mut iv = [0u8; 16];
        iv[8..].copy_from_slice(&val.to_be_bytes());
        iv
    }
}

#[async_trait]
impl SegmentTransformer for SegmentProcessor {
    async fn process_segment_from_job(
        &self,
        raw_data_input: Bytes,
        job: &ScheduledSegmentJob,
    ) -> Result<HlsData, HlsDownloaderError> {
        let mut current_data = raw_data_input;

        // Decryption (if needed)
        if let Some(key_info) = &job.key {
            if key_info.method == m3u8_rs::KeyMethod::AES128 {
                let iv_override = if key_info.iv.is_none() {
                    Some(Self::u64_to_iv_bytes(job.media_sequence_number))
                } else {
                    None
                };

                current_data = self
                    .decryption_service
                    .decrypt(current_data, key_info, iv_override, &job.base_url)
                    .await?;
            } else if key_info.method != m3u8_rs::KeyMethod::None {
                return Err(HlsDownloaderError::DecryptionError(format!(
                    "Segment processing encountered unsupported encryption method: {:?}",
                    key_info.method
                )));
            }
        }

        // Construct HlsData
        let segment_url = url::Url::parse(&job.segment_uri)
            .map_err(|e| HlsDownloaderError::SegmentProcessError(format!("Invalid URL: {e}")))?;
        let len = current_data.len();
        let current_data_clone = current_data.clone();
        let hls_data = create_hls_data(
            job.media_segment.clone(),
            current_data,
            &segment_url,
            job.is_init_segment,
        );

        if let Some(cache_service) = &self.cache_service {
            // Cache the decrypted raw segment
            let cache_key =
                CacheKey::new(CacheResourceType::Segment, job.segment_uri.clone(), None);
            let metadata = CacheMetadata::new(len as u64)
                .with_expiration(self.config.processor_config.processed_segment_ttl);

            if let Err(e) = cache_service
                .put(cache_key, current_data_clone, metadata)
                .await
            {
                error!(
                    "Warning: Failed to cache decrypted segment {}: {}",
                    job.segment_uri, e
                );
            }
        }

        Ok(hls_data)
    }
}
