use std::sync::Arc;

use crate::credentials::CredentialSource;

use super::MergedConfig;

/// Resolved streamer context.
///
/// This is a sidecar for the resolved `MergedConfig` that carries additional runtime-only
/// information that must not be exposed via API serialization (e.g. refresh tokens).
pub struct ResolvedStreamerContext {
    pub config: Arc<MergedConfig>,
    pub credential_source: Option<CredentialSource>,
}
