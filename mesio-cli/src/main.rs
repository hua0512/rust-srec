use std::process::exit;
use std::str::FromStr;
use std::{path::PathBuf, sync::Arc, time::Duration};

use clap::Parser;
use config::ProgramConfig;
use flv_fix::PipelineConfig;
use flv_fix::RepairStrategy;
use flv_fix::ScriptFillerConfig;
use indicatif::MultiProgress;
use mesio_engine::flv::FlvConfig;
use mesio_engine::{DownloaderConfig, HlsProtocolBuilder, ProxyAuth, ProxyConfig, ProxyType};
use output::provider::OutputFormat;
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
use utils::{format_bytes, format_duration, parse_size, parse_time};

#[tokio::main]
async fn main() {
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
        .open("mesio.log")
        .unwrap_or_else(|e| {
            eprintln!(
                "Warning: Failed to create log file 'mesio.log': {e}. Logging to console only."
            );
            // Return a dummy writer that discards everything
            std::fs::File::create(std::env::temp_dir().join("mesio_dummy.log"))
                .expect("Failed to create temporary log file")
        });

    let multi_writer = MakeWriterExt::and(std::io::stdout, log_file);

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_writer(multi_writer)
        .with_ansi(true)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

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

    // Parse size and duration with units
    let file_size_limit = match parse_size(&args.max_size) {
        Ok(size) => size,
        Err(e) => {
            error!("Invalid size format '{}': {}", args.max_size, e);
            exit(1);
        }
    };

    let duration_limit = match parse_time(&args.max_duration) {
        Ok(duration) => duration,
        Err(e) => {
            error!("Invalid duration format '{}': {}", args.max_duration, e);
            exit(1);
        }
    };

    // Log the parsed values
    if file_size_limit > 0 {
        info!("File size limit set to {}", format_bytes(file_size_limit));
    } else {
        info!("No file size limit set");
    }

    if duration_limit > 0.0 {
        info!("Duration limit set to {}", format_duration(duration_limit));
    } else {
        info!("No duration limit set");
    }

    // Log HTTP timeout settings
    info!(
        "HTTP timeout configuration: overall={}s, connect={}s, read={}s, write={}s",
        args.timeout, args.connect_timeout, args.read_timeout, args.write_timeout
    );

    // Configure pipeline
    let pipeline_config = PipelineConfig {
        duplicate_tag_filtering: false,
        file_size_limit,
        duration_limit,
        repair_strategy: RepairStrategy::Strict, // Fixed to Strict
        continuity_mode: flv_fix::ContinuityMode::Reset, // Fixed to Reset
        keyframe_index_config: if args.keyframe_index {
            if duration_limit > 0.0 {
                info!("Keyframe index will be injected into metadata for better seeking");
                Some(ScriptFillerConfig {
                    keyframe_duration_ms: (duration_limit * 1000.0) as u32,
                })
            } else {
                info!("Keyframe index enabled with default configuration");
                Some(ScriptFillerConfig::default())
            }
        } else {
            None
        },
    };

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
                error!(
                    "Invalid proxy type: '{}'. Using 'http' as default.",
                    args.proxy_type
                );
                ProxyType::Http
            }
            _ => {
                error!(
                    "Invalid proxy type: '{}'. Using 'http' as default.",
                    args.proxy_type
                );
                ProxyType::Http
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
    let download_config = DownloaderConfig {
        // do not cache by default
        cache_config: None,
        timeout: Duration::from_secs(args.timeout),
        connect_timeout: Duration::from_secs(args.connect_timeout),
        read_timeout: Duration::from_secs(args.read_timeout),
        write_timeout: Duration::from_secs(args.write_timeout),
        follow_redirects: true,
        headers: crate::utils::parse_headers(&args.headers),
        proxy: proxy_config,
        use_system_proxy: args.use_system_proxy,
        ..DownloaderConfig::default()
    };

    // Create FLV-specific configuration
    let flv_config = FlvConfig {
        base: download_config.clone(),
        buffer_size: args.download_buffer,
    };

    // Create HLS-specific configuration
    let hls_config = HlsProtocolBuilder::new()
        .with_base_config(download_config.clone())
        .download_concurrency(args.hls_concurrency.try_into().unwrap())
        .segment_retry_count(args.hls_retries)
        .get_config();

    let output_format = OutputFormat::from_str(&args.output_format).unwrap_or_else(|_| {
        error!(
            "Invalid output format: '{}'. Defaulting to 'file'.",
            args.output_format
        );
        OutputFormat::File
    });

    // Create the program configuration
    let mut program_config = ProgramConfig {
        pipeline_config,
        flv_config: Some(flv_config),
        hls_config: Some(hls_config),
        enable_processing: args.enable_fix,
        channel_size: args.buffer_size,
        output_format: Some(output_format),
    };

    // Process input files
    match processor::process_inputs(
        &args.input,
        &output_dir,
        &mut program_config,
        &args.output_name_template,
        Some(Arc::new(move |event| {
            progress_manager.handle_event(event);
        })),
    )
    .await
    {
        Ok(_) => {
            info!("All processing completed");
        }
        Err(e) => {
            error!(error = ?e, "Processing failed");
            exit(1);
        }
    }
}
