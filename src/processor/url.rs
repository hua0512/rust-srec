use flv::data::FlvData;
use flv::error::FlvError;
use flv_fix::context::StreamerContext;
use flv_fix::pipeline::{FlvPipeline, PipelineConfig};
use flv_fix::writer_task::{FlvWriterTask, WriterError};
use futures::StreamExt;
use reqwest::Url;
use siphon::FlvDownloader;
use siphon::downloader::DownloaderConfig;
use std::path::Path;
use std::sync::mpsc;
use tracing::info;

use crate::utils::expand_filename_template;

/// Process a single URL with proxy support
pub async fn process_url(
    url_str: &str,
    output_dir: &Path,
    config: PipelineConfig,
    download_config: DownloaderConfig,
    enable_processing: bool,
    name_template: Option<&str>,
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
        expand_filename_template(template, Some(&url), None)
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

    let downloader = FlvDownloader::with_config(download_config)?;

    if !enable_processing {
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

        // Write the stream directly to the file
        let bytes_written = write_raw_stream_to_file(raw_byte_stream, file).await?;

        let elapsed = start_time.elapsed();

        info!(
            url = %url_str,
            output_file = %raw_output_path.display(),
            bytes_written = bytes_written,
            duration = ?elapsed,
            "Raw download complete"
        );
    } else {
        // Processing mode: download and process through the pipeline
        // info!(
        //     url = %url_str,
        //     "Starting to download and process FLV from URL (processing enabled)"
        // );

        // Download the FLV stream
        let input_stream = downloader.download(url_str).await?;

        // Process through the pipeline
        let context = StreamerContext::default();
        let pipeline = FlvPipeline::with_config(context, config);
        let mut processed_stream = pipeline.process(input_stream);

        let (sender, receiver) = mpsc::sync_channel::<Result<FlvData, FlvError>>(8);
        // Create writer task and run it
        let reader_handle = tokio::spawn(async move {
            while let Some(result) = processed_stream.next().await {
                sender.send(result).unwrap();
            }
        });

        let output_dir = output_dir.to_path_buf();
        // Create writer task and run it
        let writer_handle = tokio::task::spawn_blocking(|| {
            let mut writer_task = FlvWriterTask::new(output_dir, base_name)?;

            writer_task.run(receiver)?;

            Ok::<_, WriterError>((
                writer_task.total_tags_written(),
                writer_task.files_created(),
            ))
        });

        reader_handle.await?;
        let (total_tags_written, files_created) = writer_handle.await??;

        let elapsed = start_time.elapsed();

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
    mut stream: siphon::RawByteStream,
    file: tokio::fs::File,
) -> Result<u64, Box<dyn std::error::Error>> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    let mut writer = tokio::io::BufWriter::new(file);
    let mut total_bytes = 0u64;

    // Process the stream and write bytes to file
    while let Some(result) = stream.next().await {
        match result {
            Ok(bytes) => {
                writer.write_all(&bytes).await?;
                total_bytes += bytes.len() as u64;
            }
            Err(e) => return Err(Box::new(e)),
        }
    }

    // Flush any remaining buffered data
    writer.flush().await?;

    Ok(total_bytes)
}
