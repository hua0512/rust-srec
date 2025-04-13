use std::process::exit;
use std::{path::PathBuf, time::Duration};

use clap::Parser;
use flv_fix::operators::RepairStrategy;
use flv_fix::operators::script_filler::ScriptFillerConfig;
use flv_fix::pipeline::PipelineConfig;
use siphon::{DownloaderConfig, ProxyAuth, ProxyConfig, ProxyType};
use tracing::{Level, error, info};
use tracing_subscriber::FmtSubscriber;
mod cli;
mod processor;
mod utils;

use cli::CliArgs;
use utils::{format_bytes, format_duration, parse_size, parse_time};

#[tokio::main]
async fn main() {
    // Parse command-line arguments
    // Using parse() instead of try_parse() to let clap automatically handle --help
    let args = CliArgs::parse();

    // Setup logging
    let log_level = if args.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };
    let subscriber = FmtSubscriber::builder().with_max_level(log_level).finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    info!("FLV Processing Tool - Part of the stream-rec project by hua0512");
    info!("GitHub: https://github.com/hua0512/rust-srec");

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
    let config = PipelineConfig {
        duplicate_tag_filtering: false,
        file_size_limit,
        duration_limit,
        repair_strategy: RepairStrategy::Strict, // Fixed to Strict
        continuity_mode: flv_fix::operators::ContinuityMode::Reset, // Fixed to Reset
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
        channel_buffer_size: args.buffer_size,
    };

    // Determine output directory
    let output_dir = args.output_dir.unwrap_or_else(|| PathBuf::from("./fix"));

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

    // Create download configuration
    let download_config = DownloaderConfig::with_config(DownloaderConfig {
        buffer_size: args.download_buffer,
        timeout: Duration::from_secs(args.timeout),
        connect_timeout: Duration::from_secs(args.connect_timeout),
        read_timeout: Duration::from_secs(args.read_timeout),
        write_timeout: Duration::from_secs(args.write_timeout),
        follow_redirects: true,
        headers: crate::utils::parse_headers(&args.headers),
        proxy: proxy_config,
        use_system_proxy: args.use_system_proxy,
        ..DownloaderConfig::default()
    });

    // Process input files
    match processor::process_inputs(
        &args.input,
        &output_dir,
        config,
        download_config,
        args.enable_fix,
        args.output_name_template.as_deref(),
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
