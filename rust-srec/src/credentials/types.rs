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
    /// The scope of a profile owner, labelled with the owner's display name.
    pub fn for_owner(owner: &super::CredentialOwner, name: String) -> Self {
        match owner {
            super::CredentialOwner::Platform { platform_id } => Self::Platform {
                platform_id: platform_id.clone(),
                platform_name: name,
            },
            super::CredentialOwner::Template { template_id } => Self::Template {
                template_id: template_id.clone(),
                template_name: name,
            },
            super::CredentialOwner::Streamer { streamer_id } => Self::Streamer {
                streamer_id: streamer_id.clone(),
                streamer_name: name,
            },
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
}

/// Credential event for notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialEvent {
    /// Selection exhaustion does not assert that any individual account is invalid.
    Unavailable {
        scope: CredentialScope,
        platform: String,
        reason_code: super::UnavailableReason,
        timestamp: DateTime<Utc>,
    },
    /// Credentials were successfully refreshed.
    Refreshed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_label: Option<String>,
        scope: CredentialScope,
        platform: String,
        expires_at: Option<DateTime<Utc>>,
        timestamp: DateTime<Utc>,
    },

    /// Credential refresh failed - action may be required.
    RefreshFailed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_label: Option<String>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_label: Option<String>,
        scope: CredentialScope,
        platform: String,
        reason: String,
        /// Error code from platform API (e.g., -101 for Bilibili).
        error_code: Option<i32>,
        timestamp: DateTime<Utc>,
    },

    /// Credentials are expiring soon - proactive warning.
    ExpiringSoon {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_label: Option<String>,
        scope: CredentialScope,
        platform: String,
        expires_at: DateTime<Utc>,
        days_remaining: u32,
        timestamp: DateTime<Utc>,
    },

    /// Session cookies obtained during a check (e.g. SOOP reactive login)
    /// could not be stored; the next check has to log in again.
    SessionSaveFailed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_label: Option<String>,
        scope: CredentialScope,
        platform: String,
        error: String,
        timestamp: DateTime<Utc>,
    },
}

impl CredentialEvent {
    /// Event name for notification subscription matching.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "credential_unavailable",
            Self::Refreshed { .. } => "credential_refreshed",
            Self::RefreshFailed { .. } => "credential_refresh_failed",
            Self::Invalid { .. } => "credential_invalid",
            Self::ExpiringSoon { .. } => "credential_expiring",
            Self::SessionSaveFailed { .. } => "credential_session_save_failed",
        }
    }

    /// Severity level for filtering.
    pub fn severity(&self) -> NotificationPriority {
        match self {
            Self::Unavailable { .. } => NotificationPriority::High,
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
            Self::SessionSaveFailed { .. } => NotificationPriority::High,
        }
    }

    /// The scope as shown to users, naming the account for profile events.
    pub fn scope_text_in(&self, locale: &str) -> String {
        let (scope, label) = match self {
            Self::Unavailable { scope, .. } => (scope, None),
            Self::SessionSaveFailed {
                scope,
                profile_label,
                ..
            }
            | Self::Refreshed {
                scope,
                profile_label,
                ..
            }
            | Self::RefreshFailed {
                scope,
                profile_label,
                ..
            }
            | Self::Invalid {
                scope,
                profile_label,
                ..
            }
            | Self::ExpiringSoon {
                scope,
                profile_label,
                ..
            } => (scope, profile_label.as_deref()),
        };
        match label {
            Some(label) => crate::t_str_in!(
                locale,
                "notification.credential.scope_with_account",
                scope = scope.describe().as_str(),
                account = label,
            ),
            None => scope.describe(),
        }
    }

    /// Generate a human-readable message for notifications, in `locale`.
    pub fn to_message_in(&self, locale: &str) -> String {
        let scope_text = self.scope_text_in(locale);
        match self {
            Self::Unavailable {
                platform,
                reason_code,
                ..
            } => {
                let reason = crate::t_str_in!(locale, unavailable_reason_key(*reason_code));
                crate::t_str_in!(
                    locale,
                    "notification.credential.unavailable.message",
                    platform = platform.as_str(),
                    scope = scope_text.as_str(),
                    reason = reason.as_str(),
                )
            }
            Self::Refreshed { platform, .. } => crate::t_str_in!(
                locale,
                "notification.credential.refreshed.message",
                platform = platform.as_str(),
                scope = scope_text.as_str(),
            ),
            Self::RefreshFailed {
                platform,
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
                crate::t_str_in!(
                    locale,
                    key,
                    platform = platform.as_str(),
                    scope = scope_text.as_str(),
                    error = error.as_str(),
                    failure_count = failure_count.to_string().as_str(),
                )
            }
            Self::Invalid {
                platform,
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
                crate::t_str_in!(
                    locale,
                    "notification.credential.invalid.message",
                    platform = platform.as_str(),
                    scope = scope_text.as_str(),
                    reason = reason.as_str(),
                    error_code = error_code.as_str(),
                )
            }
            Self::ExpiringSoon {
                platform,
                days_remaining,
                expires_at,
                ..
            } => {
                let expires_at = expires_at.format("%Y-%m-%d").to_string();
                crate::t_str_in!(
                    locale,
                    "notification.credential.expiring_soon.message",
                    platform = platform.as_str(),
                    scope = scope_text.as_str(),
                    days_remaining = days_remaining.to_string().as_str(),
                    expires_at = expires_at.as_str(),
                )
            }
            Self::SessionSaveFailed {
                platform, error, ..
            } => crate::t_str_in!(
                locale,
                "notification.credential.session_save_failed.message",
                platform = platform.as_str(),
                scope = scope_text.as_str(),
                error = error.as_str(),
            ),
        }
    }
}

/// Notification text key for each unavailable reason.
fn unavailable_reason_key(reason: super::UnavailableReason) -> &'static str {
    use super::UnavailableReason;
    match reason {
        UnavailableReason::LoginRequired => {
            "notification.credential.unavailable.reason.login_required"
        }
        UnavailableReason::ProfilesDisabled => {
            "notification.credential.unavailable.reason.profiles_disabled"
        }
        UnavailableReason::BoundProfileUnavailable => {
            "notification.credential.unavailable.reason.bound_profile_unavailable"
        }
        UnavailableReason::BindingPolicyChanged => {
            "notification.credential.unavailable.reason.binding_policy_changed"
        }
        UnavailableReason::AttemptsExhausted => {
            "notification.credential.unavailable.reason.attempts_exhausted"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CredentialScope;
    use crate::credentials::platform_reauth_extra;

    /// Unavailable notices read as text rather than codes, and
    /// profile events name the account beside the owner it belongs to.
    #[test]
    fn credential_notices_render_reasons_and_account_labels() {
        use super::CredentialEvent;
        use crate::credentials::profile::UNAVAILABLE_REASON_CODES;
        let scope = CredentialScope::Template {
            template_id: "template-1".to_string(),
            template_name: "Night shift".to_string(),
        };
        for locale in ["en", "zh-CN"] {
            for code in UNAVAILABLE_REASON_CODES {
                let message = CredentialEvent::Unavailable {
                    scope: scope.clone(),
                    platform: "bilibili".to_string(),
                    reason_code: *code,
                    timestamp: chrono::Utc::now(),
                }
                .to_message_in(locale);
                let code = code.as_str();
                assert!(!message.contains(code), "{locale}/{code}: {message}");
                assert!(
                    !message.contains("notification."),
                    "{locale}/{code}: {message}"
                );
                assert!(message.contains("Night shift"), "{message}");
            }
            let invalid = CredentialEvent::Invalid {
                profile_id: Some("profile-1".to_string()),
                profile_label: Some("Backup account".to_string()),
                scope: scope.clone(),
                platform: "bilibili".to_string(),
                reason: "login_required".to_string(),
                error_code: None,
                timestamp: chrono::Utc::now(),
            };
            let text = invalid.scope_text_in(locale);
            assert!(text.contains("Template: Night shift"), "{text}");
            assert!(text.contains("Backup account"), "{text}");
            assert!(invalid.to_message_in(locale).contains("Backup account"));
        }
    }

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
}
