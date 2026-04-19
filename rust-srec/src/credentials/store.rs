//! Credential persistence abstraction.
//!
//! The credentials feature needs to persist refreshed cookies / tokens back to the DB.
//! The concrete SQL implementation lives in the database repository layer.

use super::error::CredentialError;
use super::manager::RefreshedCredentials;
use super::types::{CredentialScope, CredentialSource};

#[dynosaur::dynosaur(pub DynCredentialStore = dyn(box) CredentialStore)]
pub trait CredentialStore: Send + Sync {
    /// Persist refreshed credentials to the correct configuration layer.
    fn update_credentials(
        &self,
        source: &CredentialSource,
        credentials: &RefreshedCredentials,
    ) -> impl std::future::Future<Output = Result<(), CredentialError>> + Send;

    /// Persist a "checked today" result for hydration on restart.
    fn update_check_result(
        &self,
        scope: &CredentialScope,
        result: &str,
    ) -> impl std::future::Future<Output = Result<(), CredentialError>> + Send;
}
