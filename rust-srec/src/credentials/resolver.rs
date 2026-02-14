//! Credential source resolver.
//!
//! Resolves which configuration layer provides credentials for a streamer.

use std::sync::Arc;

use tracing::{debug, instrument};

use crate::Result;
use crate::database::repositories::config::ConfigRepository;
use crate::domain::streamer::Streamer;
use crate::streamer::StreamerMetadata;

use super::types::{CredentialScope, CredentialSource};

/// Resolves credential source for a given streamer.
///
/// Checks configuration layers in order: Streamer → Template → Platform.
/// Returns the first layer that has cookies defined.
pub struct CredentialResolver<R: ConfigRepository> {
    config_repo: Arc<R>,
}

impl<R: ConfigRepository> CredentialResolver<R> {
    /// Create a new credential resolver.
    pub fn new(config_repo: Arc<R>) -> Self {
        Self { config_repo }
    }

    /// Find which configuration layer provides cookies for a streamer.
    ///
    /// Checks in order: Streamer → Template → Platform.
    /// Returns the first layer that has cookies defined.
    ///
    /// # Performance
    /// - Platform config is always fetched (needed for platform_name)
    /// - Template config is fetched only if exists and streamer doesn't have cookies
    ///
    /// # Corner Cases
    /// - Returns `None` if no cookies at any layer
    /// - Empty string cookies (`""`) are treated as "no cookies"
    #[instrument(skip(self), fields(streamer_id = %streamer.id))]
    pub async fn find_cookie_source(
        &self,
        streamer: &Streamer,
    ) -> Result<Option<CredentialSource>> {
        // Layer 2: Platform config (always fetch for platform_name lookup)
        let platform = self
            .config_repo
            .get_platform_config(&streamer.platform_config_id)
            .await?;
        let platform_name = platform.platform_name.clone();

        debug!(platform_name = %platform_name, "Resolving credentials");

        // Layer 4: Streamer-specific cookies (highest priority)
        if let Some(config) = streamer.streamer_specific_config.as_ref()
            && let Some(cookies) = config.get("cookies").and_then(|v| v.as_str())
            && !cookies.trim().is_empty()
        {
            debug!("Found credentials at streamer level");
            let refresh_token = config
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .map(String::from);
            let access_token = config
                .get("access_token")
                .and_then(|v| v.as_str())
                .map(String::from);

            return Ok(Some(
                CredentialSource::new(
                    CredentialScope::Streamer {
                        streamer_id: streamer.id.clone(),
                        streamer_name: streamer.name.clone(),
                    },
                    cookies.to_string(),
                    refresh_token,
                    platform_name,
                )
                .with_access_token(access_token),
            ));
        }

        // Layer 3: Template cookies
        if let Some(template_id) = streamer.template_config_id.as_ref() {
            let template = self.config_repo.get_template_config(template_id).await?;
            if let Some(cookies) = template.cookies.as_ref()
                && !cookies.trim().is_empty()
            {
                debug!(template_id = %template_id, "Found credentials at template level");
                // Parse refresh_token from template's platform_overrides (keyed by platform_name)
                let refresh_token = Self::extract_template_refresh_token(
                    template.platform_overrides.as_deref(),
                    &platform_name,
                );
                let access_token = Self::extract_template_access_token(
                    template.platform_overrides.as_deref(),
                    &platform_name,
                );

                return Ok(Some(
                    CredentialSource::new(
                        CredentialScope::Template {
                            template_id: template_id.clone(),
                            template_name: template.name.clone(),
                        },
                        cookies.clone(),
                        refresh_token,
                        platform_name,
                    )
                    .with_access_token(access_token),
                ));
            }
        }

        // Layer 2: Platform cookies
        if let Some(cookies) = platform.cookies.as_ref()
            && !cookies.trim().is_empty()
        {
            debug!("Found credentials at platform level");
            // Parse refresh_token from platform_specific_config
            let refresh_token =
                Self::extract_platform_refresh_token(platform.platform_specific_config.as_deref());
            let access_token =
                Self::extract_platform_access_token(platform.platform_specific_config.as_deref());

            return Ok(Some(
                CredentialSource::new(
                    CredentialScope::Platform {
                        platform_id: streamer.platform_config_id.clone(),
                        platform_name: platform.platform_name.clone(),
                    },
                    cookies.clone(),
                    refresh_token,
                    platform.platform_name,
                )
                .with_access_token(access_token),
            ));
        }

        // No cookies configured at any layer
        debug!("No credentials found at any level");
        Ok(None)
    }

    /// Find cookie source for StreamerMetadata (used by StreamMonitor integration).
    ///
    /// Delegates to the core logic after parsing the JSON config string.
    #[instrument(skip(self), fields(streamer_id = %metadata.id))]
    pub async fn find_cookie_source_for_metadata(
        &self,
        metadata: &StreamerMetadata,
    ) -> Result<Option<CredentialSource>> {
        // Platform config (always fetch for platform_name lookup)
        let platform = self
            .config_repo
            .get_platform_config(&metadata.platform_config_id)
            .await?;
        let platform_name = platform.platform_name.clone();

        debug!(platform_name = %platform_name, "Resolving credentials for metadata");

        // Layer 4: Streamer-specific cookies (highest priority)
        if let Some(config_str) = metadata.streamer_specific_config.as_ref()
            && let Ok(config) = serde_json::from_str::<serde_json::Value>(config_str)
            && let Some(cookies) = config.get("cookies").and_then(|v| v.as_str())
            && !cookies.trim().is_empty()
        {
            debug!("Found credentials at streamer level");
            let refresh_token = config
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .map(String::from);
            let access_token = config
                .get("access_token")
                .and_then(|v| v.as_str())
                .map(String::from);

            return Ok(Some(
                CredentialSource::new(
                    CredentialScope::Streamer {
                        streamer_id: metadata.id.clone(),
                        streamer_name: metadata.name.clone(),
                    },
                    cookies.to_string(),
                    refresh_token,
                    platform_name,
                )
                .with_access_token(access_token),
            ));
        }

        // Layer 3: Template cookies
        if let Some(template_id) = metadata.template_config_id.as_ref() {
            let template = self.config_repo.get_template_config(template_id).await?;
            if let Some(cookies) = template.cookies.as_ref()
                && !cookies.trim().is_empty()
            {
                debug!(template_id = %template_id, "Found credentials at template level");
                let refresh_token = Self::extract_template_refresh_token(
                    template.platform_overrides.as_deref(),
                    &platform_name,
                );
                let access_token = Self::extract_template_access_token(
                    template.platform_overrides.as_deref(),
                    &platform_name,
                );

                return Ok(Some(
                    CredentialSource::new(
                        CredentialScope::Template {
                            template_id: template_id.clone(),
                            template_name: template.name.clone(),
                        },
                        cookies.clone(),
                        refresh_token,
                        platform_name,
                    )
                    .with_access_token(access_token),
                ));
            }
        }

        // Layer 2: Platform cookies
        if let Some(cookies) = platform.cookies.as_ref()
            && !cookies.trim().is_empty()
        {
            debug!("Found credentials at platform level");
            let refresh_token =
                Self::extract_platform_refresh_token(platform.platform_specific_config.as_deref());
            let access_token =
                Self::extract_platform_access_token(platform.platform_specific_config.as_deref());

            return Ok(Some(
                CredentialSource::new(
                    CredentialScope::Platform {
                        platform_id: metadata.platform_config_id.clone(),
                        platform_name: platform.platform_name.clone(),
                    },
                    cookies.clone(),
                    refresh_token,
                    platform.platform_name,
                )
                .with_access_token(access_token),
            ));
        }

        debug!("No credentials found at any level");
        Ok(None)
    }

    /// Extract refresh_token from platform_specific_config JSON.
    fn extract_platform_refresh_token(config: Option<&str>) -> Option<String> {
        config
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .and_then(|v| {
                v.get("refresh_token")
                    .and_then(|t| t.as_str())
                    .map(String::from)
            })
    }

    /// Extract access_token from platform_specific_config JSON.
    fn extract_platform_access_token(config: Option<&str>) -> Option<String> {
        config
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .and_then(|v| {
                v.get("access_token")
                    .and_then(|t| t.as_str())
                    .map(String::from)
            })
    }

    /// Extract refresh_token from template's platform_overrides for a specific platform.
    fn extract_template_refresh_token(
        overrides: Option<&str>,
        platform_name: &str,
    ) -> Option<String> {
        overrides
            .and_then(|o| serde_json::from_str::<serde_json::Value>(o).ok())
            .and_then(|v| v.get(platform_name).cloned())
            .and_then(|p| p.get("refresh_token").cloned())
            .and_then(|t| t.as_str().map(String::from))
    }

    /// Extract access_token from template's platform_overrides for a specific platform.
    fn extract_template_access_token(
        overrides: Option<&str>,
        platform_name: &str,
    ) -> Option<String> {
        overrides
            .and_then(|o| serde_json::from_str::<serde_json::Value>(o).ok())
            .and_then(|v| v.get(platform_name).cloned())
            .and_then(|p| p.get("access_token").cloned())
            .and_then(|t| t.as_str().map(String::from))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_platform_refresh_token() {
        let config = r#"{"quality": 30000, "refresh_token": "abc123"}"#;
        let result = CredentialResolver::<
            crate::database::repositories::config::SqlxConfigRepository,
        >::extract_platform_refresh_token(Some(config));
        assert_eq!(result, Some("abc123".to_string()));

        // Missing refresh_token
        let config = r#"{"quality": 30000}"#;
        let result = CredentialResolver::<
            crate::database::repositories::config::SqlxConfigRepository,
        >::extract_platform_refresh_token(Some(config));
        assert_eq!(result, None);

        // Invalid JSON
        let result = CredentialResolver::<
            crate::database::repositories::config::SqlxConfigRepository,
        >::extract_platform_refresh_token(Some("not json"));
        assert_eq!(result, None);

        // None
        let result = CredentialResolver::<
            crate::database::repositories::config::SqlxConfigRepository,
        >::extract_platform_refresh_token(None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_template_refresh_token() {
        let overrides = r#"{"bilibili": {"quality": 30000, "refresh_token": "xyz789"}}"#;
        let result = CredentialResolver::<
            crate::database::repositories::config::SqlxConfigRepository,
        >::extract_template_refresh_token(Some(overrides), "bilibili");
        assert_eq!(result, Some("xyz789".to_string()));

        // Different platform
        let result = CredentialResolver::<
            crate::database::repositories::config::SqlxConfigRepository,
        >::extract_template_refresh_token(Some(overrides), "huya");
        assert_eq!(result, None);
    }
}
