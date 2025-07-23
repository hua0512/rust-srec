use clap::Parser;
use std::path::PathBuf;

/// Define CLI arguments
#[derive(Parser)]
#[command(
    author = "hua0512 <https://github.com/hua0512>",
    version,
    about = "Media stream processing and download tool",
    long_about = "A powerful tool for downloading, processing, and repairing media streams.\n\
                  Part of the stream-rec project: https://github.com/hua0512/rust-rec\n\
                  \n\
                  This tool supports multiple protocols (FLV, HLS) and can fix common issues\n\
                  such as timestamp anomalies, out-of-order frames, and metadata inconsistencies.\n\
                  It supports processing individual files, entire directories, or downloading\n\
                  directly from URLs with automatic protocol detection."
)]
pub struct CliArgs {
    /// Input file(s), directory, or URL(s) to process
    #[arg(
        required = true,
        help = "Path to file(s), directory containing media files, or URL(s) to download"
    )]
    pub input: Vec<String>,

    /// Output directory for processed files
    #[arg(
        short,
        long,
        help = "Directory where processed files will be saved (default: ./fix)"
    )]
    pub output_dir: Option<PathBuf>,

    /// Maximum file size with optional unit (B, KB, MB, GB, TB)
    /// Examples: "4GB", "500MB", "2048KB"
    #[arg(
        short,
        long,
        default_value = "0",
        help = "Maximum size for output files with optional unit (B, KB, MB, GB, TB). Examples: \"4GB\", \"500MB\". Use 0 for unlimited."
    )]
    pub max_size: String,

    /// Maximum duration with optional unit (s, m, h)
    /// Examples: "30m", "1.5h", "90s"
    #[arg(
        short = 'd',
        long,
        default_value = "0",
        help = "Maximum duration for output files with optional unit (s, m, h). Examples: \"30m\", \"1.5h\", \"90s\". Use 0 for unlimited."
    )]
    pub max_duration: String,

    /// Enable verbose logging
    #[arg(short, long, help = "Enable detailed debug logging")]
    pub verbose: bool,

    /// Enable keyframe index injection
    #[arg(
        short = 'k',
        long,
        default_value = "true",
        help = "Inject keyframe index in metadata for better seeking"
    )]
    pub keyframe_index: bool,

    /// Enable processing pipeline (disabled by default)
    #[arg(
        long = "fix",
        help = "Enable processing/fixing pipeline (by default streams are downloaded as raw data)"
    )]
    pub enable_fix: bool,

    /// Buffer size for processing channels
    #[arg(
        short = 'b',
        long,
        default_value = "16",
        help = "Buffer size for internal processing channels"
    )]
    pub buffer_size: usize,

    /// Download buffer size
    #[arg(
        long,
        default_value = "65536",
        help = "Buffer size for downloading in bytes"
    )]
    pub download_buffer: usize,

    /// Connection timeout in seconds
    #[arg(
        long,
        default_value = "0",
        help = "Overall timeout in seconds for HTTP requests"
    )]
    pub timeout: u64,

    /// Connection timeout in seconds
    #[arg(
        long,
        default_value = "30",
        help = "Connection timeout in seconds (time to establish initial connection)"
    )]
    pub connect_timeout: u64,

    /// Read timeout in seconds
    #[arg(
        long,
        default_value = "30",
        help = "Read timeout in seconds (maximum time between receiving data chunks)"
    )]
    pub read_timeout: u64,

    /// Write timeout in seconds
    #[arg(
        long,
        default_value = "30",
        help = "Write timeout in seconds (maximum time for sending request data)"
    )]
    pub write_timeout: u64,

    /// Proxy URL (e.g., "http://proxy.example.com:8080")
    #[arg(
        long,
        help = "Proxy server URL for downloads (e.g., \"http://proxy.example.com:8080\")"
    )]
    pub proxy: Option<String>,

    /// Proxy type (http, https, socks5, all)
    #[arg(
        long,
        default_value = "http",
        help = "Proxy type (http, https, socks5, all)",
        value_parser = ["http", "https", "socks5", "all"]
    )]
    pub proxy_type: String,

    /// Proxy username
    #[arg(long, help = "Username for proxy authentication")]
    pub proxy_user: Option<String>,

    /// Proxy password
    #[arg(long, help = "Password for proxy authentication")]
    pub proxy_pass: Option<String>,

    /// Use system proxy settings for downloads
    #[arg(
        long,
        default_value = "true",
        help = "Use system proxy settings for downloads if no explicit proxy is configured"
    )]
    pub use_system_proxy: bool,

    /// Output file name template with placeholders
    #[arg(
        short = 'n',
        long = "name",
        help = "Output file name template with placeholders (e.g., %u%Y%m%d_%H%M%S_p%i'). Supported placeholders: %u (unique identifier), %Y (year), %m (month), %d (day), %H (hour), %M (minute), %S (second), %i (output index)",
        default_value = "%u%Y%m%d_%H%M%S_p%i"
    )]
    pub output_name_template: String,

    /// Custom HTTP headers for download requests
    #[arg(
        long = "header",
        short = 'H',
        help = "Add custom HTTP header to requests (can be used multiple times). Format: 'Name: Value'",
        value_name = "HEADER"
    )]
    pub headers: Vec<String>,

    /// Show progress bars for operations
    #[arg(
        short = 'P',
        long = "progress",
        default_value = "false",
        help = "Show progress bars for download and processing operations"
    )]
    pub show_progress: bool,

    /// Disable all proxy settings for downloads
    #[arg(
        long,
        help = "Disable all proxy settings (including system proxy) for downloads"
    )]
    pub no_proxy: bool,

    /// Number of concurrent HLS segment downloads
    #[arg(
        long,
        default_value = "4",
        help = "Maximum number of concurrent segment downloads for HLS streams"
    )]
    pub hls_concurrency: u32,

    /// Segment retry attempts for HLS downloads
    #[arg(
        long,
        default_value = "3",
        help = "Number of retry attempts for failed HLS segment downloads"
    )]
    pub hls_retries: u32,

    /// HLS segment timeout in seconds
    #[arg(
        long,
        default_value = "30",
        help = "Timeout for individual HLS segment downloads in seconds"
    )]
    pub hls_segment_timeout: u64,

    /// Enable HLS playlist caching
    #[arg(
        long,
        default_value = "true",
        help = "Enable caching of HLS playlists to reduce redundant downloads"
    )]
    pub hls_cache_playlists: bool,

    /// Select output format (file, stdout, stderr)
    #[arg(
            long,
            default_value = "file",
            help = "Output format for downloaded content",
            value_parser = ["file", "stdout", "stderr"]
        )]
    pub output_format: String,
}
