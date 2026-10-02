//! Profile records keep authentication bundles distinct from public summaries.

use serde::{Deserialize, Serialize};

use super::CredentialOwner;

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("credential profile version changed")]
    StaleVersion,
    #[error("credential source changed")]
    SourceChanged,
    #[error("credential profile is disabled")]
    Disabled,
    #[error("credential profile is referenced by: {}", .0.join(", "))]
    Referenced(Vec<String>),
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
    pub fn validate(&self, platform: &str) -> Result<(), ProfileError> {
        if reqwest::header::HeaderValue::from_str(&self.cookies).is_err() {
            return Err(ProfileError::InvalidMaterial(
                "cookies must be a valid HTTP header",
            ));
        }
        if let Some(config) = &self.reauth_config
            && (!platform.eq_ignore_ascii_case("soop")
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
        let token_auth = if platform.eq_ignore_ascii_case("twitch") {
            if let Some(token) = &self.access_token {
                if token.trim().is_empty()
                    || reqwest::header::HeaderValue::from_str(&format!("OAuth {token}")).is_err()
                {
                    return Err(ProfileError::InvalidMaterial(
                        "Twitch access token must be nonblank and valid HTTP header material",
                    ));
                }
                true
            } else {
                false
            }
        } else {
            false
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
    pub owner_kind: String,
    pub template_id: Option<String>,
    pub streamer_id: Option<String>,
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
    pub fn owner(&self) -> Result<CredentialOwner, ProfileError> {
        match (
            self.owner_kind.as_str(),
            &self.template_id,
            &self.streamer_id,
        ) {
            ("platform", None, None) => Ok(CredentialOwner::Platform {
                platform_id: self.platform_config_id.clone(),
            }),
            ("template", Some(id), None) => Ok(CredentialOwner::Template {
                template_id: id.clone(),
            }),
            ("streamer", None, Some(id)) => Ok(CredentialOwner::Streamer {
                streamer_id: id.clone(),
            }),
            _ => Err(ProfileError::InvalidOwner),
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

    pub fn summary(&self) -> Result<CredentialProfileSummary, ProfileError> {
        Ok(CredentialProfileSummary {
            id: self.id.clone(),
            platform_config_id: self.platform_config_id.clone(),
            owner: self.owner()?,
            label: self.label.clone(),
            enabled: self.enabled,
            revision: self.revision,
            version: self.version,
            has_cookies: !self.cookies.is_empty(),
            has_refresh_token: self.refresh_token.is_some(),
            has_access_token: self.access_token.is_some(),
            has_reauth: self.reauth_config.is_some(),
        })
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CredentialProfileSummary {
    pub id: String,
    pub platform_config_id: String,
    pub owner: CredentialOwner,
    pub label: String,
    pub enabled: bool,
    pub revision: i64,
    pub version: i64,
    pub has_cookies: bool,
    pub has_refresh_token: bool,
    pub has_access_token: bool,
    pub has_reauth: bool,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct CredentialProfileHealth {
    pub profile_id: String,
    pub revision: i64,
    pub validity: String,
    pub last_check_at: Option<i64>,
    pub last_refresh_at: Option<i64>,
    pub cooldown_until: Option<i64>,
    pub throttle_count: i64,
    pub refresh_failure_count: i64,
    pub last_failure_at: Option<i64>,
    pub last_notified_failure_count: i64,
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CredentialUnavailable {
    pub reason: String,
    pub policy_generation: String,
    pub retry_at: Option<i64>,
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
