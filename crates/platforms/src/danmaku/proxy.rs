//! Tunnels danmu connections through a proxy.
//!
//! The WebSocket client has no proxy support, so the tunnel is opened here and
//! the WebSocket handshake, including its TLS, runs inside it.

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use base64::prelude::*;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use url::{Host, Url};

use crate::danmaku::error::{DanmakuError, Result};
use crate::extractor::create_client_builder;
use crate::proxy::{ProxyEndpoint, redacted_url};

/// Bound on reaching the proxy and having it open the tunnel.
const TUNNEL_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest CONNECT response accepted, headers included.
const MAX_CONNECT_RESPONSE: usize = 8 * 1024;
const SOCKS_DEFAULT_PORT: u16 = 1080;

pub(crate) trait Transport: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Transport for T {}

/// The byte stream a WebSocket runs over: a direct TCP connection or a
/// tunnel through a proxy.
pub(crate) type BoxedTransport = Box<dyn Transport>;

/// A proxy danmu connections go through: an `http`, `https`, `socks5` or
/// `socks5h` endpoint, and the hosts that bypass it. `Debug` never shows the
/// login.
#[derive(Clone, PartialEq, Eq)]
pub struct DanmuProxy {
    endpoint: ProxyEndpoint,
    /// The endpoint's address.
    url: Url,
    /// A `NO_PROXY`-style list of hosts reached directly.
    no_proxy: Option<String>,
}

#[derive(Clone, Copy)]
enum Kind {
    Http,
    Https,
    Socks5 { remote_dns: bool },
}

impl DanmuProxy {
    /// Rejects proxies the tunnel cannot use, so a bad proxy never turns into
    /// a direct connection.
    pub fn new(endpoint: ProxyEndpoint) -> Result<Self> {
        let invalid = |reason: &str| DanmakuError::connection(format!("danmu proxy {reason}"));
        let url = Url::parse(endpoint.url.trim()).map_err(|_| invalid("is not a valid URL"))?;
        if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") {
            return Err(invalid("must use http, https, socks5 or socks5h"));
        }
        if url.host_str().is_none_or(str::is_empty) {
            return Err(invalid("must name a host"));
        }
        Ok(Self {
            endpoint,
            url,
            no_proxy: None,
        })
    }

    /// A proxy from a URL that may carry a percent-encoded login, as proxy
    /// environment variables do.
    pub fn parse(url: &str) -> Result<Self> {
        Self::new(ProxyEndpoint::from_url(url))
    }

    /// Hosts matching this `NO_PROXY`-style list are reached directly: comma
    /// separated domains (matching subdomains too), IP addresses, CIDR
    /// ranges, or `*` for every host. A blank list bypasses nothing.
    pub fn with_no_proxy(mut self, no_proxy: Option<String>) -> Self {
        self.no_proxy = no_proxy
            .map(|list| list.trim().to_owned())
            .filter(|list| !list.is_empty());
        self
    }

    /// Whether connections to `host` skip the proxy.
    pub fn bypasses(&self, host: &str) -> bool {
        self.no_proxy
            .as_deref()
            .is_some_and(|list| no_proxy_matches(list, host))
    }

    fn kind(&self) -> Kind {
        match self.url.scheme() {
            "http" => Kind::Http,
            "https" => Kind::Https,
            "socks5" => Kind::Socks5 { remote_dns: false },
            _ => Kind::Socks5 { remote_dns: true },
        }
    }

    /// The client for requests a protocol makes before connecting, so they
    /// leave through the same proxy as the WebSocket.
    pub(crate) fn http_client(&self) -> Result<reqwest::Client> {
        // Errors may quote the URL, so they are not passed on.
        let proxy = self
            .endpoint
            .reqwest_proxy()
            .map_err(|_| DanmakuError::connection("danmu proxy is not supported"))?
            .no_proxy(
                self.no_proxy
                    .as_deref()
                    .and_then(reqwest::NoProxy::from_string),
            );
        create_client_builder(None)
            .proxy(proxy)
            .build()
            .map_err(|_| DanmakuError::connection("could not build the danmu proxy's HTTP client"))
    }

    /// Opens a tunnel to `host:port` through the proxy, or a direct
    /// connection when the host bypasses it.
    pub(crate) async fn tunnel(&self, host: &str, port: u16) -> Result<BoxedTransport> {
        if self.bypasses(host) {
            return tokio::time::timeout(TUNNEL_TIMEOUT, connect_direct(host, port))
                .await
                .map_err(|_| DanmakuError::connection("danmu host did not answer in time"))?;
        }
        tokio::time::timeout(TUNNEL_TIMEOUT, self.open(host, port))
            .await
            .map_err(|_| DanmakuError::connection("danmu proxy did not open a tunnel in time"))?
    }

    async fn open(&self, host: &str, port: u16) -> Result<BoxedTransport> {
        let (proxy_host, proxy_port) = endpoint_host(&self.url)?;
        let proxy_port = proxy_port.unwrap_or(SOCKS_DEFAULT_PORT);
        let mut stream = TcpStream::connect((proxy_host.as_str(), proxy_port))
            .await
            .map_err(|error| DanmakuError::connection(format!("reach danmu proxy: {error}")))?;
        stream.set_nodelay(true)?;
        let login = self.endpoint.login();
        match self.kind() {
            Kind::Http => {
                http_connect(&mut stream, host, port, login).await?;
                Ok(Box::new(stream))
            }
            Kind::Https => {
                let name = rustls::pki_types::ServerName::try_from(proxy_host)
                    .map_err(|_| DanmakuError::connection("danmu proxy host is not a TLS name"))?;
                let mut stream =
                    tokio_rustls::TlsConnector::from(crate::danmaku::websocket::rustls_config())
                        .connect(name, stream)
                        .await
                        .map_err(|error| {
                            DanmakuError::connection(format!("TLS to danmu proxy: {error}"))
                        })?;
                http_connect(&mut stream, host, port, login).await?;
                Ok(Box::new(stream))
            }
            Kind::Socks5 { remote_dns } => {
                socks5_connect(&mut stream, host, port, login, remote_dns).await?;
                Ok(Box::new(stream))
            }
        }
    }
}

impl fmt::Debug for DanmuProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DanmuProxy({}", redacted_url(self.url.as_str()))?;
        if let Some(no_proxy) = &self.no_proxy {
            write!(f, ", no_proxy: {no_proxy:?}")?;
        }
        f.write_str(")")
    }
}

/// Whether `host` matches the `NO_PROXY`-style `list`, as curl and reqwest
/// read it.
fn no_proxy_matches(list: &str, host: &str) -> bool {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    let address = host.parse::<IpAddr>().ok();
    list.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            if entry == "*" {
                return true;
            }
            if let Some(address) = address {
                return entry_matches_address(entry, address);
            }
            let domain = strip_entry_port(entry)
                .trim_start_matches("*.")
                .trim_start_matches('.')
                .trim_end_matches('.')
                .to_ascii_lowercase();
            !domain.is_empty()
                && (host == domain
                    || host
                        .strip_suffix(domain.as_str())
                        .is_some_and(|rest| rest.ends_with('.')))
        })
}

/// An IP or CIDR entry matched against an address.
fn entry_matches_address(entry: &str, address: IpAddr) -> bool {
    if let Some((network, prefix)) = entry.split_once('/') {
        let (Ok(network), Ok(prefix)) = (network.trim().parse::<IpAddr>(), prefix.parse::<u8>())
        else {
            return false;
        };
        return match (network, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) if prefix <= 32 => {
                let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
                u32::from(network) & mask == u32::from(address) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(address)) if prefix <= 128 => {
                let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
                u128::from(network) & mask == u128::from(address) & mask
            }
            _ => false,
        };
    }
    let entry = entry
        .strip_prefix('[')
        .and_then(|inner| inner.split_once(']').map(|(address, _)| address))
        .unwrap_or_else(|| strip_entry_port(entry));
    entry.parse::<IpAddr>().is_ok_and(|entry| entry == address)
}

/// `host:port` without the port. A bare IPv6 address has several colons and
/// is kept whole.
fn strip_entry_port(entry: &str) -> &str {
    match entry.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => {
            host
        }
        _ => entry,
    }
}

async fn connect_direct(host: &str, port: u16) -> Result<BoxedTransport> {
    let stream = TcpStream::connect((host, port)).await?;
    stream.set_nodelay(true)?;
    Ok(Box::new(stream))
}

/// The host to connect to for `url`, without IPv6 brackets, and the port its
/// scheme implies.
fn endpoint_host(url: &Url) -> Result<(String, Option<u16>)> {
    let host = match url.host() {
        Some(Host::Domain(domain)) => domain.to_owned(),
        Some(Host::Ipv4(address)) => address.to_string(),
        Some(Host::Ipv6(address)) => address.to_string(),
        None => return Err(DanmakuError::connection("URL has no host")),
    };
    Ok((host, url.port_or_known_default()))
}

/// The host and port a WebSocket URL connects to.
pub(crate) fn endpoint(url: &Url) -> Result<(String, u16)> {
    let (host, port) = endpoint_host(url)?;
    let port = port.ok_or_else(|| DanmakuError::connection("WebSocket URL has no port"))?;
    Ok((host, port))
}

/// Opens the byte stream for a WebSocket at `host:port`.
pub(crate) async fn open_transport(
    host: &str,
    port: u16,
    proxy: Option<&DanmuProxy>,
) -> Result<BoxedTransport> {
    match proxy {
        Some(proxy) => proxy.tunnel(host, port).await,
        None => connect_direct(host, port).await,
    }
}

async fn http_connect<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    host: &str,
    port: u16,
    login: Option<(&str, &str)>,
) -> Result<()> {
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some((username, password)) = login {
        let token = BASE64_STANDARD.encode(format!("{username}:{password}"));
        request.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    // One byte at a time: anything after the blank line belongs to the tunnel.
    let mut response = Vec::with_capacity(128);
    while !response.ends_with(b"\r\n\r\n") {
        if response.len() >= MAX_CONNECT_RESPONSE {
            return Err(DanmakuError::connection(
                "danmu proxy sent an oversized CONNECT response",
            ));
        }
        response.push(stream.read_u8().await?);
    }
    let status = std::str::from_utf8(&response)
        .ok()
        .and_then(|response| response.lines().next())
        .filter(|line| line.starts_with("HTTP/1."))
        .and_then(|line| line.split_whitespace().nth(1));
    match status {
        Some(code) if code.len() == 3 && code.starts_with('2') => Ok(()),
        Some("407") => Err(DanmakuError::connection("danmu proxy rejected its login")),
        Some(code) => Err(DanmakuError::connection(format!(
            "danmu proxy refused the tunnel (HTTP {code})"
        ))),
        None => Err(DanmakuError::connection(
            "danmu proxy sent an invalid CONNECT response",
        )),
    }
}

async fn socks5_connect<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    host: &str,
    port: u16,
    login: Option<(&str, &str)>,
    remote_dns: bool,
) -> Result<()> {
    let failed = |reason: &str| DanmakuError::connection(format!("SOCKS5 proxy {reason}"));

    let greeting: &[u8] = if login.is_some() {
        &[0x05, 0x02, 0x00, 0x02]
    } else {
        &[0x05, 0x01, 0x00]
    };
    stream.write_all(greeting).await?;
    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice).await?;
    if choice[0] != 0x05 {
        return Err(failed("sent an invalid reply"));
    }
    match (choice[1], login) {
        (0x00, _) => {}
        (0x02, Some((username, password))) => {
            let (Ok(username_len), Ok(password_len)) =
                (u8::try_from(username.len()), u8::try_from(password.len()))
            else {
                return Err(failed("login is longer than 255 bytes"));
            };
            let mut auth = Vec::with_capacity(3 + username.len() + password.len());
            auth.extend_from_slice(&[0x01, username_len]);
            auth.extend_from_slice(username.as_bytes());
            auth.push(password_len);
            auth.extend_from_slice(password.as_bytes());
            stream.write_all(&auth).await?;
            let mut status = [0u8; 2];
            stream.read_exact(&mut status).await?;
            if status[1] != 0x00 {
                return Err(failed("rejected its login"));
            }
        }
        _ => return Err(failed("accepts none of the offered login methods")),
    }

    let mut request = vec![0x05, 0x01, 0x00];
    let address = match host.parse::<IpAddr>() {
        Ok(address) => Some(address),
        // `socks5` resolves the name here; `socks5h` leaves it to the proxy.
        Err(_) if !remote_dns => Some(
            tokio::net::lookup_host((host, port))
                .await?
                .next()
                .ok_or_else(|| failed("target did not resolve"))?
                .ip(),
        ),
        Err(_) => None,
    };
    match address {
        Some(IpAddr::V4(address)) => {
            request.push(0x01);
            request.extend_from_slice(&address.octets());
        }
        Some(IpAddr::V6(address)) => {
            request.push(0x04);
            request.extend_from_slice(&address.octets());
        }
        None => {
            let length = u8::try_from(host.len()).map_err(|_| failed("target name is too long"))?;
            request.extend_from_slice(&[0x03, length]);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;

    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await?;
    if reply[0] != 0x05 {
        return Err(failed("sent an invalid reply"));
    }
    if reply[1] != 0x00 {
        return Err(DanmakuError::connection(format!(
            "SOCKS5 proxy could not connect (reply {})",
            reply[1]
        )));
    }
    let bound_address = match reply[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => usize::from(stream.read_u8().await?),
        _ => return Err(failed("sent an invalid reply")),
    };
    let mut rest = vec![0u8; bound_address + 2];
    stream.read_exact(&mut rest).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt;
    use tokio::net::TcpListener;

    #[test]
    fn only_tunnelable_proxies_parse_and_none_shows_its_login() {
        for url in [
            "http://proxy.example:8080",
            "https://proxy.example",
            "socks5://127.0.0.1:1080",
            "socks5h://user:p%40ss@proxy.example",
        ] {
            assert!(DanmuProxy::parse(url).is_ok(), "{url}");
        }
        for url in [
            "",
            "proxy.example:8080",
            "socks4://proxy.example",
            "file:///tmp",
        ] {
            assert!(DanmuProxy::parse(url).is_err(), "{url}");
        }
        let proxy = DanmuProxy::parse("socks5h://user:p%40ss@proxy.example:1080").unwrap();
        assert_eq!(proxy.endpoint.login(), Some(("user", "p@ss")));
        let shown = format!("{proxy:?}");
        assert_eq!(shown, "DanmuProxy(socks5h://proxy.example:1080)");
    }

    #[tokio::test]
    async fn set_up_requests_send_the_login_as_written() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let address = crate::proxy::login_checking_proxy("us@r:p%41 @:x/%").await;
            let proxy = DanmuProxy::new(ProxyEndpoint::new(
                format!("http://{address}"),
                Some(("us@r".to_owned(), "p%41 @:x/%".to_owned())),
            ))
            .unwrap();
            let status = proxy
                .http_client()
                .unwrap()
                .get("http://danmu.example/")
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, reqwest::StatusCode::NO_CONTENT);
        })
        .await
        .unwrap();
    }

    /// Accepts one CONNECT, answers with `status`, then echoes the tunnel.
    async fn http_proxy(status: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = tokio::io::BufReader::new(stream);
            let mut head = String::new();
            loop {
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            stream
                .get_mut()
                .write_all(format!("HTTP/1.1 {status}\r\nVia: test\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut tunnelled = [0u8; 4];
            if stream.read_exact(&mut tunnelled).await.is_ok() {
                stream.get_mut().write_all(&tunnelled).await.unwrap();
            }
            head
        });
        (port, task)
    }

    #[tokio::test]
    async fn an_http_proxy_opens_a_tunnel_with_its_login() {
        let (port, proxy_task) = http_proxy("200 Connection established").await;
        let proxy = DanmuProxy::parse(&format!("http://user:p%40ss@127.0.0.1:{port}")).unwrap();
        let mut tunnel = proxy.tunnel("danmu.example", 443).await.unwrap();
        tunnel.write_all(b"ping").await.unwrap();
        let mut echoed = [0u8; 4];
        tunnel.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"ping");
        let head = proxy_task.await.unwrap();
        assert!(head.starts_with("CONNECT danmu.example:443 HTTP/1.1\r\n"));
        assert!(head.contains("Host: danmu.example:443\r\n"));
        let token = BASE64_STANDARD.encode("user:p@ss");
        assert!(head.contains(&format!("Proxy-Authorization: Basic {token}\r\n")));
    }

    #[test]
    fn no_proxy_entries_match_like_curl() {
        let cases: &[(&str, &str, bool)] = &[
            ("example.com", "example.com", true),
            ("example.com", "danmu.example.com", true),
            ("example.com", "EXAMPLE.com", true),
            ("example.com", "badexample.com", false),
            (".example.com", "example.com", true),
            (".example.com", "a.b.example.com", true),
            ("*.example.com", "a.example.com", true),
            ("example.com:443", "example.com", true),
            ("other.org, example.com", "x.example.com", true),
            ("*", "anything.example", true),
            ("other.org", "example.com", false),
            ("", "example.com", false),
            ("127.0.0.1", "127.0.0.1", true),
            ("127.0.0.1:8080", "127.0.0.1", true),
            ("127.0.0.1", "127.0.0.2", false),
            ("10.0.0.0/8", "10.1.2.3", true),
            ("10.0.0.0/8", "11.0.0.1", false),
            ("192.168.1.0/24", "192.168.1.200", true),
            ("0.0.0.0/0", "8.8.8.8", true),
            ("::1", "[::1]", true),
            ("[::1]:443", "::1", true),
            ("fd00::/8", "[fd12::1]", true),
            ("fd00::/8", "fe80::1", false),
            ("10.0.0.0/8", "fd00::1", false),
            ("example.com", "10.0.0.1", false),
        ];
        for (list, host, expected) in cases {
            assert_eq!(
                no_proxy_matches(list, host),
                *expected,
                "{list:?} vs {host:?}"
            );
        }
        let proxy = DanmuProxy::parse("http://proxy.example:8080").unwrap();
        assert!(!proxy.bypasses("example.com"));
        let blank = proxy.clone().with_no_proxy(Some("  ".into()));
        assert_eq!(blank, proxy);
        let listed = proxy.clone().with_no_proxy(Some(" example.com ".into()));
        assert_ne!(listed, proxy);
        assert!(listed.bypasses("live.example.com"));
        assert!(format!("{listed:?}").contains("example.com"));
    }

    #[tokio::test]
    async fn a_bypassed_host_connects_directly() {
        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = proxy_listener.local_addr().unwrap().port();
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target.local_addr().unwrap().port();
        let echo = tokio::spawn(async move {
            let (mut stream, _) = target.accept().await.unwrap();
            let mut received = [0u8; 4];
            stream.read_exact(&mut received).await.unwrap();
            stream.write_all(&received).await.unwrap();
        });
        let proxy = DanmuProxy::parse(&format!("http://127.0.0.1:{proxy_port}"))
            .unwrap()
            .with_no_proxy(Some("localhost, 127.0.0.0/8".into()));
        let mut stream = proxy.tunnel("127.0.0.1", target_port).await.unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut echoed = [0u8; 4];
        stream.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"ping");
        echo.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), proxy_listener.accept())
                .await
                .is_err(),
            "the proxy must not be contacted"
        );
    }

    #[tokio::test]
    async fn a_refused_tunnel_is_an_error() {
        let (port, _proxy_task) = http_proxy("407 Proxy Authentication Required").await;
        let proxy = DanmuProxy::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let error = proxy.tunnel("danmu.example", 443).await.err().unwrap();
        assert!(error.to_string().contains("rejected its login"), "{error}");
    }

    /// Plays one SOCKS5 exchange that requires a login and records the
    /// CONNECT request it received, then echoes the tunnel.
    async fn socks_proxy() -> (u16, tokio::task::JoinHandle<(Vec<u8>, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut greeting = [0u8; 4];
            stream.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting, [0x05, 0x02, 0x00, 0x02]);
            stream.write_all(&[0x05, 0x02]).await.unwrap();
            let mut auth = vec![0u8; 2];
            stream.read_exact(&mut auth).await.unwrap();
            let mut username = vec![0u8; usize::from(auth[1])];
            stream.read_exact(&mut username).await.unwrap();
            let mut password = vec![0u8; usize::from(stream.read_u8().await.unwrap())];
            stream.read_exact(&mut password).await.unwrap();
            stream.write_all(&[0x01, 0x00]).await.unwrap();
            let mut request = vec![0u8; 5];
            stream.read_exact(&mut request).await.unwrap();
            let mut rest = vec![0u8; usize::from(request[4]) + 2];
            stream.read_exact(&mut rest).await.unwrap();
            request.extend(rest);
            stream
                .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0x1f, 0x90])
                .await
                .unwrap();
            let mut tunnelled = [0u8; 4];
            stream.read_exact(&mut tunnelled).await.unwrap();
            stream.write_all(&tunnelled).await.unwrap();
            let mut login = username;
            login.push(b':');
            login.extend(password);
            (login, request)
        });
        (port, task)
    }

    #[tokio::test]
    async fn a_socks5h_proxy_resolves_the_target_itself() {
        let (port, proxy_task) = socks_proxy().await;
        let proxy = DanmuProxy::parse(&format!("socks5h://user:secret@127.0.0.1:{port}")).unwrap();
        let mut tunnel = proxy.tunnel("danmu.example", 443).await.unwrap();
        tunnel.write_all(b"ping").await.unwrap();
        let mut echoed = [0u8; 4];
        tunnel.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"ping");
        let (login, request) = proxy_task.await.unwrap();
        assert_eq!(login, b"user:secret");
        let mut expected = vec![0x05, 0x01, 0x00, 0x03, 13];
        expected.extend_from_slice(b"danmu.example");
        expected.extend_from_slice(&443u16.to_be_bytes());
        assert_eq!(request, expected);
    }
}
