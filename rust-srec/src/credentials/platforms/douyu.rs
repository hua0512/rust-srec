//! Douyu account provider.
//!
//! An account signed in by QR code holds the main-site cookies, which last
//! about six days, and the passport credential `LTP0` as its refresh token.
//! Douyu treats a lapsed session as a logged-out viewer instead of failing a
//! request, so no check or extraction notices the expiry: the cookies are
//! renewed from `LTP0` once they are four days old. Cookies pasted from a
//! browser renew the same way when they include `dy_did` and `LTP0` is the
//! refresh token or one of the cookies.

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use crate::credentials::CredentialMaterial;
use crate::credentials::error::CredentialError;
use crate::credentials::provider::{
    CredentialProvider, ProviderCapabilities, QrLoginPoll, QrLoginStart, RefreshedCredentials,
};

use platforms_parser::extractor::platforms::douyu::passport::{
    self, DEVICE_ID_COOKIE, PASSPORT_CREDENTIAL_COOKIE, PassportError, QrStatus,
};

/// Renewal leaves two days of the main-site session's lifetime to retry in.
const RENEW_AFTER: Duration = Duration::from_secs(4 * 24 * 60 * 60);
const SESSION_LIFETIME: chrono::Duration = chrono::Duration::days(6);

pub struct DouyuProvider;

/// What a poll needs between requests: the scan code and the login's passport
/// cookies. Stored as the login's provider auth code, which is cleared when
/// the login ends.
#[derive(Serialize, Deserialize)]
struct LoginHandle {
    code: String,
    passport_cookies: String,
}

/// `LTP0`: the refresh token, else the cookie of that name.
fn passport_credential(material: &CredentialMaterial) -> Option<String> {
    material
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .or_else(|| passport::cookie_value(&material.cookies, PASSPORT_CREDENTIAL_COOKIE))
}

fn map_error(error: PassportError) -> CredentialError {
    match error {
        PassportError::Response(error) => error.into(),
        PassportError::Network(error) => CredentialError::Network(error),
        PassportError::Parse(what) => CredentialError::ParseError(what.to_owned()),
        // Douyu documents no codes for this request, so a refusal is not
        // taken to mean the login was revoked: the account keeps working on
        // its current cookies, and the failure is reported and retried.
        PassportError::Api(code) => {
            CredentialError::RefreshFailed(format!("Douyu passport error {code}"))
        }
        PassportError::AccountMismatch => {
            CredentialError::RefreshFailed("Douyu renewed a different account".into())
        }
    }
}

#[async_trait]
impl CredentialProvider for DouyuProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            refresh_token: true,
            qr_login: true,
            refresh: true,
            ..ProviderCapabilities::default()
        }
    }

    /// Profiles holding only a device ID, as the automatic upgrade creates
    /// them, have no session to renew.
    fn refreshable(&self, material: &CredentialMaterial) -> bool {
        passport_credential(material).is_some()
            && passport::cookie_value(&material.cookies, DEVICE_ID_COOKIE).is_some()
    }

    fn renew_after(&self) -> Option<Duration> {
        Some(RENEW_AFTER)
    }

    #[instrument(skip_all)]
    async fn refresh(
        &self,
        client: &Client,
        material: &CredentialMaterial,
    ) -> Result<RefreshedCredentials, CredentialError> {
        let credential = passport_credential(material)
            .filter(|_| self.refreshable(material))
            .ok_or(CredentialError::MissingRefreshToken)?;
        let renewal = passport::renew_session(client, &material.cookies, &credential)
            .await
            .map_err(map_error)?;
        debug!("Douyu main-site session renewed");
        Ok(RefreshedCredentials {
            cookies: renewal.cookies,
            // A credential kept among the cookies was replaced there.
            refresh_token: material
                .refresh_token
                .as_ref()
                .and(renewal.passport_credential),
            access_token: None,
            expires_at: Some(Utc::now() + SESSION_LIFETIME),
        })
    }

    #[instrument(skip_all)]
    async fn start_qr_login(&self, client: &Client) -> Result<QrLoginStart, CredentialError> {
        let qr = passport::generate_qr(client).await.map_err(map_error)?;
        let handle = serde_json::to_string(&LoginHandle {
            code: qr.code,
            passport_cookies: qr.passport_cookies,
        })
        .map_err(|_| CredentialError::Internal("could not store the Douyu login".into()))?;
        Ok(QrLoginStart {
            url: qr.url,
            auth_code: handle,
            expires_in: qr.expires_in,
        })
    }

    #[instrument(skip_all)]
    async fn poll_qr_login(
        &self,
        client: &Client,
        auth_code: &str,
    ) -> Result<QrLoginPoll, CredentialError> {
        let handle: LoginHandle = serde_json::from_str(auth_code)
            .map_err(|_| CredentialError::Internal("unreadable Douyu login".into()))?;
        let status = passport::poll_qr(client, &handle.code, &handle.passport_cookies)
            .await
            .map_err(map_error)?;
        Ok(match status {
            QrStatus::NotScanned => QrLoginPoll::Waiting { scanned: false },
            QrStatus::Scanned => QrLoginPoll::Waiting { scanned: true },
            QrStatus::Expired => QrLoginPoll::Expired,
            QrStatus::Confirmed(session) => QrLoginPoll::Completed(CredentialMaterial {
                cookies: session.cookies,
                refresh_token: Some(session.passport_credential),
                access_token: None,
                reauth_config: None,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{OperationDeadline, PlatformAdmission, provider_client};
    use crate::monitor::RateLimiterManager;

    fn material(cookies: &str, refresh_token: Option<&str>) -> CredentialMaterial {
        CredentialMaterial {
            cookies: cookies.into(),
            refresh_token: refresh_token.map(str::to_owned),
            access_token: None,
            reauth_config: None,
        }
    }

    #[test]
    fn only_a_device_bound_passport_credential_makes_an_account_refreshable() {
        let signed_in = "dy_did=bdevice; acf_did=bdevice; acf_uid=1; acf_auth=a";
        assert!(DouyuProvider.refreshable(&material(signed_in, Some("ltp0"))));
        assert!(DouyuProvider.refreshable(&material("dy_did=bdevice; LTP0=ltp0", None)));
        for unrenewable in [
            material(signed_in, None),
            material(signed_in, Some("  ")),
            // The automatic upgrade's device-only profile.
            material("acf_did=0123456789abcdef0123456789abcdef", None),
            material("acf_did=bdevice; acf_uid=1", Some("ltp0")),
        ] {
            assert!(!DouyuProvider.refreshable(&unrenewable));
        }
        let capabilities = DouyuProvider.capabilities();
        assert!(capabilities.qr_login && capabilities.refresh && capabilities.refresh_token);
        assert!(!capabilities.check);
        assert_eq!(DouyuProvider.renew_after(), Some(RENEW_AFTER));
    }

    #[tokio::test]
    async fn refresh_and_poll_without_their_material_make_no_request() {
        // No server listens: a request would fail as a network error.
        let error = DouyuProvider
            .refresh(
                &provider_client(&platforms_parser::proxy::ProxyTarget::Direct).unwrap(),
                &material("acf_did=bdevice", None),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, CredentialError::MissingRefreshToken));
        assert!(error.requires_relogin());
        assert!(matches!(
            DouyuProvider
                .poll_qr_login(
                    &provider_client(&platforms_parser::proxy::ProxyTarget::Direct).unwrap(),
                    "not a handle"
                )
                .await,
            Err(CredentialError::Internal(_))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn passport_failures_are_classified_without_secrets() {
        let refused = map_error(PassportError::Api(-1));
        assert!(matches!(refused, CredentialError::RefreshFailed(_)));
        assert!(!refused.requires_relogin());
        assert!(!map_error(PassportError::AccountMismatch).requires_relogin());

        let response = reqwest::Response::from(
            axum::http::Response::builder()
                .status(429)
                .header("Retry-After", "120")
                .body("LTP0=private")
                .unwrap(),
        );
        let throttle = platforms_parser::extractor::error::ExtractorError::check_response(response)
            .unwrap_err();
        let error = map_error(PassportError::Response(throttle));
        assert!(
            matches!(error, CredentialError::RateLimited { retry_after: Some(delay) } if delay == Duration::from_secs(120))
        );
        assert!(!error.to_string().contains("private"));
        // A throttle seen by a provider call pauses the account's connection.
        let admission = PlatformAdmission::new(RateLimiterManager::new());
        admission.observe_provider("douyu", &crate::proxies::ResolvedRoute::default(), &error);
        assert!(matches!(
            admission
                .admit(
                    "douyu",
                    &crate::proxies::RouteKey::Direct,
                    OperationDeadline::new(Duration::from_secs(60))
                )
                .await,
            Err(CredentialError::DeadlineExceeded)
        ));
    }
}
