//! Credential persistence abstraction.
//!
//! The credentials feature needs to persist refreshed cookies / tokens back to the DB.
//! The concrete SQL implementation lives in the database repository layer.

use async_trait::async_trait;

use super::error::CredentialError;
use super::manager::RefreshedCredentials;
use super::types::{CredentialScope, CredentialSource};

#[async_trait]
pub trait CredentialStore: Send + Sync {
    /// Read the current credentials at exactly this scope. Callers hold the
    /// scope's refresh lock so a queued refresh cannot reuse a rotated token.
    async fn reload_source(
        &self,
        source: &CredentialSource,
    ) -> Result<CredentialSource, CredentialError>;

    /// Persist refreshed credentials to the correct configuration layer.
    async fn update_credentials(
        &self,
        source: &CredentialSource,
        credentials: &RefreshedCredentials,
    ) -> Result<(), CredentialError>;

    /// Persist a "checked today" result for hydration on restart.
    async fn update_check_result(
        &self,
        scope: &CredentialScope,
        result: &str,
    ) -> Result<(), CredentialError>;
}
