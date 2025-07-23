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
use tokio::sync::{broadcast, mpsc};
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
    #[allow(dead_code)]
    shutdown_rx: broadcast::Receiver<()>,
}

impl SegmentScheduler {
    pub fn new(
        config: Arc<HlsConfig>,
        segment_fetcher: Arc<dyn SegmentDownloader>,
        segment_processor: Arc<dyn SegmentTransformer>,
        segment_request_rx: mpsc::Receiver<ScheduledSegmentJob>,
        output_tx: mpsc::Sender<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Self {
        Self {
            config,
            segment_fetcher,
            segment_processor,
            segment_request_rx,
            output_tx,
            shutdown_rx,
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
        // Unordered futures, as we don't care about the order of completion
        // OutputManager will handle the order of segments
        // We could use buffer_unordered here, but we have to convert into a ReceiverStream
        let mut futures = FuturesUnordered::new();

        loop {
            // Current count of in-progress futures
            let in_progress_count = futures.len();

            tokio::select! {
                biased;

                // Receive new segment jobs
                // Only accept new jobs if we are below the concurrency limit
                maybe_job_request = self.segment_request_rx.recv(), if in_progress_count < self.config.scheduler_config.download_concurrency => {
                    match maybe_job_request {
                        Some(job_request) => {
                            debug!(uri = %job_request.segment_uri, msn = %job_request.media_sequence_number, "Received new segment job.");
                            let fetcher_clone = Arc::clone(&self.segment_fetcher);
                            let processor_clone = Arc::clone(&self.segment_processor);
                            // Push the new job to the futures queue
                            futures.push(Self::perform_segment_processing(
                                fetcher_clone,
                                processor_clone,
                                job_request,
                            ));
                        }
                        None => {
                            info!("Segment request channel closed. No new jobs will be accepted.");
                            // If the input channel is closed and no tasks are in progress, we can shut down.
                            if futures.is_empty() {
                                info!("All pending segments processed. SegmentScheduler shutting down.");
                                break;
                            }
                            // Otherwise, we just stop receiving new jobs but continue processing existing ones.
                        }
                    }
                }

                // Handle completed futures
                // This will be triggered when any of the futures in `futures` completes
                Some(processed_result_from_spawn) = futures.next(), if in_progress_count > 0 => {
                    match processed_result_from_spawn {
                        Ok(processed_output) => {
                            // debug!(uri = %processed_output.original_segment_uri, msn = %processed_output.media_sequence_number, "Segment processed, sending to output.");
                            if self.output_tx.send(Ok(processed_output)).await.is_err() {
                                error!("Output channel closed while trying to send processed segment. Shutting down scheduler.");
                                break;
                            }
                        }
                        Err(e) => {
                            // Error from perform_segment_processing
                            warn!(error = %e, "Segment processing task failed.");
                            if self.output_tx.send(Err(e)).await.is_err() {
                                error!("Output channel closed while trying to send segment processing error. Shutting down scheduler.");
                                break; // Exit loop if output channel is closed
                            }
                        }
                    }
                }

                // Branch to handle shutdown signal
                // _ = self.shutdown_rx.recv() => {
                //     info!("Shutdown signal received. SegmentScheduler will stop accepting new jobs and complete in-progress tasks.");
                //     // We could implement a more graceful shutdown by not accepting new jobs
                //     // and waiting for existing futures to complete, or by cancelling them.
                //     // For now, just break the loop.
                //     // To prevent new jobs, we can close self.segment_request_rx or simply rely on the select behavior.
                //     // The current logic will stop accepting new jobs if segment_request_rx is None.
                //     // If shutdown is received, we might want to drain futures or set a flag.
                //     // For simplicity, we break. If futures are still running, they will be dropped.
                //     break;
                // }

                // This 'else' branch is crucial for `tokio::select!`.
                // It ensures the loop terminates if all other branches are disabled or complete.
                // Specifically, if `segment_request_rx` is closed (becomes None) AND `futures` is empty.
                else => {
                    info!("All channels closed or futures completed. SegmentScheduler shutting down.");
                    break;
                }
            }
        }
        info!("SegmentScheduler finished.");
    }
}
