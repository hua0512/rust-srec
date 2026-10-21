//! JWT authentication service.
//!
//! Provides JWT token generation and validation for API authentication.

use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// JWT claims structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Claims {
    /// User ID (subject)
    pub sub: String,
    /// User roles for authorization
    pub roles: Vec<String>,
    /// Token issuer
    pub iss: String,
    /// Token audience
    pub aud: String,
    /// Expiration timestamp (Unix)
    pub exp: u64,
    /// Issued at timestamp (Unix)
    pub iat: u64,
    /// Login-session lineage. Legacy unbound JWTs can decode but cannot authorize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
}

/// JWT service error types.
#[derive(Debug, thiserror::Error)]
pub enum JwtError {
    #[error("Token generation failed: {0}")]
    TokenGeneration(String),
    #[error("Token validation failed: {0}")]
    TokenValidation(String),
    #[error("Token expired")]
    TokenExpired,
    #[error("Invalid token")]
    InvalidToken,
}

/// JWT service for token generation and validation.
#[derive(Clone)]
pub struct JwtService {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    issuer: String,
    audience: String,
    expiration_secs: u64,
    /// Validation rules preconfigured with `issuer`/`audience`. Built once in
    /// `new` so `validate_token` (called per authenticated request) does not
    /// reallocate the allowed-issuer/audience sets on every call.
    validation: Validation,
}

use tracing::info;

impl JwtService {
    /// Create a new JWT service.
    ///
    /// # Arguments
    /// * `secret` - The secret key for signing tokens
    /// * `issuer` - The token issuer claim
    /// * `audience` - The token audience claim
    /// * `expiration_secs` - Token expiration time in seconds (default: 3600)
    pub fn new(secret: &str, issuer: &str, audience: &str, expiration_secs: Option<u64>) -> Self {
        let mut validation = Validation::default();
        validation.set_issuer(&[issuer]);
        validation.set_audience(&[audience]);
        Self {
            encoding_key: EncodingKey::from_secret(secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
            issuer: issuer.to_string(),
            audience: audience.to_string(),
            expiration_secs: expiration_secs.unwrap_or(3600),
            validation,
        }
    }

    /// Create a new JWT service from environment variables.
    ///
    /// # Arguments
    /// * `expiration_secs` - Token expiration time in seconds
    pub fn from_env(expiration_secs: u64) -> Option<Self> {
        let secret = std::env::var("JWT_SECRET")
            .ok()
            .filter(|value| !value.trim().is_empty())?;
        let issuer = std::env::var("JWT_ISSUER").unwrap_or_else(|_| "rust-srec".to_string());
        let audience =
            std::env::var("JWT_AUDIENCE").unwrap_or_else(|_| "rust-srec-api".to_string());

        info!(
            "JWT service initialized (issuer: {}, audience: {}, expiration: {}s)",
            issuer, audience, expiration_secs
        );

        Some(Self::new(
            &secret,
            &issuer,
            &audience,
            Some(expiration_secs),
        ))
    }

    /// Generate a JWT token for a user.
    ///
    /// # Arguments
    /// * `user_id` - The user's unique identifier
    /// * `roles` - The user's roles
    ///
    /// # Returns
    /// A JWT token string or an error
    #[cfg(test)]
    pub fn generate_token(&self, user_id: &str, roles: Vec<String>) -> Result<String, JwtError> {
        self.issue_token(user_id, roles, None)
            .map(|(token, _)| token)
    }

    pub(crate) fn generate_session_token(
        &self,
        user_id: &str,
        roles: Vec<String>,
        session_id: &str,
    ) -> Result<(String, u64), JwtError> {
        self.issue_token(user_id, roles, Some(session_id.to_owned()))
    }

    #[cfg(test)]
    pub(crate) fn generate_test_token(
        &self,
        user_id: &str,
        roles: Vec<String>,
    ) -> Result<String, JwtError> {
        self.generate_session_token(user_id, roles, "test-session")
            .map(|(token, _)| token)
    }

    fn issue_token(
        &self,
        user_id: &str,
        roles: Vec<String>,
        sid: Option<String>,
    ) -> Result<(String, u64), JwtError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| JwtError::TokenGeneration(e.to_string()))?
            .as_secs();

        let claims = Claims {
            sub: user_id.to_string(),
            roles,
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            exp: now.checked_add(self.expiration_secs).ok_or_else(|| {
                JwtError::TokenGeneration("Token expiration exceeds supported range".to_owned())
            })?,
            iat: now,
            sid,
        };

        encode(&Header::default(), &claims, &self.encoding_key)
            .map(|token| (token, claims.exp))
            .map_err(|e| JwtError::TokenGeneration(e.to_string()))
    }

    /// Validate a JWT token and extract claims.
    ///
    /// # Arguments
    /// * `token` - The JWT token string
    ///
    /// # Returns
    /// The token claims or an error
    pub fn validate_token(&self, token: &str) -> Result<Claims, JwtError> {
        decode::<Claims>(token, &self.decoding_key, &self.validation)
            .map(|data| data.claims)
            .map_err(|e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtError::TokenExpired,
                jsonwebtoken::errors::ErrorKind::InvalidToken
                | jsonwebtoken::errors::ErrorKind::InvalidSignature => JwtError::InvalidToken,
                _ => JwtError::TokenValidation(e.to_string()),
            })
    }
}

impl std::fmt::Debug for JwtService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtService")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("expiration_secs", &self.expiration_secs)
            .finish()
    }
}
