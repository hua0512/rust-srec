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
#[derive(Clone)]
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

/// Renders `cookies`, `refresh_token`, `access_token` and `reauth_extra` as
/// `[redacted]` so that recording a `CredentialSource` as a `tracing` field cannot
/// write platform secrets to the log sinks installed by `crate::logging`. `scope`
/// and `platform_name` stay visible because they identify the credential.
impl std::fmt::Debug for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn redact(present: bool) -> &'static str {
            if present { "[redacted]" } else { "[unset]" }
        }

        f.debug_struct("CredentialSource")
            .field("scope", &self.scope)
            .field("platform_name", &self.platform_name)
            .field("cookies", &redact(!self.cookies.is_empty()))
            .field("refresh_token", &redact(self.refresh_token.is_some()))
            .field("access_token", &redact(self.access_token.is_some()))
            .field("reauth_extra", &redact(self.reauth_extra.is_some()))
            .finish()
    }
}

impl CredentialSource {
    /// Compare provider inputs, excluding display names and unrelated configuration.
    pub(crate) fn same_credentials(&self, other: &Self) -> bool {
        self.scope.cache_key() == other.scope.cache_key()
            && self
                .platform_name
                .eq_ignore_ascii_case(&other.platform_name)
            && self.cookies == other.cookies
            && self.refresh_token == other.refresh_token
            && self.access_token == other.access_token
            && self.reauth_extra == other.reauth_extra
    }

    pub(crate) fn after_refresh(&self, credentials: &super::manager::RefreshedCredentials) -> Self {
        let mut current = self.clone();
        current.cookies = credentials.cookies.clone();
        if let Some(token) = &credentials.refresh_token {
            current.refresh_token = Some(token.clone());
        }
        if let Some(token) = &credentials.access_token {
            current.access_token = Some(token.clone());
        }
        current
    }

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
    /// Selection exhaustion does not assert that any individual account is invalid.
    Unavailable {
        scope: CredentialScope,
        platform: String,
        reason_code: String,
        retry_at: Option<i64>,
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
        }
    }

    /// The scope as shown to users, naming the account for profile events.
    pub fn scope_text_in(&self, locale: &str) -> String {
        let (scope, label) = match self {
            Self::Unavailable { scope, .. } => (scope, None),
            Self::Refreshed {
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

    /// Generate a human-readable message for notifications, in the process-wide locale.
    pub fn to_message(&self) -> String {
        self.to_message_in(&crate::i18n::current_locale())
    }

    /// Generate a human-readable message for notifications, in `locale`.
    pub fn to_message_in(&self, locale: &str) -> String {
        let scope_text = self.scope_text_in(locale);
        match self {
            Self::Unavailable {
                platform,
                reason_code,
                retry_at,
                ..
            } => {
                let reason = unavailable_reason_key(reason_code)
                    .map_or_else(|| reason_code.clone(), |key| crate::t_str_in!(locale, key));
                let retry_at = retry_at
                    .and_then(DateTime::<Utc>::from_timestamp_millis)
                    .map_or_else(
                        || "—".to_string(),
                        |time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                    );
                crate::t_str_in!(
                    locale,
                    "notification.credential.unavailable.message",
                    platform = platform.as_str(),
                    scope = scope_text.as_str(),
                    reason = reason.as_str(),
                    retry_at = retry_at.as_str()
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
        }
    }
}

/// Unknown codes are shown as stored rather than hidden.
fn unavailable_reason_key(code: &str) -> Option<&'static str> {
    Some(match code {
        "cooling_down" => "notification.credential.unavailable.reason.cooling_down",
        "login_required" => "notification.credential.unavailable.reason.login_required",
        "profiles_disabled" => "notification.credential.unavailable.reason.profiles_disabled",
        "bound_profile_unavailable" => {
            "notification.credential.unavailable.reason.bound_profile_unavailable"
        }
        "binding_policy_changed" => {
            "notification.credential.unavailable.reason.binding_policy_changed"
        }
        "attempts_exhausted" => "notification.credential.unavailable.reason.attempts_exhausted",
        _ => return None,
    })
}

/// Every reason code `CredentialExecutionService` can report.
#[cfg(test)]
pub(crate) const UNAVAILABLE_REASON_CODES: &[&str] = &[
    "cooling_down",
    "login_required",
    "profiles_disabled",
    "bound_profile_unavailable",
    "binding_policy_changed",
    "attempts_exhausted",
];

#[cfg(test)]
mod tests {
    use super::{
        CredentialScope, CredentialSource, extractor_platform_extras, platform_reauth_extra,
    };

    /// Unavailable notices read as text rather than codes or epoch values, and
    /// profile events name the account beside the owner it belongs to.
    #[test]
    fn credential_notices_render_reasons_times_and_account_labels() {
        use super::{CredentialEvent, UNAVAILABLE_REASON_CODES};
        let scope = CredentialScope::Template {
            template_id: "template-1".to_string(),
            template_name: "Night shift".to_string(),
        };
        for locale in ["en", "zh-CN"] {
            for code in UNAVAILABLE_REASON_CODES {
                let message = CredentialEvent::Unavailable {
                    scope: scope.clone(),
                    platform: "bilibili".to_string(),
                    reason_code: code.to_string(),
                    retry_at: Some(1_788_784_496_123),
                    timestamp: chrono::Utc::now(),
                }
                .to_message_in(locale);
                assert!(!message.contains(code), "{locale}/{code}: {message}");
                assert!(
                    !message.contains("notification."),
                    "{locale}/{code}: {message}"
                );
                assert!(message.contains("2026-09-07 12:34:56 UTC"), "{message}");
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
    fn debug_output_redacts_credential_material() {
        let source = CredentialSource::new(
            CredentialScope::Platform {
                platform_id: "platform-1".to_string(),
                platform_name: "bilibili".to_string(),
            },
            "SESSDATA=cookie-sentinel".to_string(),
            Some("refresh-sentinel".to_string()),
            "bilibili".to_string(),
        )
        .with_access_token(Some("access-sentinel".to_string()))
        .with_reauth_extra(Some(serde_json::json!({
            "username": "viewer",
            "password": "password-sentinel",
        })));

        let rendered = format!("{source:?}");

        for secret in [
            "cookie-sentinel",
            "refresh-sentinel",
            "access-sentinel",
            "password-sentinel",
        ] {
            assert!(
                !rendered.contains(secret),
                "Debug output leaked {secret}: {rendered}"
            );
        }
        // Field names and provenance stay readable for diagnostics.
        assert!(rendered.contains("cookies"));
        assert!(rendered.contains("refresh_token"));
        assert!(rendered.contains("bilibili"));
    }

    #[test]
    fn debug_output_distinguishes_absent_credential_material() {
        let source = CredentialSource::new(
            CredentialScope::Platform {
                platform_id: "platform-1".to_string(),
                platform_name: "bilibili".to_string(),
            },
            String::new(),
            None,
            "bilibili".to_string(),
        );

        let rendered = format!("{source:?}");

        assert!(rendered.contains("cookies: \"[unset]\""), "{rendered}");
        assert!(
            rendered.contains("refresh_token: \"[unset]\""),
            "{rendered}"
        );
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
