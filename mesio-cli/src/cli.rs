use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use mesio_engine::dash::{AudioSelection, LiveStartPolicy, VideoSelection};
use mesio_engine::{
    DashConfig, DashProtocolBuilder, DownloaderConfig, ProtocolSelection, ProxyType,
};

use crate::error::AppError;
use crate::output::provider::OutputFormat;

/// Define CLI arguments
#[derive(Parser)]
#[command(
    author = "hua0512 <https://github.com/hua0512>",
    version,
    about = "Media stream processing and download tool",
    long_about = "A powerful tool for downloading, processing, and repairing media streams.\n\
                  Part of the stream-rec project: https://github.com/hua0512/rust-rec\n\
                  \n\
                  This tool supports multiple protocols (FLV, HLS, MPEG-DASH) and can fix common\n\
                  issues such as timestamp anomalies, out-of-order frames, and metadata\n\
                  inconsistencies. It supports processing individual files, entire directories,\n\
                  or downloading directly from URLs with automatic protocol detection."
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

    /// Output format for processed data
    #[arg(
        short = 'O',
        long = "output-format",
        default_value = "file",
        value_enum,
        help = "Output format: 'file' for files, 'stdout' or 'stderr' to pipe the media to that stream"
    )]
    pub output_format: OutputFormat,

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

    /// Disable writing logs to mesio.log
    #[arg(
        long = "disable-log-file",
        alias = "disable_log_file",
        help = "Disable writing logs to mesio.log"
    )]
    pub disable_log_file: bool,

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

    /// Channel size for processing channels
    #[arg(
        short = 'b',
        long,
        default_value = "64",
        help = "Channel size for internal processing channels"
    )]
    pub channel_size: usize,

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

    /// Proxy type
    #[arg(long, default_value = "http", help = "Proxy type", value_enum)]
    pub proxy_type: ProxyType,

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

    /// Custom parameters for download requests
    #[arg(
        long = "param",
        short = 'p',
        help = "Add custom parameter to requests (can be used multiple times). Format: 'Name=Value'",
        value_name = "PARAM"
    )]
    pub params: Vec<String>,

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
        default_value = "3",
        help = "Maximum number of concurrent segment downloads for HLS streams"
    )]
    pub hls_concurrency: u32,

    /// HLS playlist fetch timeout in seconds
    #[arg(
        long,
        default_value = "15",
        help = "Timeout for fetching the initial HLS playlist in seconds"
    )]
    pub hls_playlist_fetch_timeout: u64,

    /// HLS playlist minimum refresh interval in seconds
    #[arg(
        long,
        default_value = "1",
        help = "Minimum interval for refreshing HLS playlists in seconds"
    )]
    pub hls_playlist_min_refresh_interval: u64,

    /// HLS playlist retry attempts
    #[arg(
        long,
        default_value = "5",
        help = "Number of retry attempts for failed HLS playlist downloads"
    )]
    pub hls_playlist_retries: u32,

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
        default_value = "10",
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

    /// Protocol of URL inputs
    #[arg(
        long,
        default_value = "auto",
        value_enum,
        help = "Protocol of URL inputs: 'auto' detects it from the URL (.flv, .m3u8, .mpd)"
    )]
    pub protocol: ProtocolArg,

    /// DASH video Representation policy
    #[arg(
        long,
        default_value = "highest",
        value_name = "POLICY",
        value_parser = parse_video_quality,
        help = "DASH video quality: 'highest', 'lowest', 'max-height=N' (highest bandwidth at most N pixels tall) or 'bandwidth=N' (closest to N bits/s)"
    )]
    pub dash_video_quality: VideoSelection,

    /// DASH audio Representation policy
    #[arg(
        long,
        default_value = "highest",
        value_name = "POLICY",
        value_parser = parse_audio_quality,
        help = "DASH audio quality within the chosen language: 'highest', 'lowest' or 'bandwidth=N' (closest to N bits/s)"
    )]
    pub dash_audio_quality: AudioSelection,

    /// Preferred DASH audio language
    #[arg(
        long,
        value_name = "LANG",
        value_parser = parse_language_tag,
        help = "Preferred DASH audio language as a language tag (e.g. 'en', 'pt-BR'); any language is used when none matches"
    )]
    pub dash_audio_lang: Option<String>,

    /// Skip separate DASH audio tracks; audio carried inside the video
    /// Representation is kept.
    #[arg(
        long,
        conflicts_with = "dash_no_video",
        help = "Do not record separate audio tracks of DASH streams (audio inside the video track is kept)"
    )]
    pub dash_no_audio: bool,

    /// Record DASH audio only
    #[arg(long, help = "Do not record the video track of DASH streams")]
    pub dash_no_video: bool,

    /// Number of concurrent DASH segment downloads per track
    #[arg(
        long,
        default_value = "5",
        value_parser = clap::value_parser!(u32).range(1..),
        help = "Maximum number of concurrent segment downloads per DASH track"
    )]
    pub dash_concurrency: u32,

    /// DASH manifest fetch timeout in seconds
    #[arg(
        long,
        default_value = "15",
        value_parser = clap::value_parser!(u64).range(1..),
        help = "Timeout for each DASH manifest (MPD) or segment index request in seconds"
    )]
    pub dash_manifest_timeout: u64,

    /// DASH manifest minimum refresh interval in seconds
    #[arg(
        long,
        default_value = "1",
        value_parser = clap::value_parser!(u64).range(1..),
        help = "Minimum interval between refreshes of a live DASH manifest in seconds; the MPD's minimumUpdatePeriod applies when longer"
    )]
    pub dash_min_refresh_interval: u64,

    /// DASH manifest refresh retry attempts
    #[arg(
        long,
        default_value = "5",
        help = "Number of consecutive failed DASH manifest refreshes tolerated before the stream fails; also the retries of a failed segment index request"
    )]
    pub dash_manifest_retries: u32,

    /// DASH segment timeout in seconds
    #[arg(
        long,
        default_value = "10",
        value_parser = clap::value_parser!(u64).range(1..),
        help = "Timeout for individual DASH segment downloads in seconds"
    )]
    pub dash_segment_timeout: u64,

    /// Segment retry attempts for DASH downloads
    #[arg(
        long,
        default_value = "3",
        help = "Number of retry attempts for failed DASH segment downloads"
    )]
    pub dash_retries: u32,

    /// Where recording of a live DASH stream starts
    #[arg(
        long,
        default_value = "auto",
        value_name = "POLICY",
        value_parser = parse_live_start,
        help = "Where recording of a live DASH stream starts: 'auto' (the MPD's suggested presentation delay, else three segments behind the live edge), 'edge' (newest segment) or 'delay=SECONDS' behind the live edge"
    )]
    pub dash_live_start: LiveStartPolicy,

    /// Force IPv4
    #[arg(
        short = '4',
        long = "ipv4",
        help = "Force IPv4 for downloads",
        default_value = "false"
    )]
    pub force_ipv4: bool,

    /// Force IPv6
    #[arg(
        short = '6',
        long = "ipv6",
        help = "Force IPv6 for downloads",
        default_value = "false"
    )]
    pub force_ipv6: bool,

    /// HTTP version preference
    #[arg(
        long = "http-version",
        help = "HTTP version preference: 'auto' (default, prefers HTTP/2), 'http2' (prefer HTTP/2), 'http1' (force HTTP/1.1 only)",
        default_value = "auto",
        value_parser = ["auto", "http2", "http1"]
    )]
    pub http_version: String,

    /// TCP keep-alive interval for HTTP/2 connections (seconds)
    #[arg(
        long = "http2-keepalive",
        help = "TCP keep-alive interval in seconds for maintaining HTTP/2 connections (default: 20)",
        default_value = "20"
    )]
    pub http2_keepalive: u64,
}

/// Protocol of URL inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum ProtocolArg {
    /// Detect from the URL
    #[default]
    Auto,
    /// HTTP-FLV
    Flv,
    /// HLS playlist
    Hls,
    /// MPEG-DASH manifest (MPD)
    Dash,
}

impl From<ProtocolArg> for ProtocolSelection {
    fn from(protocol: ProtocolArg) -> Self {
        match protocol {
            ProtocolArg::Auto => ProtocolSelection::Auto,
            ProtocolArg::Flv => ProtocolSelection::Flv(Default::default()),
            ProtocolArg::Hls => ProtocolSelection::Hls(Default::default()),
            ProtocolArg::Dash => ProtocolSelection::Dash(Default::default()),
        }
    }
}

impl CliArgs {
    /// DASH engine configuration from the `--dash-*` options over the shared
    /// HTTP settings in `base`.
    pub fn dash_config(&self, base: DownloaderConfig) -> Result<DashConfig, AppError> {
        let concurrency = usize::try_from(self.dash_concurrency)
            .map_err(|_| AppError::InvalidInput("Invalid DASH concurrency".to_string()))?;
        let mut builder = DashProtocolBuilder::new()
            .with_base_config(base)
            .video_selection(self.dash_video_quality)
            .audio_selection(self.dash_audio_quality)
            .include_video(!self.dash_no_video)
            .include_audio(!self.dash_no_audio)
            .download_concurrency(concurrency)
            .manifest_fetch_timeout(Duration::from_secs(self.dash_manifest_timeout))
            .min_refresh_interval(Duration::from_secs(self.dash_min_refresh_interval))
            .max_refresh_retries(self.dash_manifest_retries)
            .segment_download_timeout(Duration::from_secs(self.dash_segment_timeout))
            .max_segment_retries(self.dash_retries)
            .live_start(self.dash_live_start);
        if let Some(language) = &self.dash_audio_lang {
            builder = builder.audio_language(language.clone());
        }
        let config = builder.get_config();
        config
            .validate()
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        Ok(config)
    }
}

/// Parses `--dash-video-quality`.
fn parse_video_quality(value: &str) -> Result<VideoSelection, String> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "highest" => return Ok(VideoSelection::Highest),
        "lowest" => return Ok(VideoSelection::Lowest),
        _ => {}
    }
    if let Some(height) = value.strip_prefix("max-height=") {
        return parse_positive(height, "max-height").map(VideoSelection::MaxHeight);
    }
    if let Some(bandwidth) = value.strip_prefix("bandwidth=") {
        return parse_positive(bandwidth, "bandwidth").map(VideoSelection::ClosestBandwidth);
    }
    Err(format!(
        "expected 'highest', 'lowest', 'max-height=N' or 'bandwidth=N', got '{value}'"
    ))
}

/// Parses `--dash-audio-quality`.
fn parse_audio_quality(value: &str) -> Result<AudioSelection, String> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "highest" => return Ok(AudioSelection::Highest),
        "lowest" => return Ok(AudioSelection::Lowest),
        _ => {}
    }
    if let Some(bandwidth) = value.strip_prefix("bandwidth=") {
        return parse_positive(bandwidth, "bandwidth").map(AudioSelection::ClosestBandwidth);
    }
    Err(format!(
        "expected 'highest', 'lowest' or 'bandwidth=N', got '{value}'"
    ))
}

/// Parses `--dash-live-start`.
fn parse_live_start(value: &str) -> Result<LiveStartPolicy, String> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "auto" => return Ok(LiveStartPolicy::PresentationDelay(None)),
        "edge" => return Ok(LiveStartPolicy::LiveEdge),
        _ => {}
    }
    if let Some(seconds) = value.strip_prefix("delay=") {
        let delay = seconds
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
            .ok_or_else(|| {
                format!("delay must be a non-negative number of seconds, got '{seconds}'")
            })?;
        return Ok(LiveStartPolicy::PresentationDelay(Some(delay)));
    }
    Err(format!(
        "expected 'auto', 'edge' or 'delay=SECONDS', got '{value}'"
    ))
}

/// Parses `--dash-audio-lang` as a language tag shape: subtags of 1 to 8
/// ASCII letters or digits separated by '-'. Matching against the MPD is
/// case-insensitive, so the case is kept as given.
fn parse_language_tag(value: &str) -> Result<String, String> {
    let tag = value.trim();
    let valid = !tag.is_empty()
        && tag.split('-').all(|subtag| {
            (1..=8).contains(&subtag.len()) && subtag.chars().all(|c| c.is_ascii_alphanumeric())
        });
    if valid {
        Ok(tag.to_string())
    } else {
        Err(format!(
            "expected a language tag such as 'en' or 'pt-BR', got '{value}'"
        ))
    }
}

fn parse_positive<T>(value: &str, name: &str) -> Result<T, String>
where
    T: std::str::FromStr + PartialEq + Default,
{
    match value.trim().parse::<T>() {
        Ok(number) if number != T::default() => Ok(number),
        _ => Err(format!("{name} must be a positive integer, got '{value}'")),
    }
}

#[cfg(test)]
mod tests {
    use mesio_engine::{ProxyConfig, ProxyType};
    use reqwest::header::{HeaderMap, HeaderValue, REFERER};

    use super::*;

    fn parse(extra: &[&str]) -> Result<CliArgs, clap::Error> {
        let mut argv = vec!["mesio"];
        argv.extend_from_slice(extra);
        argv.push("https://example.com/live.mpd");
        CliArgs::try_parse_from(argv)
    }

    fn dash_config(extra: &[&str]) -> DashConfig {
        parse(extra)
            .unwrap()
            .dash_config(DownloaderConfig::default())
            .unwrap()
    }

    #[test]
    fn command_definition_is_valid() {
        use clap::CommandFactory;
        CliArgs::command().debug_assert();
    }

    #[test]
    fn dash_defaults_match_the_engine() {
        let config = dash_config(&[]);
        let engine = DashConfig::default();
        assert_eq!(config.selection.video, engine.selection.video);
        assert_eq!(config.selection.audio, engine.selection.audio);
        assert_eq!(config.selection.audio_language, None);
        assert!(config.selection.include_video);
        assert!(config.selection.include_audio);
        assert_eq!(
            config.scheduler.download_concurrency,
            engine.scheduler.download_concurrency
        );
        assert_eq!(config.manifest.fetch_timeout, engine.manifest.fetch_timeout);
        assert_eq!(
            config.manifest.min_refresh_interval,
            engine.manifest.min_refresh_interval
        );
        assert_eq!(
            config.manifest.max_refresh_retries,
            engine.manifest.max_refresh_retries
        );
        assert_eq!(
            config.fetcher.segment_download_timeout,
            engine.fetcher.segment_download_timeout
        );
        assert_eq!(
            config.fetcher.max_segment_retries,
            engine.fetcher.max_segment_retries
        );
        assert_eq!(config.live_start, engine.live_start);
    }

    #[test]
    fn dash_flags_map_to_the_engine_config() {
        let config = dash_config(&[
            "--dash-video-quality",
            "max-height=720",
            "--dash-audio-quality",
            "bandwidth=128000",
            "--dash-audio-lang",
            "pt-BR",
            "--dash-no-video",
            "--dash-concurrency",
            "2",
            "--dash-manifest-timeout",
            "7",
            "--dash-min-refresh-interval",
            "3",
            "--dash-manifest-retries",
            "9",
            "--dash-segment-timeout",
            "20",
            "--dash-retries",
            "4",
            "--dash-live-start",
            "delay=12.5",
        ]);
        assert_eq!(config.selection.video, VideoSelection::MaxHeight(720));
        assert_eq!(
            config.selection.audio,
            AudioSelection::ClosestBandwidth(128_000)
        );
        assert_eq!(config.selection.audio_language.as_deref(), Some("pt-BR"));
        assert!(!config.selection.include_video);
        assert!(config.selection.include_audio);
        assert_eq!(config.scheduler.download_concurrency, 2);
        assert_eq!(config.manifest.fetch_timeout, Duration::from_secs(7));
        assert_eq!(config.manifest.min_refresh_interval, Duration::from_secs(3));
        assert_eq!(config.manifest.max_refresh_retries, 9);
        assert_eq!(
            config.fetcher.segment_download_timeout,
            Duration::from_secs(20)
        );
        assert_eq!(config.fetcher.max_segment_retries, 4);
        assert_eq!(
            config.live_start,
            LiveStartPolicy::PresentationDelay(Some(Duration::from_millis(12_500)))
        );

        assert!(!dash_config(&["--dash-no-audio"]).selection.include_audio);
    }

    #[test]
    fn dash_requests_use_the_shared_http_settings() {
        let mut headers = HeaderMap::new();
        headers.insert(REFERER, HeaderValue::from_static("https://example.com/"));
        let base = DownloaderConfig::builder()
            .with_timeout(Duration::from_secs(40))
            .with_connect_timeout(Duration::from_secs(5))
            .with_read_timeout(Duration::from_secs(6))
            .with_write_timeout(Duration::from_secs(8))
            .with_headers(headers.clone())
            .with_params(vec![("token".to_string(), "abc".to_string())])
            .with_force_ipv4(true)
            .with_proxy(ProxyConfig {
                url: "socks5://127.0.0.1:1080".to_string(),
                proxy_type: ProxyType::Socks5,
                auth: None,
            })
            .build();

        let config = parse(&[]).unwrap().dash_config(base.clone()).unwrap();
        assert_eq!(config.base.timeout, base.timeout);
        assert_eq!(config.base.connect_timeout, base.connect_timeout);
        assert_eq!(config.base.read_timeout, base.read_timeout);
        assert_eq!(config.base.write_timeout, base.write_timeout);
        assert_eq!(config.base.headers, headers);
        assert_eq!(config.base.params, base.params);
        assert_eq!(config.base.user_agent, base.user_agent);
        assert!(config.base.force_ipv4);
        let proxy = config.base.proxy.expect("proxy is kept");
        assert_eq!(proxy.url, "socks5://127.0.0.1:1080");
        assert_eq!(proxy.proxy_type, ProxyType::Socks5);
    }

    #[test]
    fn video_quality_accepts_each_policy() {
        assert_eq!(parse_video_quality("highest"), Ok(VideoSelection::Highest));
        assert_eq!(parse_video_quality("LOWEST"), Ok(VideoSelection::Lowest));
        assert_eq!(
            parse_video_quality("max-height=1080"),
            Ok(VideoSelection::MaxHeight(1080))
        );
        assert_eq!(
            parse_video_quality("bandwidth=2500000"),
            Ok(VideoSelection::ClosestBandwidth(2_500_000))
        );
        for invalid in [
            "best",
            "max-height=0",
            "max-height=-1",
            "max-height=",
            "bandwidth=0",
            "bandwidth=fast",
        ] {
            assert!(parse_video_quality(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn audio_quality_accepts_each_policy() {
        assert_eq!(parse_audio_quality("highest"), Ok(AudioSelection::Highest));
        assert_eq!(parse_audio_quality("lowest"), Ok(AudioSelection::Lowest));
        assert_eq!(
            parse_audio_quality("bandwidth=96000"),
            Ok(AudioSelection::ClosestBandwidth(96_000))
        );
        for invalid in ["max-height=720", "bandwidth=0", ""] {
            assert!(parse_audio_quality(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn live_start_accepts_each_policy() {
        assert_eq!(
            parse_live_start("auto"),
            Ok(LiveStartPolicy::PresentationDelay(None))
        );
        assert_eq!(parse_live_start("edge"), Ok(LiveStartPolicy::LiveEdge));
        assert_eq!(
            parse_live_start("delay=30"),
            Ok(LiveStartPolicy::PresentationDelay(Some(
                Duration::from_secs(30)
            )))
        );
        for invalid in ["delay=-1", "delay=", "delay=NaN", "start"] {
            assert!(parse_live_start(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn audio_language_must_look_like_a_language_tag() {
        assert_eq!(parse_language_tag("en"), Ok("en".to_string()));
        assert_eq!(
            parse_language_tag(" zh-Hans-CN "),
            Ok("zh-Hans-CN".to_string())
        );
        for invalid in ["", "en-", "-en", "en_US", "english language", "abcdefghi"] {
            assert!(parse_language_tag(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn invalid_dash_flags_are_rejected() {
        for invalid in [
            &["--dash-no-audio", "--dash-no-video"][..],
            &["--dash-concurrency", "0"],
            &["--dash-manifest-timeout", "0"],
            &["--dash-min-refresh-interval", "0"],
            &["--dash-segment-timeout", "0"],
            &["--dash-video-quality", "best"],
            &["--dash-live-start", "delay=x"],
            &["--protocol", "rtmp"],
        ] {
            assert!(parse(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn protocol_flag_maps_to_the_protocol_selection() {
        let selection = |value: &str| -> ProtocolSelection {
            parse(&["--protocol", value]).unwrap().protocol.into()
        };
        assert!(matches!(
            ProtocolSelection::from(parse(&[]).unwrap().protocol),
            ProtocolSelection::Auto
        ));
        assert!(matches!(selection("auto"), ProtocolSelection::Auto));
        assert!(matches!(selection("flv"), ProtocolSelection::Flv(_)));
        assert!(matches!(selection("hls"), ProtocolSelection::Hls(_)));
        assert!(matches!(selection("dash"), ProtocolSelection::Dash(_)));
    }
}
