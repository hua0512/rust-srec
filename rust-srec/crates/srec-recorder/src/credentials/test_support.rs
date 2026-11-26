//! Test doubles shared by the tests that drive the credential refresh flow.

use std::sync::Arc;

use async_trait::async_trait;
use srec_core::domain::notification::{RuntimeAlert, RuntimeAlertSink};
use tokio::sync::broadcast;

use super::error::CredentialError;
use super::{
    AccountStatus, CredentialMaterial, CredentialProvider, PlatformAdmission, ProviderCapabilities,
    RefreshedCredentials,
};

/// A [`CredentialProvider`] that reports every account as needing a refresh
/// and hands back fixed [`RefreshedCredentials`].
///
/// Lets a test drive a profile refresh to success without a platform API.
/// Register it with `CredentialProviderRegistry::register_provider` under the
/// `platform_name` of the config layer under test.
pub struct StubCredentialProvider {
    cookies: String,
    refresh_token: String,
}

impl StubCredentialProvider {
    pub fn new(cookies: &str, refresh_token: &str) -> Self {
        Self {
            cookies: cookies.to_string(),
            refresh_token: refresh_token.to_string(),
        }
    }
}

#[async_trait]
impl CredentialProvider for StubCredentialProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            check: true,
            refresh: true,
            ..ProviderCapabilities::default()
        }
    }

    async fn check(
        &self,
        _client: &reqwest::Client,
        _material: &CredentialMaterial,
    ) -> Result<AccountStatus, CredentialError> {
        Ok(AccountStatus::Repairable)
    }

    async fn refresh(
        &self,
        _client: &reqwest::Client,
        _material: &CredentialMaterial,
    ) -> Result<RefreshedCredentials, CredentialError> {
        Ok(RefreshedCredentials {
            cookies: self.cookies.clone(),
            refresh_token: Some(self.refresh_token.clone()),
            access_token: None,
            expires_at: None,
        })
    }
}

/// Admission with enough tokens that tests never wait on the rate limiter.
pub fn unthrottled_admission() -> Arc<PlatformAdmission> {
    Arc::new(PlatformAdmission::new(
        crate::monitor::RateLimiterManager::with_config(crate::monitor::RateLimiterConfig {
            max_tokens: 100,
            initial_tokens: 100,
            refill_rate: 100.0,
        }),
    ))
}

/// In-memory [`RuntimeAlertSink`] capturing emitted [`RuntimeAlert`]s without
/// depending on `srec-notify`.
pub struct TestAlertSink {
    tx: broadcast::Sender<RuntimeAlert>,
}

impl TestAlertSink {
    pub fn new() -> (Arc<Self>, broadcast::Receiver<RuntimeAlert>) {
        let (tx, rx) = broadcast::channel(64);
        (Arc::new(Self { tx }), rx)
    }
}

impl RuntimeAlertSink for TestAlertSink {
    fn emit(self: Arc<Self>, alert: RuntimeAlert) {
        let _ = self.tx.send(alert);
    }
}

/// Serve a single synthetic HTTP/1.1 response over loopback and return the
/// resulting [`reqwest::Response`] without pulling in an HTTP server crate.
#[cfg(test)]
pub(crate) async fn mock_http_response(
    status: u16,
    reason: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> reqwest::Response {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback mock HTTP listener");
    let addr = listener.local_addr().expect("listener addr");
    let mut raw = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in headers {
        raw.push_str(&format!("{name}: {value}\r\n"));
    }
    raw.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    let server = tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let _ = stream.write_all(raw.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build loopback reqwest client");
    let response = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .expect("fetch synthetic response");
    let _ = server.await;
    response
}
