//! Checking that a proxy reaches a site.

use std::error::Error as _;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::{ProxyEndpoint, ProxyTarget};

/// Bound on one check, connection included.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a check failed. Error text is not returned: it can quote addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeErrorKind {
    /// The proxy refused the login (HTTP 407).
    ProxyAuthenticationRequired,
    /// The proxy, or the site through it, could not be reached.
    ConnectFailed,
    /// No answer within the time limit.
    Timeout,
    /// The TLS handshake with the site failed.
    Tls,
    /// Any other failure.
    Failed,
}

/// The result of one check.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeOutcome {
    /// The site answered through the proxy. Any HTTP status except 407
    /// counts: the check is about the connection, not the page.
    pub ok: bool,
    pub status: Option<u16>,
    pub latency_ms: u64,
    pub error: Option<ProbeErrorKind>,
}

/// The home page a platform check requests.
pub fn platform_homepage(platform: &str) -> Option<&'static str> {
    Some(match platform.to_ascii_lowercase().as_str() {
        "acfun" => "https://live.acfun.cn",
        "bigo" => "https://www.bigo.tv",
        "bilibili" => "https://live.bilibili.com",
        "douyin" => "https://live.douyin.com",
        "douyu" => "https://www.douyu.com",
        "huya" => "https://www.huya.com",
        "pandatv" => "https://www.pandalive.co.kr",
        "picarto" => "https://picarto.tv",
        "redbook" => "https://www.xiaohongshu.com",
        "soop" => "https://www.sooplive.co.kr",
        "tiktok" => "https://www.tiktok.com",
        "twitcasting" => "https://twitcasting.tv",
        "twitch" => "https://www.twitch.tv",
        "weibo" => "https://weibo.com",
        _ => return None,
    })
}

/// Requests `target` through `endpoint` once, without following redirects.
pub async fn probe(endpoint: &ProxyEndpoint, target: &url::Url) -> ProbeOutcome {
    srec_core::utils::http_client::install_rustls_provider();
    let started = Instant::now();
    let client = srec_core::utils::http_client::apply_proxy(
        // Stays on HTTP/1.1 like the other backend clients (see `build_client`).
        reqwest::Client::builder()
            .http1_only()
            .timeout(PROBE_TIMEOUT)
            .connect_timeout(PROBE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none()),
        &ProxyTarget::Explicit(endpoint.clone()),
    )
    .build();
    let client = match client {
        Ok(client) => client,
        Err(_) => {
            return ProbeOutcome {
                ok: false,
                status: None,
                latency_ms: 0,
                error: Some(ProbeErrorKind::Failed),
            };
        }
    };
    let response = client
        .get(target.clone())
        .header(
            reqwest::header::USER_AGENT,
            platforms_parser::extractor::DEFAULT_UA,
        )
        .send()
        .await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match response {
        Ok(response) => {
            let status = response.status();
            let auth = status == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED;
            ProbeOutcome {
                ok: !auth,
                status: Some(status.as_u16()),
                latency_ms,
                error: auth.then_some(ProbeErrorKind::ProxyAuthenticationRequired),
            }
        }
        Err(error) => ProbeOutcome {
            ok: false,
            status: None,
            latency_ms,
            error: Some(classify(&error)),
        },
    }
}

fn classify(error: &reqwest::Error) -> ProbeErrorKind {
    let mut chain = String::new();
    let mut source: Option<&dyn std::error::Error> = error.source();
    while let Some(cause) = source {
        chain.push_str(&cause.to_string().to_ascii_lowercase());
        chain.push(' ');
        source = cause.source();
    }
    if chain.contains("407") || chain.contains("proxy authentication") {
        ProbeErrorKind::ProxyAuthenticationRequired
    } else if error.is_timeout() {
        ProbeErrorKind::Timeout
    } else if chain.contains("certificate") || chain.contains("tls") || chain.contains("handshake")
    {
        ProbeErrorKind::Tls
    } else if error.is_connect() {
        ProbeErrorKind::ConnectFailed
    } else {
        ProbeErrorKind::Failed
    }
}
