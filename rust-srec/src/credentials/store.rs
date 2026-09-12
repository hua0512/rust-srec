//! Credential persistence abstraction.
//!
//! The credentials feature needs to persist refreshed cookies / tokens back to the DB.
//! The concrete SQL implementation lives in the database repository layer.

use async_trait::async_trait;

use super::error::CredentialError;
use super::manager::RefreshedCredentials;
use super::types::CredentialSource;

#[async_trait]
pub trait CredentialStore: Send + Sync {
    fn bind_committed_streamers(
        &self,
        _state: std::sync::Arc<crate::streamer::CommittedStreamerState>,
    ) {
    }
    /// Read the current credentials at exactly this scope. Callers hold the
    /// scope's refresh lock so a queued refresh cannot reuse a rotated token.
    async fn reload_source(
        &self,
        source: &CredentialSource,
    ) -> Result<CredentialSource, CredentialError>;

    /// Persist only if the provider inputs in `source` still match the stored credentials.
    /// The comparison and writes must share a transaction; unrelated config edits are allowed.
    async fn update_credentials(
        &self,
        source: &CredentialSource,
        credentials: &RefreshedCredentials,
    ) -> Result<(), CredentialError>;

    /// Verify `source` is current and persist a "checked today" result where supported.
    /// A stale check must not overwrite the status of newer credentials.
    async fn update_check_result(
        &self,
        source: &CredentialSource,
        result: &str,
    ) -> Result<(), CredentialError>;
}
