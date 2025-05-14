mod flv;
mod hls;

use siphon_engine::{DownloadManagerConfig, ProtocolType, SiphonDownloaderFactory};
use std::path::{Path, PathBuf};
use tracing::{error, info};

use crate::{config::ProgramConfig, utils::progress::ProgressManager};

/// Determine the type of input and process accordingly
pub async fn process_inputs(
    inputs: &[String],
    output_dir: &Path,
    config: &mut ProgramConfig,
    name_template: Option<&str>,
    progress_manager: &mut ProgressManager,
) -> Result<(), Box<dyn std::error::Error>> {
    if inputs.is_empty() {
        return Err("No input files or URLs provided".into());
    }

    let inputs_len = inputs.len();
    info!(
        inputs_count = inputs_len,
        "Starting processing of {} input{}",
        inputs_len,
        if inputs_len == 1 { "" } else { "s" }
    );

    // Preallocate a string builder for status messages to avoid repeated allocations
    let mut status_buffer = String::with_capacity(100);

    let factory = SiphonDownloaderFactory::new()
        .with_download_config(DownloadManagerConfig::default())
        .with_flv_config(config.flv_config.clone().unwrap_or_default())
        .with_hls_config(config.hls_config.clone().unwrap_or_default());

    // Process each input
    for (index, input) in inputs.iter().enumerate() {
        let input_index = index + 1;

        // Log which input we're processing
        info!(
            input_index = input_index,
            total_inputs = inputs_len,
            input = %input,
            "Processing input ({}/{})",
            input_index,
            inputs_len
        );

        // Update progress manager if it's not disabled - reuse the string buffer
        if !progress_manager.is_disabled() {
            status_buffer.clear();
            status_buffer.push_str("Processing input (");
            status_buffer.push_str(&input_index.to_string());
            status_buffer.push('/');
            status_buffer.push_str(&inputs_len.to_string());
            status_buffer.push_str(") - ");
            status_buffer.push_str(input);
            progress_manager.set_status(&status_buffer);
        }

        // Process based on input type
        if input.starts_with("http://") || input.starts_with("https://") {
            let mut downloader = factory.create_for_url(input, ProtocolType::Auto).await?;

            let protocol_type = downloader.protocol_type();

            match protocol_type {
                ProtocolType::Flv => {
                    flv::process_flv_stream(
                        input,
                        output_dir,
                        config,
                        name_template,
                        progress_manager,
                        &mut downloader,
                    )
                    .await?;
                }
                ProtocolType::Hls => {
                    hls::process_hls_stream(
                        input,
                        output_dir,
                        config,
                        name_template,
                        progress_manager,
                        &mut downloader,
                    )
                    .await?;
                }
                _ => {
                    error!("Unsupported protocol for: {}", input);
                    return Err(format!("Unsupported protocol: {}", input).into());
                }
            }
        } else {
            // It's a file path
            let path = PathBuf::from(input);
            if path.exists() && path.is_file() {
                // For files, check the extension to determine the type
                if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
                    match extension.to_lowercase().as_str() {
                        "flv" => {
                            flv::process_file(&path, output_dir, config, progress_manager).await?;
                        }
                        // "m3u8" | "m3u" => {
                        //     hls::process_hls_file(&path, output_dir, config, progress_manager).await?;
                        // },
                        _ => {
                            error!("Unsupported file extension for: {}", input);
                            return Err(format!("Unsupported file extension: {}", input).into());
                        }
                    }
                } else {
                    error!("File without extension: {}", input);
                    return Err(format!("File without extension: {}", input).into());
                }
            } else {
                error!(
                    "Input is neither a valid URL nor an existing file: {}",
                    input
                );
                return Err(format!("Invalid input: {}", input).into());
            }
        }
    }

    Ok(())
}
