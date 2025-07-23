use futures::StreamExt;
use hls::HlsData;
use hls_fix::HlsWriter;
use hls_fix::{HlsPipeline, HlsPipelineConfig};
use pipeline_common::{OnProgress, PipelineError, StreamerContext};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

use mesio_engine::DownloaderInstance;

use crate::config::ProgramConfig;
use crate::output::provider::OutputFormat;

/// Process an HLS stream
pub async fn process_hls_stream(
    url_str: &str,
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    on_progress: Option<OnProgress>,
    downloader: &mut DownloaderInstance,
) -> Result<u64, Box<dyn std::error::Error>> {
    let start_time = Instant::now();

    // Create output directory if it doesn't exist
    tokio::fs::create_dir_all(output_dir).await?;

    // Extract name from URL
    let url = url_str.parse::<reqwest::Url>()?;
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
    // Add the source URL with priority 0 (for potential fallback)
    downloader.add_source(url_str, 0);

    // Create output with appropriate format
    let output_format = config.output_format.unwrap_or(OutputFormat::File);

    // Start the download
    let mut stream = match downloader {
        DownloaderInstance::Hls(hls_manager) => hls_manager.download_with_sources(url_str).await?,
        _ => return Err("Expected HLS downloader".into()),
    };

    // Peek at the first segment to determine the file extension
    let first_segment = match stream.next().await {
        Some(Ok(segment)) => segment,
        Some(Err(e)) => return Err(format!("Failed to get first HLS segment: {e}").into()),
        None => {
            info!("HLS stream is empty.");
            return Ok(0);
        }
    };

    let extension = match first_segment {
        HlsData::TsData(_) => "ts",
        HlsData::M4sData(_) => "m4s",
        HlsData::EndMarker => {
            // This should not happen for the first segment, but we'll default to "ts"
            "ts"
        }
    };

    info!(
        "Detected HLS stream type: {}. Saving with .{} extension.",
        extension.to_uppercase(),
        extension
    );
    info!("Saving HLS stream to {} output", output_format);

    let context = StreamerContext::default();

    let pipeline_config = HlsPipelineConfig {
        max_duration_limit: Some((config.pipeline_config.duration_limit * 1000.0) as u64),
        max_file_size: config.pipeline_config.file_size_limit,
    };

    debug!("Pipeline config: {:?}", pipeline_config);
    let pipeline = HlsPipeline::new(Arc::new(context), pipeline_config);

    // sender channel
    let (sender, receiver) =
        std::sync::mpsc::sync_channel::<Result<HlsData, PipelineError>>(config.channel_size);

    // output channel
    let (output_tx, output_rx) =
        std::sync::mpsc::sync_channel::<Result<HlsData, PipelineError>>(config.channel_size);

    let process_task = tokio::task::spawn_blocking(move || {
        let pipeline = pipeline.build_pipeline();

        let input = std::iter::from_fn(|| receiver.recv().map(Some).unwrap_or(None));

        let mut output = |result: Result<HlsData, PipelineError>| {
            if output_tx.send(result).is_err() {
                warn!("Output channel closed, stopping processing");
            }
        };

        pipeline.process(input, &mut output).unwrap();
    });

    let output_dir = output_dir.to_path_buf();
    let extension = extension.to_string();

    let writer_handle = tokio::task::spawn_blocking(move || {
        let mut writer_task = HlsWriter::new(output_dir, base_name, extension, on_progress);
        writer_task.run(output_rx)
    });

    // Send the first segment that we peeked at
    if sender.send(Ok(first_segment)).is_err() {
        warn!("Failed to send the first segment. Receiver has been dropped.");
        // In this case, we can just stop, as the processing pipeline is not running.
    } else {
        // Pipe the rest of the data from the stream to the pipeline
        while let Some(result) = stream.next().await {
            match result {
                Ok(segment) => {
                    if sender.send(Ok(segment)).is_err() {
                        warn!("Sender channel closed, stopping processing");
                        break;
                    }
                }
                Err(e) => {
                    let err_msg = format!("HLS segment error: {e}");
                    if sender
                        .send(Err(PipelineError::Processing(err_msg.clone())))
                        .is_err()
                    {
                        warn!("Failed to send error to pipeline. Receiver has been dropped.");
                    }
                    return Err(err_msg.into());
                }
            }
        }
    }

    drop(sender);

    let (_ts_segments_written, total_segments_written) = writer_handle.await??;

    process_task.await?;

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
