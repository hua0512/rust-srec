//! Authentication persistence store (users, API keys, refresh tokens, sessions).
//!
//! Keeps authentication tables, row models, repositories, and the single-connection
//! transactional configuration-import hook inside the API layer.

pub mod api_key;
pub(crate) mod import;
pub mod models;
pub mod refresh_token;
pub mod user;

#[cfg(test)]
pub use api_key::InMemoryApiKeyRepository;
pub use api_key::{ApiKeyRepository, SqlxApiKeyRepository};
pub(crate) use import::{
    AuthImportError, apply_auth_import_tx, export_users, validate_auth_import_tx,
};
pub use models::{
    ApiKeyAccessLevel, ApiKeyDbModel, AuthSessionDbModel, RefreshTokenDbModel, UserDbModel,
};
pub use refresh_token::{RefreshTokenRepository, RefreshTokenRotation, SqlxRefreshTokenRepository};
#[cfg(test)]
pub(crate) use user::import_user;
pub use user::{SqlxUserRepository, UserRepository};
