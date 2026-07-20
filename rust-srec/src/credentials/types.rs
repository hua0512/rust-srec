//! Core credential types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::notification::NotificationPriority;

const EXTRACTOR_CREDENTIAL_FIELDS: [&str; 5] = [
    "refresh_token",
    "access_token",
    "last_cookie_check_date",
    "last_cookie_check_result",
    "session_cookies",
];

pub(crate) fn platform_reauth_extra(
    platform_name: &str,
    platform_specific: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    if !platform_name.eq_ignore_ascii_case("soop") {
        return None;
    }

    let config = platform_specific?;
    let username = config
        .get("username")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let password = config
        .get("password")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    Some(serde_json::json!({
        "username": username,
        "password": password,
    }))
}

pub(crate) fn extractor_platform_extras(
    mut platform_specific: serde_json::Value,
) -> serde_json::Value {
    if let serde_json::Value::Object(ref mut fields) = platform_specific {
        for field in EXTRACTOR_CREDENTIAL_FIELDS {
            fields.remove(field);
        }
    }

    platform_specific
}

/// Represents the configuration layer where credentials are defined.
///
/// Credentials can be defined at Platform, Template, or Streamer scope.
/// Global scope is explicitly NOT supported for credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialScope {
    /// Platform-wide credentials (shared by all streamers on this platform)
    Platform {
        platform_id: String,
        platform_name: String,
    },
    /// Template-specific credentials
    Template {
        template_id: String,
        template_name: String,
    },
    /// Streamer-specific credentials (highest priority)
    Streamer {
        streamer_id: String,
        streamer_name: String,
    },
}

impl CredentialScope {
    /// Returns the database table name for this scope.
    #[inline]
    pub fn table_name(&self) -> &'static str {
        match self {
            Self::Platform { .. } => "platform_config",
            Self::Template { .. } => "template_config",
            Self::Streamer { .. } => "streamers",
        }
    }

    /// Returns the record ID for this scope.
    #[inline]
    pub fn record_id(&self) -> &str {
        match self {
            Self::Platform { platform_id, .. } => platform_id,
            Self::Template { template_id, .. } => template_id,
            Self::Streamer { streamer_id, .. } => streamer_id,
        }
    }

    /// Returns the platform name (for Platform scope) or empty string.
    pub fn platform_name(&self) -> Option<&str> {
        match self {
            Self::Platform { platform_name, .. } => Some(platform_name),
            _ => None,
        }
    }

    /// Human-readable description of the scope.
    pub fn describe(&self) -> String {
        match self {
            Self::Platform { platform_name, .. } => {
                format!("Platform: {}", platform_name)
            }
            Self::Template { template_name, .. } => {
                format!("Template: {}", template_name)
            }
            Self::Streamer { streamer_name, .. } => {
                format!("Streamer: {}", streamer_name)
            }
        }
    }

    /// Generate a unique key for caching/locking.
    pub fn cache_key(&self) -> String {
        format!("{}:{}", self.table_name(), self.record_id())
    }
}

/// Complete credential information with source tracking.
#[derive(Debug, Clone)]
pub struct CredentialSource {
    /// Which configuration layer the credentials came from.
    pub scope: CredentialScope,
    /// The cookie string.
    pub cookies: String,
    /// Refresh token (if available).
    pub refresh_token: Option<String>,
    /// OAuth2 access token (if available, e.g. from Bilibili TV QR login).
    pub access_token: Option<String>,
    /// Platform name for this credential (e.g., "bilibili").
    pub platform_name: String,
    /// Platform-specific re-login material (e.g. SOOP username/password).
    pub reauth_extra: Option<serde_json::Value>,
}

impl CredentialSource {
    /// Create a new credential source.
    pub fn new(
        scope: CredentialScope,
        cookies: String,
        refresh_token: Option<String>,
        platform_name: String,
    ) -> Self {
        Self {
            scope,
            cookies,
            refresh_token,
            access_token: None,
            platform_name,
            reauth_extra: None,
        }
    }

    /// Create a new credential source with an access token.
    pub fn with_access_token(mut self, access_token: Option<String>) -> Self {
        self.access_token = access_token;
        self
    }

    /// Attach re-login material (username/password, etc.).
    pub fn with_reauth_extra(mut self, reauth_extra: Option<serde_json::Value>) -> Self {
        self.reauth_extra = reauth_extra;
        self
    }

    /// Check if this credential has a refresh token.
    #[inline]
    pub fn has_refresh_token(&self) -> bool {
        self.refresh_token.is_some()
    }

    /// Password-based re-login material is present (e.g. SOOP).
    #[inline]
    pub fn has_reauth_extra(&self) -> bool {
        self.reauth_extra.as_ref().is_some_and(|v| {
            v.get("username")
                .and_then(|u| u.as_str())
                .is_some_and(|s| !s.trim().is_empty())
                && v.get("password")
                    .and_then(|p| p.as_str())
                    .is_some_and(|s| !s.trim().is_empty())
        })
    }
}

/// Credential event for notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialEvent {
    /// Credentials were successfully refreshed.
    Refreshed {
        scope: CredentialScope,
        platform: String,
        expires_at: Option<DateTime<Utc>>,
        timestamp: DateTime<Utc>,
    },

    /// Credential refresh failed - action may be required.
    RefreshFailed {
        scope: CredentialScope,
        platform: String,
        error: String,
        /// Whether manual re-login is required.
        requires_relogin: bool,
        /// Number of consecutive failures.
        failure_count: u32,
        timestamp: DateTime<Utc>,
    },

    /// Credentials are invalid - manual re-login required.
    Invalid {
        scope: CredentialScope,
        platform: String,
        reason: String,
        /// Error code from platform API (e.g., -101 for Bilibili).
        error_code: Option<i32>,
        timestamp: DateTime<Utc>,
    },

    /// Credentials are expiring soon - proactive warning.
    ExpiringSoon {
        scope: CredentialScope,
        platform: String,
        expires_at: DateTime<Utc>,
        days_remaining: u32,
        timestamp: DateTime<Utc>,
    },
}

impl CredentialEvent {
    /// Event name for notification subscription matching.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Refreshed { .. } => "credential_refreshed",
            Self::RefreshFailed { .. } => "credential_refresh_failed",
            Self::Invalid { .. } => "credential_invalid",
            Self::ExpiringSoon { .. } => "credential_expiring",
        }
    }

    /// Severity level for filtering.
    pub fn severity(&self) -> NotificationPriority {
        match self {
            Self::Refreshed { .. } => NotificationPriority::Normal,
            Self::RefreshFailed {
                requires_relogin: true,
                ..
            } => NotificationPriority::Critical,
            Self::RefreshFailed {
                requires_relogin: false,
                ..
            } => NotificationPriority::High,
            Self::Invalid { .. } => NotificationPriority::Critical,
            Self::ExpiringSoon { days_remaining, .. } if *days_remaining <= 3 => {
                NotificationPriority::High
            }
            Self::ExpiringSoon { .. } => NotificationPriority::Normal,
        }
    }

    /// Generate a human-readable message for notifications.
    pub fn to_message(&self) -> String {
        match self {
            Self::Refreshed {
                platform, scope, ..
            } => crate::t_str!(
                "notification.credential.refreshed.message",
                platform = platform.as_str(),
                scope = scope.describe().as_str(),
            ),
            Self::RefreshFailed {
                platform,
                scope,
                error,
                requires_relogin,
                failure_count,
                ..
            } => {
                let key = if *requires_relogin {
                    "notification.credential.refresh_failed.message.requires_relogin"
                } else {
                    "notification.credential.refresh_failed.message.retrying"
                };
                crate::t_str!(
                    key,
                    platform = platform.as_str(),
                    scope = scope.describe().as_str(),
                    error = error.as_str(),
                    failure_count = failure_count.to_string().as_str(),
                )
            }
            Self::Invalid {
                platform,
                scope,
                reason,
                error_code,
                ..
            } => {
                // Pre-format the optional error_code as a string so the YAML
                // can just interpolate without conditional syntax. "N/A" is
                // intentional — the zh-CN translation renders it verbatim,
                // avoiding a branch on Option inside the YAML.
                let error_code = error_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "N/A".to_string());
                crate::t_str!(
                    "notification.credential.invalid.message",
                    platform = platform.as_str(),
                    scope = scope.describe().as_str(),
                    reason = reason.as_str(),
                    error_code = error_code.as_str(),
                )
            }
            Self::ExpiringSoon {
                platform,
                scope,
                days_remaining,
                expires_at,
                ..
            } => {
                let expires_at = expires_at.format("%Y-%m-%d").to_string();
                crate::t_str!(
                    "notification.credential.expiring_soon.message",
                    platform = platform.as_str(),
                    scope = scope.describe().as_str(),
                    days_remaining = days_remaining.to_string().as_str(),
                    expires_at = expires_at.as_str(),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{extractor_platform_extras, platform_reauth_extra};

    #[test]
    fn extracts_soop_reauthentication_fields() {
        let config = serde_json::json!({
            "username": " viewer ",
            "password": " secret-password ",
            "stream_password": "room-password",
        });

        assert_eq!(
            platform_reauth_extra("SOOP", Some(&config)),
            Some(serde_json::json!({
                "username": "viewer",
                "password": "secret-password",
            }))
        );
        assert!(platform_reauth_extra("twitch", Some(&config)).is_none());
    }

    #[test]
    fn rejects_incomplete_soop_reauthentication_fields() {
        let missing_password = serde_json::json!({ "username": "viewer" });
        assert!(platform_reauth_extra("soop", Some(&missing_password)).is_none());
    }

    #[test]
    fn strips_non_extractor_credential_metadata() {
        let extras = extractor_platform_extras(serde_json::json!({
            "username": "viewer",
            "password": "secret-password",
            "stream_password": "room-password",
            "refresh_token": "refresh",
            "access_token": "access",
            "session_cookies": "AuthTicket=secret",
        }));

        assert_eq!(
            extras,
            serde_json::json!({
                "username": "viewer",
                "password": "secret-password",
                "stream_password": "room-password",
            })
        );
    }
}
