use bytes::Bytes;
use futures::StreamExt;
use hls::HlsData;
use hls_fix::writer_task::HlsWriterTask;
use hls_fix::{HlsPipeline, HlsPipelineConfig};
use indicatif::HumanBytes;
use pipeline_common::{PipelineError, StreamerContext};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use siphon_engine::downloader::DownloadManager;
use siphon_engine::{DownloadStream, DownloaderInstance, ProtocolType};

use crate::config::ProgramConfig;
use crate::output;
use crate::output::output::{OutputFormat, create_output};
use crate::utils::progress::ProgressManager;

/// Process an HLS stream
pub async fn process_hls_stream(
    url_str: &str,
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: Option<&str>,
    pb_manager: &mut ProgressManager,
    downloader: &mut DownloaderInstance,
) -> Result<u64, Box<dyn std::error::Error>> {
    let start_time = Instant::now();

    // Create output directory if it doesn't exist
    tokio::fs::create_dir_all(output_dir).await?;

    // Parse the URL for file naming
    let base_name = if let Some(template) = name_template {
        template.to_string()
    } else {
        // Extract name from URL
        let url = url_str.parse::<reqwest::Url>()?;
        let file_name = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .unwrap_or("playlist")
            .to_string();

        // Remove any file extension
        match file_name.rfind('.') {
            Some(pos) => file_name[..pos].to_string(),
            None => file_name,
        }
    };

    // Setup progress reporting
    pb_manager.add_url_progress(url_str);
    let progress_clone = pb_manager.clone();

    // Add the source URL with priority 0 (for potential fallback)
    downloader.add_source(url_str, 0);

    // Create output with appropriate format
    let output_format = config.output_format.unwrap_or(OutputFormat::File);

    let mut output_manager = create_output(
        output_format,
        output_dir,
        &base_name,
        "ts", // Use .ts extension for HLS content
        Some(pb_manager.clone()),
    )?;

    // Add a file progress bar if progress manager is enabled
    if !pb_manager.is_disabled() {
        pb_manager.add_file_progress(&format!("{}.ts", base_name));
    }

    // Start the download
    let mut stream = match downloader {
        DownloaderInstance::Hls(hls_manager) => hls_manager.download_with_sources(url_str).await?,
        _ => return Err("Expected HLS downloader".into()),
    };

    info!("Saving HLS stream to {} output", output_format);

    let context = StreamerContext::default();

    let pipeline = HlsPipeline::new(
        Arc::new(context),
        HlsPipelineConfig {
            max_segment_duration: None,
            max_segments: None,
        },
    );

    // sender channel
    let (sender, receiver) =
        std::sync::mpsc::sync_channel::<Result<HlsData, PipelineError>>(config.channel_size);

    // output channel
    let (output_tx, output_rx) =
        std::sync::mpsc::sync_channel::<Result<HlsData, PipelineError>>(config.channel_size);

    let process_task = tokio::task::spawn_blocking(move || {
        let pipeline = pipeline.build_pipeline();

        let input = std::iter::from_fn(|| {
            receiver.recv().map(Some).unwrap_or(None)
            // .map(|result| result.map_err(flv_error_to_pipeline_error))
            // .map(Some)
            // .unwrap_or(None)
        });

        let mut output = |result: Result<HlsData, PipelineError>| {
            if output_tx.send(result).is_err() {
                warn!("Output channel closed, stopping processing");
            }
        };

        pipeline.process(input, &mut output).unwrap();
    });

    let writer_progress = pb_manager.clone();

    // Add a file progress bar if progress manager is enabled
    if !pb_manager.is_disabled() {
        pb_manager.add_file_progress(&base_name);
    }

    let use_base_name_directly = name_template.is_some();

    let output_dir = output_dir.to_path_buf();

    let writer_handle = tokio::task::spawn_blocking(move || {
        let mut writer_task = HlsWriterTask::new(output_dir, base_name)?;

        // Configure writer to use the base name directly if a template was provided
        writer_task.use_base_name_directly(use_base_name_directly);

        // Set up progress bar callbacks if progress is enabled
        writer_progress.setup_hls_writer_task_callbacks(&mut writer_task);

        let result = writer_task.run(output_rx);

        result.map(|_| {
            (
                writer_task.ts_segments_written(),
                writer_task.total_segments_written(),
            )
        })
    });

    // Pipe data from the stream to the pipeline
    while let Some(result) = stream.next().await {
        match result {
            Ok(segment) => {
                if sender.send(Ok(segment)).is_err() {
                    warn!("Sender channel closed, stopping processing");
                    break;
                }
            }
            Err(e) => {
                return Err(format!("HLS segment error: {}", e).into());
            }
        }
    }

    drop(sender);

    let (ts_segments_written, total_segments_written) = writer_handle.await??;

    let _ = process_task.await?;

    let elapsed = start_time.elapsed();

    // Log summary
    info!(
        url = %url_str,
        segments = total_segments_written,
        duration = ?elapsed,
        "HLS download complete"
    );

    pb_manager.finish(&format!("Download complete"));

    Ok(total_segments_written)
}
