use bytes::Bytes;
use futures::{Stream, StreamExt};
use m3u8_rs::{self, MediaPlaylist};
use reqwest::{Client, Url};
use std::{
    collections::{HashSet, VecDeque}, // Import VecDeque
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinSet,
    time,
};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, info, warn};

use crate::{
    DownloadError, DownloaderConfig,
    hls::{
        playlist_utils::{refresh_live_playlist, resolve_url},
        segment_processor::process_segment_download,
    },
};

/// Type alias for a boxed stream of HLS playlist segments
pub type SegmentStream = Pin<Box<dyn Stream<Item = Result<Bytes, DownloadError>> + Send>>;

const MAX_TRACKED_SEGMENTS: usize = 20; // Define the maximum number of segments to track

/// Download media segments from a VOD (non-live) playlist
pub(crate) async fn download_vod_playlist(
    client: &Client,
    playlist: &MediaPlaylist,
    base_url: &Url,
    config: &DownloaderConfig,
) -> Result<SegmentStream, DownloadError> {
    // Clone values that need to be moved into the stream
    let client = client.clone();
    let segments = playlist.segments.clone();
    let base = base_url.clone();
    let max_concurrent = config.max_concurrent_hls_downloads;

    // Create a stream of segment download futures
    let segment_stream = futures::stream::iter(segments.into_iter().enumerate())
        .map(move |(i, segment)| {
            let client = client.clone();
            let base_clone = base.clone(); // Clone base for the async block
            let segment_uri = segment.uri.clone(); // Clone URI for logging

            async move {
                let segment_url = match resolve_url(&segment_uri, &base_clone) {
                    Ok(url) => url,
                    Err(e) => return Err(e), // Propagate URL resolution error
                };

                debug!(index = i, url = %segment_url, "Downloading HLS segment (VOD)");
                // Directly download the segment for VOD, no complex retry/channel logic needed here
                // as buffering handles concurrency.
                let response = client.get(segment_url.clone()).send().await?;
                if !response.status().is_success() {
                    return Err(DownloadError::StatusCode(response.status()));
                }
                response.bytes().await.map_err(DownloadError::from)
            }
        })
        .buffered(max_concurrent) // Use value from config
        .boxed();

    Ok(segment_stream)
}

/// Process new segments that haven't been downloaded yet, optimizing by sequence number.
fn process_new_segments(
    playlist: &MediaPlaylist, // Pass the whole playlist to access media_sequence
    processed_uris: &Arc<Mutex<HashSet<String>>>,
    uri_order: &Arc<Mutex<VecDeque<String>>>, // Add queue to track order and limit size
    last_processed_sequence: &mut Option<u64>, // Track the last processed sequence number
) -> Vec<m3u8_rs::MediaSegment> {
    // Handle potential mutex poisoning gracefully
    let mut processed = processed_uris.lock().unwrap_or_else(|poisoned| {
        warn!("Mutex was poisoned, recovering data (HashSet)");
        poisoned.into_inner()
    });
    let mut order = uri_order.lock().unwrap_or_else(|poisoned| {
        warn!("Mutex was poisoned, recovering data (VecDeque)");
        poisoned.into_inner()
    });

    let mut new_segments = Vec::new();
    let start_sequence = playlist.media_sequence;
    let mut current_max_sequence = *last_processed_sequence; // Keep track of the max sequence in this batch

    for (i, segment) in playlist.segments.iter().enumerate() {
        let current_sequence = start_sequence + i as u64;

        // Check if the sequence number is potentially new
        let is_potentially_new =
            last_processed_sequence.map_or(true, |last_seq| current_sequence > last_seq);

        // Only process if potentially new based on sequence and not already seen by URI
        if is_potentially_new && !processed.contains(&segment.uri) {
            let uri_to_add = segment.uri.clone();
            processed.insert(uri_to_add.clone());
            order.push_back(uri_to_add); // Add to the back of the queue

            // If the queue exceeds the limit, remove the oldest element from the front
            if order.len() > MAX_TRACKED_SEGMENTS {
                if let Some(oldest_uri) = order.pop_front() {
                    processed.remove(&oldest_uri); // Remove from the set as well
                }
            }

            new_segments.push(segment.clone());
            // Update the maximum sequence number seen in this processing batch
            current_max_sequence = Some(
                current_max_sequence
                    .map_or(current_sequence, |max_seq| max_seq.max(current_sequence)),
            );
        }
    }

    // Update the last processed sequence number for the next iteration
    *last_processed_sequence = current_max_sequence;

    new_segments
}

/// Download media segments from a live playlist, continuously polling for updates
pub(crate) async fn download_live_playlist(
    client: &Client,
    initial_playlist: &MediaPlaylist,
    playlist_url: &Url, // The URL of the media playlist itself for refreshing
    base_url: &Url,     // The base URL for resolving segment URIs
    config: &DownloaderConfig, // Add config parameter
) -> Result<SegmentStream, DownloadError> {
    let client = client.clone();
    let base_url = base_url.clone();
    let playlist_url = playlist_url.clone(); // Clone the specific playlist URL for refreshing
    let max_concurrent = config.max_concurrent_hls_downloads; // Get value from config

    // Use a Tokio channel
    let (tx, rx) = mpsc::channel(16);

    // Track segments we've already processed by their URIs (for fast lookup)
    let processed_segments = Arc::new(Mutex::new(HashSet::new()));
    // Track the order of processed segments to limit history
    let segment_order = Arc::new(Mutex::new(VecDeque::with_capacity(
        MAX_TRACKED_SEGMENTS + 1,
    )));
    // Track the last processed media sequence number
    let last_processed_sequence: Option<u64> = None; // Initialize here

    // Calculate refresh interval based on target duration
    // Refresh every half target duration, but at least every 1 second.
    let refresh_interval = Duration::from_secs_f64(
        (initial_playlist.target_duration as f64 / 2.0).max(1.0), // Use f64 division and min 1.0 sec
    );

    // Create a semaphore to limit the number of concurrent segment downloads
    let semaphore = Arc::new(Semaphore::new(max_concurrent)); // Use value from config

    info!(
        refresh_interval_secs = refresh_interval.as_secs_f64(), // Log f64 value
        playlist_url = %playlist_url,
        base_url = %base_url,
        max_concurrent_downloads = max_concurrent, // Log the configured value
        "Starting live HLS stream with periodic refresh"
    );

    // Spawn a task that continuously polls for playlist updates
    tokio::spawn({
        let processed_segments = processed_segments.clone();
        let segment_order = segment_order.clone(); // Clone the order queue Arc
        let client = client.clone();
        let semaphore = semaphore.clone();
        let tx = tx.clone();
        // Clone the initial_playlist before moving it into the async block
        let initial_playlist = initial_playlist.clone();
        // Clone base_url and playlist_url for the async block
        let base_url = base_url.clone();
        let playlist_url = playlist_url.clone();
        // Move last_processed_sequence into the task
        let mut last_processed_sequence = last_processed_sequence; // Shadowing outer variable, this one is mutable

        async move {
            let mut current_playlist = initial_playlist;
            // Create a JoinSet for work-stealing thread pool
            let mut segment_tasks = JoinSet::new();
            let mut retry_count = 0;
            const MAX_RETRIES: usize = 5;

            loop {
                // Process new segments from the current playlist using sequence number optimization
                let new_segments = process_new_segments(
                    &current_playlist, // Pass the whole playlist
                    &processed_segments,
                    &segment_order,               // Pass the order queue
                    &mut last_processed_sequence, // Pass mutable reference
                );

                // Add segment download tasks to the JoinSet
                for segment in new_segments {
                    // Use the correct base_url for resolving segment URIs
                    let segment_url = match resolve_url(&segment.uri, &base_url) {
                        Ok(url) => url,
                        Err(e) => {
                            warn!(uri = %segment.uri, error = %e, "Invalid segment URL");
                            continue;
                        }
                    };

                    let client = client.clone();
                    let tx = tx.clone();
                    let permit_semaphore = semaphore.clone();

                    // Add task to the JoinSet using the function from segment_processor
                    segment_tasks.spawn(process_segment_download(
                        client,
                        segment_url,
                        tx,
                        permit_semaphore,
                    ));
                }

                // Process completed tasks in batches for efficiency
                let mut completed = 0;
                while let Some(result) = segment_tasks.join_next().await {
                    match result {
                        Ok((_segment_url, Ok(()))) => {
                            completed += 1;
                        }
                        Ok((segment_url, Err(()))) => {
                            // Task encountered an error (likely channel closed)
                            warn!(url = %segment_url, "Segment download canceled (channel closed?)");
                        }
                        Err(join_err) => {
                            // Log task panic
                            if join_err.is_panic() {
                                warn!(error = ?join_err, "Task panicked during segment download");
                            } else {
                                warn!(error = ?join_err, "Task canceled during segment download");
                            }
                        }
                    }

                    // Process in batches of at most 8 to avoid blocking too long
                    if completed >= 8 {
                        break;
                    }
                }

                // Wait before refreshing the playlist
                time::sleep(refresh_interval).await;

                // Refresh the playlist using the specific playlist_url
                match refresh_live_playlist(&client, &playlist_url).await {
                    Ok(updated_playlist) => {
                        // Reset retry counter on success
                        retry_count = 0;

                        // Check if the playlist indicates the stream has ended
                        if updated_playlist.end_list {
                            info!("Live stream has ended (playlist end_list=true)");
                            break;
                        }

                        current_playlist = updated_playlist;
                    }
                    Err(e) => {
                        retry_count += 1;
                        warn!(
                            url = %playlist_url,
                            retry = retry_count,
                            max_retries = MAX_RETRIES,
                            error = %e,
                            "Failed to refresh playlist"
                        );

                        // If we've exceeded the retry limit, send error and break the loop
                        if retry_count >= MAX_RETRIES {
                            warn!(url = %playlist_url, "Exceeded maximum retries for playlist refresh, stopping stream.");
                            // Send a final error to the receiver
                            let err_msg = format!(
                                "Failed to refresh playlist {} after {} retries: {}",
                                playlist_url, MAX_RETRIES, e
                            );
                            if tx
                                .send(Err(DownloadError::IoError(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    err_msg,
                                ))))
                                .await
                                .is_err()
                            {
                                warn!("Receiver closed before final error could be sent.");
                            }
                            break;
                        }

                        // Continue with the current (stale) playlist for now
                    }
                }
            }

            // Abort any remaining tasks in the JoinSet before exiting
            segment_tasks.abort_all();
            info!("Live playlist download task finished.");
            // Channel sender (tx) will be dropped when this task finishes, closing the stream.
        }
    });

    // Return a stream that yields segments from the channel
    let segment_stream = ReceiverStream::new(rx).boxed();

    Ok(segment_stream)
}
