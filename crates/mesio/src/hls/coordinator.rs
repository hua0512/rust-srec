// HLS Stream Coordinator: Sets up and spawns all HLS download pipeline components.

use crate::CacheManager;
use crate::hls::buffer_pool::BufferPool;
use crate::hls::config::HlsConfig;
use crate::hls::decryption::{DecryptionService, KeyFetcher};
use crate::hls::events::HlsStreamEvent;
use crate::hls::fetcher::{Http2Stats, SegmentDownloader, SegmentFetcher};
use crate::hls::metrics::PerformanceMetrics;
use crate::hls::output::OutputManager;
use crate::hls::playlist::{InitialPlaylist, PlaylistEngine, PlaylistProvider};
use crate::hls::processor::{SegmentProcessor, SegmentTransformer};
use crate::hls::scheduler::{ScheduledSegmentJob, SegmentScheduler};
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, error};

use super::HlsDownloaderError;

// Struct to hold all spawned task handles and shared resources
pub struct AllTaskHandles {
    pub playlist_engine_handle: Option<JoinHandle<Result<(), HlsDownloaderError>>>,
    pub scheduler_handle: JoinHandle<()>,
    pub output_manager_handle: JoinHandle<()>,
    /// HTTP/2 connection statistics for observability
    pub http2_stats: Arc<Http2Stats>,
}

/// HLS Stream Coordinator: Sets up and spawns all HLS download pipeline components.
pub struct HlsStreamCoordinator;

impl HlsStreamCoordinator {
    /// Sets up all components, spawns their tasks, and returns the client event receiver,
    /// a shutdown sender, and handles to the spawned tasks.
    ///
    /// Optional parent_span can be provided for progress bar hierarchy.
    pub async fn setup_and_spawn(
        initial_url: String,
        config: Arc<HlsConfig>,
        http_client: Client,
        cache_manager: Option<Arc<CacheManager>>,
        token: CancellationToken,
        parent_span: Option<tracing::Span>,
    ) -> Result<
        (
            mpsc::Receiver<Result<HlsStreamEvent, HlsDownloaderError>>, // client_event_rx
            AllTaskHandles,
        ),
        HlsDownloaderError,
    > {
        // Initialize Services & Components

        // Create shared performance metrics for the pipeline
        let performance_metrics = Arc::new(PerformanceMetrics::new());

        // Create shared buffer pool for decryption operations with metrics
        let buffer_pool = Arc::new(BufferPool::with_metrics(
            config.performance_config.buffer_pool.clone(),
            Arc::clone(&performance_metrics),
        ));

        let key_fetcher = Arc::new(KeyFetcher::new(http_client.clone(), Arc::clone(&config)));
        let decryption_service = Arc::new(DecryptionService::with_buffer_pool(
            Arc::clone(&config),
            Arc::clone(&key_fetcher),
            cache_manager.clone(),
            Arc::clone(&buffer_pool),
        ));
        // Create shared HTTP/2 stats tracker
        let http2_stats = Arc::new(Http2Stats::new());
        let segment_fetcher: Arc<dyn SegmentDownloader> = Arc::new(SegmentFetcher::with_metrics(
            http_client.clone(),
            Arc::clone(&config),
            cache_manager.clone(),
            Arc::clone(&http2_stats),
            Arc::clone(&performance_metrics),
        ));
        let segment_processor: Arc<dyn SegmentTransformer> =
            Arc::new(SegmentProcessor::with_metrics(
                Arc::clone(&config),
                Arc::clone(&decryption_service),
                cache_manager.clone(),
                Arc::clone(&performance_metrics),
            ));
        let playlist_engine: Arc<dyn PlaylistProvider> = Arc::new(PlaylistEngine::new(
            http_client.clone(),
            cache_manager,
            Arc::clone(&config),
        ));

        // Channels - sized for optimal throughput
        let (client_event_tx, client_event_rx) = mpsc::channel(32);
        // Processed segments buffer: concurrency * multiplier for smooth flow
        let processed_segments_buffer_size = config.scheduler_config.download_concurrency
            * config.scheduler_config.processed_segment_buffer_multiplier;
        let (processed_segments_tx, processed_segments_rx) =
            mpsc::channel(processed_segments_buffer_size);
        // Segment request buffer: enough headroom for scheduler
        let (segment_request_tx, segment_request_rx) =
            mpsc::channel::<ScheduledSegmentJob>(config.scheduler_config.download_concurrency * 2);

        let token_for_playlist_engine = token.clone();
        let token_for_scheduler = token.clone();
        let token_for_output_manager = token;

        // initial playlist
        let initial_playlist_data = playlist_engine.load_initial_playlist(&initial_url).await?;
        let (initial_media_playlist, base_url, is_live, selected_media_playlist_url) =
            match &initial_playlist_data {
                InitialPlaylist::Master(_master, _) => {
                    // master is not directly used here after selection
                    let media_details = playlist_engine
                        .select_media_playlist(
                            &initial_playlist_data,
                            &config.playlist_config.variant_selection_policy,
                        )
                        .await?;
                    let end_list = media_details.playlist.end_list;

                    if end_list {
                        debug!("Selected media playlist is VOD.");
                    }

                    (
                        media_details.playlist,
                        media_details.base_url,
                        !end_list,
                        Some(media_details.url),
                    )
                }
                InitialPlaylist::Media(media, base) => {
                    (media.clone(), base.clone(), !media.end_list, None)
                }
            };

        // OutputManager is responsible for managing the output of the stream
        // Pass performance_metrics to log summary on stream end (Requirements 7.3)
        let mut output_manager = OutputManager::with_performance_metrics(
            Arc::clone(&config),
            processed_segments_rx,
            client_event_tx.clone(),
            is_live,
            initial_media_playlist.media_sequence,
            token_for_output_manager,
            Arc::clone(&performance_metrics),
        );

        let mut segment_scheduler = SegmentScheduler::with_metrics(
            Arc::clone(&config),
            segment_fetcher,
            segment_processor,
            segment_request_rx,
            processed_segments_tx,
            token_for_scheduler,
            Arc::clone(&performance_metrics),
        );

        let output_manager_handle = tokio::spawn(async move {
            output_manager.run().await;
            debug!("OutputManager task finished.");
        });

        let scheduler_parent_span = parent_span.clone();
        let scheduler_handle = if let Some(span) = scheduler_parent_span {
            tokio::spawn(
                async move {
                    segment_scheduler.run().await;
                }
                .instrument(span),
            )
        } else {
            tokio::spawn(async move {
                segment_scheduler.run().await;
            })
        };

        let playlist_engine_handle = {
            let playlist_url = selected_media_playlist_url.unwrap_or(initial_url);
            let playlist_engine_clone = playlist_engine.clone();
            let base_url_clone = base_url.clone();

            Some(tokio::spawn(async move {
                let res = playlist_engine_clone
                    .monitor_media_playlist(
                        &playlist_url,
                        initial_media_playlist,
                        base_url_clone,
                        segment_request_tx,
                        token_for_playlist_engine,
                    )
                    .await;

                if let Err(e) = &res {
                    error!("Playlist engine monitoring task ended with error: {:?}", e);
                    // Unlike before, we don't signal other tasks to shut down.
                    // The CancellationToken is the single source of truth for cancellation.
                    // If the playlist fails, the scheduler will eventually run out of jobs
                    // and terminate, which will close the output channel, and the
                    // output manager will then terminate.
                }

                res
            }))
        };

        let handles = AllTaskHandles {
            playlist_engine_handle,
            scheduler_handle,
            output_manager_handle,
            http2_stats,
        };

        Ok((client_event_rx, handles))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_USER_AGENT;
    use crate::hls::config::HlsConfig;
    use crate::{CacheConfig, CacheManager, create_client};
    use std::sync::Arc;
    use std::time::Duration;
    use tracing::{debug, info};

    #[tokio::test]
    #[ignore] // Ignore this test by default
    async fn test_setup_and_spawn() {
        // configure tracing
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();

        let cache_config = CacheConfig {
            enabled: true,
            disk_cache_path: None,  // If None, we'll use system temp dir
            max_disk_cache_size: 0, // 500MB
            max_memory_cache_size: 30 * 1024 * 1024, // 30MB
            default_ttl: Duration::from_secs(3600), // 1 hour
            segment_ttl: Duration::from_secs(2 * 60), // 2 minutes
            playlist_ttl: Duration::from_secs(60), // 1 minute
        };

        let cache_manager = CacheManager::new(cache_config).await.unwrap();
        let cache_manager = Arc::new(cache_manager);
        let config = HlsConfig::default();

        let config = Arc::new(config);
        let downloader_config = crate::DownloaderConfig::builder()
            .with_user_agent(DEFAULT_USER_AGENT)
            .with_timeout(std::time::Duration::from_secs(30))
            .with_header("referer", "http://live.douyin.com")
            .build();
        // Create the crypto provider
        let client = create_client(&downloader_config).unwrap();
        // let initial_url = "https://demo.unified-streaming.com/k8s/features/stable/video/tears-of-steel/tears-of-steel.ism/.m3u8".to_string();
        let initial_url = "http://pull-hls-f11.douyincdn.com/thirdgame/stream-693725261315179274.m3u8?arch_hrchy=h1&exp_hrchy=h1&expire=1747991885&major_anchor_level=common&sign=f0e1fa5f131404440612b895d83316bc&t_id=037-20250516171805D14BA54D125D402EA0DF-ytZ138".to_string();

        let cache = Some(cache_manager);

        let result = HlsStreamCoordinator::setup_and_spawn(
            initial_url,
            config,
            client,
            cache,
            CancellationToken::new(),
            None, // No parent span for test
        )
        .await;

        assert!(result.is_ok());
        let (mut client_event_rx, _handles) = result.unwrap();

        while let Some(event) = client_event_rx.recv().await {
            match event {
                Ok(HlsStreamEvent::Data(hls_data)) => {
                    info!(
                        "Final data: {:?}",
                        hls_data.media_segment().map(|seg| seg.uri.clone())
                    );
                }
                Ok(HlsStreamEvent::PlaylistRefreshed {
                    media_sequence_base,
                    target_duration,
                }) => {
                    debug!(
                        "Received PlaylistRefreshed event: seq_base={}, target_dur={}",
                        media_sequence_base, target_duration
                    );
                }

                Ok(HlsStreamEvent::DiscontinuityTagEncountered { .. }) => {
                    debug!("Received DiscontinuityTagEncountered event");
                }
                Ok(HlsStreamEvent::StreamEnded) => {
                    debug!("Received StreamEnded event");
                    break;
                }
                Ok(HlsStreamEvent::SegmentTimeout {
                    sequence_number,
                    waited_duration,
                }) => {
                    debug!(
                        "Received SegmentTimeout event: seq={}, waited={:?}",
                        sequence_number, waited_duration
                    );
                }
                Ok(HlsStreamEvent::GapSkipped {
                    from_sequence,
                    to_sequence,
                    reason,
                }) => {
                    debug!(
                        "Received GapSkipped event: from={}, to={}, reason={:?}",
                        from_sequence, to_sequence, reason
                    );
                }
                Err(e) => {
                    debug!("Received error event: {:?}", e);
                }
            }
        }
    }
}
