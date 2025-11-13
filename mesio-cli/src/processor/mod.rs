mod flv;
mod generic;
mod hls;

use crate::{config::ProgramConfig, error::AppError};
use mesio_engine::{DownloadManagerConfig, MesioDownloaderFactory, ProtocolType};
use pipeline_common::CancellationToken;
use std::path::{Path, PathBuf};
use tracing::{Level, error, info, span};

/// Determine the type of input and process accordingly
pub async fn process_inputs(
    inputs: &[String],
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    token: &CancellationToken,
) -> Result<(), AppError> {
    if inputs.is_empty() {
        return Err(AppError::InvalidInput(
            "No input files or URLs provided".to_string(),
        ));
    }

    let inputs_len = inputs.len();

    // Create a span for overall processing
    let processing_span = span!(Level::INFO, "processing_inputs", count = inputs_len);
    let _enter = processing_span.enter();

    info!(
        inputs_count = inputs_len,
        "Starting processing of {} input{}",
        inputs_len,
        if inputs_len == 1 { "" } else { "s" }
    );

    let factory = MesioDownloaderFactory::new()
        .with_download_config(DownloadManagerConfig::default())
        .with_flv_config(config.flv_config.clone().unwrap_or_default())
        .with_hls_config(config.hls_config.clone().unwrap_or_default())
        .with_token(token.clone());

    // Process each input
    for (index, input) in inputs.iter().enumerate() {
        let input_index = index + 1;

        // trim urls for better usability
        let input = input.trim();

        // Create a span for this specific input
        let input_span = span!(Level::INFO, "process_input", index = input_index, input = %input);
        let _input_enter = input_span.enter();

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
                        &mut downloader,
                        token,
                    )
                    .await?;
                }
                ProtocolType::Hls => {
                    hls::process_hls_stream(
                        input,
                        output_dir,
                        config,
                        name_template,
                        &mut downloader,
                        token,
                    )
                    .await?;
                }
                _ => {
                    error!("Unsupported protocol for: {input}");
                    return Err(AppError::InvalidInput(format!(
                        "Unsupported protocol: {input}"
                    )));
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
                            flv::process_file(&path, output_dir, config, token).await?;
                        }
                        // "m3u8" | "m3u" => {
                        //     hls::process_hls_file(&path, output_dir, config, &progress_manager).await?;
                        // },
                        _ => {
                            error!("Unsupported file extension for: {input}");
                            return Err(AppError::InvalidInput(format!(
                                "Unsupported file extension: {input}"
                            )));
                        }
                    }
                } else {
                    error!("File without extension: {input}");
                    return Err(AppError::InvalidInput(format!(
                        "File without extension: {input}"
                    )));
                }
            } else {
                error!(
                    "Input is neither a valid URL nor an existing file: {}",
                    input
                );
                return Err(AppError::InvalidInput(format!("Invalid input: {input}")));
            }
        }
    }

    Ok(())
}
