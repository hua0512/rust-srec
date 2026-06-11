// HLS Segment Scheduler: Manages the pipeline of segments to be downloaded and processed.

use crate::hls::HlsDownloaderError;
use crate::hls::config::{BatchSchedulerConfig, HlsConfig};
use crate::hls::fetcher::SegmentDownloader;
use crate::hls::metrics::PerformanceMetrics;
use crate::hls::processor::SegmentTransformer;
use crate::hls::segment_lifecycle::{SegmentJobKind, SegmentJobOutcome, SegmentJobResult};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use hls::HlsData;
use m3u8_rs::MediaSegment;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};
use url::Url;

#[derive(Debug, Clone)]
pub struct ScheduledSegmentJob {
    pub identity: Arc<str>,
    pub base_url: Arc<str>,
    pub media_sequence_number: u64,
    pub media_segment: Arc<MediaSegment>,
    pub kind: SegmentJobKind,
    pub is_init_segment: bool,
    /// Whether this job is a prefetch request (lower priority)
    pub is_prefetch: bool,
    /// Pre-parsed segment URL to avoid re-parsing in fetcher and processor
    pub parsed_url: Option<Arc<Url>>,
}

/// Batches segment jobs for efficient dispatch.
///
/// The BatchScheduler collects incoming segment jobs within a configurable time window
/// and dispatches them as a batch, sorted by media sequence number for optimal ordering.
pub struct BatchScheduler {
    config: BatchSchedulerConfig,
    pending: Vec<ScheduledSegmentJob>,
    pending_identities: HashSet<Arc<str>>,
    batch_start: Option<Instant>,
}

impl BatchScheduler {
    /// Create a new BatchScheduler with the given configuration
    pub fn new(config: BatchSchedulerConfig) -> Self {
        let capacity = config.max_batch_size;
        Self {
            config,
            pending: Vec::with_capacity(capacity),
            pending_identities: HashSet::with_capacity(capacity),
            batch_start: None,
        }
    }

    /// Add a job to the current batch
    pub fn add_job(&mut self, job: ScheduledSegmentJob) -> bool {
        if !self.pending_identities.insert(Arc::clone(&job.identity)) {
            trace!(
                identity = %job.identity,
                msn = job.media_sequence_number,
                "Skipping duplicate pending segment job"
            );
            return false;
        }

        if self.pending.is_empty() {
            // Start the batch window timer when first job arrives
            self.batch_start = Some(Instant::now());
        }
        self.pending.push(job);
        true
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
        if self.pending.len() >= self.config.max_batch_size.max(1) {
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
        self.pending_identities.clear();
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

        for job in jobs.drain(..) {
            if self.pending_identities.insert(Arc::clone(&job.identity)) {
                self.pending.push(job);
            }
        }
    }

    /// Get the number of pending jobs in the current batch
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn has_pending_capacity(&self) -> bool {
        self.pending.len() < self.config.max_batch_size.max(1)
    }

    pub fn contains_identity(&self, identity: &Arc<str>) -> bool {
        self.pending_identities.contains(identity)
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

        if self.pending.len() >= self.config.max_batch_size.max(1) {
            return Some(Duration::ZERO);
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

pub struct SegmentSchedulerChannels {
    pub segment_request_rx: mpsc::Receiver<ScheduledSegmentJob>,
    pub output_tx: mpsc::Sender<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
    pub outcome_tx: mpsc::UnboundedSender<SegmentJobOutcome>,
}

type SegmentProcessingResult = (
    u64,
    bool,
    bool,
    ScheduledSegmentJob,
    Result<ProcessedSegmentOutput, HlsDownloaderError>,
);

type SegmentFuture = Pin<Box<dyn Future<Output = SegmentProcessingResult> + Send>>;

pub struct SegmentScheduler {
    config: Arc<HlsConfig>,
    segment_fetcher: Arc<dyn SegmentDownloader>,
    segment_processor: Arc<dyn SegmentTransformer>,
    segment_request_rx: mpsc::Receiver<ScheduledSegmentJob>,
    output_tx: mpsc::Sender<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
    outcome_tx: mpsc::UnboundedSender<SegmentJobOutcome>,
    token: CancellationToken,
    batch_scheduler: BatchScheduler,
    /// Segment identities currently being downloaded.
    active_job_identities: HashSet<Arc<str>>,
    /// Performance metrics for tracking scheduler operations
    metrics: Option<Arc<PerformanceMetrics>>,
}

impl SegmentScheduler {
    pub fn new(
        config: Arc<HlsConfig>,
        segment_fetcher: Arc<dyn SegmentDownloader>,
        segment_processor: Arc<dyn SegmentTransformer>,
        channels: SegmentSchedulerChannels,
        token: CancellationToken,
    ) -> Self {
        let SegmentSchedulerChannels {
            segment_request_rx,
            output_tx,
            outcome_tx,
        } = channels;
        let batch_scheduler =
            BatchScheduler::new(config.performance_config.batch_scheduler.clone());
        Self {
            config,
            segment_fetcher,
            segment_processor,
            segment_request_rx,
            output_tx,
            outcome_tx,
            token,
            batch_scheduler,
            active_job_identities: HashSet::new(),
            metrics: None,
        }
    }

    /// Create a new SegmentScheduler with performance metrics
    pub fn with_metrics(
        config: Arc<HlsConfig>,
        segment_fetcher: Arc<dyn SegmentDownloader>,
        segment_processor: Arc<dyn SegmentTransformer>,
        channels: SegmentSchedulerChannels,
        token: CancellationToken,
        metrics: Arc<PerformanceMetrics>,
    ) -> Self {
        let mut scheduler = Self::new(config, segment_fetcher, segment_processor, channels, token);
        scheduler.metrics = Some(metrics);
        scheduler
    }

    /// Result of segment processing, including metadata for lifecycle tracking
    async fn perform_segment_processing(
        segment_fetcher: Arc<dyn SegmentDownloader>,
        segment_processor: Arc<dyn SegmentTransformer>,
        job: ScheduledSegmentJob,
    ) -> (
        u64,
        bool,
        bool,
        ScheduledSegmentJob,
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
                return (msn, is_prefetch, is_init_segment, job, Err(e));
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

        (msn, is_prefetch, is_init_segment, job, result)
    }

    fn build_outcome(
        identity: Arc<str>,
        media_sequence_number: u64,
        kind: SegmentJobKind,
        result: &Result<ProcessedSegmentOutput, HlsDownloaderError>,
    ) -> SegmentJobOutcome {
        let result = match result {
            Ok(_) => SegmentJobResult::Completed,
            Err(error) => SegmentJobResult::Failed {
                retryable: Self::is_retryable_segment_error(error),
                reason: error.to_string(),
            },
        };

        SegmentJobOutcome {
            identity,
            media_sequence_number,
            kind,
            result,
        }
    }

    fn is_retryable_segment_error(error: &HlsDownloaderError) -> bool {
        match error {
            HlsDownloaderError::HttpStatus { status, .. } => {
                status.is_server_error()
                    || *status == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || *status == reqwest::StatusCode::NOT_FOUND
            }
            HlsDownloaderError::SegmentFetch { retryable, .. } => *retryable,
            HlsDownloaderError::Network { .. } | HlsDownloaderError::Timeout { .. } => true,
            _ => false,
        }
    }

    /// Take all pending jobs from the batch scheduler and push them into the
    /// `FuturesUnordered` work-set, respecting `max_concurrency`. Any jobs that
    /// cannot be dispatched because the concurrency limit has been reached are
    /// re-queued into the batch scheduler for the next dispatch cycle.
    fn dispatch_batch_to_futures(
        batch_scheduler: &mut BatchScheduler,
        futures: &mut FuturesUnordered<SegmentFuture>,
        active_job_identities: &mut HashSet<Arc<str>>,
        segment_fetcher: &Arc<dyn SegmentDownloader>,
        segment_processor: &Arc<dyn SegmentTransformer>,
        max_concurrency: usize,
    ) {
        let batch = batch_scheduler.take_batch();
        let mut leftovers = Vec::new();
        for job in batch {
            if futures.len() >= max_concurrency {
                leftovers.push(job);
                continue;
            }
            if !active_job_identities.insert(Arc::clone(&job.identity)) {
                trace!(
                    identity = %job.identity,
                    msn = job.media_sequence_number,
                    "Skipping duplicate active segment job"
                );
                continue;
            }
            futures.push(Box::pin(Self::perform_segment_processing(
                Arc::clone(segment_fetcher),
                Arc::clone(segment_processor),
                job,
            )));
        }
        batch_scheduler.requeue_ready_jobs(leftovers);
    }

    /// Push a single job directly into the futures work-set (no batching).
    fn dispatch_single_to_futures(
        job: ScheduledSegmentJob,
        futures: &mut FuturesUnordered<SegmentFuture>,
        active_job_identities: &mut HashSet<Arc<str>>,
        segment_fetcher: &Arc<dyn SegmentDownloader>,
        segment_processor: &Arc<dyn SegmentTransformer>,
    ) -> bool {
        if !active_job_identities.insert(Arc::clone(&job.identity)) {
            trace!(
                identity = %job.identity,
                msn = job.media_sequence_number,
                "Skipping duplicate active segment job"
            );
            return false;
        }
        futures.push(Box::pin(Self::perform_segment_processing(
            Arc::clone(segment_fetcher),
            Arc::clone(segment_processor),
            job,
        )));
        true
    }

    pub async fn run(&mut self) {
        info!("Segment scheduler started.");
        let mut futures: FuturesUnordered<SegmentFuture> = FuturesUnordered::new();
        let mut draining = false;
        let batch_enabled = self.batch_scheduler.is_enabled();

        loop {
            let max_concurrency = self.config.scheduler_config.download_concurrency.max(1);
            let in_progress_count = futures.len();
            let can_accept_more = in_progress_count < max_concurrency;
            let can_receive_more = !draining
                && if batch_enabled {
                    self.batch_scheduler.has_pending_capacity()
                } else {
                    can_accept_more
                };

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
                    info!("Segment scheduler received cancellation token; entering drain mode.");
                    draining = true;
                    // Close the segment request channel to prevent new jobs from being added
                    // while we drain the existing ones.
                    self.segment_request_rx.close();

                    // Dispatch any remaining batched jobs before draining
                    if batch_enabled && self.batch_scheduler.pending_count() > 0 {
                        Self::dispatch_batch_to_futures(
                            &mut self.batch_scheduler,
                            &mut futures,
                            &mut self.active_job_identities,
                            &self.segment_fetcher,
                            &self.segment_processor,
                            max_concurrency,
                        );
                    }
                }

                // 2. Batch window timeout - dispatch partial batch when window expires
                _ = tokio::time::sleep(batch_timeout.unwrap_or(Duration::MAX)), if batch_enabled && batch_timeout.is_some() && can_accept_more => {
                    if self.batch_scheduler.is_ready() {
                        trace!(batch_size = self.batch_scheduler.pending_count(), "Batch window expired, dispatching partial batch");
                        Self::dispatch_batch_to_futures(
                            &mut self.batch_scheduler,
                            &mut futures,
                            &mut self.active_job_identities,
                            &self.segment_fetcher,
                            &self.segment_processor,
                            max_concurrency,
                        );
                    }
                }

                // 3. Receive new segment jobs
                // This branch is disabled when `draining` is true.
                maybe_job_request = self.segment_request_rx.recv(), if can_receive_more => {
                    if let Some(job_request) = maybe_job_request {
                        trace!(uri = %job_request.media_segment.uri, msn = %job_request.media_sequence_number, "Received new segment job.");

                        if self.active_job_identities.contains(&job_request.identity)
                            || self.batch_scheduler.contains_identity(&job_request.identity)
                        {
                            trace!(
                                identity = %job_request.identity,
                                msn = job_request.media_sequence_number,
                                "Skipping duplicate segment job request"
                            );
                            continue;
                        }

                        if job_request.is_prefetch
                            && let Some(metrics) = &self.metrics
                        {
                            metrics.record_prefetch_initiated();
                        }

                        if batch_enabled {
                            // Add to batch scheduler
                            self.batch_scheduler.add_job(job_request);

                            // Check if batch is ready (max size reached)
                            if self.batch_scheduler.is_ready() {
                                trace!(batch_size = self.batch_scheduler.pending_count(), "Batch ready (max size), dispatching");
                                Self::dispatch_batch_to_futures(
                                    &mut self.batch_scheduler,
                                    &mut futures,
                                    &mut self.active_job_identities,
                                    &self.segment_fetcher,
                                    &self.segment_processor,
                                    max_concurrency,
                                );
                            }
                        } else {
                            // Direct dispatch without batching
                            Self::dispatch_single_to_futures(
                                job_request,
                                &mut futures,
                                &mut self.active_job_identities,
                                &self.segment_fetcher,
                                &self.segment_processor,
                            );
                        }
                    } else {
                        // The input channel was closed by the PlaylistEngine.
                        // This is a natural end, so we start draining.
                        info!("Segment request channel closed; no new jobs will be accepted. Draining in-progress tasks.");
                        draining = true;

                        // Dispatch any remaining batched jobs
                        if batch_enabled && self.batch_scheduler.pending_count() > 0 {
                            debug!(batch_size = self.batch_scheduler.pending_count(), "Dispatching remaining batch on channel close");
                            Self::dispatch_batch_to_futures(
                                &mut self.batch_scheduler,
                                &mut futures,
                                &mut self.active_job_identities,
                                &self.segment_fetcher,
                                &self.segment_processor,
                                max_concurrency,
                            );
                        }
                    }
                }

                // 4. Handle completed futures
                // This branch remains active during draining to finish in-progress work.
                Some((completed_msn, is_prefetch, _is_init_segment, completed_job, processed_result)) = futures.next() => {
                    self.active_job_identities.remove(&completed_job.identity);

                    let outcome = Self::build_outcome(
                        Arc::clone(&completed_job.identity),
                        completed_msn,
                        completed_job.kind,
                        &processed_result,
                    );
                    if self.outcome_tx.send(outcome).is_err() {
                        debug!("Segment lifecycle outcome receiver closed.");
                    }

                    match processed_result {
                        Ok(processed_output) => {
                            // Record prefetch usage metric when a prefetched segment completes successfully
                            if is_prefetch
                                && let Some(metrics) = &self.metrics
                            {
                                metrics.record_prefetch_used();
                            }

                            if self.output_tx.send(Ok(processed_output)).await.is_err() {
                                error!("Output channel closed while sending processed segment. Shutting down scheduler.");
                                break;
                            }
                        }
                        Err(e) => {
                            // Transient per-segment errors should not kill the entire stream.
                            // Network/timeout errors (e.g. CDN body decode timeouts) are
                            // recoverable — the reorder buffer will treat the missing segment
                            // as a gap and skip it, just like a 404.
                            let should_ignore = Self::is_retryable_segment_error(&e);

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
                                    error!("Output channel closed while sending segment-processing error. Shutting down scheduler.");
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
                        break;
                    }
                    if !draining && self.segment_request_rx.is_closed() && futures.is_empty() {
                        break;
                    }
                    // If we get here, it means we are waiting for new jobs or for futures to complete.
                    // The select will keep polling.
                }
            }
        }
        info!("Segment scheduler finished.");
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
        let kind = if is_init_segment {
            SegmentJobKind::Init
        } else if is_prefetch {
            SegmentJobKind::Prefetch
        } else {
            SegmentJobKind::Media
        };
        ScheduledSegmentJob {
            identity: Arc::<str>::from(format!("{kind:?}:segment_{msn}.ts")),
            base_url: Arc::<str>::from("https://example.com/"),
            media_sequence_number: msn,
            media_segment: Arc::new(MediaSegment {
                uri: format!("segment_{}.ts", msn),
                ..Default::default()
            }),
            kind,
            is_init_segment,
            is_prefetch,
            parsed_url: None,
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

        assert!(scheduler.add_job(create_test_job(1)));
        assert_eq!(scheduler.pending_count(), 1);

        assert!(scheduler.add_job(create_test_job(2)));
        assert_eq!(scheduler.pending_count(), 2);
    }

    #[test]
    fn test_batch_scheduler_deduplicates_by_identity() {
        let config = BatchSchedulerConfig::default();
        let mut scheduler = BatchScheduler::new(config);

        assert!(scheduler.add_job(create_test_job(1)));
        assert!(!scheduler.add_job(create_test_job(1)));
        assert_eq!(scheduler.pending_count(), 1);
    }

    #[test]
    fn test_batch_scheduler_tracks_requeued_identity() {
        let config = BatchSchedulerConfig {
            enabled: true,
            batch_window_ms: 50,
            max_batch_size: 5,
        };
        let mut scheduler = BatchScheduler::new(config);
        let job = create_test_job(1);
        let identity = Arc::clone(&job.identity);

        scheduler.requeue_ready_jobs(vec![job]);

        assert_eq!(scheduler.pending_count(), 1);
        assert!(scheduler.contains_identity(&identity));
        assert_eq!(scheduler.time_until_ready(), Some(Duration::ZERO));
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

    #[test]
    fn segment_error_classification_treats_404_as_retryable() {
        let err = HlsDownloaderError::http_status(
            reqwest::StatusCode::NOT_FOUND,
            "https://example.com/segment.ts",
            "hls segment fetch",
        );

        assert!(SegmentScheduler::is_retryable_segment_error(&err));
    }

    #[test]
    fn segment_error_classification_treats_403_as_terminal() {
        let err = HlsDownloaderError::http_status(
            reqwest::StatusCode::FORBIDDEN,
            "https://example.com/segment.ts",
            "hls segment fetch",
        );

        assert!(!SegmentScheduler::is_retryable_segment_error(&err));
    }

    #[test]
    fn segment_error_classification_treats_500_as_retryable() {
        let err = HlsDownloaderError::http_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "https://example.com/segment.ts",
            "hls segment fetch",
        );

        assert!(SegmentScheduler::is_retryable_segment_error(&err));
    }
}
