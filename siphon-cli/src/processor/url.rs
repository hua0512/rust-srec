use flv::data::FlvData;
use flv::error::FlvError;
use flv_fix::flv_error_to_pipeline_error;
use flv_fix::pipeline::FlvPipeline;
use flv_fix::writer_task::FlvWriterTask;
use futures::StreamExt;
use indicatif::HumanBytes;
use pipeline_common::{PipelineError, StreamerContext};
use reqwest::Url;
use siphon_engine::FlvDownloader;
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::info;

use crate::config::ProgramConfig;
use crate::utils::progress::ProgressManager;

/// Process a single URL with proxy support
pub async fn process_url(
    url_str: &str,
    output_dir: &Path,
    config: ProgramConfig,
    name_template: Option<&str>,
    pb_manager: &mut ProgressManager,
) -> Result<(), Box<dyn std::error::Error>> {
    let start_time = std::time::Instant::now();

    // Create output directory if it doesn't exist
    tokio::fs::create_dir_all(output_dir).await?;

    // Parse the URL
    let url = Url::parse(url_str)?;

    // Get default base name from URL if no template is provided
    let base_name = if let Some(template) = name_template {
        // Use template with placeholder expansion
        info!("Using custom filename template: {}", template);
        // expand_filename_template(template, Some(&url))
        // use directly the template as the base name
        template.to_string()
    } else {
        // Default behavior: extract from URL path
        let file_name = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .unwrap_or("download")
            .to_string();

        // Remove any file extension from the base name
        match file_name.rfind('.') {
            Some(pos) => file_name[..pos].to_string(),
            None => file_name,
        }
    };

    info!(url = %url_str, filename = %base_name, "Processing with output filename");

    // check if download config exists in the config
    let download_config = config.download_config.unwrap_or_default();

    let downloader = FlvDownloader::with_config(download_config)?;

    pb_manager.set_status(&format!("Processing URL: {}", url_str));

    if !config.enable_processing {
        // Raw mode: download without parsing through FlvDecoderStream
        info!(
            url = %url_str,
            "Starting to download raw FLV stream (default mode)"
        );

        // Download the raw stream without parsing
        let raw_byte_stream = downloader.download_raw(url_str).await?;

        // Create a file writer for the raw data
        let raw_output_path = output_dir.join(format!("{}.flv", base_name));
        let file = tokio::fs::File::create(&raw_output_path).await?;

        info!("Saving raw stream to {}", raw_output_path.display());

        // Set up file progress bar if progress manager is enabled
        let filename = raw_output_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();

        if !pb_manager.is_disabled() {
            pb_manager.add_file_progress(&filename);
            pb_manager.set_status(&format!("Downloading {}", filename));
        }

        // Write the stream directly to the file with progress reporting
        let bytes_written = write_raw_stream_to_file(raw_byte_stream, file, pb_manager).await?;

        let elapsed = start_time.elapsed();

        pb_manager.finish(&format!(
            "Downloaded {} in {:?}",
            HumanBytes(bytes_written),
            elapsed
        ));

        info!(
            url = %url_str,
            output_file = %raw_output_path.display(),
            bytes_written = bytes_written,
            duration = ?elapsed,
            "Raw download complete"
        );
    } else {
        // Processing mode: download and process through the pipeline
        pb_manager.set_status(&format!("Starting download from {}", url_str));

        // Download the FLV stream
        let mut input_stream = downloader.download(url_str).await?;

        // Process through the pipeline
        let context = StreamerContext::default();
        let pipeline = FlvPipeline::with_config(context, config.pipeline_config);

        // Create the input stream
        let (sender, receiver) =
            std::sync::mpsc::sync_channel::<Result<FlvData, FlvError>>(config.channel_size);
        let (output_tx, output_rx) =
            std::sync::mpsc::sync_channel::<Result<FlvData, FlvError>>(config.channel_size);

        let process_task = Some(tokio::task::spawn_blocking(move || {
            let pipeline = pipeline.build_pipeline();

            let input = std::iter::from_fn(|| {
                // Read from the receiver channel
                receiver
                    .recv()
                    .map(|result| result.map_err(flv_error_to_pipeline_error))
                    .map(Some)
                    .unwrap_or(None)
            });

            let mut output = |result: Result<FlvData, PipelineError>| {
                // Convert PipelineError back to FlvError for output
                let flv_result = result.map_err(|e| {
                    FlvError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Pipeline error: {}", e),
                    ))
                });

                if output_tx.send(flv_result).is_err() {
                    tracing::warn!("Output channel closed, stopping processing");
                }
            };

            pipeline.process(input, &mut output).unwrap();
        }));

        let output_dir = output_dir.to_path_buf();

        // Add a file progress bar if progress manager is enabled
        if !pb_manager.is_disabled() {
            pb_manager.add_file_progress(&base_name);
        }

        // Clone progress manager for the writer task
        let progress_clone = pb_manager.clone();

        // Clone the name_template Option before moving it to the task
        let has_template = name_template.is_some();

        // Create writer task and run it
        let writer_handle = tokio::task::spawn_blocking(move || {
            let mut writer_task = FlvWriterTask::new(output_dir, base_name)?;

            // // Configure writer to use the base name directly if a template was provided
            if has_template {
                writer_task.use_base_name_directly(true);
            }

            // Set up progress bar callbacks if progress is enabled
            progress_clone.setup_writer_task_callbacks(&mut writer_task);

            let result = writer_task.run(output_rx);

            result.map(|_| {
                (
                    writer_task.total_tags_written(),
                    writer_task.files_created(),
                )
            })
        });

        while let Some(result) = input_stream.next().await {
            sender.send(result).unwrap()
        }

        drop(sender); // Close the channel to signal completion
        let (total_tags_written, files_created) = writer_handle.await??;

        if let Some(p) = process_task {
            p.await?;
        }

        let elapsed = start_time.elapsed();

        // Finish all progress bars
        pb_manager.finish(&format!(
            "Processed {} tags into {} files in {:?}",
            total_tags_written, files_created, elapsed
        ));

        info!(
            url = %url_str,
            duration = ?elapsed,
            tags_processed = total_tags_written,
            files_created = files_created,
            "Download and processing complete"
        );
    }

    Ok(())
}

/// Write a raw byte stream to a file without any processing
async fn write_raw_stream_to_file(
    mut stream: siphon_engine::RawByteStream,
    file: tokio::fs::File,
    progress: &ProgressManager,
) -> Result<u64, Box<dyn std::error::Error>> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    let mut writer = tokio::io::BufWriter::new(file);
    let mut total_bytes = 0u64;

    // Update progress every 100ms at most
    let mut last_update = Instant::now();

    // Process the stream and write bytes to file
    while let Some(result) = stream.next().await {
        match result {
            Ok(bytes) => {
                writer.write_all(&bytes).await?;
                total_bytes += bytes.len() as u64;

                // Update the progress bar (but not too frequently) if progress is enabled
                if !progress.is_disabled() {
                    let now = Instant::now();
                    if now.duration_since(last_update) > Duration::from_millis(100) {
                        progress.update_main_progress(total_bytes);

                        // Get the current file progress bar (if any)
                        if let Some(file_pb) = progress.get_file_progress() {
                            file_pb.set_position(total_bytes);
                            file_pb.set_length(total_bytes);
                        }

                        last_update = now;
                    }
                }
            }
            Err(e) => return Err(Box::new(e)),
        }
    }

    // Final progress update
    progress.update_main_progress(total_bytes);

    // Flush any remaining buffered data
    writer.flush().await?;

    Ok(total_bytes)
}
