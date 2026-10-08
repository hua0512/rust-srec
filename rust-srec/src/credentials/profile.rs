//! Profile records keep authentication bundles distinct from public summaries.

use serde::{Deserialize, Serialize};

use super::CredentialOwner;
use crate::proxies::ProxyRoute;

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("credential profile version changed")]
    StaleVersion,
    #[error("credential source changed")]
    SourceChanged,
    #[error("credential profile is disabled")]
    Disabled,
    #[error("credential profile is referenced by: {0}")]
    Referenced(ProfileReferences),
    #[error("invalid credential material: {0}")]
    InvalidMaterial(&'static str),
    #[error("credential owner is missing, retired, or inaccessible")]
    InvalidOwner,
    /// The write would leave these configs referring to profiles they cannot use.
    #[error("credential references would become inaccessible: {}", .0.join(", "))]
    InaccessibleReferences(Vec<String>),
    /// The provider or its admission could not answer; retrying later may work.
    #[error("credential provider unavailable: {0}")]
    ProviderUnavailable(&'static str),
    /// The selection is not allowed on a platform whose accounts are chosen
    /// per streamer, one at a time.
    #[error("{0}")]
    PerStreamerOnly(&'static str),
    /// A platform cannot be deleted while streamers record from it or
    /// templates choose accounts on it.
    #[error("platform is used by streamers {streamer_ids:?} and templates {template_ids:?}")]
    PlatformInUse {
        streamer_ids: Vec<String>,
        template_ids: Vec<String>,
    },
}

/// What keeps a profile in use: the scopes whose selections list it and the
/// live recordings bound to it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ProfileReferences {
    pub selections: Vec<SelectionReference>,
    pub recordings: Vec<RecordingReference>,
}

impl ProfileReferences {
    pub fn is_empty(&self) -> bool {
        self.selections.is_empty() && self.recordings.is_empty()
    }

    pub fn extend(&mut self, other: Self) {
        self.selections.extend(other.selections);
        self.recordings.extend(other.recordings);
    }
}

impl std::fmt::Display for ProfileReferences {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let selections = self.selections.iter().map(|selection| {
            format!(
                "{} {:?} ({}) on {}",
                selection.owner.kind(),
                selection.name,
                selection.owner.id(),
                selection.platform_name
            )
        });
        let recordings = self.recordings.iter().map(|recording| {
            format!(
                "recording {} of {:?}",
                recording.session_id, recording.streamer_name
            )
        });
        f.write_str(&selections.chain(recordings).collect::<Vec<_>>().join(", "))
    }
}

/// A scope whose selection lists the profile, with its display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct SelectionReference {
    pub owner: CredentialOwner,
    /// The platform's, template's or streamer's name; its ID when the owner
    /// is being removed concurrently.
    pub name: String,
    /// The platform the selection chooses accounts on. A template selects per
    /// platform, so this names which of its platforms.
    pub platform_id: String,
    pub platform_name: String,
}

/// A live recording bound to the profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RecordingReference {
    pub session_id: String,
    /// Absent once the streamer itself has been deleted.
    pub streamer_id: Option<String>,
    /// The streamer's current name, else the name it was recorded under.
    pub streamer_name: String,
}

/// Input-only material: never derive Serialize or Debug for secrets.
#[derive(Clone, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialMaterial {
    pub cookies: String,
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
    pub reauth_config: Option<serde_json::Value>,
}

impl std::fmt::Debug for CredentialMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialMaterial")
            .field("material", &"[redacted]")
            .finish()
    }
}

impl CredentialMaterial {
    /// Header safety plus the platform provider's rules for which material
    /// its accounts accept.
    pub fn validate(&self, platform: &str) -> Result<(), ProfileError> {
        if reqwest::header::HeaderValue::from_str(&self.cookies).is_err() {
            return Err(ProfileError::InvalidMaterial(
                "cookies must be a valid HTTP header",
            ));
        }
        let capabilities = super::provider(platform).capabilities();
        if let Some(config) = &self.reauth_config
            && (!capabilities.reauth_login
                || config.as_object().is_none_or(|fields| {
                    fields
                        .keys()
                        .any(|key| key != "username" && key != "password")
                })
                || super::platform_reauth_extra(platform, Some(config)).is_none())
        {
            return Err(ProfileError::InvalidMaterial(
                "unsupported or incomplete login material",
            ));
        }
        let token_auth = capabilities.token_only
            && match &self.access_token {
                Some(token) => {
                    if token.trim().is_empty()
                        || reqwest::header::HeaderValue::from_str(token).is_err()
                    {
                        return Err(ProfileError::InvalidMaterial(
                            "access token must be nonblank and valid HTTP header material",
                        ));
                    }
                    true
                }
                None => false,
            };
        if self.cookies.trim().is_empty() && self.reauth_config.is_none() && !token_auth {
            return Err(ProfileError::InvalidMaterial(
                "cookies or supported login material are required",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, sqlx::FromRow)]
pub struct CredentialProfile {
    pub id: String,
    pub platform_config_id: String,
    pub label: String,
    pub enabled: bool,
    pub cookies: String,
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
    pub reauth_config: Option<String>,
    pub revision: i64,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    /// When an operation last received the account's material. Coalesced, so
    /// it may trail the latest use by a few minutes; kept across revisions.
    pub last_used_at: Option<i64>,
    /// The account's own route, as stored; see [`Self::route`].
    pub proxy_route: String,
    pub proxy_id: Option<String>,
}

impl std::fmt::Debug for CredentialProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialProfile")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("material", &"[redacted]")
            .finish()
    }
}

impl CredentialProfile {
    /// Profiles belong to their platform; any scope on it may select them.
    pub fn owner(&self) -> CredentialOwner {
        CredentialOwner::Platform {
            platform_id: self.platform_config_id.clone(),
        }
    }

    pub fn material(&self) -> crate::Result<CredentialMaterial> {
        Ok(CredentialMaterial {
            cookies: self.cookies.clone(),
            refresh_token: self.refresh_token.clone(),
            access_token: self.access_token.clone(),
            reauth_config: self
                .reauth_config
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
        })
    }

    /// The account's own route; `inherit` follows the operation using it.
    pub fn route(&self) -> crate::Result<ProxyRoute> {
        Ok(ProxyRoute::from_columns(
            &self.proxy_route,
            self.proxy_id.as_deref(),
        )?)
    }

    pub fn summary(&self) -> CredentialProfileSummary {
        CredentialProfileSummary {
            id: self.id.clone(),
            platform_config_id: self.platform_config_id.clone(),
            label: self.label.clone(),
            enabled: self.enabled,
            revision: self.revision,
            version: self.version,
            has_cookies: !self.cookies.is_empty(),
            has_refresh_token: self.refresh_token.is_some(),
            has_access_token: self.access_token.is_some(),
            has_reauth: self.reauth_config.is_some(),
            // The schema admits only well-formed routes.
            proxy_route: self.route().unwrap_or_default(),
            last_used_at: self.last_used_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CredentialProfileSummary {
    pub id: String,
    pub platform_config_id: String,
    pub label: String,
    pub enabled: bool,
    pub revision: i64,
    pub version: i64,
    pub has_cookies: bool,
    pub has_refresh_token: bool,
    pub has_access_token: bool,
    pub has_reauth: bool,
    /// The account's own route; `inherit` follows the recording using it.
    pub proxy_route: ProxyRoute,
    /// When an operation last received the account's material, in epoch
    /// milliseconds; it may trail the latest use by a few minutes.
    pub last_used_at: Option<i64>,
}

/// The last conclusion about an account at its current revision.
///
/// The column's CHECK constraint admits exactly these values, so any other
/// stored text fails decoding instead of being guessed at.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum CredentialValidity {
    Unknown,
    Valid,
    NeedsRefresh,
    Invalid,
}

/// Why the stored health conclusion was reached.
///
/// Only this crate writes the column, always through this type, so an
/// unrecognized value means the database was written by an incompatible build
/// and fails decoding rather than being reported as a different reason.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum HealthReason {
    /// The provider rejected the account; only a new login restores it.
    LoginRequired,
    /// A user-requested validation produced the conclusion.
    ManualValidation,
    /// A provider refresh failed without demanding a new login.
    RefreshFailed,
    /// A status check found the account needs repair before use.
    RepairRequired,
    /// An extraction was rejected and a repair is queued.
    AuthenticationFailed,
}

impl HealthReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LoginRequired => "login_required",
            Self::ManualValidation => "manual_validation",
            Self::RefreshFailed => "refresh_failed",
            Self::RepairRequired => "repair_required",
            Self::AuthenticationFailed => "authentication_failed",
        }
    }
}

/// Why a credential policy has no account to offer right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// The remaining candidates need a new login.
    LoginRequired,
    /// Every candidate is disabled.
    ProfilesDisabled,
    /// A bound session's account cannot be used and switching is not allowed.
    BoundProfileUnavailable,
    /// The selection changed under a bound session that cannot switch.
    BindingPolicyChanged,
    /// The attempt budget ran out before an account succeeded.
    AttemptsExhausted,
}

/// Every reason code `CredentialExecutionService` can report.
#[cfg(test)]
pub(crate) const UNAVAILABLE_REASON_CODES: &[UnavailableReason] = &[
    UnavailableReason::LoginRequired,
    UnavailableReason::ProfilesDisabled,
    UnavailableReason::BoundProfileUnavailable,
    UnavailableReason::BindingPolicyChanged,
    UnavailableReason::AttemptsExhausted,
];

impl UnavailableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LoginRequired => "login_required",
            Self::ProfilesDisabled => "profiles_disabled",
            Self::BoundProfileUnavailable => "bound_profile_unavailable",
            Self::BindingPolicyChanged => "binding_policy_changed",
            Self::AttemptsExhausted => "attempts_exhausted",
        }
    }
}

impl std::fmt::Display for UnavailableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct CredentialProfileHealth {
    pub profile_id: String,
    pub revision: i64,
    pub validity: CredentialValidity,
    pub last_check_at: Option<i64>,
    pub last_refresh_at: Option<i64>,
    pub refresh_failure_count: i64,
    pub last_failure_at: Option<i64>,
    pub last_notified_failure_count: i64,
    pub reason_code: Option<HealthReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CredentialUnavailable {
    pub reason: UnavailableReason,
    pub policy_generation: String,
}

impl std::fmt::Display for CredentialUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "credentials unavailable ({})", self.reason)
    }
}

impl std::error::Error for CredentialUnavailable {}

#[cfg(test)]
mod material_tests {
    use super::*;

    #[test]
    fn twitch_token_only_bundle_is_supported_without_mixing_other_platforms() {
        let mut material = CredentialMaterial {
            cookies: String::new(),
            refresh_token: None,
            access_token: Some("account-token".into()),
            reauth_config: None,
        };
        assert!(material.validate("Twitch").is_ok());
        assert!(material.validate("bilibili").is_err());
        material.access_token = Some("injected\r\nHeader: value".into());
        assert!(material.validate("twitch").is_err());
        material.access_token = Some(" ".into());
        assert!(material.validate("twitch").is_err());
    }
}

#[cfg(test)]
mod health_code_tests {
    use super::*;

    const VALIDITIES: [CredentialValidity; 4] = [
        CredentialValidity::Unknown,
        CredentialValidity::Valid,
        CredentialValidity::NeedsRefresh,
        CredentialValidity::Invalid,
    ];
    const REASONS: [HealthReason; 5] = [
        HealthReason::LoginRequired,
        HealthReason::ManualValidation,
        HealthReason::RefreshFailed,
        HealthReason::RepairRequired,
        HealthReason::AuthenticationFailed,
    ];

    /// The API and the database share one spelling per code.
    #[test]
    fn json_spelling_matches_stored_text() {
        for reason in REASONS {
            assert_eq!(serde_json::json!(reason), reason.as_str());
        }
        for reason in UNAVAILABLE_REASON_CODES {
            assert_eq!(serde_json::json!(reason), reason.as_str());
        }
    }

    #[tokio::test]
    async fn stored_text_round_trips_and_unknown_text_is_rejected() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        for validity in VALIDITIES {
            let (decoded, text): (CredentialValidity, String) = sqlx::query_as("SELECT ?1, ?1")
                .bind(validity)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(decoded, validity);
            assert_eq!(serde_json::json!(validity), text);
        }
        for reason in REASONS {
            let (decoded, text): (HealthReason, String) = sqlx::query_as("SELECT ?1, ?1")
                .bind(reason)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(decoded, reason);
            assert_eq!(text, reason.as_str());
        }
        assert!(
            sqlx::query_scalar::<_, CredentialValidity>("SELECT 'expired'")
                .fetch_one(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query_scalar::<_, HealthReason>("SELECT 'expired'")
                .fetch_one(&pool)
                .await
                .is_err()
        );
    }
}
