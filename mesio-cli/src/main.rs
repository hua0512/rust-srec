use std::io::{self, IsTerminal};
use std::{path::PathBuf, time::Duration};

use clap::Parser;
use config::ProgramConfig;
use error::AppError;
use flv_fix::FlvPipelineConfig;
use flv_fix::ScriptFillerConfig;
use hls_fix::HlsPipelineConfig;
use mesio_engine::flv::FlvProtocolConfig;
use mesio_engine::{DownloaderConfig, HlsProtocolBuilder, ProxyAuth, ProxyConfig, ProxyType};
use output::provider::PipeStream;
use pipeline_common::{CancellationToken, config::PipelineConfig};
use tracing::{Level, error, info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_indicatif::IndicatifLayer;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod cli;
mod config;
mod cut;
mod error;
mod input;
mod output;
mod processor;
mod signal;
mod utils;

use cli::CliArgs;
use input::input_handler;
use utils::{parse_headers, parse_params, parse_size, parse_time};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let result = bootstrap();
    // The input thread may still hold the terminal in raw mode.
    if let Err(error) = input::restore_terminal() {
        error!(%error, "Failed to restore the terminal mode");
    }
    if let Err(e) = result {
        // Check if it's a broken pipe error - this is expected behavior
        // when the consumer closes the pipe (e.g., `mesio ... | head -c 1000`)
        if matches!(e, AppError::BrokenPipe) {
            // Exit code 141 is the standard for SIGPIPE (128 + 13)
            // This indicates the pipe was closed by the consumer, which is normal
            eprintln!("Pipe closed by consumer");
            std::process::exit(141);
        }

        if matches!(e, AppError::StreamRestarted(_)) {
            // EX_TEMPFAIL (sysexits.h): the recording ended normally, and
            // running again picks up the restarted stream.
            eprintln!("{e}");
            std::process::exit(75);
        }

        eprintln!("Error: {e}");
        // Log the full error for debugging
        error!(error = ?e, "Application failed");
        std::process::exit(1);
    }
}

/// Installs the global subscriber: console output, plus a daily log file
/// unless disabled. The returned guard flushes the log file when dropped.
fn init_logging(args: &CliArgs) -> Option<WorkerGuard> {
    let log_level = if args.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };
    let filter = tracing_subscriber::filter::LevelFilter::from_level(log_level);

    let (file_layer, file_guard) = if args.disable_log_file {
        (None, None)
    } else {
        let file_appender = tracing_appender::rolling::daily(".", "mesio.log");
        let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(non_blocking_file)
            .with_ansi(false);
        (Some(file_layer), Some(guard))
    };

    // Pipe output writes media to stdout or stderr. Progress bars and logs
    // must stay off that stream: progress bars are shown only for file
    // output, and logs go to the other stream.
    let pipe_stream = PipeStream::of(args.output_format);
    let indicatif_layer = (args.show_progress && pipe_stream.is_none())
        .then(|| IndicatifLayer::new().with_max_progress_bars(8, None));
    let (console_writer, console_ansi) = match (&indicatif_layer, pipe_stream) {
        // Logs are drawn with the progress bars, on stderr.
        (Some(indicatif), _) => (
            BoxMakeWriter::new(indicatif.get_stderr_writer()),
            io::stderr().is_terminal(),
        ),
        // Media is on stdout.
        (None, Some(PipeStream::Stdout)) => {
            (BoxMakeWriter::new(io::stderr), io::stderr().is_terminal())
        }
        // File output without progress bars, or media on stderr.
        (None, _) => (BoxMakeWriter::new(io::stdout), io::stdout().is_terminal()),
    };
    let console_layer = tracing_subscriber::fmt::layer()
        .with_writer(console_writer)
        .with_ansi(console_ansi)
        .without_time()
        .with_target(true)
        .compact();

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .with(indicatif_layer)
        .init();
    file_guard
}

#[tokio::main]
async fn bootstrap() -> Result<(), AppError> {
    // Parse command-line arguments
    let args = CliArgs::parse();

    // Create a cancellation token
    let token = CancellationToken::new();
    // Where `s`, a line of `s` on stdin and `SIGUSR1` request a cut.
    let (cut_target, cut_notices) = cut::CutTarget::new();

    // A thread of its own rather than a runtime task: it blocks, and runtime
    // shutdown would wait for it. The process exits without joining it.
    let input_thread = std::thread::Builder::new()
        .name("mesio-input".to_owned())
        .spawn({
            let token = token.clone();
            let cut_target = cut_target.clone();
            move || input_handler(token, cut_target)
        });

    let _log_file_guard = init_logging(&args);

    if let Err(error) = input_thread {
        warn!(%error, "Failed to start the input handler; `q` will not stop mesio");
    }

    if let Err(error) = cut_notices {
        warn!(%error, "Failed to start the cut notice thread; cut requests will not be acknowledged");
    }

    if let Err(error) = signal::spawn(token.clone(), cut_target.clone()) {
        warn!(
            %error,
            "Failed to install signal handlers; a signal will stop mesio without finishing the current file"
        );
    }

    info!("███╗   ███╗███████╗███████╗██╗ ██████╗ ");
    info!("████╗ ████║██╔════╝██╔════╝██║██╔═══██╗");
    info!("██╔████╔██║█████╗  ███████╗██║██║   ██║");
    info!("██║╚██╔╝██║██╔══╝  ╚════██║██║██║   ██║");
    info!("██║ ╚═╝ ██║███████╗███████║██║╚██████╔╝");
    info!("╚═╝     ╚═╝╚══════╝╚══════╝╚═╝ ╚═════╝ ");
    info!("");
    info!("Media Streaming Downloader");
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

    // Check if we're in pipe mode (stdout/stderr output)
    let is_pipe_mode = PipeStream::of(args.output_format).is_some();

    // Configure flv pipeline config
    let flv_pipeline_config = FlvPipelineConfig::builder()
        .duplicate_tag_filtering(false)
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
        .pipe_mode(is_pipe_mode)
        .build();

    // Configure HLS pipeline config
    let hls_pipeline_config = HlsPipelineConfig::builder().build();

    // Determine output directory
    let output_dir = args
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("./fix"));

    // Handle proxy configuration
    let (proxy_config, use_system_proxy) = if args.no_proxy {
        // No proxy flag overrides everything else
        info!("All proxy settings disabled (--no-proxy flag)");
        (None, false)
    } else if let Some(proxy_url) = args.proxy.as_ref() {
        // Explicit proxy configuration
        // Parse proxy type
        let proxy_type: ProxyType = args.proxy_type;

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
        use mesio_engine::config::HttpVersionPreference;

        let mut builder = DownloaderConfig::builder()
            .with_timeout(Duration::from_secs(args.timeout))
            .with_connect_timeout(Duration::from_secs(args.connect_timeout))
            .with_read_timeout(Duration::from_secs(args.read_timeout))
            .with_write_timeout(Duration::from_secs(args.write_timeout))
            .with_headers(parse_headers(&args.headers))
            .with_params(parse_params(&args.params)?)
            .with_caching_enabled(false)
            .with_force_ipv4(args.force_ipv4)
            .with_force_ipv6(args.force_ipv6);

        // Configure HTTP version preference
        let http_version = match args.http_version.as_str() {
            "http1" => HttpVersionPreference::Http1Only,
            "http2" => HttpVersionPreference::Http2Only,
            _ => HttpVersionPreference::Auto,
        };
        builder = builder
            .with_http_version(http_version)
            .with_http2_keep_alive_interval(Duration::from_secs(args.http2_keepalive));

        if let Some(proxy) = proxy_config {
            builder = builder.with_proxy(proxy);
        } else {
            builder = builder.with_system_proxy(use_system_proxy);
        }
        builder.build()
    };

    // Create FLV-specific configuration
    let flv_config = FlvProtocolConfig::builder()
        .with_base_config(download_config.clone())
        .buffer_size(args.download_buffer)
        .build();

    // DASH requests share the HTTP settings of FLV and HLS.
    let dash_config = args.dash_config(download_config.clone())?;

    // Create HLS-specific configuration
    let hls_config = HlsProtocolBuilder::new()
        .with_base_config(download_config)
        .download_concurrency(
            args.hls_concurrency
                .try_into()
                .map_err(|_| AppError::InvalidInput("Invalid HLS concurrency".to_string()))?,
        )
        .initial_playlist_fetch_timeout(Duration::from_secs(args.hls_playlist_fetch_timeout))
        .live_refresh_interval(Duration::from_secs(args.hls_playlist_min_refresh_interval))
        .live_max_refresh_retries(args.hls_playlist_retries)
        .max_segment_retries(args.hls_retries)
        .segment_download_timeout(Duration::from_secs(args.hls_segment_timeout))
        .get_config();

    // Create the program configuration
    let program_config = ProgramConfig::builder()
        .pipeline_config(pipeline_config)
        .flv_pipeline_config(flv_pipeline_config)
        .hls_pipeline_config(hls_pipeline_config)
        .flv_config(flv_config)
        .hls_config(hls_config)
        .dash_config(dash_config)
        .protocol(args.protocol.into())
        .enable_processing(args.enable_fix)
        .output_format(args.output_format)
        .build()
        .map_err(|err| AppError::InvalidInput(err.to_string()))?;

    // Log output format
    info!("Output format: {}", args.output_format);

    // Process input files
    let result = processor::process_inputs(
        &args.input,
        &output_dir,
        &program_config,
        &args.output_name_template,
        &token,
        &cut_target,
    )
    .await;

    let final_result = if token.is_cancelled() {
        info!("Operation cancelled by user. Exiting gracefully.");
        Ok(())
    } else {
        result
    };

    // Give a moment for any background spans to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    final_result
}
