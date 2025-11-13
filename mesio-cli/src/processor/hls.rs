use crate::utils::spans;
use crate::{config::ProgramConfig, error::AppError, utils::create_dirs, utils::expand_name_url};
use futures::{StreamExt, stream};
use hls::HlsData;
use hls_fix::{HlsPipeline, HlsWriter};
use mesio_engine::{DownloadError, DownloaderInstance};
use pipeline_common::CancellationToken;
use pipeline_common::{PipelineError, ProtocolWriter};
use std::path::Path;
use std::time::Instant;
use tracing::{Level, debug, info, span};
use tracing_indicatif::span_ext::IndicatifSpanExt;

/// Process an HLS stream
pub async fn process_hls_stream(
    url_str: &str,
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    downloader: &mut DownloaderInstance,
    token: &CancellationToken,
) -> Result<u64, AppError> {
    // Create output directory if it doesn't exist
    create_dirs(output_dir).await?;

    let start_time = Instant::now();

    let base_name = expand_name_url(name_template, url_str)?;
    downloader.add_source(url_str, 10);

    // Create the writer progress span up-front so downloads inherit it
    let writer_span = span!(Level::INFO, "writer_processing");
    spans::init_writing_span(&writer_span, format!("Writing HLS {}", base_name));

    let download_span = span!(parent: &writer_span, Level::INFO, "download_hls", url = %url_str);
    spans::init_spinner_span(&download_span, format!("Downloading {}", url_str));

    // Start the download while the download span is active so child spans attach correctly
    let mut stream = {
        let _writer_enter = writer_span.enter();
        let _download_enter = download_span.enter();
        match downloader {
            DownloaderInstance::Hls(hls_manager) => {
                hls_manager.download_with_sources(url_str).await?
            }
            _ => {
                return Err(AppError::InvalidInput(
                    "Expected HLS downloader".to_string(),
                ));
            }
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

    // Prepend the first segment back to the stream
    let stream_with_first_segment = stream::once(async { Ok(first_segment) }).chain(stream);
    let stream = stream_with_first_segment;

    let hls_pipe_config = config.hls_pipeline_config.clone();
    debug!("Pipeline config: {:?}", hls_pipe_config);

    let stream = stream.map(|r| r.map_err(|e| PipelineError::Processing(e.to_string())));

    let (total_items_written, files_created) =
        crate::processor::generic::process_stream_with_span::<HlsPipeline, HlsWriter>(
            &config.pipeline_config,
            hls_pipe_config,
            Box::pin(stream),
            writer_span.clone(),
            |_writer_span| {
                use std::collections::HashMap;
                let mut extras = HashMap::new();
                // Pass max_file_size to writer for progress bar length
                if config.pipeline_config.max_file_size > 0 {
                    extras.insert(
                        "max_file_size".to_string(),
                        config.pipeline_config.max_file_size.to_string(),
                    );
                }
                HlsWriter::new(
                    output_dir.to_path_buf(),
                    base_name.to_string(),
                    extension.to_string(),
                    if extras.is_empty() {
                        None
                    } else {
                        Some(extras)
                    },
                )
            },
            token.clone(),
        )
        .await?;

    download_span.pb_set_finish_message(&format!("Downloaded {}", url_str));
    drop(download_span);

    let elapsed = start_time.elapsed();

    // Log summary
    info!(
        url = %url_str,
        items = total_items_written,
        files = files_created,
        duration = ?elapsed,
        "HLS download complete"
    );

    Ok(total_items_written as u64)
}
