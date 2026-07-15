//! SOOP credential manager.
//!
//! Validates session cookies via `get_private_info.php` and re-logins with
//! username/password from platform config when the session is invalid.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use reqwest::Client;
use std::sync::OnceLock;
use tracing::{debug, instrument, warn};

use crate::credentials::error::CredentialError;
use crate::credentials::manager::{
    CredentialManager, CredentialStatus, RefreshState, RefreshedCredentials,
};

use platforms_parser::extractor::platforms::soop::{login_for_cookies, validate_session};

/// SOOP credential manager.
pub struct SoopCredentialManager {
    client: OnceLock<Client>,
}

impl SoopCredentialManager {
    pub fn new(client: Client) -> Result<Self, CredentialError> {
        let cell = OnceLock::new();
        let _ = cell.set(client);
        Ok(Self { client: cell })
    }

    pub fn new_lazy() -> Result<Self, CredentialError> {
        Ok(Self {
            client: OnceLock::new(),
        })
    }

    fn client(&self) -> &Client {
        self.client.get_or_init(Client::new)
    }

    fn username_password(state: &RefreshState) -> Option<(String, String)> {
        let extra = state.extra.as_ref()?;
        let username = extra
            .get("username")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?
            .to_string();
        let password = extra
            .get("password")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?
            .to_string();
        Some((username, password))
    }
}

#[async_trait]
impl CredentialManager for SoopCredentialManager {
    fn platform_id(&self) -> &'static str {
        "soop"
    }

    #[instrument(skip(self, cookies))]
    async fn check_status(&self, cookies: &str) -> Result<CredentialStatus, CredentialError> {
        if cookies.trim().is_empty() {
            // No session yet — re-login if username/password are available.
            return Ok(CredentialStatus::NeedsRefresh {
                refresh_deadline: None,
            });
        }

        match validate_session(self.client(), cookies).await {
            Ok(true) => {
                debug!("SOOP session cookies are valid");
                Ok(CredentialStatus::Valid)
            }
            Ok(false) => {
                debug!("SOOP session cookies are invalid; need re-login");
                Ok(CredentialStatus::NeedsRefresh {
                    refresh_deadline: None,
                })
            }
            Err(e) => {
                warn!(error = %e, "SOOP session check failed");
                Err(CredentialError::RefreshFailed(format!(
                    "SOOP session check failed: {e}"
                )))
            }
        }
    }

    #[instrument(skip(self, state))]
    async fn refresh(&self, state: &RefreshState) -> Result<RefreshedCredentials, CredentialError> {
        let (username, password) = Self::username_password(state).ok_or_else(|| {
            CredentialError::RefreshFailed(
                "SOOP re-login requires username/password in platform config".to_string(),
            )
        })?;

        // If we somehow still have a working session, keep it.
        if !state.cookies.trim().is_empty()
            && validate_session(self.client(), &state.cookies)
                .await
                .unwrap_or(false)
        {
            debug!("SOOP session still valid; skipping re-login");
            return Ok(RefreshedCredentials {
                cookies: state.cookies.clone(),
                refresh_token: None,
                access_token: None,
                expires_at: Some(Utc::now() + Duration::days(7)),
            });
        }

        debug!("SOOP re-login with configured username/password");
        let cookies = login_for_cookies(self.client(), &username, &password)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("login failed") {
                    CredentialError::InvalidCredentials(msg)
                } else {
                    CredentialError::RefreshFailed(msg)
                }
            })?;

        Ok(RefreshedCredentials {
            cookies,
            refresh_token: None,
            access_token: None,
            expires_at: Some(Utc::now() + Duration::days(7)),
        })
    }

    async fn validate(&self, cookies: &str) -> Result<bool, CredentialError> {
        validate_session(self.client(), cookies)
            .await
            .map_err(|e| CredentialError::RefreshFailed(e.to_string()))
    }

    fn supports_auto_refresh(&self) -> bool {
        true
    }

    fn required_refresh_fields(&self) -> &'static [&'static str] {
        &["username", "password"]
    }
}
