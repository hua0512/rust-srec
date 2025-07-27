use std::{path::PathBuf, sync::Arc, time::Duration};

use clap::Parser;
use config::ProgramConfig;
use error::AppError;
use flv_fix::FlvPipelineConfig;
use flv_fix::RepairStrategy;
use flv_fix::ScriptFillerConfig;
use hls_fix::HlsPipelineConfig;
use indicatif::MultiProgress;
use mesio_engine::flv::FlvConfig;
use mesio_engine::{DownloaderConfig, HlsProtocolBuilder, ProxyAuth, ProxyConfig, ProxyType};
use pipeline_common::config::PipelineConfig;
use tracing::{Level, error, info};
use tracing_subscriber::FmtSubscriber;
use tracing_subscriber::fmt::writer::MakeWriterExt;

mod cli;
mod config;
mod error;
mod output;
mod processor;
mod utils;

use cli::CliArgs;
use utils::progress::ProgressManager;
use utils::{parse_size, parse_time};

fn main() {
    if let Err(e) = bootstrap() {
        eprintln!("Error: {e}");
        // Log the full error for debugging
        error!(error = ?e, "Application failed");
        std::process::exit(1);
    }
}

#[tokio::main]
async fn bootstrap() -> Result<(), AppError> {
    // Parse command-line arguments
    let args = CliArgs::parse();

    // Setup logging
    let log_level = if args.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("mesio.log")?;

    let multi_writer = MakeWriterExt::and(std::io::stdout, log_file);

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_writer(multi_writer)
        .with_ansi(true)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| AppError::Initialization(e.to_string()))?;

    info!("███╗   ███╗███████╗███████╗██╗ ██████╗ ");
    info!("████╗ ████║██╔════╝██╔════╝██║██╔═══██╗");
    info!("██╔████╔██║█████╗  ███████╗██║██║   ██║");
    info!("██║╚██╔╝██║██╔══╝  ╚════██║██║██║   ██║");
    info!("██║ ╚═╝ ██║███████╗███████║██║╚██████╔╝");
    info!("╚═╝     ╚═╝╚══════╝╚══════╝╚═╝ ╚═════╝ ");
    info!("");
    info!("Media Streaming Downloader - Part of the rust-srec project by hua0512");
    info!("GitHub: https://github.com/hua0512/rust-srec");
    info!("==================================================================");

    // Max size in bytes
    let file_size_limit = parse_size(&args.max_size)?;

    // Max duration in seconds
    let duration_limit_s = parse_time(&args.max_duration)?;

    let pipeline_config = PipelineConfig::builder()
        .max_file_size(file_size_limit)
        .max_duration_s(duration_limit_s)
        .channel_size(args.channel_size)
        .build();

    info!("{pipeline_config}");

    // Log HTTP timeout settings
    info!(
        "HTTP timeout configuration: overall={}s, connect={}s, read={}s, write={}s",
        args.timeout, args.connect_timeout, args.read_timeout, args.write_timeout
    );

    // Configure flv pipeline config
    let flv_pipeline_config = FlvPipelineConfig::builder()
        .duplicate_tag_filtering(false)
        .repair_strategy(RepairStrategy::Strict)
        .continuity_mode(flv_fix::ContinuityMode::Reset)
        .keyframe_index_config(if args.keyframe_index {
            if duration_limit_s > 0.0 {
                info!("Keyframe index will be injected into metadata for better seeking");
                Some(ScriptFillerConfig {
                    keyframe_duration_ms: (duration_limit_s * 1000.0) as u32,
                })
            } else {
                info!("Keyframe index enabled with default configuration");
                Some(ScriptFillerConfig::default())
            }
        } else {
            None
        })
        .build();

    // Configure HLS pipeline config
    let hls_pipeline_config = HlsPipelineConfig::builder().build();

    // Determine output directory
    let output_dir = args.output_dir.unwrap_or_else(|| PathBuf::from("./fix"));

    // Create a progress manager based on show_progress flag
    let multi = MultiProgress::new();
    let progress_manager = if args.show_progress {
        ProgressManager::new(multi.clone())
    } else {
        ProgressManager::new_disabled(multi.clone())
    };

    // Handle proxy configuration
    let (proxy_config, _use_system_proxy) = if args.no_proxy {
        // No proxy flag overrides everything else
        info!("All proxy settings disabled (--no-proxy flag)");
        (None, false)
    } else if let Some(proxy_url) = args.proxy.as_ref() {
        // Explicit proxy configuration
        // Parse proxy type
        let proxy_type = match args.proxy_type.as_str() {
            "http" => ProxyType::Http,
            "https" => ProxyType::Https,
            "socks5" => ProxyType::Socks5,
            "all" => {
                return Err(AppError::InvalidInput(format!(
                    "Invalid proxy type: '{}'",
                    args.proxy_type
                )));
            }
            _ => {
                return Err(AppError::InvalidInput(format!(
                    "Invalid proxy type: '{}'",
                    args.proxy_type
                )));
            }
        };

        // Configure proxy authentication if both username and password are provided
        let auth = if let (Some(username), Some(password)) = (&args.proxy_user, &args.proxy_pass) {
            Some(ProxyAuth {
                username: username.clone(),
                password: password.clone(),
            })
        } else {
            None
        };

        info!(
            proxy_url = %proxy_url,
            proxy_type = ?proxy_type,
            has_auth = auth.is_some(),
            "Using explicit proxy configuration for downloads"
        );

        // Create the proxy configuration
        let proxy = ProxyConfig {
            url: proxy_url.clone(),
            proxy_type,
            auth,
        };

        (Some(proxy), false) // Don't use system proxy when explicit proxy is configured
    } else if args.use_system_proxy {
        // Use system proxy settings
        info!("Using system proxy settings for downloads");
        (None, true)
    } else {
        // No proxy settings at all
        info!("No proxy settings configured for downloads");
        (None, false)
    };

    // Create common download configuration
    let download_config = {
        let mut builder = DownloaderConfig::builder()
            .with_timeout(Duration::from_secs(args.timeout))
            .with_connect_timeout(Duration::from_secs(args.connect_timeout))
            .with_read_timeout(Duration::from_secs(args.read_timeout))
            .with_write_timeout(Duration::from_secs(args.write_timeout))
            .with_headers(crate::utils::parse_headers(&args.headers))
            .with_caching_enabled(false);

        builder = match (args.force_ipv4, args.force_ipv6) {
            (true, false) => builder.with_force_ipv4(),
            (false, true) => builder.with_force_ipv6(),
            _ => builder,
        };

        if let Some(proxy) = proxy_config {
            builder = builder.with_proxy(proxy);
        } else {
            builder = builder.with_system_proxy(args.use_system_proxy);
        }
        builder.build()
    };

    // Create FLV-specific configuration
    let flv_config = FlvConfig::builder()
        .with_base_config(download_config.clone())
        .buffer_size(args.download_buffer)
        .build();

    // Create HLS-specific configuration
    let hls_config = HlsProtocolBuilder::new()
        .with_base_config(download_config)
        .download_concurrency(
            args.hls_concurrency
                .try_into()
                .map_err(|_| AppError::InvalidInput("Invalid HLS concurrency".to_string()))?,
        )
        .segment_retry_count(args.hls_retries)
        .get_config();

    // Create the program configuration
    let program_config = ProgramConfig::builder()
        .pipeline_config(pipeline_config)
        .flv_pipeline_config(flv_pipeline_config)
        .hls_pipeline_config(hls_pipeline_config)
        .flv_config(flv_config)
        .hls_config(hls_config)
        .enable_processing(args.enable_fix)
        .build()
        .map_err(|err| AppError::InvalidInput(err.to_string()))?;

    // Process input files
    processor::process_inputs(
        &args.input,
        &output_dir,
        &program_config,
        &args.output_name_template,
        Some(Arc::new(move |event| {
            progress_manager.handle_event(event);
        })),
    )
    .await?;
    Ok(())
}
