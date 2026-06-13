mod flv;
mod generic;
mod hls;

use crate::{config::ProgramConfig, error::AppError};
use mesio_engine::{
    DownloadRequest, DownloaderSession, MesioConfig, MesioDownloader, ProtocolSelection,
};
use pipeline_common::CancellationToken;
use std::path::{Path, PathBuf};
use tracing::{Instrument, Level, error, info, span};

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
    processing_span.in_scope(|| {
        info!(
            inputs_count = inputs_len,
            "Starting processing of {} input{}",
            inputs_len,
            if inputs_len == 1 { "" } else { "s" }
        );
    });

    let downloader = MesioDownloader::new(MesioConfig {
        flv: config.flv_config.clone().unwrap_or_default(),
        hls: config.hls_config.clone().unwrap_or_default(),
        token: token.clone(),
    });

    // Process each input
    for (index, input) in inputs.iter().enumerate() {
        let input_index = index + 1;

        // trim urls for better usability
        let input = input.trim();

        // Create a span for this specific input
        let input_span = span!(Level::INFO, "process_input", index = input_index, input = %input);

        // Process based on input type
        if input.starts_with("http://") || input.starts_with("https://") {
            let request = DownloadRequest::from_url(input)?
                .with_protocol(ProtocolSelection::Auto)
                .with_cancel(token.clone());
            let session = downloader.start(request).await?;

            match session {
                DownloaderSession::Flv(session) => {
                    flv::process_flv_stream(
                        input,
                        output_dir,
                        config,
                        name_template,
                        session,
                        token,
                    )
                    .instrument(input_span.clone())
                    .await?;
                }
                DownloaderSession::Hls(session) => {
                    hls::process_hls_stream(
                        input,
                        output_dir,
                        config,
                        name_template,
                        session,
                        token,
                    )
                    .instrument(input_span.clone())
                    .await?;
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
                            flv::process_file(&path, output_dir, config, token)
                                .instrument(input_span.clone())
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
