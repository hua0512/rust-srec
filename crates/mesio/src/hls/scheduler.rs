// HLS Segment Scheduler: Manages the pipeline of segments to be downloaded and processed.

use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use crate::hls::fetcher::SegmentDownloader;
use crate::hls::processor::SegmentTransformer;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use hls::HlsData;
use m3u8_rs::{ByteRange as M3u8ByteRange, Key as M3u8Key, MediaSegment};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct ScheduledSegmentJob {
    pub segment_uri: String,
    pub base_url: String,
    pub media_sequence_number: u64,
    pub duration: f32,
    pub key: Option<M3u8Key>,
    pub byte_range: Option<M3u8ByteRange>,
    pub discontinuity: bool,
    pub media_segment: MediaSegment,
    pub is_init_segment: bool,
}

#[derive(Debug)]
pub struct ProcessedSegmentOutput {
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
        Self {
            config,
            segment_fetcher,
            segment_processor,
            segment_request_rx,
            output_tx,
            token,
        }
    }

    async fn perform_segment_processing(
        segment_fetcher: Arc<dyn SegmentDownloader>,
        segment_processor: Arc<dyn SegmentTransformer>,
        job: ScheduledSegmentJob,
    ) -> Result<ProcessedSegmentOutput, HlsDownloaderError> {
        // debug!(uri = %job.segment_uri, msn = %job.media_sequence_number, "Starting segment processing");
        let raw_data_result = segment_fetcher.download_segment_from_job(&job).await;

        let raw_data = match raw_data_result {
            Ok(data) => data,
            Err(e) => {
                error!(uri = %job.segment_uri, error = %e, "Segment download failed");
                return Err(e);
            }
        };

        let processed_result = segment_processor
            .process_segment_from_job(raw_data, &job)
            .await;

        match processed_result {
            Ok(hls_data) => {
                let output = ProcessedSegmentOutput {
                    original_segment_uri: job.segment_uri.clone(),
                    data: hls_data,
                    media_sequence_number: job.media_sequence_number,
                    discontinuity: job.discontinuity,
                };
                debug!(uri = %job.segment_uri, msn = %job.media_sequence_number, "Segment processing successful");
                Ok(output)
            }
            Err(e) => {
                warn!(uri = %job.segment_uri, error = %e, "Segment transformation failed");
                Err(e)
            }
        }
    }

    pub async fn run(&mut self) {
        info!("SegmentScheduler started.");
        let mut futures = FuturesUnordered::new();
        let mut draining = false;

        loop {
            let in_progress_count = futures.len();

            tokio::select! {
                biased;

                // 1. Cancellation Token
                _ = self.token.cancelled(), if !draining => {
                    info!("Cancellation token received. SegmentScheduler entering draining state.");
                    draining = true;
                    // Close the segment request channel to prevent new jobs from being added
                    // while we drain the existing ones.
                    self.segment_request_rx.close();
                }

                // 2. Receive new segment jobs
                // This branch is disabled when `draining` is true.
                maybe_job_request = self.segment_request_rx.recv(), if !draining && in_progress_count < self.config.scheduler_config.download_concurrency => {
                    if let Some(job_request) = maybe_job_request {
                        debug!(uri = %job_request.segment_uri, msn = %job_request.media_sequence_number, "Received new segment job.");
                        let fetcher_clone = Arc::clone(&self.segment_fetcher);
                        let processor_clone = Arc::clone(&self.segment_processor);
                        futures.push(Self::perform_segment_processing(
                            fetcher_clone,
                            processor_clone,
                            job_request,
                        ));
                    } else {
                        // The input channel was closed by the PlaylistEngine.
                        // This is a natural end, so we start draining.
                        info!("Segment request channel closed. No new jobs will be accepted. Draining in-progress tasks.");
                        draining = true;
                    }
                }

                // 3. Handle completed futures
                // This branch remains active during draining to finish in-progress work.
                Some(processed_result) = futures.next() => {
                    match processed_result {
                        Ok(processed_output) => {
                            if self.output_tx.send(Ok(processed_output)).await.is_err() {
                                error!("Output channel closed while trying to send processed segment. Shutting down scheduler.");
                                break;
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "Segment processing task failed.");
                            if self.output_tx.send(Err(e)).await.is_err() {
                                error!("Output channel closed while trying to send segment processing error. Shutting down scheduler.");
                                break;
                            }
                        }
                    }
                }

                // 4. Shutdown condition
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
