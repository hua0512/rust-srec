//! Bilibili account provider.
//!
//! Delegates to the platforms crate for the provider calls:
//! - QR code login: via qr_login utilities
//! - Token refresh: via token_refresh utilities (OAuth2/APP flow)
//! - Account check: via the NAV API, which accepts the recording cookies

use async_trait::async_trait;
use chrono::{Duration, Utc};
use reqwest::Client;
use tracing::{debug, instrument, warn};

use crate::credentials::CredentialMaterial;
use crate::credentials::error::CredentialError;
use crate::credentials::provider::RefreshedCredentials;
use crate::credentials::provider::{
    AccountStatus, CredentialProvider, ProviderCapabilities, QrLoginPoll, QrLoginStart,
};

use platforms_parser::extractor::platforms::bilibili::{
    QrLoginError, QrPollStatus, TokenRefreshError, generate_qr, poll_qr,
    refresh_token as platforms_refresh_token, validate_token as platforms_validate_token,
};

const NAV_URL: &str = "https://api.bilibili.com/x/web-interface/nav";

pub struct BilibiliProvider;

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

/// Whether the NAV API accepts the cookies as a signed-in session.
async fn validate_via_nav(client: &Client, cookies: &str) -> Result<bool, CredentialError> {
    let response = client
        .get(NAV_URL)
        .header("Cookie", cookies)
        .header("User-Agent", platforms_parser::extractor::DEFAULT_UA)
        .header(reqwest::header::REFERER, "https://www.bilibili.com")
        .send()
        .await?;

    let response = platforms_parser::extractor::error::ExtractorError::check_response(response)?;
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| CredentialError::ParseError(e.to_string()))?;

    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    Ok(code == 0)
}

#[async_trait]
impl CredentialProvider for BilibiliProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            refresh_token: true,
            access_token: true,
            qr_login: true,
            check: true,
            refresh: true,
            ..ProviderCapabilities::default()
        }
    }

    #[instrument(skip_all)]
    async fn check(
        &self,
        client: &Client,
        material: &CredentialMaterial,
    ) -> Result<AccountStatus, CredentialError> {
        // Recording uses the cookies, so the NAV API checks those. Token
        // staleness is checked during refresh (validate_token is called there).
        if validate_via_nav(client, &material.cookies).await? {
            debug!("Bilibili credentials are valid (NAV check)");
            return Ok(AccountStatus::Valid);
        }
        // The OAuth2 refresh needs both tokens; cookies pasted without a QR
        // login have neither.
        if material.access_token.is_some() && material.refresh_token.is_some() {
            debug!("Bilibili NAV check failed; the tokens may refresh it");
            Ok(AccountStatus::Repairable)
        } else {
            debug!("Bilibili NAV check failed and the account has no tokens");
            Ok(AccountStatus::Revoked)
        }
    }

    #[instrument(skip_all)]
    async fn refresh(
        &self,
        client: &Client,
        material: &CredentialMaterial,
    ) -> Result<RefreshedCredentials, CredentialError> {
        let refresh_token = material
            .refresh_token
            .as_ref()
            .ok_or(CredentialError::MissingRefreshToken)?;
        let Some(access_token) = &material.access_token else {
            // Cookies pasted without a QR login cannot use the OAuth2 flow.
            warn!(
                "No access_token available; OAuth2 refresh not possible. Re-login via QR required."
            );
            return Err(CredentialError::MissingRefreshToken);
        };

        debug!("Performing Bilibili OAuth2 token refresh");
        match platforms_validate_token(client, access_token).await {
            Ok(false) => {
                debug!("Token validation says no refresh needed; returning current cookies");
                return Ok(RefreshedCredentials {
                    cookies: material.cookies.clone(),
                    refresh_token: Some(refresh_token.clone()),
                    access_token: Some(access_token.clone()),
                    expires_at: Some(Utc::now() + Duration::days(30)),
                });
            }
            Ok(true) => {}
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

        let result = platforms_refresh_token(client, access_token, refresh_token)
            .await
            .map_err(map_token_refresh_error)?;

        debug!("Bilibili OAuth2 refresh completed successfully");
        Ok(RefreshedCredentials {
            cookies: result.cookies,
            refresh_token: Some(result.refresh_token),
            access_token: Some(result.access_token),
            expires_at: Some(Utc::now() + Duration::seconds(result.expires_in as i64)),
        })
    }

    #[instrument(skip_all)]
    async fn start_qr_login(&self, client: &Client) -> Result<QrLoginStart, CredentialError> {
        let qr = generate_qr(client).await.map_err(map_qr_error)?;
        Ok(QrLoginStart {
            url: qr.url,
            auth_code: qr.auth_code,
            expires_in: qr.expires_in,
        })
    }

    #[instrument(skip_all)]
    async fn poll_qr_login(
        &self,
        client: &Client,
        auth_code: &str,
    ) -> Result<QrLoginPoll, CredentialError> {
        let result = poll_qr(client, auth_code).await.map_err(map_qr_error)?;
        Ok(match result.status {
            QrPollStatus::NotScanned => QrLoginPoll::Waiting { scanned: false },
            QrPollStatus::ScannedNotConfirmed => QrLoginPoll::Waiting { scanned: true },
            QrPollStatus::Expired => QrLoginPoll::Expired,
            QrPollStatus::Success => QrLoginPoll::Completed(CredentialMaterial {
                cookies: result.cookies.ok_or_else(|| {
                    CredentialError::RefreshFailed("Bilibili QR login returned no cookies".into())
                })?,
                refresh_token: result.refresh_token,
                access_token: result.access_token,
                reauth_config: None,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{OperationDeadline, PlatformAdmission};
    use crate::monitor::RateLimiterManager;

    #[tokio::test(start_paused = true)]
    async fn qr_and_refresh_throttles_defer_admission_up_to_the_cap() {
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
            admission.observe_provider(
                "bilibili",
                &crate::proxies::ResolvedRoute::default(),
                &error,
            );
            assert!(matches!(
                admission
                    .admit(
                        "bilibili",
                        &crate::proxies::RouteKey::Direct,
                        OperationDeadline::new(
                            crate::credentials::admission::MAX_REMOTE_BACKOFF
                                - std::time::Duration::from_secs(1)
                        )
                    )
                    .await,
                Err(CredentialError::DeadlineExceeded)
            ));
            admission
                .admit(
                    "bilibili",
                    &crate::proxies::RouteKey::Direct,
                    OperationDeadline::new(std::time::Duration::from_secs(2)),
                )
                .await
                .unwrap();
        }
    }
}
