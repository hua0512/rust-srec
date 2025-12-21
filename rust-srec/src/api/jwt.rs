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
    #[error("Missing claims")]
    MissingClaims,
}

/// JWT service for token generation and validation.
#[derive(Clone)]
pub struct JwtService {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    issuer: String,
    audience: String,
    expiration_secs: u64,
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
        Self {
            encoding_key: EncodingKey::from_secret(secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
            issuer: issuer.to_string(),
            audience: audience.to_string(),
            expiration_secs: expiration_secs.unwrap_or(3600),
        }
    }

    /// Create a new JWT service from environment variables.
    ///
    /// # Arguments
    /// * `expiration_secs` - Token expiration time in seconds
    pub fn from_env(expiration_secs: u64) -> Option<Self> {
        let secret = std::env::var("JWT_SECRET").ok()?;
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
    pub fn generate_token(&self, user_id: &str, roles: Vec<String>) -> Result<String, JwtError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| JwtError::TokenGeneration(e.to_string()))?
            .as_secs();

        let claims = Claims {
            sub: user_id.to_string(),
            roles,
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            exp: now + self.expiration_secs,
            iat: now,
        };

        encode(&Header::default(), &claims, &self.encoding_key)
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
        let mut validation = Validation::default();
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);

        decode::<Claims>(token, &self.decoding_key, &validation)
            .map(|data| data.claims)
            .map_err(|e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtError::TokenExpired,
                jsonwebtoken::errors::ErrorKind::InvalidToken
                | jsonwebtoken::errors::ErrorKind::InvalidSignature => JwtError::InvalidToken,
                _ => JwtError::TokenValidation(e.to_string()),
            })
    }

    /// Get the configured expiration time in seconds.
    pub fn expiration_secs(&self) -> u64 {
        self.expiration_secs
    }

    /// Get the configured issuer.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Get the configured audience.
    pub fn audience(&self) -> &str {
        &self.audience
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

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_service() -> JwtService {
        JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(3600),
        )
    }

    #[test]
    fn test_generate_and_validate_token() {
        let service = create_test_service();
        let token = service
            .generate_token("user123", vec!["admin".to_string(), "user".to_string()])
            .expect("Token generation should succeed");

        let claims = service
            .validate_token(&token)
            .expect("Token validation should succeed");

        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.roles, vec!["admin", "user"]);
        assert_eq!(claims.iss, "test-issuer");
        assert_eq!(claims.aud, "test-audience");
    }

    #[test]
    fn test_invalid_token() {
        let service = create_test_service();
        let result = service.validate_token("invalid.token.here");

        assert!(matches!(
            result,
            Err(JwtError::InvalidToken) | Err(JwtError::TokenValidation(_))
        ));
    }

    #[test]
    fn test_wrong_secret() {
        let service1 =
            JwtService::new("secret1-32-chars-long-key!!!!!", "issuer", "audience", None);
        let service2 =
            JwtService::new("secret2-32-chars-long-key!!!!!", "issuer", "audience", None);

        let token = service1
            .generate_token("user", vec![])
            .expect("Token generation should succeed");

        let result = service2.validate_token(&token);
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn test_claims_contain_required_fields() {
        let service = create_test_service();
        let token = service
            .generate_token("user456", vec!["readonly".to_string()])
            .expect("Token generation should succeed");

        let claims = service
            .validate_token(&token)
            .expect("Token validation should succeed");

        // Verify all required claims are present
        assert!(!claims.sub.is_empty(), "sub claim should not be empty");
        assert!(!claims.iss.is_empty(), "iss claim should not be empty");
        assert!(!claims.aud.is_empty(), "aud claim should not be empty");
        assert!(claims.exp > 0, "exp claim should be set");
        assert!(claims.iat > 0, "iat claim should be set");
        assert!(claims.exp > claims.iat, "exp should be after iat");
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    // **Feature: jwt-auth-and-api-implementation, Property 1: JWT Token Contains Required Claims**
    // **Validates: Requirements 1.6**
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_jwt_token_contains_required_claims(
            user_id in "[a-zA-Z0-9_-]{1,50}",
            roles in prop::collection::vec("[a-zA-Z0-9_]{1,20}", 0..5),
        ) {
            let service = JwtService::new(
                "test-secret-key-32-chars-long!!",
                "test-issuer",
                "test-audience",
                Some(3600),
            );

            let token = service
                .generate_token(&user_id, roles.clone())
                .expect("Token generation should succeed");

            let claims = service
                .validate_token(&token)
                .expect("Token validation should succeed");

            // Property: All required claims must be present and correct
            prop_assert_eq!(&claims.sub, &user_id, "sub claim must match user_id");
            prop_assert_eq!(&claims.roles, &roles, "roles claim must match input roles");
            prop_assert_eq!(&claims.iss, "test-issuer", "iss claim must match issuer");
            prop_assert_eq!(&claims.aud, "test-audience", "aud claim must match audience");
            prop_assert!(claims.exp > 0, "exp claim must be set");
            prop_assert!(claims.iat > 0, "iat claim must be set");
            prop_assert!(claims.exp > claims.iat, "exp must be after iat");
        }
    }
}
