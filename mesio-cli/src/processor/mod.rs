mod flv;
mod generic;
mod hls;

use mesio_engine::{DownloadManagerConfig, MesioDownloaderFactory, ProtocolType};
use pipeline_common::progress::ProgressEvent;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{error, info};

use crate::{config::ProgramConfig, error::AppError};

/// Determine the type of input and process accordingly
pub async fn process_inputs<F>(
    inputs: &[String],
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    on_progress: Option<Arc<F>>,
) -> Result<(), AppError>
where
    F: Fn(ProgressEvent) + Send + Sync + 'static,
{
    if inputs.is_empty() {
        return Err(AppError::InvalidInput(
            "No input files or URLs provided".to_string(),
        ));
    }

    let inputs_len = inputs.len();
    info!(
        inputs_count = inputs_len,
        "Starting processing of {} input{}",
        inputs_len,
        if inputs_len == 1 { "" } else { "s" }
    );

    let factory = MesioDownloaderFactory::new()
        .with_download_config(DownloadManagerConfig::default())
        .with_flv_config(config.flv_config.clone().unwrap_or_default())
        .with_hls_config(config.hls_config.clone().unwrap_or_default());

    // Process each input
    for (index, input) in inputs.iter().enumerate() {
        let _input_index = index + 1;

        // trim urls for better usability
        let input = input.trim();
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
                        on_progress.clone(),
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
                        on_progress.clone(),
                        &mut downloader,
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
                            flv::process_file(&path, output_dir, config, on_progress.clone())
                                .await?;
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
