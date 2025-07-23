use flv::data::FlvData;
use flv::error::FlvError;
use flv::parser_async::FlvDecoderStream;
use flv_fix::FlvPipeline;
use flv_fix::flv_error_to_pipeline_error;
use flv_fix::writer::{FlvWriter, FlvWriterError};
use futures::StreamExt;
use pipeline_common::{OnProgress, PipelineError, StreamerContext};
use mesio_engine::DownloaderInstance;
use std::path::Path;
use std::time::Instant;
use tokio::fs::File;
use tokio::io::BufReader;
use tracing::{info, warn};

use crate::config::ProgramConfig;
use crate::output::provider::{OutputFormat, create_output};
use crate::utils::format_bytes;

/// Process a single FLV file
pub async fn process_file(
    input_path: &Path,
    output_dir: &Path,
    config: &ProgramConfig,
    on_progress: Option<OnProgress>,
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
                    FlvError::Io(std::io::Error::other(format!("Pipeline error: {e}")))
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
        receiver
    };

    let output_dir = output_dir.to_path_buf();

    let writer_handle = tokio::task::spawn_blocking(move || {
        let mut flv_writer = FlvWriter::new(output_dir.clone(), base_name.clone(), on_progress);
        flv_writer.run(processed_stream)
    });

    // Process the FLV data
    while let Some(result) = decoder_stream.next().await {
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
    on_progress: Option<OnProgress>,
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

        let mut output_manager = create_output(output_format, output_dir, &base_name, "flv")?;

        info!("Saving raw FLV stream to {} output", output_format);

        let _last_update = Instant::now();
        while let Some(data) = stream.next().await {
            // Write bytes to output
            let data = data.map_err(|e| {
                FlvError::Io(std::io::Error::other(e.to_string()))
            })?;
            output_manager.write_bytes(&data)?;
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
                    FlvError::Io(std::io::Error::other(format!("Pipeline error: {e}")))
                });

                if output_tx.send(flv_result).is_err() {
                    warn!("Output channel closed, stopping processing");
                }
            };

            pipeline.process(input, &mut output).unwrap();
        });

        let output_dir_clone = output_dir.to_path_buf();
        let base_name_clone = base_name.clone();
        // Write task
        let writer_handle = tokio::task::spawn_blocking(move || {
            // Create and run the new writer
            let mut flv_writer = FlvWriter::new(output_dir_clone, base_name_clone, on_progress);
            flv_writer.run(output_rx)
        });

        // Pipe data from the downloader to the processing pipeline
        while let Some(result) = stream.next().await {
            // Convert the result to the expected type
            let converted_result =
                result.map_err(|e| FlvError::Io(std::io::Error::other(e.to_string())));

            if sender.send(converted_result).is_err() {
                warn!("Sender channel closed prematurely");
                break;
            }
        }

        // Close the sender channel to signal completion
        drop(sender);

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
        process_task.await?; // Ensure task is finished

        let elapsed = start_time.elapsed();

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
