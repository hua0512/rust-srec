use crate::error::AppError;
use crate::processor::generic::process_stream;
use crate::{config::ProgramConfig, utils::create_dirs};
use futures::{StreamExt, stream};
use hls::HlsData;
use hls_fix::{HlsPipeline, HlsPipelineConfig, HlsWriter};
use mesio_engine::{DownloadError, DownloaderInstance};
use pipeline_common::{OnProgress, PipelineError};
use std::path::Path;
use std::time::Instant;
use tracing::{debug, info};

/// Process an HLS stream
pub async fn process_hls_stream(
    url_str: &str,
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    on_progress: Option<OnProgress>,
    downloader: &mut DownloaderInstance,
) -> Result<u64, AppError> {
    // Create output directory if it doesn't exist
    create_dirs(output_dir).await?;

    let start_time = Instant::now();

    // Extract name from URL
    let url = url_str
        .parse::<reqwest::Url>()
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
    let file_name = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or("playlist")
        .to_string();

    // Remove any file extension
    let base_name = match file_name.rfind('.') {
        Some(pos) => file_name[..pos].to_string(),
        None => file_name,
    };
    let base_name = name_template.replace("%u", &base_name);
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

    let (_ts_segments_written, total_segments_written) =
        process_stream::<HlsPipelineConfig, HlsData, HlsPipeline, HlsWriter, _, _>(
            &config.pipeline_config,
            hls_pipe_config,
            stream_with_first_segment,
            output_dir,
            &base_name,
            extension,
            on_progress,
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
