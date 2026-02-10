//! Core credential types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::notification::NotificationPriority;

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
        }
    }

    /// Create a new credential source with an access token.
    pub fn with_access_token(mut self, access_token: Option<String>) -> Self {
        self.access_token = access_token;
        self
    }

    /// Check if this credential has a refresh token.
    #[inline]
    pub fn has_refresh_token(&self) -> bool {
        self.refresh_token.is_some()
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
            } => {
                format!(
                    "✅ {} credentials refreshed successfully ({})",
                    platform,
                    scope.describe()
                )
            }
            Self::RefreshFailed {
                platform,
                scope,
                error,
                requires_relogin,
                failure_count,
                ..
            } => {
                if *requires_relogin {
                    format!(
                        "❌ {} credential refresh failed - MANUAL RE-LOGIN REQUIRED\n\
                         Scope: {}\n\
                         Error: {}\n\
                         Failures: {}",
                        platform,
                        scope.describe(),
                        error,
                        failure_count
                    )
                } else {
                    format!(
                        "⚠️ {} credential refresh failed (attempt {})\n\
                         Scope: {}\n\
                         Error: {}",
                        platform,
                        failure_count,
                        scope.describe(),
                        error
                    )
                }
            }
            Self::Invalid {
                platform,
                scope,
                reason,
                error_code,
                ..
            } => {
                format!(
                    "🚫 {} credentials are INVALID - manual re-login required\n\
                     Scope: {}\n\
                     Reason: {}\n\
                     Error code: {}",
                    platform,
                    scope.describe(),
                    reason,
                    error_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "N/A".to_string())
                )
            }
            Self::ExpiringSoon {
                platform,
                scope,
                days_remaining,
                expires_at,
                ..
            } => {
                format!(
                    "⏰ {} credentials expiring in {} days ({})\n\
                     Scope: {}\n\
                     Action: Consider refreshing soon",
                    platform,
                    days_remaining,
                    expires_at.format("%Y-%m-%d"),
                    scope.describe()
                )
            }
        }
    }
}
