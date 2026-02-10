//! Platform-specific credential manager trait.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::CredentialError;

/// Status of credential validity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialStatus {
    /// Credentials are valid and do not need refresh.
    Valid,
    /// Credentials need refresh (platform signals expiring soon).
    NeedsRefresh {
        /// Unix timestamp when refresh becomes mandatory.
        refresh_deadline: Option<u64>,
    },
    /// Credentials are invalid (expired or revoked).
    Invalid {
        reason: String,
        /// Error code from platform API (e.g., -101 for Bilibili).
        error_code: Option<i32>,
    },
}

impl CredentialStatus {
    /// Check if credentials need refresh.
    #[inline]
    pub fn needs_refresh(&self) -> bool {
        matches!(self, Self::NeedsRefresh { .. })
    }

    /// Check if credentials are invalid.
    #[inline]
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid { .. })
    }

    /// Check if credentials are valid.
    #[inline]
    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

/// Result of a successful credential refresh.
#[derive(Debug, Clone)]
pub struct RefreshedCredentials {
    /// New cookie string (semicolon-separated key=value pairs).
    pub cookies: String,
    /// New refresh token (if applicable).
    pub refresh_token: Option<String>,
    /// New OAuth2 access token (if applicable).
    pub access_token: Option<String>,
    /// Expected expiration time (if known).
    pub expires_at: Option<DateTime<Utc>>,
}

/// State required to perform a refresh.
#[derive(Debug, Clone)]
pub struct RefreshState {
    /// Current cookies (may be partially expired).
    pub cookies: String,
    /// Refresh token from initial login.
    pub refresh_token: Option<String>,
    /// Platform-specific state (e.g., additional tokens).
    pub extra: Option<serde_json::Value>,
}

impl RefreshState {
    /// Create a new refresh state.
    pub fn new(cookies: String, refresh_token: Option<String>) -> Self {
        Self {
            cookies,
            refresh_token,
            extra: None,
        }
    }

    /// Check if refresh token is available.
    #[inline]
    pub fn has_refresh_token(&self) -> bool {
        self.refresh_token.is_some()
    }
}

/// Platform-specific credential management trait.
///
/// Implementations handle the specific refresh protocols for each platform.
#[async_trait]
pub trait CredentialManager: Send + Sync {
    /// Platform identifier (e.g., "bilibili", "douyin").
    fn platform_id(&self) -> &'static str;

    /// Check if credentials need refresh.
    ///
    /// # Arguments
    /// * `cookies` - Current cookie string
    ///
    /// # Returns
    /// * `Ok(CredentialStatus)` - Check completed successfully
    /// * `Err(...)` - Network or parsing error during check
    async fn check_status(&self, cookies: &str) -> Result<CredentialStatus, CredentialError>;

    /// Perform credential refresh.
    ///
    /// # Arguments
    /// * `state` - Current credentials and tokens
    ///
    /// # Returns
    /// * `Ok(RefreshedCredentials)` - Refresh successful
    /// * `Err(...)` - Refresh failed (may need re-login)
    async fn refresh(&self, state: &RefreshState) -> Result<RefreshedCredentials, CredentialError>;

    /// Validate credentials are working (e.g., make authenticated API call).
    ///
    /// # Arguments
    /// * `cookies` - Cookie string to validate
    ///
    /// # Returns
    /// * `Ok(true)` - Credentials are working
    /// * `Ok(false)` - Credentials are invalid
    /// * `Err(...)` - Validation check failed
    async fn validate(&self, cookies: &str) -> Result<bool, CredentialError>;

    /// Whether this manager supports automatic refresh.
    fn supports_auto_refresh(&self) -> bool {
        true
    }

    /// Required fields for refresh (for UI hints).
    fn required_refresh_fields(&self) -> &'static [&'static str] {
        &["refresh_token"]
    }
}
