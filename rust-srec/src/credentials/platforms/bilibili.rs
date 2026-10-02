//! Bilibili credential manager.
//!
//! Delegates to the platforms crate for actual implementation:
//! - QR code login: via qr_login utilities
//! - Token refresh: via token_refresh utilities (OAuth2/APP flow)
//! - Fallback validation: via NAV API for cookie-only users

use async_trait::async_trait;
use chrono::{Duration, Utc};
use reqwest::Client;
use std::sync::OnceLock;
use tracing::{debug, instrument, warn};

use crate::credentials::error::CredentialError;
use crate::credentials::manager::{
    CredentialManager, CredentialStatus, RefreshState, RefreshedCredentials,
};

// Re-export QR login types from platforms crate
pub use platforms_parser::extractor::platforms::bilibili::{
    QrGenerateResponse, QrLoginError, QrPollResult, QrPollStatus,
    generate_qr as platforms_generate_qr, poll_qr as platforms_poll_qr,
};

// Import token refresh utilities from platforms crate
use platforms_parser::extractor::platforms::bilibili::{
    TokenRefreshError, refresh_token as platforms_refresh_token,
    validate_token as platforms_validate_token,
};

// NAV API URL for cookie-only validation fallback
const NAV_URL: &str = "https://api.bilibili.com/x/web-interface/nav";

/// Bilibili credential manager.
pub struct BilibiliCredentialManager {
    client: OnceLock<Client>,
}

fn map_token_refresh_error(err: TokenRefreshError) -> CredentialError {
    match err {
        TokenRefreshError::Response(error) => error.into(),
        TokenRefreshError::Network(e) => CredentialError::Network(e.without_url()),
        TokenRefreshError::Parse(e) => CredentialError::ParseError(e),
        TokenRefreshError::Api { code, .. } => match code {
            -101 | -111 => {
                CredentialError::InvalidCredentials(format!("Bilibili login required ({code})"))
            }
            -663 => CredentialError::InvalidRefreshToken,
            _ => CredentialError::RefreshFailed(format!("Bilibili API error {code}")),
        },
        TokenRefreshError::SystemTime => CredentialError::Internal("System time error".to_string()),
    }
}

fn map_qr_error(error: QrLoginError) -> CredentialError {
    match error {
        QrLoginError::Response(error) => error.into(),
        QrLoginError::Network(error) => CredentialError::Network(error.without_url()),
        _ => CredentialError::RefreshFailed("Bilibili QR request failed".into()),
    }
}

impl BilibiliCredentialManager {
    pub fn new(client: Client) -> Result<Self, CredentialError> {
        let cell = OnceLock::new();
        // Best-effort: this only fails if set twice (which cannot happen here).
        let _ = cell.set(client);
        Ok(Self { client: cell })
    }

    /// Create a manager that lazily initializes its underlying HTTP client.
    ///
    /// This avoids paying `reqwest::Client` initialization costs on startup.
    pub fn new_lazy() -> Result<Self, CredentialError> {
        Ok(Self {
            client: OnceLock::new(),
        })
    }

    fn client(&self) -> &Client {
        self.client.get_or_init(Client::new)
    }

    /// Generate a QR code for Bilibili TV login.
    /// Delegates to platforms crate utility.
    #[instrument(skip(self))]
    pub async fn generate_qr(&self) -> Result<QrGenerateResponse, CredentialError> {
        platforms_generate_qr(self.client())
            .await
            .map_err(map_qr_error)
    }

    /// Poll the status of a QR code login.
    /// Delegates to platforms crate utility.
    #[instrument(skip_all)]
    pub async fn poll_qr(&self, auth_code: &str) -> Result<QrPollResult, CredentialError> {
        platforms_poll_qr(self.client(), auth_code)
            .await
            .map_err(map_qr_error)
    }

    /// Validate cookies using NAV API (fallback for cookie-only users without access_token).
    async fn validate_via_nav(&self, cookies: &str) -> Result<bool, CredentialError> {
        let response = self
            .client()
            .get(NAV_URL)
            .header("Cookie", cookies)
            .header("User-Agent", platforms_parser::extractor::DEFAULT_UA)
            .header(reqwest::header::REFERER, "https://www.bilibili.com")
            .send()
            .await?;

        let response =
            platforms_parser::extractor::error::ExtractorError::check_response(response)?;
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| CredentialError::ParseError(e.to_string()))?;

        let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        Ok(code == 0)
    }

    /// Extract access_token from RefreshState.extra JSON.
    fn extract_access_token(state: &RefreshState) -> Option<String> {
        state
            .extra
            .as_ref()
            .and_then(|v| v.get("access_token"))
            .and_then(|t| t.as_str())
            .map(String::from)
    }
}

#[async_trait]
impl CredentialManager for BilibiliCredentialManager {
    fn platform_id(&self) -> &'static str {
        "bilibili"
    }

    #[instrument(skip(self, cookies))]
    async fn check_status(&self, cookies: &str) -> Result<CredentialStatus, CredentialError> {
        debug!("Checking Bilibili credential status");

        // We can't check token validity without an access_token from this interface.
        // The check_status method only receives cookies; the access_token is passed via
        // RefreshState.extra during refresh. So here we fall back to the NAV API check.
        //
        // The service layer calls check_status before refresh, so this just validates
        // that the cookies are still working. The actual token staleness check happens
        // implicitly during refresh (validate_token is called there).
        let is_valid = self.validate_via_nav(cookies).await?;

        if is_valid {
            debug!("Bilibili credentials are valid (NAV check)");
            Ok(CredentialStatus::Valid)
        } else {
            // Cookies are invalid — if there's an access_token + refresh_token,
            // the refresh flow may still recover. Signal NeedsRefresh rather than Invalid
            // to give the refresh path a chance.
            debug!("Bilibili NAV check failed, signaling NeedsRefresh");
            Ok(CredentialStatus::NeedsRefresh {
                refresh_deadline: None,
            })
        }
    }

    #[instrument(skip(self, state))]
    async fn refresh(&self, state: &RefreshState) -> Result<RefreshedCredentials, CredentialError> {
        let refresh_token = state
            .refresh_token
            .as_ref()
            .ok_or(CredentialError::MissingRefreshToken)?;

        let access_token = Self::extract_access_token(state);

        // If we have both access_token and refresh_token, use the OAuth2 flow.
        if let Some(ref access_token) = access_token {
            debug!("Performing Bilibili OAuth2 token refresh");

            // Validate first to confirm refresh is needed
            match platforms_validate_token(self.client(), access_token).await {
                Ok(needs_refresh) => {
                    if !needs_refresh {
                        debug!(
                            "Token validation says no refresh needed; returning current cookies"
                        );
                        return Ok(RefreshedCredentials {
                            cookies: state.cookies.clone(),
                            refresh_token: state.refresh_token.clone(),
                            access_token: Some(access_token.clone()),
                            expires_at: Some(Utc::now() + Duration::days(30)),
                        });
                    }
                }
                Err(error @ TokenRefreshError::Response(_))
                | Err(error @ TokenRefreshError::Network(_)) => {
                    // A transport failure or shared throttle cannot establish
                    // token expiry and must not launch another provider request.
                    return Err(map_token_refresh_error(error));
                }
                Err(_) => {
                    // Validation failed — token may be expired. Still try refreshing.
                    warn!("Token validation failed, attempting refresh anyway");
                }
            }

            let result = platforms_refresh_token(self.client(), access_token, refresh_token)
                .await
                .map_err(map_token_refresh_error)?;

            debug!("Bilibili OAuth2 refresh completed successfully");
            Ok(RefreshedCredentials {
                cookies: result.cookies,
                refresh_token: Some(result.refresh_token),
                access_token: Some(result.access_token),
                expires_at: Some(Utc::now() + Duration::seconds(result.expires_in as i64)),
            })
        } else {
            // No access_token — cannot use OAuth2 flow.
            // This happens when users manually paste cookies without going through QR login.
            warn!(
                "No access_token available; OAuth2 refresh not possible. Re-login via QR required."
            );
            Err(CredentialError::MissingRefreshToken)
        }
    }

    #[instrument(skip(self, cookies))]
    async fn validate(&self, cookies: &str) -> Result<bool, CredentialError> {
        self.validate_via_nav(cookies).await
    }

    fn required_refresh_fields(&self) -> &'static [&'static str] {
        &["refresh_token", "access_token", "SESSDATA", "bili_jct"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{
        CredentialScope, CredentialSource, OperationDeadline, PlatformAdmission,
    };
    use crate::monitor::RateLimiterManager;

    #[tokio::test(start_paused = true)]
    async fn qr_and_refresh_throttles_preserve_provider_delay_across_all_admission_paths() {
        for qr in [true, false] {
            let response = reqwest::Response::from(
                axum::http::Response::builder()
                    .status(429)
                    .header("Retry-After", "7200")
                    .body("private provider body")
                    .unwrap(),
            );
            let error =
                platforms_parser::extractor::error::ExtractorError::check_response(response)
                    .unwrap_err();
            let error = if qr {
                map_qr_error(QrLoginError::Response(error))
            } else {
                map_token_refresh_error(TokenRefreshError::Response(error))
            };
            assert!(
                matches!(error, CredentialError::RateLimited { retry_after: Some(delay) } if delay == std::time::Duration::from_secs(7200))
            );
            assert!(!error.to_string().contains("private"));
            let admission = PlatformAdmission::new(RateLimiterManager::new());
            let source = CredentialSource {
                scope: CredentialScope::Platform {
                    platform_id: "bilibili".into(),
                    platform_name: "bilibili".into(),
                },
                platform_name: "bilibili".into(),
                cookies: "account=A".into(),
                refresh_token: None,
                access_token: None,
                reauth_extra: None,
            };
            admission.observe_source(&source, &error).await;
            admission.observe_platform_name("bilibili", &error).await;
            admission.observe_provider("bilibili", &error);
            assert!(matches!(
                admission
                    .admit(
                        "bilibili",
                        OperationDeadline::new(std::time::Duration::from_secs(7199))
                    )
                    .await,
                Err(CredentialError::DeadlineExceeded)
            ));
            admission
                .admit(
                    "bilibili",
                    OperationDeadline::new(std::time::Duration::from_secs(2)),
                )
                .await
                .unwrap();
        }
    }
}
