//! Bilibili credential manager.
//!
//! Delegates to the platforms crate for actual implementation:
//! - QR code login: via qr_login utilities
//! - Cookie refresh: via cookie_refresh utilities

use async_trait::async_trait;
use chrono::{Duration, Utc};
use reqwest::Client;
use tracing::{debug, instrument};

use crate::credentials::error::CredentialError;
use crate::credentials::manager::{
    CredentialManager, CredentialStatus, RefreshState, RefreshedCredentials,
};

// Re-export QR login types from platforms crate
pub use platforms_parser::extractor::platforms::bilibili::{
    QrGenerateResponse, QrLoginError, QrPollResult, QrPollStatus,
    generate_qr as platforms_generate_qr, poll_qr as platforms_poll_qr,
};

// Import cookie refresh utilities from platforms crate
use platforms_parser::extractor::platforms::bilibili::{
    CookieStatus as PlatformsCookieStatus, check_cookie_status as platforms_check_status,
    refresh_cookies as platforms_refresh, validate_cookies as platforms_validate,
};

/// Bilibili credential manager.
pub struct BilibiliCredentialManager {
    client: Client,
}

impl BilibiliCredentialManager {
    pub fn new(client: Client) -> Result<Self, CredentialError> {
        Ok(Self { client })
    }

    /// Generate a QR code for Bilibili TV login.
    /// Delegates to platforms crate utility.
    #[instrument(skip(self))]
    pub async fn generate_qr(&self) -> Result<QrGenerateResponse, CredentialError> {
        platforms_generate_qr(&self.client)
            .await
            .map_err(|e| CredentialError::RefreshFailed(e.to_string()))
    }

    /// Poll the status of a QR code login.
    /// Delegates to platforms crate utility.
    #[instrument(skip(self))]
    pub async fn poll_qr(&self, auth_code: &str) -> Result<QrPollResult, CredentialError> {
        platforms_poll_qr(&self.client, auth_code)
            .await
            .map_err(|e| CredentialError::RefreshFailed(e.to_string()))
    }
}

#[async_trait]
impl CredentialManager for BilibiliCredentialManager {
    fn platform_id(&self) -> &'static str {
        "bilibili"
    }

    #[instrument(skip(self, cookies))]
    async fn check_status(&self, cookies: &str) -> Result<CredentialStatus, CredentialError> {
        debug!("Checking Bilibili cookie status");

        let result = platforms_check_status(&self.client, cookies)
            .await
            .map_err(|e| CredentialError::RefreshFailed(e.to_string()))?;

        match result {
            PlatformsCookieStatus::Valid => {
                debug!("Bilibili credentials are valid");
                Ok(CredentialStatus::Valid)
            }
            PlatformsCookieStatus::NeedsRefresh { deadline_timestamp } => {
                debug!(?deadline_timestamp, "Bilibili credentials need refresh");
                Ok(CredentialStatus::NeedsRefresh {
                    refresh_deadline: deadline_timestamp,
                })
            }
            PlatformsCookieStatus::Invalid { reason, code } => {
                debug!(?reason, ?code, "Bilibili credentials invalid");
                Ok(CredentialStatus::Invalid {
                    reason,
                    error_code: code.map(|c| c as i32),
                })
            }
        }
    }

    #[instrument(skip(self, state))]
    async fn refresh(&self, state: &RefreshState) -> Result<RefreshedCredentials, CredentialError> {
        let refresh_token = state
            .refresh_token
            .as_ref()
            .ok_or(CredentialError::MissingRefreshToken)?;

        debug!("Performing Bilibili cookie refresh");

        let result = platforms_refresh(&self.client, &state.cookies, refresh_token)
            .await
            .map_err(|e| CredentialError::RefreshFailed(e.to_string()))?;

        debug!("Bilibili refresh completed successfully");
        Ok(RefreshedCredentials {
            cookies: result.cookies,
            refresh_token: Some(result.refresh_token),
            expires_at: Some(Utc::now() + Duration::days(30)),
        })
    }

    #[instrument(skip(self, cookies))]
    async fn validate(&self, cookies: &str) -> Result<bool, CredentialError> {
        platforms_validate(&self.client, cookies)
            .await
            .map_err(|e| CredentialError::RefreshFailed(e.to_string()))
    }

    fn required_refresh_fields(&self) -> &'static [&'static str] {
        &["refresh_token", "SESSDATA", "bili_jct"]
    }
}
