use bytes::Bytes;
use futures::Stream;
use reqwest::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use rustls::{ClientConfig, crypto::ring};
use rustls_platform_verifier::BuilderVerifierExt;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncRead;
use tracing::{debug, info};

use crate::proxy::ProxyConfig;
use crate::{DownloadError, proxy::build_proxy_from_config};

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// Configurable options for the downloader
#[derive(Debug, Clone)]
pub struct DownloaderConfig {
    /// Buffer size for download chunks (in bytes)
    pub buffer_size: usize,

    /// Overall timeout for the entire HTTP request
    pub timeout: Duration,

    /// Connection timeout (time to establish initial connection)
    pub connect_timeout: Duration,

    /// Read timeout (maximum time between receiving data chunks)
    pub read_timeout: Duration,

    /// Write timeout (maximum time for sending request data)
    pub write_timeout: Duration,

    /// Whether to follow redirects
    pub follow_redirects: bool,

    /// User agent string
    pub user_agent: String,

    /// Custom HTTP headers for requests
    pub headers: HeaderMap,

    /// Proxy configuration (optional)
    pub proxy: Option<ProxyConfig>,

    /// Whether to use system proxy settings if available
    pub use_system_proxy: bool,

    /// Maximum concurrent segment downloads for HLS streams
    pub max_concurrent_hls_downloads: usize,
}

impl Default for DownloaderConfig {
    fn default() -> Self {
        Self {
            buffer_size: 64 * 1024, // 64 KB chunks
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            follow_redirects: true,
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            headers: DownloaderConfig::get_default_headers(),
            proxy: None,
            use_system_proxy: true, // Enable system proxy by default
            max_concurrent_hls_downloads: 4,
        }
    }
}

impl DownloaderConfig {
    pub fn with_config(config: DownloaderConfig) -> Self {
        let mut headers = DownloaderConfig::get_default_headers();

        if !config.headers.is_empty() {
            // If custom headers are provided, merge them with defaults
            // Custom headers take precedence over defaults for the same fields
            for (name, value) in config.headers.iter() {
                headers.insert(name.clone(), value.clone());
            }
        }

        Self {
            buffer_size: config.buffer_size,
            timeout: config.timeout,
            connect_timeout: config.connect_timeout,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
            follow_redirects: config.follow_redirects,
            user_agent: config.user_agent,
            headers,
            proxy: config.proxy,
            use_system_proxy: config.use_system_proxy,
            max_concurrent_hls_downloads: config.max_concurrent_hls_downloads, // Add here
        }
    }

    pub fn get_default_headers() -> HeaderMap {
        let mut default_headers = HeaderMap::new();

        // Add common headers for streaming content
        default_headers.insert(reqwest::header::ACCEPT, HeaderValue::from_static("*/*"));

        default_headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br"),
        );

        default_headers.insert(
            reqwest::header::CONNECTION,
            HeaderValue::from_static("keep-alive"),
        );

        default_headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );

        default_headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.5,zh-CN;q=0.3,zh;q=0.2"),
        );
        default_headers
    }
}

/// Create a reqwest Client with the provided configuration
pub fn create_client(config: &DownloaderConfig) -> Result<Client, DownloadError> {
    // Create the crypto provider
    let provider = Arc::new(ring::default_provider());

    // Build platform default TLS configuration
    let tls_config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("Failed to configure default TLS protocol versions")
        .with_platform_verifier()
        .with_no_client_auth();

    let mut client_builder = Client::builder()
        .pool_max_idle_per_host(5) // Allow multiple connections to same host
        .user_agent(&config.user_agent)
        .default_headers(config.headers.clone())
        .use_preconfigured_tls(tls_config)
        .redirect(if config.follow_redirects {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        });

    if !config.timeout.is_zero() {
        client_builder = client_builder.timeout(config.timeout);
    }

    if !config.connect_timeout.is_zero() {
        client_builder = client_builder.connect_timeout(config.connect_timeout);
    }

    if !config.read_timeout.is_zero() {
        client_builder = client_builder.pool_idle_timeout(config.read_timeout);
    }

    // Set up proxy configuration
    if let Some(proxy_config) = &config.proxy {
        // Explicit proxy configuration takes precedence
        let proxy = match build_proxy_from_config(proxy_config) {
            Ok(p) => p,
            Err(e) => return Err(DownloadError::ProxyError(e)),
        };
        client_builder = client_builder.proxy(proxy);
        info!(proxy_url = %proxy_config.url, "Using explicitly configured proxy for downloads");
    } else if config.use_system_proxy {
        // No explicit proxy but system proxy enabled
        // reqwest will use system proxy settings by default when we don't call no_proxy()
        info!("Using system proxy settings for downloads");
    } else {
        // Explicitly disable proxy
        client_builder = client_builder.no_proxy();
        debug!("Proxy disabled for downloads");
    }

    client_builder.build().map_err(DownloadError::from)
}

/// A reader adapter that wraps a bytes stream for AsyncRead compatibility
pub struct BytesStreamReader {
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    current_chunk: Option<Bytes>,
    position: usize,
}

impl BytesStreamReader {
    /// Create a new BytesStreamReader from a reqwest bytes stream
    pub fn new(stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self {
            stream: Box::pin(stream),
            current_chunk: None,
            position: 0,
        }
    }
}

impl AsyncRead for BytesStreamReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;

        loop {
            // If we have a chunk with data remaining, copy it to the buffer
            if let Some(chunk) = &self.current_chunk {
                if self.position < chunk.len() {
                    let bytes_to_copy = std::cmp::min(buf.remaining(), chunk.len() - self.position);
                    buf.put_slice(&chunk[self.position..self.position + bytes_to_copy]);
                    self.position += bytes_to_copy;
                    return Poll::Ready(Ok(()));
                }
                // We've consumed this chunk entirely
                self.current_chunk = None;
                self.position = 0;
            }

            // Need to get a new chunk from the stream
            match self.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    if chunk.is_empty() {
                        continue; // Skip empty chunks
                    }
                    self.current_chunk = Some(chunk);
                    self.position = 0;
                    // Continue the loop to process this chunk
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Download error: {}", e),
                    )));
                }
                Poll::Ready(None) => {
                    // End of stream
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}
