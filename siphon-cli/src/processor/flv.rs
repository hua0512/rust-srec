use flv::data::FlvData;
use flv::error::FlvError;
use flv::parser_async::FlvDecoderStream;
use flv_fix::FlvPipeline;
use flv_fix::flv_error_to_pipeline_error;
use flv_fix::writer::{FlvWriter, FlvWriterError};
use futures::StreamExt;
use indicatif::HumanBytes;
use pipeline_common::{PipelineError, StreamerContext};
use siphon_engine::DownloaderInstance;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
        // Define the status callback closure
        let status_callback = {
            let pb_clone = progress_clone.clone();
            Arc::new(
                move |path: Option<&PathBuf>, size: u64, _rate: f64, duration: Option<u32>| {
                    if let Some(file_pb) = pb_clone.get_file_progress() {
                        file_pb.set_position(size);
                        if let Some(d) = duration {
                            file_pb.set_message(format!("{}s", d / 1000));
                        }
                    }
                    if let Some(p) = path {
                        pb_clone.set_status(&format!("Writing to {}", p.display()));
                    }
                },
            )
        };

        // Create and run the new writer
        let mut flv_writer =
            FlvWriter::new(output_dir.clone(), base_name.clone(), Some(status_callback));
        flv_writer.run(processed_stream)
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

    let writer_result = writer_handle.await?;
    let (tags_written, files_created) = match writer_result {
        Ok(stats) => stats,
        Err(e) => match e {
            FlvWriterError::InputError(e) => {
                warn!("Writer channel closed prematurely: {}", e);
                (0, 0) // Default stats on input error
            }
            FlvWriterError::Task(e) => return Err(e.into()),
        },
    };

    if let Some(p) = process_task {
        p.await?;
    }

    let elapsed = start_time.elapsed();

    // Finish progress bars with summary
    pb_manager.finish(&format!(
        "Processed {} in {:?}. Tags written: {}, Files created: {}",
        HumanBytes(bytes_processed),
        elapsed,
        tags_written,
        files_created
    ));

    info!(
        path = %input_path.display(),
        input_size = %format_bytes(file_size),
        duration = ?elapsed,
        tags_written,
        files_created,
        processing_enabled = config.enable_processing,
        "Processing complete"
    );

    Ok(())
}

/// Process an FLV stream
pub async fn process_flv_stream(
    url_str: &str,
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    pb_manager: &mut ProgressManager,
    downloader: &mut DownloaderInstance,
) -> Result<u64, Box<dyn std::error::Error>> {
    let start_time = Instant::now();

    // Create output directory if it doesn't exist
    tokio::fs::create_dir_all(output_dir).await?;

    // Parse the URL for file naming
    let url_name = {
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

    let base_name = name_template.replace("%u", &url_name);
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
                    e.to_string(),
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

        // Add a file progress bar if progress manager is enabled
        if !pb_manager.is_disabled() {
            pb_manager.add_file_progress(&base_name);
        }

        let output_dir_clone = output_dir.to_path_buf();
        let base_name_clone = base_name.clone();
        // Write task
        let writer_handle = tokio::task::spawn_blocking(move || {
            // Define the status callback closure
            let status_callback = {
                let pb_clone = progress_clone.clone();
                Arc::new(
                    move |path: Option<&PathBuf>, size: u64, _rate: f64, duration: Option<u32>| {
                        if let Some(file_pb) = pb_clone.get_file_progress() {
                            file_pb.set_position(size);
                            if let Some(d) = duration {
                                file_pb.set_message(format!("{}s", d / 1000));
                            }
                        }
                        if let Some(p) = path {
                            pb_clone.set_status(&format!("Writing to {}", p.display()));
                        }
                    },
                )
            };

            // Create and run the new writer
            let mut flv_writer =
                FlvWriter::new(output_dir_clone, base_name_clone, Some(status_callback));
            flv_writer.run(output_rx)
        });

        pb_manager.set_status("Downloading and processing FLV stream...");

        // Pipe data from the downloader to the processing pipeline
        while let Some(result) = stream.next().await {
            // Convert the result to the expected type
            let converted_result = result.map_err(|e| {
                FlvError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
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
        let writer_result = writer_handle.await?;
        let (tags_written, files_created) = match writer_result {
            Ok(stats) => stats,
            Err(e) => match e {
                FlvWriterError::InputError(e) => {
                    warn!("Writer channel closed prematurely: {}", e);
                    (0, 0) // Default stats on input error
                }
                FlvWriterError::Task(e) => return Err(e.into()),
            },
        };

        // Wait for processing task to finish
        let _ = process_task.await?; // Ensure task is finished

        let elapsed = start_time.elapsed();

        // Final progress update
        pb_manager.finish(&format!(
            "Processed stream in {:?}. Tags written: {}, Files created: {}",
            elapsed, tags_written, files_created
        ));

        info!(
            url = %url_str,
            duration = ?elapsed,
            tags_written,
            files_created,
            "FLV processing complete"
        );

        Ok(tags_written as u64)
    }
}
