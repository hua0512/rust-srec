//! Pure credential value types, policies, sites, and validation rules shared
//! across storage and runtime credential services.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::proxy::ProxyRoute;
use crate::{Error, Result};

// ============================================================================
// Platform account policy rules (sync'd with registered process providers)
// ============================================================================

/// Platform-level material capabilities needed by pure validation and config
/// field stripping without depending on provider HTTP implementations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PlatformAccountPolicy {
    pub token_only: bool,
    pub reauth_login: bool,
    pub accounts_optional: bool,
}

static REGISTERED_POLICIES: RwLock<Vec<(String, PlatformAccountPolicy)>> = RwLock::new(Vec::new());

/// Record a runtime-registered provider's account policy so pure validation
/// and field isolation match the registered provider.
pub(crate) fn register_platform_account_policy(platform: &str, policy: PlatformAccountPolicy) {
    let platform = platform.to_ascii_lowercase();
    let mut policies = REGISTERED_POLICIES.write();
    if let Some((_, existing)) = policies.iter_mut().find(|(name, _)| *name == platform) {
        *existing = policy;
    } else {
        policies.push((platform, policy));
    }
}

pub(crate) fn platform_account_policy(platform: &str) -> PlatformAccountPolicy {
    if let Some((_, policy)) = REGISTERED_POLICIES
        .read()
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(platform))
    {
        return *policy;
    }
    if platform.eq_ignore_ascii_case("twitch") {
        PlatformAccountPolicy {
            token_only: true,
            reauth_login: false,
            accounts_optional: false,
        }
    } else if platform.eq_ignore_ascii_case("soop") {
        PlatformAccountPolicy {
            token_only: false,
            reauth_login: true,
            accounts_optional: false,
        }
    } else if platform.eq_ignore_ascii_case("weibo") {
        PlatformAccountPolicy {
            token_only: false,
            reauth_login: false,
            accounts_optional: true,
        }
    } else {
        PlatformAccountPolicy::default()
    }
}

/// Configuration keys that are account login on `platform` rather than
/// content settings. Elsewhere `username`/`password` are room passwords.
pub fn login_fields(platform: &str) -> &'static [&'static str] {
    if platform_account_policy(platform).reauth_login {
        &["username", "password"]
    } else {
        &[]
    }
}

/// The automatic sign-in login in `config`, when `platform` supports one and
/// both fields are present.
pub(crate) fn platform_reauth_extra(
    platform: &str,
    config: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    if !platform_account_policy(platform).reauth_login {
        return None;
    }
    let field = |name: &str| {
        config?
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    Some(serde_json::json!({
        "username": field("username")?,
        "password": field("password")?,
    }))
}

// ============================================================================
// Selection and binding policies
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolStrategy {
    RoundRobin,
    Priority,
}

/// A scope that stores no selection inherits; at the platform that is anonymous.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialSelection {
    Inherit,
    None,
    Fixed {
        credential_id: String,
    },
    Pool {
        credential_ids: Vec<String>,
        strategy: PoolStrategy,
        failover: bool,
        max_attempts: u8,
        /// Check without an account when no account in the pool can be used.
        /// Omitted when off, so pools saved before the setting existed keep
        /// their policy generation and the sessions bound to it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        anonymous_fallback: bool,
    },
}

impl<'de> Deserialize<'de> for CredentialSelection {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Inherit {},
            None {},
            Fixed {
                credential_id: String,
            },
            Pool {
                credential_ids: Vec<String>,
                #[serde(default = "default_strategy")]
                strategy: PoolStrategy,
                #[serde(default = "default_failover")]
                failover: bool,
                #[serde(default = "default_max_attempts")]
                max_attempts: u8,
                #[serde(default)]
                anonymous_fallback: bool,
            },
        }
        let selection = match Wire::deserialize(deserializer)? {
            Wire::Inherit {} => Self::Inherit,
            Wire::None {} => Self::None,
            Wire::Fixed { credential_id } => Self::Fixed { credential_id },
            Wire::Pool {
                credential_ids,
                strategy,
                failover,
                max_attempts,
                anonymous_fallback,
            } => Self::Pool {
                credential_ids,
                strategy,
                failover,
                max_attempts,
                anonymous_fallback,
            },
        };
        selection.validate().map_err(serde::de::Error::custom)?;
        Ok(selection)
    }
}

fn default_strategy() -> PoolStrategy {
    PoolStrategy::Priority
}

fn default_failover() -> bool {
    true
}

fn default_max_attempts() -> u8 {
    3
}

impl CredentialSelection {
    /// Stable label for structured logs.
    pub fn strategy_name(&self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::None => "none",
            Self::Fixed { .. } => "fixed",
            Self::Pool {
                strategy: PoolStrategy::RoundRobin,
                ..
            } => "round_robin",
            Self::Pool {
                strategy: PoolStrategy::Priority,
                ..
            } => "priority",
        }
    }

    /// Whether checks fall back to no account when the pool has none usable.
    pub fn anonymous_fallback(&self) -> bool {
        matches!(
            self,
            Self::Pool {
                anonymous_fallback: true,
                ..
            }
        )
    }

    pub fn profile_ids(&self) -> Vec<&str> {
        match self {
            Self::Fixed { credential_id } => vec![credential_id.as_str()],
            Self::Pool { credential_ids, .. } => {
                credential_ids.iter().map(String::as_str).collect()
            }
            Self::Inherit | Self::None => Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        let ids = self.profile_ids();
        let mut seen = HashSet::new();
        if ids
            .iter()
            .any(|id| id.trim().is_empty() || !seen.insert(*id))
        {
            return Err(Error::validation(
                "credential profile IDs must be nonblank and unique",
            ));
        }
        if let Self::Pool { max_attempts, .. } = self
            && (ids.is_empty() || !(1..=10).contains(max_attempts))
        {
            return Err(Error::validation(
                "credential pools need at least one profile and 1..10 attempts",
            ));
        }
        Ok(())
    }

    pub fn from_value(value: serde_json::Value) -> Result<Self> {
        serde_json::from_value(value).map_err(|_| Error::validation("invalid credential selection"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialOwner {
    Platform { platform_id: String },
    Template { template_id: String },
    Streamer { streamer_id: String },
}

impl CredentialOwner {
    /// Resolution order: a streamer's selection beats its template's, which
    /// beats its platform's. Lower ranks win.
    pub(crate) fn precedence(&self) -> u8 {
        match self {
            Self::Streamer { .. } => 0,
            Self::Template { .. } => 1,
            Self::Platform { .. } => 2,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Platform { .. } => "platform",
            Self::Template { .. } => "template",
            Self::Streamer { .. } => "streamer",
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Platform { platform_id } => platform_id,
            Self::Template { template_id } => template_id,
            Self::Streamer { streamer_id } => streamer_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCredentialPolicy {
    pub platform_id: String,
    pub owner: CredentialOwner,
    pub selection: CredentialSelection,
    /// Opaque digest of canonical policy/owner/platform, independent of account material.
    pub generation: String,
}

impl ResolvedCredentialPolicy {
    pub fn new(
        platform_id: String,
        owner: CredentialOwner,
        selection: CredentialSelection,
    ) -> Result<Self> {
        selection.validate()?;
        let canonical = serde_json::to_vec(&(&platform_id, &owner, &selection))?;
        let generation = hex::encode(Sha256::digest(canonical));
        Ok(Self {
            platform_id,
            owner,
            selection,
            generation,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialIdentity {
    Profile { profile_id: String },
    Anonymous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialBinding {
    pub identity: CredentialIdentity,
    pub revision: u64,
    pub policy: ResolvedCredentialPolicy,
    pub epoch: u64,
}

/// Represents the configuration layer where credentials are defined.
///
/// Credentials can be defined at Platform, Template, or Streamer scope.
/// Global scope is explicitly not supported for credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialScope {
    Platform {
        platform_id: String,
        platform_name: String,
    },
    Template {
        template_id: String,
        template_name: String,
    },
    Streamer {
        streamer_id: String,
        streamer_name: String,
    },
}

impl CredentialScope {
    /// The scope of a profile owner, labelled with the owner's display name.
    pub fn for_owner(owner: &CredentialOwner, name: String) -> Self {
        match owner {
            CredentialOwner::Platform { platform_id } => Self::Platform {
                platform_id: platform_id.clone(),
                platform_name: name,
            },
            CredentialOwner::Template { template_id } => Self::Template {
                template_id: template_id.clone(),
                template_name: name,
            },
            CredentialOwner::Streamer { streamer_id } => Self::Streamer {
                streamer_id: streamer_id.clone(),
                streamer_name: name,
            },
        }
    }

    /// Human-readable description of the scope.
    pub fn describe(&self) -> String {
        match self {
            Self::Platform { platform_name, .. } => format!("Platform: {}", platform_name),
            Self::Template { template_name, .. } => format!("Template: {}", template_name),
            Self::Streamer { streamer_name, .. } => format!("Streamer: {}", streamer_name),
        }
    }
}

// ============================================================================
// Profiles, health, and errors
// ============================================================================

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
    /// A site of a Streamlink account is not a host name, or the account is
    /// not on the Streamlink platform.
    #[error("{0}")]
    InvalidSite(String),
    /// Another account already names the site.
    #[error("site {site} already belongs to account {label:?}")]
    SiteTaken {
        site: String,
        profile_id: String,
        label: String,
    },
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordingReference {
    pub session_id: String,
    /// Absent once the streamer itself has been deleted.
    pub streamer_id: Option<String>,
    /// The streamer's current name, else the name it was recorded under.
    pub streamer_name: String,
}

/// Input-only material: never derive Serialize or Debug for secrets.
#[derive(Clone, Deserialize)]
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
    /// Header safety plus the platform's rules for which material its accounts
    /// accept.
    pub fn validate(&self, platform: &str) -> std::result::Result<(), ProfileError> {
        if reqwest::header::HeaderValue::from_str(&self.cookies).is_err() {
            return Err(ProfileError::InvalidMaterial(
                "cookies must be a valid HTTP header",
            ));
        }
        let policy = platform_account_policy(platform);
        if let Some(config) = &self.reauth_config
            && (!policy.reauth_login
                || config.as_object().is_none_or(|fields| {
                    fields
                        .keys()
                        .any(|key| key != "username" && key != "password")
                })
                || platform_reauth_extra(platform, Some(config)).is_none())
        {
            return Err(ProfileError::InvalidMaterial(
                "unsupported or incomplete login material",
            ));
        }
        let token_auth = policy.token_only
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

#[derive(Clone)]
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

    pub fn material(&self) -> Result<CredentialMaterial> {
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
    pub fn route(&self) -> Result<ProxyRoute> {
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
            proxy_route: self.route().unwrap_or_default(),
            last_used_at: self.last_used_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialValidity {
    Unknown,
    Valid,
    NeedsRefresh,
    Invalid,
}

impl CredentialValidity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Valid => "valid",
            Self::NeedsRefresh => "needs_refresh",
            Self::Invalid => "invalid",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            "valid" => Some(Self::Valid),
            "needs_refresh" => Some(Self::NeedsRefresh),
            "invalid" => Some(Self::Invalid),
            _ => None,
        }
    }
}

/// Why the stored health conclusion was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "login_required" => Some(Self::LoginRequired),
            "manual_validation" => Some(Self::ManualValidation),
            "refresh_failed" => Some(Self::RefreshFailed),
            "repair_required" => Some(Self::RepairRequired),
            "authentication_failed" => Some(Self::AuthenticationFailed),
            _ => None,
        }
    }
}

/// Why a credential policy has no account to offer right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Every reason code the credential executor can report.
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

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Credential lifecycle event carried into notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialEvent {
    /// Selection exhaustion does not assert that any individual account is invalid.
    Unavailable {
        scope: CredentialScope,
        platform: String,
        reason_code: UnavailableReason,
        timestamp: Timestamp,
    },
    /// Credentials were successfully refreshed.
    Refreshed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_label: Option<String>,
        scope: CredentialScope,
        platform: String,
        expires_at: Option<Timestamp>,
        timestamp: Timestamp,
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
        timestamp: Timestamp,
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
        timestamp: Timestamp,
    },
    /// Credentials are expiring soon - proactive warning.
    ExpiringSoon {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_label: Option<String>,
        scope: CredentialScope,
        platform: String,
        expires_at: Timestamp,
        days_remaining: u32,
        timestamp: Timestamp,
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
        timestamp: Timestamp,
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

    /// Timestamp when this credential event occurred.
    pub fn timestamp(&self) -> Timestamp {
        match self {
            Self::Unavailable { timestamp, .. }
            | Self::Refreshed { timestamp, .. }
            | Self::RefreshFailed { timestamp, .. }
            | Self::Invalid { timestamp, .. }
            | Self::ExpiringSoon { timestamp, .. }
            | Self::SessionSaveFailed { timestamp, .. } => *timestamp,
        }
    }

    /// The owning scope and optional account label carried by this event.
    pub fn scope_and_label(&self) -> (&CredentialScope, Option<&str>) {
        match self {
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
        }
    }
}

/// Replacement material from a successful refresh. Absent tokens keep the
/// profile's current ones.
#[derive(Clone)]
pub struct RefreshedCredentials {
    pub cookies: String,
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
    /// Expected expiration time, if known.
    pub expires_at: Option<Timestamp>,
}

impl std::fmt::Debug for RefreshedCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshedCredentials")
            .field("material", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

// ============================================================================
// Attention summaries
// ============================================================================

/// Consecutive failed refreshes after which a rejected account is reported.
/// The repository restarts the count when a failure follows the previous one
/// by more than six hours. The third failure is also when the refresh-failure
/// notification first repeats, so one transient failure lists nothing.
pub const ATTENTION_REFRESH_FAILURES: i64 = 3;

/// What the user has to do about an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    /// Only a new login or new cookies make the account usable again.
    LoginRequired,
    /// The platform rejected the account and its automatic refresh keeps
    /// failing; a new login is the likely fix.
    RefreshFailing,
}

impl AttentionReason {
    /// The reason an account's health calls for, if it calls for one.
    pub fn for_health(health: &CredentialProfileHealth) -> Option<Self> {
        match health.validity {
            CredentialValidity::Invalid => Some(Self::LoginRequired),
            CredentialValidity::NeedsRefresh
                if health.refresh_failure_count >= ATTENTION_REFRESH_FAILURES =>
            {
                Some(Self::RefreshFailing)
            }
            _ => None,
        }
    }
}

/// An enabled account that needs the user, with its platform's name.
#[derive(Debug, Clone, Serialize)]
pub struct CredentialAttention {
    pub profile: CredentialProfileSummary,
    pub platform_name: String,
    pub reason: AttentionReason,
    /// Health at the account's current revision.
    pub health: CredentialProfileHealth,
}

// ============================================================================
// Streamlink site normalization and matching
// ============================================================================

/// How many sites one account may name.
pub const MAX_SITES: usize = 32;

/// A Streamlink streamer's site and the accounts that name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamerSite {
    /// The host of the streamer's URL, without a leading `www.`.
    pub host: String,
    /// The most specific site of an account that covers the host. Without
    /// an account of its own, the streamer uses that account.
    pub site: Option<String>,
    /// The accounts whose sites cover the host, most specific site first.
    pub accounts: Vec<String>,
}

fn host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_owned())
}

fn is_ip(host: &str) -> bool {
    host.starts_with('[') || host.parse::<std::net::Ipv4Addr>().is_ok()
}

/// The site `input` names, as stored: the host of a host name or URL, without
/// a leading `www.`. A domain needs at least two labels, so a site never
/// covers a whole top-level domain.
pub fn normalize_site(input: &str) -> std::result::Result<String, ProfileError> {
    let input = input.trim();
    let invalid =
        || ProfileError::InvalidSite(format!("{input:?} is not a site such as youtube.com"));
    if input.is_empty() {
        return Err(invalid());
    }
    let url = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let host = host(&url).ok_or_else(invalid)?;
    if is_ip(&host) {
        return Ok(host);
    }
    let host = match host.strip_prefix("www.") {
        Some(rest) if rest.contains('.') => rest.to_owned(),
        _ => host,
    };
    if !host.contains('.') || host.len() > 253 {
        return Err(invalid());
    }
    Ok(host)
}

/// `sites` as an account on `platform_name` stores them: normalized, sorted
/// and without duplicates. Only Streamlink accounts name sites.
pub fn normalize_sites(
    platform_name: &str,
    sites: &[String],
) -> std::result::Result<Vec<String>, ProfileError> {
    if sites.is_empty() {
        return Ok(Vec::new());
    }
    if !crate::domain::is_streamlink_platform(platform_name) {
        return Err(ProfileError::InvalidSite(
            "Only Streamlink accounts name the sites they are for".to_owned(),
        ));
    }
    let mut normalized = sites
        .iter()
        .map(|site| normalize_site(site))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    normalized.sort();
    normalized.dedup();
    if normalized.len() > MAX_SITES {
        return Err(ProfileError::InvalidSite(format!(
            "An account names at most {MAX_SITES} sites"
        )));
    }
    Ok(normalized)
}

/// The host of a streamer's `url` as shown beside its sites: without a
/// leading `www.`.
pub fn site_host(url: &str) -> Option<String> {
    let host = host(url)?;
    Some(match host.strip_prefix("www.") {
        Some(rest) if rest.contains('.') => rest.to_owned(),
        _ => host,
    })
}

/// The sites that would cover a streamer's `url`, most specific first: its
/// host and every parent domain with at least two labels.
pub fn covering_sites(url: &str) -> Vec<String> {
    let Some(host) = host(url) else {
        return Vec::new();
    };
    if is_ip(&host) {
        return vec![host];
    }
    let mut sites = Vec::new();
    let mut rest = host.as_str();
    while rest.contains('.') {
        sites.push(rest.to_owned());
        rest = match rest.split_once('.') {
            Some((_, parent)) => parent,
            None => break,
        };
    }
    sites
}

// ============================================================================
// Stored selections and authentication-field isolation
// ============================================================================

/// The configuration key that carries a selection on the wire.
pub const SELECTION_KEY: &str = "credential_selection";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSelection {
    pub owner: CredentialOwner,
    pub platform_id: String,
    pub platform_name: String,
    pub selection: CredentialSelection,
    /// For the account a Streamlink streamer uses through its site, that
    /// site. Such a layer is owned by the platform and never stored.
    pub site: Option<String>,
}

/// Configuration keys that carried account material before profiles. They are
/// never read from configuration: profiles supply them to the extractor.
/// `username`/`password` are account login only on SOOP; elsewhere they are
/// room passwords and stay.
pub const AUTHENTICATION_FIELDS: &[&str] = &[
    "reauth_config",
    "cookies",
    "refresh_token",
    "access_token",
    "oauth_token",
    "ttwid",
    "device_id",
    "session_cookies",
    "last_cookie_check_date",
    "last_cookie_check_result",
];

/// Objects inside configuration extras that carry extractor options too.
pub(crate) const NESTED_EXTRAS: [&str; 2] = ["platform_specific_config", "platform_extras"];

/// The first stored selection from streamer to platform wins; a scope that
/// stores none inherits, and with no selection at any layer the scope has no
/// managed policy.
pub(crate) fn resolve_authentication(
    platform_id: &str,
    layers: &[StoredSelection],
) -> Result<Option<ResolvedCredentialPolicy>> {
    layers
        .iter()
        .filter(|layer| {
            layer.platform_id == platform_id
                && !matches!(layer.selection, CredentialSelection::Inherit)
                && (matches!(layer.owner, CredentialOwner::Streamer { .. })
                    || layer.site.is_some()
                    || !crate::domain::is_streamlink_platform(&layer.platform_name))
        })
        .min_by_key(|layer| layer.owner.precedence())
        .map(|layer| {
            ResolvedCredentialPolicy::new(
                platform_id.to_owned(),
                layer.owner.clone(),
                layer.selection.clone(),
            )
        })
        .transpose()
}

fn account_keys<'a>(platform: &str, also: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    AUTHENTICATION_FIELDS
        .iter()
        .chain(login_fields(platform))
        .chain(also)
        .copied()
}

fn remove_keys(platform: &str, value: &mut serde_json::Value, also: &[&str]) {
    if let Some(fields) = value.as_object_mut() {
        for key in account_keys(platform, also) {
            fields.remove(key);
        }
        for key in NESTED_EXTRAS {
            if let Some(nested) = fields.get_mut(key) {
                remove_keys(platform, nested, also);
            }
        }
    }
}

/// Removes the account fields of `platform` from configuration `value`,
/// nested option objects included. Room passwords stay.
pub(crate) fn remove_authentication_fields(platform: &str, value: &mut serde_json::Value) {
    remove_keys(platform, value, &[]);
}

/// Whether configuration `value` sets an account field of `platform`,
/// nested option objects included.
pub(crate) fn carries_authentication_fields(platform: &str, value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|fields| {
        account_keys(platform, &[]).any(|key| fields.get(key).is_some_and(|value| !value.is_null()))
            || NESTED_EXTRAS.iter().any(|key| {
                fields
                    .get(*key)
                    .is_some_and(|nested| carries_authentication_fields(platform, nested))
            })
    })
}

fn isolate_authentication_extras(
    platform: &str,
    mut extras: serde_json::Value,
) -> serde_json::Value {
    remove_keys(platform, &mut extras, &[SELECTION_KEY]);
    extras
}

/// Normalize platform content aliases before removing configured account inputs.
pub fn isolate_platform_authentication_extras(
    platform: &str,
    mut extras: serde_json::Value,
) -> serde_json::Value {
    if platform.eq_ignore_ascii_case("bigo") {
        let room_password = extras
            .get("stream_password")
            .and_then(serde_json::Value::as_str)
            .or_else(|| extras.get("password").and_then(serde_json::Value::as_str))
            .map(str::to_owned);
        if let Some(fields) = extras.as_object_mut() {
            if let Some(password) = room_password {
                fields.insert("stream_password".into(), password.into());
            }
            fields.remove("password");
        }
    }
    isolate_authentication_extras(platform, extras)
}

/// Preserve provider-generated device/guest cookies while the selected snapshot
/// remains authoritative for every name already belonging to its account.
pub fn merge_cookie_updates<'a>(
    selected: &'a str,
    updates: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut cookies: Vec<(String, String)> = Vec::new();
    let mut positions = HashMap::new();
    for input in updates.into_iter().chain(std::iter::once(selected)) {
        for token in input
            .split(';')
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            let name = token.split_once('=').map_or(token, |(name, _)| name.trim());
            if let Some(&position) = positions.get(name) {
                cookies[position] = (name.to_string(), token.to_string());
            } else {
                positions.insert(name.to_string(), cookies.len());
                cookies.push((name.to_string(), token.to_string()));
            }
        }
    }
    cookies
        .into_iter()
        .map(|(_, token)| token)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reauth_login_needs_both_fields_on_a_platform_that_supports_it() {
        let login = serde_json::json!({"username": " user ", "password": "pass"});
        assert_eq!(
            platform_reauth_extra("soop", Some(&login)),
            Some(serde_json::json!({"username": "user", "password": "pass"}))
        );
        assert_eq!(platform_reauth_extra("bigo", Some(&login)), None);
        assert_eq!(
            platform_reauth_extra("soop", Some(&serde_json::json!({"username": "user"}))),
            None
        );
    }

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

    #[test]
    fn validity_and_reason_codes_round_trip_and_reject_unknown_text() {
        for validity in [
            CredentialValidity::Unknown,
            CredentialValidity::Valid,
            CredentialValidity::NeedsRefresh,
            CredentialValidity::Invalid,
        ] {
            let text = validity.as_str();
            assert_eq!(CredentialValidity::parse(text), Some(validity));
            assert_eq!(serde_json::json!(validity), text);
        }
        for reason in [
            HealthReason::LoginRequired,
            HealthReason::ManualValidation,
            HealthReason::RefreshFailed,
            HealthReason::RepairRequired,
            HealthReason::AuthenticationFailed,
        ] {
            let text = reason.as_str();
            assert_eq!(HealthReason::parse(text), Some(reason));
            assert_eq!(serde_json::json!(reason), text);
        }
        assert_eq!(CredentialValidity::parse("expired"), None);
        assert_eq!(HealthReason::parse("expired"), None);
    }

    #[test]
    fn policies_reject_ambiguous_or_unbounded_inputs() {
        for value in [
            serde_json::json!({"mode":"none", "cookies":"secret"}),
            serde_json::json!({"mode":"fixed", "credential_id":" "}),
            serde_json::json!({"mode":"pool", "credential_ids":[], "strategy":"priority", "failover":true, "max_attempts":3}),
            serde_json::json!({"mode":"pool", "credential_ids":["a","a"], "strategy":"priority", "failover":true, "max_attempts":3}),
            serde_json::json!({"mode":"pool", "credential_ids":["a"], "strategy":"random", "failover":true, "max_attempts":3}),
            serde_json::json!({"mode":"pool", "credential_ids":["a"], "strategy":"priority", "failover":true, "max_attempts":11}),
            serde_json::Value::Null,
        ] {
            assert!(CredentialSelection::from_value(value).is_err());
        }
        assert!(CredentialSelection::from_value(serde_json::json!({"mode":"pool", "credential_ids":["a"], "strategy":"round_robin", "failover":false, "max_attempts":1})).is_ok());
    }

    #[test]
    fn omitted_pool_settings_take_the_recommended_defaults() {
        let selection = CredentialSelection::from_value(
            serde_json::json!({"mode":"pool", "credential_ids":["a", "b"]}),
        )
        .unwrap();
        assert_eq!(
            selection,
            CredentialSelection::Pool {
                credential_ids: vec!["a".into(), "b".into()],
                strategy: PoolStrategy::Priority,
                failover: true,
                max_attempts: 3,
                anonymous_fallback: false,
            }
        );
        let owner = CredentialOwner::Platform {
            platform_id: "p".into(),
        };
        let explicit = CredentialSelection::from_value(serde_json::json!({"mode":"pool", "credential_ids":["a", "b"], "strategy":"priority", "failover":true, "max_attempts":3})).unwrap();
        assert_eq!(
            ResolvedCredentialPolicy::new("p".into(), owner.clone(), selection)
                .unwrap()
                .generation,
            ResolvedCredentialPolicy::new("p".into(), owner, explicit)
                .unwrap()
                .generation
        );
    }

    #[test]
    fn a_site_is_the_host_of_a_name_or_url() {
        for (input, site) in [
            ("youtube.com", "youtube.com"),
            ("  YouTube.COM ", "youtube.com"),
            ("www.youtube.com", "youtube.com"),
            ("https://www.youtube.com/@someone/live", "youtube.com"),
            ("m.youtube.com", "m.youtube.com"),
            ("kick.com.", "kick.com"),
            ("http://example.test:8080/path", "example.test"),
            ("www.co", "www.co"),
            ("例え.jp", "xn--r8jz45g.jp"),
            ("192.168.1.10", "192.168.1.10"),
            ("http://[::1]:80/", "[::1]"),
        ] {
            assert_eq!(normalize_site(input).unwrap(), site, "{input}");
        }
    }

    #[test]
    fn a_site_needs_a_domain_below_the_top_level() {
        for input in [
            "",
            "   ",
            "com",
            "www.",
            "localhost",
            "https://",
            "exa mple.com",
        ] {
            assert!(
                matches!(normalize_site(input), Err(ProfileError::InvalidSite(_))),
                "{input:?}"
            );
        }
    }

    #[test]
    fn only_streamlink_accounts_name_sites() {
        assert_eq!(
            normalize_sites(
                "streamlink",
                &[
                    "Kick.com".into(),
                    "www.youtube.com".into(),
                    "kick.com".into()
                ]
            )
            .unwrap(),
            ["kick.com", "youtube.com"]
        );
        assert!(normalize_sites("bilibili", &[]).unwrap().is_empty());
        assert!(matches!(
            normalize_sites("bilibili", &["bilibili.com".into()]),
            Err(ProfileError::InvalidSite(_))
        ));
        let many: Vec<String> = (0..=MAX_SITES).map(|n| format!("s{n}.example")).collect();
        assert!(matches!(
            normalize_sites("streamlink", &many),
            Err(ProfileError::InvalidSite(_))
        ));
    }

    #[test]
    fn a_url_is_covered_by_its_host_and_parent_domains() {
        assert_eq!(
            covering_sites("https://www.youtube.com/@someone/live"),
            ["www.youtube.com", "youtube.com"]
        );
        assert_eq!(covering_sites("https://kick.com/someone"), ["kick.com"]);
        assert_eq!(
            covering_sites("https://a.b.example.co.uk/"),
            [
                "a.b.example.co.uk",
                "b.example.co.uk",
                "example.co.uk",
                "co.uk"
            ]
        );
        assert_eq!(covering_sites("http://10.0.0.2:8080/live"), ["10.0.0.2"]);
        assert!(covering_sites("not a url").is_empty());
        assert_eq!(
            site_host("https://www.youtube.com/@someone").as_deref(),
            Some("youtube.com")
        );
    }
}
