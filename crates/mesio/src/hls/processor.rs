// HLS Segment Processor: Processes raw downloaded segment data.
// It also handles caching of processed segments.

use crate::CacheManager;
use crate::cache::{CacheKey, CacheMetadata, CacheResourceType};
use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use crate::hls::decryption::DecryptionService;
use crate::hls::metrics::PerformanceMetrics;
use crate::hls::scheduler::ScheduledSegmentJob;
use crate::hls::segment_utils::create_hls_data;
use async_trait::async_trait;
use bytes::Bytes;
use hls::HlsData;
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, trace};

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
    metrics: Option<Arc<PerformanceMetrics>>,
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
            metrics: None,
        }
    }

    /// Create a new SegmentProcessor with performance metrics tracking
    pub fn with_metrics(
        config: Arc<HlsConfig>,
        decryption_service: Arc<DecryptionService>,
        cache_service: Option<Arc<CacheManager>>,
        metrics: Arc<PerformanceMetrics>,
    ) -> Self {
        let mut processor = Self::new(config, decryption_service, cache_service);
        processor.metrics = Some(metrics);
        processor
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
        let zero_copy_enabled = self.config.performance_config.zero_copy_enabled;

        // Check if segment requires decryption
        let requires_decryption = job
            .media_segment
            .key
            .as_ref()
            .is_some_and(|key_info| key_info.method == m3u8_rs::KeyMethod::AES128);

        // Process data: either zero-copy forward or decrypt
        let current_data = if requires_decryption {
            // Decryption required - cannot use zero-copy
            let key_info = job.media_segment.key.as_ref().unwrap(); // Safe: we checked

            let iv_override = if key_info.iv.is_none() {
                Some(Self::u64_to_iv_bytes(job.media_sequence_number))
            } else {
                None
            };

            // Record segment size before decryption
            let segment_size = raw_data_input.len() as u64;

            // Measure decryption duration
            let decryption_start = Instant::now();
            let decrypted_data = self
                .decryption_service
                .decrypt(raw_data_input, key_info, iv_override, job.base_url.as_ref())
                .await?;
            let decryption_duration_ms = decryption_start.elapsed().as_millis() as u64;

            // Record decryption metrics
            if let Some(metrics) = &self.metrics {
                metrics.record_decryption(segment_size, decryption_duration_ms);
            }

            decrypted_data
        } else if let Some(key_info) = &job.media_segment.key {
            // Key exists but method is not AES128
            if key_info.method != m3u8_rs::KeyMethod::None {
                return Err(HlsDownloaderError::DecryptionError(format!(
                    "Segment processing encountered unsupported encryption method: {:?}",
                    key_info.method
                )));
            }
            // KeyMethod::None - no decryption needed, use zero-copy if enabled
            if zero_copy_enabled {
                trace!(
                    uri = %job.media_segment.uri,
                    "Zero-copy forwarding: unencrypted segment (KeyMethod::None)"
                );
            }
            raw_data_input
        } else {
            // No key at all - unencrypted segment, use zero-copy if enabled
            if zero_copy_enabled {
                trace!(
                    uri = %job.media_segment.uri,
                    "Zero-copy forwarding: unencrypted segment (no key)"
                );
            }
            raw_data_input
        };

        // Construct HlsData
        let segment_url = url::Url::parse(&job.media_segment.uri)
            .map_err(|e| HlsDownloaderError::SegmentProcessError(format!("Invalid URL: {e}")))?;
        let len = current_data.len();
        let current_data_clone = current_data.clone();
        let hls_data = create_hls_data(
            job.media_segment.as_ref().clone(),
            current_data,
            &segment_url,
            job.is_init_segment,
        );

        if let Some(cache_service) = &self.cache_service {
            // Cache the decrypted raw segment
            let cache_key = CacheKey::new(
                CacheResourceType::Segment,
                job.media_segment.uri.clone(),
                job.media_segment.byte_range.as_ref().map(|range| {
                    let offset = range
                        .offset
                        .map(|o| o.to_string())
                        .unwrap_or_else(|| "none".to_string());
                    format!("br={}@{}", range.length, offset)
                }),
            );
            let metadata = CacheMetadata::new(len as u64)
                .with_expiration(self.config.processor_config.processed_segment_ttl);

            if let Err(e) = cache_service
                .put(cache_key, current_data_clone, metadata)
                .await
            {
                error!(
                    "Warning: Failed to cache decrypted segment {}: {}",
                    job.media_segment.uri, e
                );
            }
        }

        Ok(hls_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hls::config::HlsConfig;
    use crate::hls::decryption::KeyFetcher;
    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use proptest::prelude::*;
    use std::sync::Arc;

    /// Helper to create a minimal DecryptionService for testing
    fn create_test_decryption_service(config: Arc<HlsConfig>) -> Arc<DecryptionService> {
        let http_client = reqwest::Client::new();
        let key_fetcher = Arc::new(KeyFetcher::new(http_client, config.clone()));
        Arc::new(DecryptionService::new(config, key_fetcher, None))
    }

    /// Helper to create a test ScheduledSegmentJob without encryption
    fn create_unencrypted_job(uri: &str, msn: u64) -> ScheduledSegmentJob {
        ScheduledSegmentJob {
            base_url: Arc::<str>::from("https://example.com/"),
            media_sequence_number: msn,
            media_segment: Arc::new(MediaSegment {
                uri: uri.to_string(),
                ..Default::default()
            }),
            is_init_segment: false,
            is_prefetch: false,
        }
    }

    /// Helper to create a test ScheduledSegmentJob with KeyMethod::None
    fn create_job_with_none_key(uri: &str, msn: u64) -> ScheduledSegmentJob {
        ScheduledSegmentJob {
            base_url: Arc::<str>::from("https://example.com/"),
            media_sequence_number: msn,
            media_segment: Arc::new(MediaSegment {
                uri: uri.to_string(),
                key: Some(m3u8_rs::Key {
                    method: m3u8_rs::KeyMethod::None,
                    uri: None,
                    iv: None,
                    keyformat: None,
                    keyformatversions: None,
                }),
                ..Default::default()
            }),
            is_init_segment: false,
            is_prefetch: false,
        }
    }

    /// Helper to extract Bytes from HlsData
    fn extract_bytes_from_hls_data(hls_data: &HlsData) -> Option<&Bytes> {
        hls_data.data()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// **Feature: hls-performance-optimization, Property 11: Zero-copy forwarding for unencrypted segments**
        ///
        ///
        /// *For any* unencrypted segment processed with zero_copy_enabled, the output Bytes
        /// SHALL share the same underlying allocation as the input Bytes (verified by Bytes::ptr_eq).
        #[test]
        fn prop_zero_copy_forwarding_unencrypted(
            // Generate segment data of various sizes (100 to 10000 bytes)
            data_len in 100usize..10000,
            msn in 0u64..1000,
        ) {
            // Create runtime for async tests
            let rt = tokio::runtime::Runtime::new().unwrap();

            // Generate test data
            let test_data: Vec<u8> = (0..data_len).map(|i| (i % 256) as u8).collect();
            let input_bytes = Bytes::from(test_data);

            // Create config with zero_copy_enabled = true
            let mut config = HlsConfig::default();
            config.performance_config.zero_copy_enabled = true;
            let config = Arc::new(config);

            // Create processor
            let decryption_service = create_test_decryption_service(config.clone());
            let processor = SegmentProcessor::new(config, decryption_service, None);

            // Create unencrypted job (no key)
            let job = create_unencrypted_job(
                &format!("https://example.com/segment_{}.ts", msn),
                msn,
            );

            // Process the segment
            let result = rt.block_on(async {
                processor.process_segment_from_job(input_bytes.clone(), &job).await
            });

            // Verify processing succeeded
            prop_assert!(result.is_ok(), "Processing should succeed for unencrypted segment");

            let hls_data = result.unwrap();

            // Extract the output bytes
            let output_bytes = extract_bytes_from_hls_data(&hls_data);
            prop_assert!(output_bytes.is_some(), "HlsData should contain bytes");

            let output_bytes = output_bytes.unwrap();

            // Verify zero-copy: the output should share the same underlying allocation
            // Bytes::ptr_eq checks if two Bytes instances point to the same memory
            // Note: We compare the data pointers since Bytes::clone() creates a new Bytes
            // that shares the same underlying buffer
            prop_assert_eq!(
                input_bytes.as_ptr(),
                output_bytes.as_ptr(),
                "Zero-copy: output Bytes should share the same underlying allocation as input"
            );

            // Also verify the data is identical
            prop_assert_eq!(
                input_bytes.as_ref(),
                output_bytes.as_ref(),
                "Data content should be identical"
            );
        }

        /// **Feature: hls-performance-optimization, Property 11: Zero-copy forwarding for KeyMethod::None**
        ///
        ///
        /// *For any* segment with KeyMethod::None processed with zero_copy_enabled, the output Bytes
        /// SHALL share the same underlying allocation as the input Bytes.
        #[test]
        fn prop_zero_copy_forwarding_key_method_none(
            // Generate segment data of various sizes (100 to 10000 bytes)
            data_len in 100usize..10000,
            msn in 0u64..1000,
        ) {
            // Create runtime for async tests
            let rt = tokio::runtime::Runtime::new().unwrap();

            // Generate test data
            let test_data: Vec<u8> = (0..data_len).map(|i| (i % 256) as u8).collect();
            let input_bytes = Bytes::from(test_data);

            // Create config with zero_copy_enabled = true
            let mut config = HlsConfig::default();
            config.performance_config.zero_copy_enabled = true;
            let config = Arc::new(config);

            // Create processor
            let decryption_service = create_test_decryption_service(config.clone());
            let processor = SegmentProcessor::new(config, decryption_service, None);

            // Create job with KeyMethod::None
            let job = create_job_with_none_key(
                &format!("https://example.com/segment_{}.ts", msn),
                msn,
            );

            // Process the segment
            let result = rt.block_on(async {
                processor.process_segment_from_job(input_bytes.clone(), &job).await
            });

            // Verify processing succeeded
            prop_assert!(result.is_ok(), "Processing should succeed for KeyMethod::None segment");

            let hls_data = result.unwrap();

            // Extract the output bytes
            let output_bytes = extract_bytes_from_hls_data(&hls_data);
            prop_assert!(output_bytes.is_some(), "HlsData should contain bytes");

            let output_bytes = output_bytes.unwrap();

            // Verify zero-copy: the output should share the same underlying allocation
            prop_assert_eq!(
                input_bytes.as_ptr(),
                output_bytes.as_ptr(),
                "Zero-copy: output Bytes should share the same underlying allocation as input"
            );

            // Also verify the data is identical
            prop_assert_eq!(
                input_bytes.as_ref(),
                output_bytes.as_ref(),
                "Data content should be identical"
            );
        }
    }

    #[tokio::test]
    async fn test_zero_copy_disabled_logs_fallback() {
        // This test verifies that when zero_copy_enabled is false,
        // the processor still works correctly (just without zero-copy optimization)

        let test_data = vec![0u8; 1000];
        let input_bytes = Bytes::from(test_data);

        // Create config with zero_copy_enabled = false
        let mut config = HlsConfig::default();
        config.performance_config.zero_copy_enabled = false;
        let config = Arc::new(config);

        // Create processor
        let decryption_service = create_test_decryption_service(config.clone());
        let processor = SegmentProcessor::new(config, decryption_service, None);

        // Create unencrypted job
        let job = create_unencrypted_job("https://example.com/segment_1.ts", 1);

        // Process the segment
        let result = processor
            .process_segment_from_job(input_bytes.clone(), &job)
            .await;

        // Verify processing succeeded
        assert!(result.is_ok(), "Processing should succeed");

        let hls_data = result.unwrap();
        let output_bytes = extract_bytes_from_hls_data(&hls_data).unwrap();

        // Data should still be identical even without zero-copy logging
        assert_eq!(input_bytes.as_ref(), output_bytes.as_ref());
    }
}
