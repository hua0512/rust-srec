//! Persisted account policies and non-secret execution identities.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PoolStrategy {
    RoundRobin,
    Priority,
}

/// A scope that stores no selection inherits; at the platform that is anonymous.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
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
    },
}

impl<'de> Deserialize<'de> for CredentialSelection {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        // Empty struct variants reject extra fields; serde's unit variants do
        // not apply deny_unknown_fields to internally tagged representations.
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
            } => Self::Pool {
                credential_ids,
                strategy,
                failover,
                max_attempts,
            },
        };
        selection.validate().map_err(serde::de::Error::custom)?;
        Ok(selection)
    }
}

// Recommended pool defaults. Deserialization stores them explicitly, so the
// policy generation does not depend on whether a client omitted them.
// Priority keeps checks on one main account with the others as backups,
// which looks less unusual to platform anti-abuse checks than spreading
// every poll across all accounts.
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
        // Deserialization already validates the selection.
        serde_json::from_value(value).map_err(|_| Error::validation("invalid credential selection"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialIdentity {
    Profile { profile_id: String },
    Anonymous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialBinding {
    pub identity: CredentialIdentity,
    pub revision: u64,
    pub policy: ResolvedCredentialPolicy,
    pub epoch: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
