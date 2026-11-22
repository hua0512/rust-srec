use std::fmt;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(
    name = "strev",
    about = "Strev (Streev) - CLI tool for streaming media extraction and retrieval from various platforms",
    version,
    author
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Commands,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Suppress all output except errors
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Configuration file path
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    /// Request timeout in seconds
    #[arg(long, global = true, default_value = "30")]
    pub timeout: u64,

    /// Number of retry attempts
    #[arg(long, global = true, default_value = "3")]
    pub retries: u32,

    /// Proxy URL (supports http, https, socks5)
    #[arg(long, global = true)]
    pub proxy: Option<String>,

    /// Proxy username (if proxy requires authentication)
    #[arg(long, global = true)]
    pub proxy_username: Option<String>,

    /// Proxy password (if proxy requires authentication)
    #[arg(long, global = true)]
    pub proxy_password: Option<String>,
}

impl Args {
    pub fn output_format(&self) -> Option<OutputFormat> {
        match &self.command {
            Commands::Extract { output, .. } => Some(*output),
            Commands::Batch { output_format, .. } => Some(*output_format),
            Commands::Resolve { output, .. } => Some(*output),
            _ => None,
        }
    }
}

impl fmt::Debug for Args {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_proxy = self
            .proxy
            .as_deref()
            .map(platforms_parser::proxy::redacted_url);
        f.debug_struct("Args")
            .field("command", &self.command)
            .field("verbose", &self.verbose)
            .field("quiet", &self.quiet)
            .field("config", &self.config)
            .field("timeout", &self.timeout)
            .field("retries", &self.retries)
            .field("proxy", &redacted_proxy)
            .field(
                "proxy_username",
                &self.proxy_username.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "proxy_password",
                &self.proxy_password.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Extract media information from a URL
    Extract {
        /// The URL of the media to parse
        #[arg(short, long)]
        url: String,

        /// The cookies to use for the request
        #[arg(long)]
        cookies: Option<String>,

        /// The extras to use for the request (JSON string)
        #[arg(long)]
        extras: Option<String>,

        /// Output format
        #[arg(short, long, default_value = "pretty")]
        output: OutputFormat,

        /// Save output to file
        #[arg(short = 'O', long)]
        output_file: Option<PathBuf>,

        /// Filter streams by quality (e.g., "1080p", "720p")
        #[arg(long)]
        quality: Option<String>,

        /// Filter streams by format (e.g., "mp4", "flv")
        #[arg(long)]
        format: Option<String>,

        /// Auto-select best quality stream without prompt
        #[arg(long)]
        auto_select: bool,
    },

    /// Process multiple URLs from a file
    Batch {
        /// Input file containing URLs (one per line)
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory for results
        #[arg(short, long)]
        output_dir: Option<PathBuf>,

        /// Output format
        #[arg(short = 'f', long, default_value = "json")]
        output_format: OutputFormat,

        /// Maximum concurrent extractions
        #[arg(long, default_value = "5")]
        max_concurrent: usize,

        /// Continue on errors
        #[arg(long)]
        continue_on_error: bool,
    },

    /// List supported platforms
    Platforms {
        /// Show detailed information about each platform
        #[arg(short, long)]
        detailed: bool,
    },

    /// Generate shell completions
    Completions {
        /// The shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    /// Show configuration information
    Config {
        /// Show current configuration
        #[arg(short, long)]
        show: bool,

        /// Reset configuration to defaults
        #[arg(long)]
        reset: bool,
    },
    /// Resolve a stream URL from a payload
    Resolve {
        /// The URL of the media to parse
        #[arg(short, long)]
        url: String,
        /// The cookies to use for the request
        #[arg(long)]
        cookies: Option<String>,

        /// The extras to use for the request (JSON string)
        #[arg(long)]
        extras: Option<String>,
        /// The payload to use for resolving the stream (JSON string)
        #[arg(long)]
        payload: Option<String>,

        /// Output format
        #[arg(short, long, default_value = "pretty")]
        output: OutputFormat,

        /// Save output to file
        #[arg(short = 'O', long)]
        output_file: Option<PathBuf>,
    },
}

impl fmt::Debug for Commands {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Commands::Extract {
                url,
                cookies,
                extras,
                output,
                output_file,
                quality,
                format,
                auto_select,
            } => f
                .debug_struct("Extract")
                .field("url", &platforms_parser::media::redact_url_for_display(url))
                .field("cookies", &cookies.as_ref().map(|_| "[redacted]"))
                .field("extras", &extras.as_ref().map(|_| "[redacted]"))
                .field("output", output)
                .field("output_file", output_file)
                .field("quality", quality)
                .field("format", format)
                .field("auto_select", auto_select)
                .finish(),
            Commands::Batch {
                input,
                output_dir,
                output_format,
                max_concurrent,
                continue_on_error,
            } => f
                .debug_struct("Batch")
                .field("input", input)
                .field("output_dir", output_dir)
                .field("output_format", output_format)
                .field("max_concurrent", max_concurrent)
                .field("continue_on_error", continue_on_error)
                .finish(),
            Commands::Platforms { detailed } => f
                .debug_struct("Platforms")
                .field("detailed", detailed)
                .finish(),
            Commands::Completions { shell } => {
                f.debug_struct("Completions").field("shell", shell).finish()
            }
            Commands::Config { show, reset } => f
                .debug_struct("Config")
                .field("show", show)
                .field("reset", reset)
                .finish(),
            Commands::Resolve {
                url,
                cookies,
                extras,
                payload,
                output,
                output_file,
            } => f
                .debug_struct("Resolve")
                .field("url", &platforms_parser::media::redact_url_for_display(url))
                .field("cookies", &cookies.as_ref().map(|_| "[redacted]"))
                .field("extras", &extras.as_ref().map(|_| "[redacted]"))
                .field("payload", &payload.as_ref().map(|_| "[redacted]"))
                .field("output", output)
                .field("output_file", output_file)
                .finish(),
        }
    }
}

#[derive(ValueEnum, Copy, Clone, Debug, Default, Serialize, Deserialize)]
pub enum OutputFormat {
    /// Pretty-printed human-readable output
    #[default]
    Pretty,
    /// JSON output
    Json,
    /// Compact JSON output
    JsonCompact,
    /// Table format
    Table,
    /// CSV format
    Csv,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Pretty => write!(f, "pretty"),
            OutputFormat::Json => write!(f, "json"),
            OutputFormat::JsonCompact => write!(f, "json-compact"),
            OutputFormat::Table => write!(f, "table"),
            OutputFormat::Csv => write!(f, "csv"),
        }
    }
}
