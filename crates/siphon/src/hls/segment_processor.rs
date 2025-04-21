use bytes::Bytes;
use reqwest::{Client, Url};
use std::time::Duration;
use tokio::{
    sync::{Semaphore, mpsc::Sender},
    time,
};
use tracing::{debug, warn};

use crate::DownloadError;

/// Helper function to download a single segment
async fn download_segment(client: &Client, segment_url: Url) -> Result<Bytes, DownloadError> {
    // Download the segment
    let response = client.get(segment_url.clone()).send().await?;

    // Check response status
    if !response.status().is_success() {
        return Err(DownloadError::StatusCode(response.status()));
    }

    // Get the segment data directly as Bytes
    let data = response.bytes().await?;

    Ok(data)
}

/// Process the download of a single segment with retries and semaphore management.
/// Returns the original segment URL and Ok(()) on success, or Err(()) if the send channel fails.
pub(crate) async fn process_segment_download(
    client: Client,
    segment_url: Url,
    tx: Sender<Result<Bytes, DownloadError>>,
    permit_semaphore: std::sync::Arc<Semaphore>,
) -> (Url, Result<(), ()>) {
    let mut retries = 0;
    const MAX_RETRIES: usize = 3;
    const RETRY_DELAY: Duration = Duration::from_secs(1);

    // Acquire a permit asynchronously
    let permit = match permit_semaphore.acquire_owned().await {
        Ok(p) => p,
        Err(_) => {
            warn!(url = %segment_url, "Semaphore closed unexpectedly");
            return (segment_url, Err(())); // Indicate failure
        }
    };

    loop {
        debug!(url = %segment_url, attempt = retries + 1, "Attempting segment download");
        match download_segment(&client, segment_url.clone()).await {
            Ok(data) => {
                debug!(url = %segment_url, bytes = data.len(), "Segment downloaded successfully");
                if tx.send(Ok(data)).await.is_err() {
                    warn!(url = %segment_url, "Failed to send segment data: receiver closed");
                    // Drop the permit when returning
                    drop(permit);
                    return (segment_url, Err(())); // Indicate failure to send
                }
                // Drop the permit when returning
                drop(permit);
                return (segment_url, Ok(())); // Indicate success
            }
            Err(e) => {
                warn!(url = %segment_url, error = %e, attempt = retries + 1, "Segment download failed");
                retries += 1;
                if retries >= MAX_RETRIES {
                    warn!(url = %segment_url, "Max retries reached for segment download");
                    if tx.send(Err(e)).await.is_err() {
                        warn!(url = %segment_url, "Failed to send final segment error: receiver closed");
                        // Drop the permit when returning
                        drop(permit);
                        return (segment_url, Err(())); // Indicate failure to send
                    }
                    // Drop the permit when returning
                    drop(permit);
                    return (segment_url, Ok(())); // Indicate success in sending the error
                }
                // Wait before retrying
                time::sleep(RETRY_DELAY * 2u32.pow(retries as u32 - 1)).await;
            }
        }
    }
    // Permit is dropped automatically when the function scope ends if not already dropped
}
