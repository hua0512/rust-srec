// HLS Segment Scheduler: Manages the pipeline of segments to be downloaded and processed.

use crate::hls::HlsDownloaderError;
use crate::hls::config::{BatchSchedulerConfig, HlsConfig};
use crate::hls::fetcher::SegmentDownloader;
use crate::hls::metrics::PerformanceMetrics;
use crate::hls::prefetch::PrefetchManager;
use crate::hls::processor::SegmentTransformer;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use hls::HlsData;
use m3u8_rs::MediaSegment;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

#[derive(Debug, Clone)]
pub struct ScheduledSegmentJob {
    pub base_url: Arc<str>,
    pub media_sequence_number: u64,
    pub media_segment: Arc<MediaSegment>,
    pub is_init_segment: bool,
    /// Whether this job is a prefetch request (lower priority)
    pub is_prefetch: bool,
}

/// Batches segment jobs for efficient dispatch.
///
/// The BatchScheduler collects incoming segment jobs within a configurable time window
/// and dispatches them as a batch, sorted by media sequence number for optimal ordering.
pub struct BatchScheduler {
    config: BatchSchedulerConfig,
    pending: Vec<ScheduledSegmentJob>,
    batch_start: Option<Instant>,
}

impl BatchScheduler {
    /// Create a new BatchScheduler with the given configuration
    pub fn new(config: BatchSchedulerConfig) -> Self {
        let capacity = config.max_batch_size;
        Self {
            config,
            pending: Vec::with_capacity(capacity),
            batch_start: None,
        }
    }

    /// Add a job to the current batch
    pub fn add_job(&mut self, job: ScheduledSegmentJob) {
        if self.pending.is_empty() {
            // Start the batch window timer when first job arrives
            self.batch_start = Some(Instant::now());
        }
        self.pending.push(job);
    }

    /// Check if batch is ready for dispatch
    ///
    /// Returns true if:
    /// - The batch has reached max_batch_size, OR
    /// - The batch window has expired (batch_window_ms elapsed since first job)
    pub fn is_ready(&self) -> bool {
        if self.pending.is_empty() {
            return false;
        }

        // Check if max batch size reached
        if self.pending.len() >= self.config.max_batch_size {
            return true;
        }

        // Check if batch window has expired
        if let Some(start) = self.batch_start {
            let window = Duration::from_millis(self.config.batch_window_ms);
            if start.elapsed() >= window {
                return true;
            }
        }

        false
    }

    /// Take the current batch, sorted by media sequence number (ascending)
    ///
    /// Returns the pending jobs sorted by MSN and resets the batch state.
    pub fn take_batch(&mut self) -> Vec<ScheduledSegmentJob> {
        let mut batch = std::mem::take(&mut self.pending);
        self.batch_start = None;

        // Sort by MSN (ascending). For identical MSNs, ensure init segments are dispatched before
        // media segments, and non-prefetch before prefetch.
        batch.sort_by_key(|job| {
            (
                job.media_sequence_number,
                !job.is_init_segment,
                job.is_prefetch,
            )
        });

        batch
    }

    /// Re-queue jobs such that they are immediately dispatchable.
    ///
    /// This is used when a batch is ready but scheduler concurrency is currently saturated.
    /// We keep the remainder in the batch scheduler without restarting the batch window.
    pub fn requeue_ready_jobs(&mut self, mut jobs: Vec<ScheduledSegmentJob>) {
        if jobs.is_empty() {
            return;
        }

        if self.pending.is_empty() {
            // Mark the window as already elapsed so `time_until_ready()` returns zero.
            let window = Duration::from_millis(self.config.batch_window_ms);
            self.batch_start = Some(
                Instant::now()
                    .checked_sub(window)
                    .unwrap_or_else(Instant::now),
            );
        }

        self.pending.append(&mut jobs);
    }

    /// Get the number of pending jobs in the current batch
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Check if batch scheduling is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the time remaining until batch window expires
    /// Returns None if no batch is in progress
    pub fn time_until_ready(&self) -> Option<Duration> {
        if self.pending.is_empty() {
            return None;
        }

        if let Some(start) = self.batch_start {
            let window = Duration::from_millis(self.config.batch_window_ms);
            let elapsed = start.elapsed();
            if elapsed >= window {
                Some(Duration::ZERO)
            } else {
                Some(window - elapsed)
            }
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct ProcessedSegmentOutput {
    #[allow(dead_code)]
    pub original_segment_uri: String,
    pub data: HlsData,
    pub media_sequence_number: u64,
    pub discontinuity: bool,
}

pub struct SegmentScheduler {
    config: Arc<HlsConfig>,
    segment_fetcher: Arc<dyn SegmentDownloader>,
    segment_processor: Arc<dyn SegmentTransformer>,
    segment_request_rx: mpsc::Receiver<ScheduledSegmentJob>,
    output_tx: mpsc::Sender<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
    token: CancellationToken,
    batch_scheduler: BatchScheduler,
    /// Prefetch manager for predictive segment downloading
    prefetch_manager: PrefetchManager,
    /// Known segments from playlist (MSN -> job template for prefetch)
    known_segments: BTreeMap<u64, ScheduledSegmentJob>,
    /// Current buffer size estimate (number of segments in flight + pending)
    buffer_size: usize,
    /// Segments currently in-flight (being downloaded), used to prevent duplicate prefetch
    in_flight_segments: HashSet<u64>,
    /// Performance metrics for tracking prefetch operations
    metrics: Option<Arc<PerformanceMetrics>>,

    /// Whether any init segment jobs have been observed for this stream.
    /// Used to gate prefetching on fMP4 streams until an init segment is seen.
    init_required: bool,

    /// Whether we've successfully processed at least one init segment job.
    init_seen: bool,
}

impl SegmentScheduler {
    pub fn new(
        config: Arc<HlsConfig>,
        segment_fetcher: Arc<dyn SegmentDownloader>,
        segment_processor: Arc<dyn SegmentTransformer>,
        segment_request_rx: mpsc::Receiver<ScheduledSegmentJob>,
        output_tx: mpsc::Sender<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
        token: CancellationToken,
    ) -> Self {
        let batch_scheduler =
            BatchScheduler::new(config.performance_config.batch_scheduler.clone());
        let prefetch_manager = PrefetchManager::new(config.performance_config.prefetch.clone());
        Self {
            config,
            segment_fetcher,
            segment_processor,
            segment_request_rx,
            output_tx,
            token,
            batch_scheduler,
            prefetch_manager,
            known_segments: BTreeMap::new(),
            buffer_size: 0,
            in_flight_segments: HashSet::new(),
            metrics: None,
            init_required: false,
            init_seen: false,
        }
    }

    /// Create a new SegmentScheduler with performance metrics
    pub fn with_metrics(
        config: Arc<HlsConfig>,
        segment_fetcher: Arc<dyn SegmentDownloader>,
        segment_processor: Arc<dyn SegmentTransformer>,
        segment_request_rx: mpsc::Receiver<ScheduledSegmentJob>,
        output_tx: mpsc::Sender<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
        token: CancellationToken,
        metrics: Arc<PerformanceMetrics>,
    ) -> Self {
        let mut scheduler = Self::new(
            config,
            segment_fetcher,
            segment_processor,
            segment_request_rx,
            output_tx,
            token,
        );
        scheduler.metrics = Some(metrics);
        scheduler
    }

    /// Result of segment processing, including metadata for prefetch tracking
    async fn perform_segment_processing(
        segment_fetcher: Arc<dyn SegmentDownloader>,
        segment_processor: Arc<dyn SegmentTransformer>,
        job: ScheduledSegmentJob,
    ) -> (
        u64,
        bool,
        bool,
        Result<ProcessedSegmentOutput, HlsDownloaderError>,
    ) {
        let msn = job.media_sequence_number;
        let is_prefetch = job.is_prefetch;
        let is_init_segment = job.is_init_segment;

        let raw_data_result = segment_fetcher.download_segment_from_job(&job).await;

        let raw_data = match raw_data_result {
            Ok(data) => data,
            Err(e) => {
                error!(uri = %job.media_segment.uri, error = %e, "Segment download failed");
                return (msn, is_prefetch, is_init_segment, Err(e));
            }
        };

        let processed_result = segment_processor
            .process_segment_from_job(raw_data, &job)
            .await;

        let result = match processed_result {
            Ok(hls_data) => {
                let output = ProcessedSegmentOutput {
                    original_segment_uri: job.media_segment.uri.clone(),
                    data: hls_data,
                    media_sequence_number: job.media_sequence_number,
                    discontinuity: job.media_segment.discontinuity,
                };
                trace!(uri = %job.media_segment.uri, msn = %job.media_sequence_number, is_prefetch = is_prefetch, "Segment processing successful");
                Ok(output)
            }
            Err(e) => {
                warn!(uri = %job.media_segment.uri, error = %e, "Segment transformation failed");
                Err(e)
            }
        };

        (msn, is_prefetch, is_init_segment, result)
    }

    /// Store a job template for potential prefetching
    fn track_segment_job(&mut self, job: &ScheduledSegmentJob) {
        // Don't track init segments or prefetch jobs
        if job.is_init_segment || job.is_prefetch {
            return;
        }

        self.known_segments
            .insert(job.media_sequence_number, job.clone());

        // Cleanup old entries to prevent unbounded growth
        // Keep only segments within a reasonable window
        const MAX_TRACKED_SEGMENTS: usize = 100;
        while self.known_segments.len() > MAX_TRACKED_SEGMENTS {
            if let Some((&oldest_msn, _)) = self.known_segments.first_key_value() {
                self.known_segments.remove(&oldest_msn);
                self.prefetch_manager.cleanup_before(oldest_msn + 1);
            }
        }
    }

    /// Get prefetch jobs to dispatch after a segment completes
    fn get_prefetch_jobs(&mut self, completed_msn: u64) -> Vec<ScheduledSegmentJob> {
        if !self.prefetch_manager.is_enabled() {
            return Vec::new();
        }

        // Get known segment MSNs
        let known_msns: Vec<u64> = self.known_segments.keys().copied().collect();

        // Get prefetch targets, excluding segments already in-flight
        let targets = self.prefetch_manager.get_prefetch_targets(
            completed_msn,
            self.buffer_size,
            &known_msns,
            &self.in_flight_segments,
        );

        if targets.is_empty() {
            return Vec::new();
        }

        debug!(
            completed_msn = completed_msn,
            targets = ?targets,
            buffer_size = self.buffer_size,
            "Initiating prefetch for segments"
        );

        // Create prefetch jobs for each target
        let mut prefetch_jobs = Vec::new();
        for msn in targets {
            if let Some(template_job) = self.known_segments.get(&msn) {
                let mut prefetch_job = template_job.clone();
                prefetch_job.is_prefetch = true;
                prefetch_jobs.push(prefetch_job);

                // Record prefetch initiation in metrics (Requirement 7.4)
                if let Some(metrics) = &self.metrics {
                    metrics.record_prefetch_initiated();
                }
            }
        }

        prefetch_jobs
    }

    pub async fn run(&mut self) {
        info!("SegmentScheduler started.");
        let mut futures = FuturesUnordered::new();
        let mut draining = false;
        let batch_enabled = self.batch_scheduler.is_enabled();
        let prefetch_enabled = self.prefetch_manager.is_enabled();

        loop {
            let in_progress_count = futures.len();
            let can_accept_more =
                in_progress_count < self.config.scheduler_config.download_concurrency;

            // Calculate batch timeout for select
            let batch_timeout = if batch_enabled {
                self.batch_scheduler.time_until_ready()
            } else {
                None
            };

            tokio::select! {
                biased;

                // 1. Cancellation Token
                _ = self.token.cancelled(), if !draining => {
                    info!("Cancellation token received. SegmentScheduler entering draining state.");
                    draining = true;
                    // Close the segment request channel to prevent new jobs from being added
                    // while we drain the existing ones.
                    self.segment_request_rx.close();

                    // Dispatch any remaining batched jobs before draining
                    if batch_enabled && self.batch_scheduler.pending_count() > 0 {
                        let batch = self.batch_scheduler.take_batch();
                        let max_concurrency = self.config.scheduler_config.download_concurrency;
                        let mut leftovers = Vec::new();
                        for job in batch {
                            if futures.len() >= max_concurrency {
                                leftovers.push(job);
                                continue;
                            }
                            self.buffer_size += 1;
                            self.in_flight_segments.insert(job.media_sequence_number);
                            let fetcher_clone = Arc::clone(&self.segment_fetcher);
                            let processor_clone = Arc::clone(&self.segment_processor);
                            futures.push(Self::perform_segment_processing(
                                fetcher_clone,
                                processor_clone,
                                job,
                            ));
                        }
                        self.batch_scheduler.requeue_ready_jobs(leftovers);
                    }
                }

                // 2. Batch window timeout - dispatch partial batch when window expires
                _ = tokio::time::sleep(batch_timeout.unwrap_or(Duration::MAX)), if batch_enabled && batch_timeout.is_some() && can_accept_more => {
                    if self.batch_scheduler.is_ready() {
                        let batch = self.batch_scheduler.take_batch();
                        trace!(batch_size = batch.len(), "Batch window expired, dispatching partial batch");
                        let max_concurrency = self.config.scheduler_config.download_concurrency;
                        let mut leftovers = Vec::new();
                        for job in batch {
                            if futures.len() >= max_concurrency {
                                leftovers.push(job);
                                continue;
                            }
                            self.buffer_size += 1;
                            self.in_flight_segments.insert(job.media_sequence_number);
                            let fetcher_clone = Arc::clone(&self.segment_fetcher);
                            let processor_clone = Arc::clone(&self.segment_processor);
                            futures.push(Self::perform_segment_processing(
                                fetcher_clone,
                                processor_clone,
                                job,
                            ));
                        }
                        self.batch_scheduler.requeue_ready_jobs(leftovers);
                    }
                }

                // 3. Receive new segment jobs
                // This branch is disabled when `draining` is true.
                maybe_job_request = self.segment_request_rx.recv(), if !draining && can_accept_more => {
                    if let Some(job_request) = maybe_job_request {
                        trace!(uri = %job_request.media_segment.uri, msn = %job_request.media_sequence_number, "Received new segment job.");

                        if job_request.is_init_segment {
                            self.init_required = true;
                        }

                        // Track segment for potential prefetching
                        if prefetch_enabled {
                            self.track_segment_job(&job_request);
                        }

                        if batch_enabled {
                            // Add to batch scheduler
                            self.batch_scheduler.add_job(job_request);

                            // Check if batch is ready (max size reached)
                            if self.batch_scheduler.is_ready() {
                                let batch = self.batch_scheduler.take_batch();
                                trace!(batch_size = batch.len(), "Batch ready (max size), dispatching");
                                let max_concurrency = self.config.scheduler_config.download_concurrency;
                                let mut leftovers = Vec::new();
                                for job in batch {
                                    if futures.len() >= max_concurrency {
                                        leftovers.push(job);
                                        continue;
                                    }
                                    self.buffer_size += 1;
                                    self.in_flight_segments.insert(job.media_sequence_number);
                                    let fetcher_clone = Arc::clone(&self.segment_fetcher);
                                    let processor_clone = Arc::clone(&self.segment_processor);
                                    futures.push(Self::perform_segment_processing(
                                        fetcher_clone,
                                        processor_clone,
                                        job,
                                    ));
                                }
                                self.batch_scheduler.requeue_ready_jobs(leftovers);
                            }
                        } else {
                            // Direct dispatch without batching
                            self.buffer_size += 1;
                            self.in_flight_segments.insert(job_request.media_sequence_number);
                            let fetcher_clone = Arc::clone(&self.segment_fetcher);
                            let processor_clone = Arc::clone(&self.segment_processor);
                            futures.push(Self::perform_segment_processing(
                                fetcher_clone,
                                processor_clone,
                                job_request,
                            ));
                        }
                    } else {
                        // The input channel was closed by the PlaylistEngine.
                        // This is a natural end, so we start draining.
                        info!("Segment request channel closed. No new jobs will be accepted. Draining in-progress tasks.");
                        draining = true;

                        // Dispatch any remaining batched jobs
                        if batch_enabled && self.batch_scheduler.pending_count() > 0 {
                            let batch = self.batch_scheduler.take_batch();
                            debug!(batch_size = batch.len(), "Dispatching remaining batch on channel close");
                            let max_concurrency = self.config.scheduler_config.download_concurrency;
                            let mut leftovers = Vec::new();
                            for job in batch {
                                if futures.len() >= max_concurrency {
                                    leftovers.push(job);
                                    continue;
                                }
                                self.buffer_size += 1;
                                self.in_flight_segments.insert(job.media_sequence_number);
                                let fetcher_clone = Arc::clone(&self.segment_fetcher);
                                let processor_clone = Arc::clone(&self.segment_processor);
                                futures.push(Self::perform_segment_processing(
                                    fetcher_clone,
                                    processor_clone,
                                    job,
                                ));
                            }
                            self.batch_scheduler.requeue_ready_jobs(leftovers);
                        }
                    }
                }

                // 4. Handle completed futures
                // This branch remains active during draining to finish in-progress work.
                Some((completed_msn, is_prefetch, is_init_segment, processed_result)) = futures.next() => {
                    // Update buffer size
                    if self.buffer_size > 0 {
                        self.buffer_size -= 1;
                    }

                    // Remove segment from in-flight tracking
                    self.in_flight_segments.remove(&completed_msn);

                    // Mark segment as completed in prefetch manager
                    if prefetch_enabled {
                        self.prefetch_manager.mark_completed(completed_msn);
                    }

                    match processed_result {
                        Ok(processed_output) => {
                            if is_init_segment {
                                self.init_seen = true;
                            }

                            // Record prefetch usage metric when a prefetched segment completes successfully
                            if is_prefetch
                                && let Some(metrics) = &self.metrics
                            {
                                metrics.record_prefetch_used();
                            }

                            // Initiate prefetch for next segments after successful download
                            // Only prefetch after non-prefetch jobs to avoid cascading prefetches
                            let should_prefetch = prefetch_enabled
                                && !is_prefetch
                                && !draining
                                && !is_init_segment
                                && (!self.init_required || self.init_seen);

                            if should_prefetch {
                                let prefetch_jobs = self.get_prefetch_jobs(completed_msn);
                                for prefetch_job in prefetch_jobs {
                                    // Only dispatch if we have capacity
                                    if futures.len() < self.config.scheduler_config.download_concurrency {
                                        self.buffer_size += 1;
                                        self.in_flight_segments.insert(prefetch_job.media_sequence_number);
                                        let fetcher_clone = Arc::clone(&self.segment_fetcher);
                                        let processor_clone = Arc::clone(&self.segment_processor);
                                        futures.push(Self::perform_segment_processing(
                                            fetcher_clone,
                                            processor_clone,
                                            prefetch_job,
                                        ));
                                    }
                                }
                            }

                            if self.output_tx.send(Ok(processed_output)).await.is_err() {
                                error!("Output channel closed while trying to send processed segment. Shutting down scheduler.");
                                break;
                            }
                        }
                        Err(e) => {
                            // Check if the error is a SegmentFetchError (e.g. 404)
                            // We should not abort the stream for a single missing segment
                            let should_ignore = matches!(e, HlsDownloaderError::SegmentFetchError(_));

                            warn!(
                                error = %e,
                                msn = completed_msn,
                                is_prefetch = is_prefetch,
                                ignored = should_ignore,
                                "Segment processing task failed."
                            );

                            // Don't propagate prefetch errors - they're opportunistic.
                            // Also don't propagate SegmentFetchErrors - treat them as gaps.
                            if !is_prefetch && !should_ignore
                                && self.output_tx.send(Err(e)).await.is_err() {
                                    error!("Output channel closed while trying to send segment processing error. Shutting down scheduler.");
                                    break;
                                }
                        }
                    }
                }

                // 5. Shutdown condition
                // This `else` branch is taken when all other branches are disabled.
                // This happens when:
                //  - `draining` is true, so `recv()` is disabled.
                //  - `futures` is empty, so `futures.next()` returns `Poll::Pending` and the branch is not taken.
                // This is our signal to exit the loop.
                else => {
                    if draining && futures.is_empty() {
                        info!("Draining complete. SegmentScheduler shutting down.");
                        break;
                    }
                    if !draining && self.segment_request_rx.is_closed() && futures.is_empty() {
                        info!("All pending segments processed and input is closed. SegmentScheduler shutting down.");
                        break;
                    }
                    // If we get here, it means we are waiting for new jobs or for futures to complete.
                    // The select will keep polling.
                }
            }
        }
        info!("SegmentScheduler finished.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Helper function to create a test ScheduledSegmentJob with a given MSN
    fn create_test_job(msn: u64) -> ScheduledSegmentJob {
        create_test_job_with_flags(msn, false, false)
    }

    fn create_test_job_with_flags(
        msn: u64,
        is_init_segment: bool,
        is_prefetch: bool,
    ) -> ScheduledSegmentJob {
        ScheduledSegmentJob {
            base_url: Arc::<str>::from("https://example.com/"),
            media_sequence_number: msn,
            media_segment: Arc::new(MediaSegment {
                uri: format!("segment_{}.ts", msn),
                ..Default::default()
            }),
            is_init_segment,
            is_prefetch,
        }
    }

    // --- Unit Tests ---

    #[test]
    fn test_batch_scheduler_new() {
        let config = BatchSchedulerConfig {
            enabled: true,
            batch_window_ms: 50,
            max_batch_size: 5,
        };
        let scheduler = BatchScheduler::new(config);
        assert_eq!(scheduler.pending_count(), 0);
        assert!(scheduler.is_enabled());
        assert!(!scheduler.is_ready());
    }

    #[test]
    fn test_batch_scheduler_add_job() {
        let config = BatchSchedulerConfig::default();
        let mut scheduler = BatchScheduler::new(config);

        scheduler.add_job(create_test_job(1));
        assert_eq!(scheduler.pending_count(), 1);

        scheduler.add_job(create_test_job(2));
        assert_eq!(scheduler.pending_count(), 2);
    }

    #[test]
    fn test_batch_scheduler_is_ready_max_size() {
        let config = BatchSchedulerConfig {
            enabled: true,
            batch_window_ms: 1000, // Long window
            max_batch_size: 3,
        };
        let mut scheduler = BatchScheduler::new(config);

        scheduler.add_job(create_test_job(1));
        assert!(!scheduler.is_ready());

        scheduler.add_job(create_test_job(2));
        assert!(!scheduler.is_ready());

        scheduler.add_job(create_test_job(3));
        assert!(scheduler.is_ready()); // Max size reached
    }

    #[test]
    fn test_batch_scheduler_take_batch_sorts_by_msn() {
        let config = BatchSchedulerConfig {
            enabled: true,
            batch_window_ms: 1000,
            max_batch_size: 10,
        };
        let mut scheduler = BatchScheduler::new(config);

        // Add jobs in random order
        scheduler.add_job(create_test_job(5));
        scheduler.add_job(create_test_job(2));
        scheduler.add_job(create_test_job(8));
        scheduler.add_job(create_test_job(1));
        scheduler.add_job(create_test_job(3));

        let batch = scheduler.take_batch();

        // Verify sorted by MSN ascending
        assert_eq!(batch.len(), 5);
        assert_eq!(batch[0].media_sequence_number, 1);
        assert_eq!(batch[1].media_sequence_number, 2);
        assert_eq!(batch[2].media_sequence_number, 3);
        assert_eq!(batch[3].media_sequence_number, 5);
        assert_eq!(batch[4].media_sequence_number, 8);
    }

    #[test]
    fn test_batch_scheduler_take_batch_orders_init_before_media_for_same_msn() {
        let config = BatchSchedulerConfig {
            enabled: true,
            batch_window_ms: 1000,
            max_batch_size: 10,
        };
        let mut scheduler = BatchScheduler::new(config);

        scheduler.add_job(create_test_job_with_flags(1, false, false));
        scheduler.add_job(create_test_job_with_flags(1, true, false));
        scheduler.add_job(create_test_job_with_flags(1, false, true));

        let batch = scheduler.take_batch();
        assert_eq!(batch.len(), 3);
        assert_eq!(batch[0].media_sequence_number, 1);
        assert!(batch[0].is_init_segment);
        assert!(!batch[0].is_prefetch);

        assert_eq!(batch[1].media_sequence_number, 1);
        assert!(!batch[1].is_init_segment);
        assert!(!batch[1].is_prefetch);

        assert_eq!(batch[2].media_sequence_number, 1);
        assert!(!batch[2].is_init_segment);
        assert!(batch[2].is_prefetch);
    }

    #[test]
    fn test_batch_scheduler_take_batch_resets_state() {
        let config = BatchSchedulerConfig::default();
        let mut scheduler = BatchScheduler::new(config);

        scheduler.add_job(create_test_job(1));
        scheduler.add_job(create_test_job(2));

        let batch = scheduler.take_batch();
        assert_eq!(batch.len(), 2);

        // After take_batch, state should be reset
        assert_eq!(scheduler.pending_count(), 0);
        assert!(!scheduler.is_ready());
        assert!(scheduler.time_until_ready().is_none());
    }

    #[test]
    fn test_batch_scheduler_empty_batch() {
        let config = BatchSchedulerConfig::default();
        let mut scheduler = BatchScheduler::new(config);

        let batch = scheduler.take_batch();
        assert!(batch.is_empty());
    }

    #[test]
    fn test_batch_scheduler_disabled() {
        let config = BatchSchedulerConfig {
            enabled: false,
            batch_window_ms: 50,
            max_batch_size: 5,
        };
        let scheduler = BatchScheduler::new(config);
        assert!(!scheduler.is_enabled());
    }
}
