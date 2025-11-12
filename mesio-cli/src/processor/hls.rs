use crate::{
    config::ProgramConfig, error::AppError, processor::generic::process_stream,
    utils::create_dirs, utils::expand_name_url,
};
use futures::{stream, StreamExt};
use hls::HlsData;
use pipeline_common::CancellationToken;
use hls_fix::{HlsPipeline, HlsWriter};
use mesio_engine::{DownloadError, DownloaderInstance};
use pipeline_common::{PipelineError, ProtocolWriter, progress::ProgressEvent};
use std::time::Instant;
use std::{path::Path, sync::Arc};
use tracing::{debug, info};

/// Process an HLS stream
pub async fn process_hls_stream(
    url_str: &str,
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    on_progress: Option<Arc<dyn Fn(ProgressEvent) + Send + Sync + 'static>>,
    downloader: &mut DownloaderInstance,
    token: &CancellationToken,
) -> Result<u64, AppError> {
    // Create output directory if it doesn't exist
    create_dirs(output_dir).await?;

    let start_time = Instant::now();

    let base_name = expand_name_url(name_template, url_str)?;
    downloader.add_source(url_str, 10);

    // Start the download
    let mut stream = match downloader {
        DownloaderInstance::Hls(hls_manager) => hls_manager.download_with_sources(url_str).await?,
        _ => {
            return Err(AppError::InvalidInput(
                "Expected HLS downloader".to_string(),
            ));
        }
    };

    // Peek at the first segment to determine the file extension
    let first_segment = match stream.next().await {
        Some(Ok(segment)) => segment,
        Some(Err(e)) => {
            return Err(AppError::InvalidInput(format!(
                "Failed to get first HLS segment: {e}"
            )));
        }
        None => {
            info!("HLS stream is empty.");
            return Err(AppError::Download(DownloadError::NoSource(
                "HLS stream is empty".to_string(),
            )));
        }
    };

    let extension = match first_segment {
        HlsData::TsData(_) => "ts",
        HlsData::M4sData(_) => "m4s",
        // should never happen
        HlsData::EndMarker => {
            return Err(AppError::Pipeline(PipelineError::InvalidData(
                "First segment is EndMarker".to_string(),
            )));
        }
    };

    info!(
        "Detected HLS stream type: {}. Saving with .{} extension.",
        extension.to_uppercase(),
        extension
    );

    let hls_pipe_config = config.hls_pipeline_config.clone();
    debug!("Pipeline config: {:?}", hls_pipe_config);

    // Prepend the first segment back to the stream
    let stream_with_first_segment = stream::once(async { Ok(first_segment) }).chain(stream);

    let stream =
        stream_with_first_segment.map(|r| r.map_err(|e| PipelineError::Processing(e.to_string())));

    let (_ts_segments_written, total_segments_written) = process_stream::<HlsPipeline, HlsWriter>(
        &config.pipeline_config,
        hls_pipe_config,
        Box::pin(stream),
        || {
            HlsWriter::new(
                output_dir.to_path_buf(),
                base_name.to_string(),
                extension.to_string(),
                on_progress,
                None,
            )
        },
        token.clone(),
    )
    .await?;

    let elapsed = start_time.elapsed();

    // Log summary
    info!(
        url = %url_str,
        segments = total_segments_written,
        duration = ?elapsed,
        "HLS download complete"
    );

    Ok(total_segments_written as u64)
}
