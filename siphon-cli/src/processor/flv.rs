use flv::data::FlvData;
use flv::error::FlvError;
use flv::parser_async::FlvDecoderStream;
use flv_fix::flv_error_to_pipeline_error;
use flv_fix::pipeline::FlvPipeline;
use flv_fix::writer_task::{FlvWriterTask, WriterError};
use futures::StreamExt;
use indicatif::HumanBytes;
use pipeline_common::{PipelineError, StreamerContext};
use siphon_engine::DownloaderInstance;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::BufReader;
use tracing::{info, warn};

use crate::config::ProgramConfig;
use crate::output::output::{OutputFormat, create_output};
use crate::utils::format_bytes;
use crate::utils::progress::ProgressManager;

/// Process a single FLV file
pub async fn process_file(
    input_path: &Path,
    output_dir: &Path,
    config: &ProgramConfig,
    pb_manager: &mut ProgressManager,
) -> Result<(), Box<dyn std::error::Error>> {
    let start_time = std::time::Instant::now();

    // Create output directory if it doesn't exist
    tokio::fs::create_dir_all(output_dir).await?;

    // Create base name for output files
    let base_name = input_path
        .file_stem()
        .ok_or("Invalid filename")?
        .to_string_lossy()
        .to_string();

    info!(
        path = %input_path.display(),
        processing_enabled = config.enable_processing,
        "Starting to process file"
    );

    // Open the file and create decoder stream
    let file = File::open(input_path).await?;
    let file_reader = BufReader::new(file);
    let file_size = file_reader.get_ref().metadata().await?.len();

    // Update progress manager status if not disabled
    pb_manager.set_status(&format!("Processing {}", input_path.display()));

    // Create a file-specific progress bar if progress manager is not disabled
    let file_name = input_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    if !pb_manager.is_disabled() {
        pb_manager.add_file_progress(&file_name);
    }

    let mut decoder_stream = FlvDecoderStream::with_capacity(
        file_reader,
        1024 * 1024, // Input buffer capacity
    );

    // Create the input stream
    let (sender, receiver) =
        std::sync::mpsc::sync_channel::<Result<FlvData, FlvError>>(config.channel_size);

    let mut process_task = None;

    let processed_stream = if config.enable_processing {
        // Processing mode: run through the processing pipeline
        info!(
            path = %input_path.display(),
            "Processing pipeline enabled, applying fixes and optimizations"
        );
        pb_manager.set_status("Processing with optimizations enabled");

        // Create streamer context and pipeline
        let context = StreamerContext::default();
        let pipeline = FlvPipeline::with_config(context, config.pipeline_config.clone());

        let (output_tx, output_rx) =
            std::sync::mpsc::sync_channel::<Result<FlvData, FlvError>>(config.channel_size);

        process_task = Some(tokio::task::spawn_blocking(move || {
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
        output_rx
    } else {
        // Raw mode: bypass the pipeline entirely
        info!(
            path = %input_path.display(),
            "Processing pipeline disabled, outputting raw data"
        );
        pb_manager.set_status("Processing without optimizations");
        receiver
    };

    let output_dir = output_dir.to_path_buf();

    // Clone progress manager for the writer task
    let progress_clone = pb_manager.clone();

    // Create writer task and run it
    let writer_handle = tokio::task::spawn_blocking(move || {
        let mut writer_task = FlvWriterTask::new(output_dir, base_name)?;

        // Set up progress bar callbacks
        progress_clone.setup_writer_task_callbacks(&mut writer_task);

        writer_task.run(processed_stream)?;

        Ok::<_, WriterError>((
            writer_task.total_tags_written(),
            writer_task.files_created(),
        ))
    });

    // Process the FLV data
    let mut bytes_processed = 0;

    while let Some(result) = decoder_stream.next().await {
        // Update the processed bytes count if applicable
        if let Ok(data) = &result {
            bytes_processed += data.size() as u64;
        }

        // Send the result to the processing pipeline
        if sender.send(result).is_err() {
            warn!("Processing channel closed prematurely");
            break;
        }
    }

    drop(sender); // Close the channel to signal completion

    let (total_tags_written, files_created) = writer_handle.await??;

    if let Some(p) = process_task {
        p.await?;
    }

    let elapsed = start_time.elapsed();

    // Finish progress bars with summary
    pb_manager.finish(&format!(
        "Processed {} ({} tags) in {:?}",
        HumanBytes(bytes_processed),
        total_tags_written,
        elapsed
    ));

    info!(
        path = %input_path.display(),
        input_size = %format_bytes(file_size),
        duration = ?elapsed,
        processing_enabled = config.enable_processing,
        tags_processed = total_tags_written,
        files_created = files_created,
        "Processing complete"
    );

    Ok(())
}

/// Process an FLV stream
pub async fn process_flv_stream(
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
            .unwrap_or(
                &std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
                    .to_string(),
            )
            .to_string();

        // Remove any file extension
        match file_name.rfind('.') {
            Some(pos) => file_name[..pos].to_string(),
            None => file_name,
        }
    };

    let use_base_name = name_template.is_some();

    // Setup progress reporting
    pb_manager.add_url_progress(url_str);
    let progress_clone = pb_manager.clone();

    // Add the source URL with priority 0
    downloader.add_source(url_str, 0);

    // Start the download with the callback

    if !config.enable_processing {
        // RAW MODE: Fast path for direct streaming without processing

        let mut stream = match downloader {
            DownloaderInstance::Flv(flv_manager) => flv_manager.download_raw(url_str).await?,
            _ => return Err("Expected FLV downloader".into()),
        };

        let output_format = config.output_format.unwrap_or(OutputFormat::File);

        let mut output_manager = create_output(
            output_format,
            output_dir,
            &base_name,
            "flv",
            Some(pb_manager.clone()),
        )?;

        // Add a file progress bar if progress manager is enabled
        if !pb_manager.is_disabled() {
            pb_manager.add_file_progress(&format!("{}.flv", base_name));
        }

        info!("Saving raw FLV stream to {} output", output_format);

        let mut bytes_written = 0;
        let mut last_update = Instant::now();

        while let Some(data) = stream.next().await {
            // Write bytes to output
            let data = data.map_err(|e| {
                FlvError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Downloader error: {}", e),
                ))
            })?;
            output_manager.write_bytes(&data)?;
            bytes_written += data.len() as u64;

            // Update progress (not too frequently)
            if !pb_manager.is_disabled() {
                let now = Instant::now();
                if now.duration_since(last_update) > Duration::from_millis(100) {
                    pb_manager.update_main_progress(bytes_written);
                    if let Some(file_pb) = pb_manager.get_file_progress() {
                        file_pb.set_position(bytes_written);
                    }
                    last_update = now;
                }
            }
        }

        // Final progress update
        if !pb_manager.is_disabled() {
            pb_manager.update_main_progress(bytes_written);
            if let Some(file_pb) = pb_manager.get_file_progress() {
                file_pb.set_position(bytes_written);
            }
        }

        // Finalize the output
        let total_bytes = output_manager.close()?;

        let elapsed = start_time.elapsed();

        // Log summary
        info!(
            url = %url_str,
            bytes_written = total_bytes,
            duration = ?elapsed,
            "Raw FLV download complete"
        );

        Ok(total_bytes)
    } else {
        // PROCESSING MODE: Apply the FLV processing pipeline

        let mut stream = match downloader {
            DownloaderInstance::Flv(flv_manager) => {
                flv_manager.download_with_sources(url_str).await?
            }
            _ => return Err("Expected FLV downloader".into()),
        };

        let context = StreamerContext::default();
        let pipeline = FlvPipeline::with_config(context, config.pipeline_config.clone());

        // sender channel
        let (sender, receiver) =
            std::sync::mpsc::sync_channel::<Result<FlvData, FlvError>>(config.channel_size);

        // output channel
        let (output_tx, output_rx) =
            std::sync::mpsc::sync_channel::<Result<FlvData, FlvError>>(config.channel_size);

        // Process task
        let process_task = tokio::task::spawn_blocking(move || {
            let pipeline = pipeline.build_pipeline();

            let input = std::iter::from_fn(|| {
                receiver
                    .recv()
                    .map(|result| result.map_err(flv_error_to_pipeline_error))
                    .map(Some)
                    .unwrap_or(None)
            });

            let mut output = |result: Result<FlvData, PipelineError>| {
                let flv_result = result.map_err(|e| {
                    FlvError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Pipeline error: {}", e),
                    ))
                });

                if output_tx.send(flv_result).is_err() {
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

        let output_dir = output_dir.to_path_buf();

        // Write task
        let writer_handle = tokio::task::spawn_blocking(move || {
            let mut writer_task = FlvWriterTask::new(output_dir, base_name)?;

            // Configure writer to use the base name directly if a template was provided
            writer_task.use_base_name_directly(use_base_name);

            // Set up progress bar callbacks if progress is enabled
            writer_progress.setup_writer_task_callbacks(&mut writer_task);

            let result = writer_task.run(output_rx);

            result.map(|_| {
                (
                    writer_task.total_tags_written(),
                    writer_task.files_created(),
                )
            })
        });

        pb_manager.set_status("Downloading and processing FLV stream...");

        // Pipe data from the downloader to the processing pipeline
        while let Some(result) = stream.next().await {
            // Convert the result to the expected type
            let converted_result = result.map_err(|e| {
                FlvError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Downloader error: {}", e),
                ))
            });

            if sender.send(converted_result).is_err() {
                warn!("Sender channel closed prematurely");
                break;
            }
        }

        // Close the sender channel to signal completion
        drop(sender);

        pb_manager.set_status("Processing FLV data...");

        // Wait for write task to finish
        let (total_tags_written, files_created) = writer_handle.await??;
        // Wait for processing task to finish
        let _ = process_task.await?; // Ensure task is finished

        let elapsed = start_time.elapsed();

        // Final progress update
        pb_manager.finish(&format!(
            "Processed {} tags into {} files in {:?}",
            total_tags_written, files_created, elapsed
        ));

        info!(
            url = %url_str,
            duration = ?elapsed,
            tags_processed = total_tags_written,
            files_created = files_created,
            "FLV processing complete"
        );

        Ok(total_tags_written as u64)
    }
}
