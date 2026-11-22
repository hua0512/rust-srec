use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Default output format
    pub default_output_format: String,

    /// Default request timeout in seconds
    pub default_timeout: u64,

    /// Default number of retries
    pub default_retries: u32,

    /// Maximum concurrent extractions for batch processing
    pub max_concurrent: usize,

    /// Default cookies to use
    pub default_cookies: Option<String>,

    /// Auto-select best quality stream by default
    pub auto_select: bool,

    /// Include extra metadata by default
    pub include_extras: bool,

    /// Default output directory for batch processing
    pub default_output_dir: Option<PathBuf>,

    /// User agent string for requests
    pub user_agent: Option<String>,

    /// Enable colored output
    pub colored_output: bool,

    /// Default proxy URL (supports http, https, socks5)
    pub default_proxy: Option<String>,

    /// Default proxy username (if proxy requires authentication)
    pub default_proxy_username: Option<String>,

    /// Default proxy password (if proxy requires authentication)
    pub default_proxy_password: Option<String>,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_proxy = self
            .default_proxy
            .as_deref()
            .map(platforms_parser::proxy::redacted_url);
        f.debug_struct("AppConfig")
            .field("default_output_format", &self.default_output_format)
            .field("default_timeout", &self.default_timeout)
            .field("default_retries", &self.default_retries)
            .field("max_concurrent", &self.max_concurrent)
            .field(
                "default_cookies",
                &self.default_cookies.as_ref().map(|_| "[redacted]"),
            )
            .field("auto_select", &self.auto_select)
            .field("include_extras", &self.include_extras)
            .field("default_output_dir", &self.default_output_dir)
            .field("user_agent", &self.user_agent)
            .field("colored_output", &self.colored_output)
            .field("default_proxy", &redacted_proxy)
            .field(
                "default_proxy_username",
                &self.default_proxy_username.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "default_proxy_password",
                &self.default_proxy_password.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            default_output_format: "pretty".to_string(),
            default_timeout: 30,
            default_retries: 3,
            max_concurrent: 5,
            default_cookies: None,
            auto_select: false,
            include_extras: true,
            default_output_dir: None,
            user_agent: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36".to_string()),
            colored_output: true,
            default_proxy: None,
            default_proxy_username: None,
            default_proxy_password: None,
        }
    }
}

impl AppConfig {
    /// Load configuration from file and environment
    pub fn load(config_path: Option<&Path>) -> Result<Self> {
        match config_path {
            Some(path) => {
                if path.exists() {
                    let content = std::fs::read_to_string(path)
                        .context("Failed to read configuration file")?;
                    toml::from_str(&content).context("Failed to parse configuration file")
                } else {
                    Ok(Self::default())
                }
            }
            None => {
                // Use dirs for default location
                let path = Self::default_config_path();
                match path {
                    Some(p) if p.exists() => {
                        let content = std::fs::read_to_string(&p)
                            .context("Failed to read configuration file")?;
                        toml::from_str(&content).context("Failed to parse configuration file")
                    }
                    _ => Ok(Self::default()),
                }
            }
        }
    }

    /// Get default configuration file path
    pub fn default_config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("streev-cli").join("default-config.toml"))
    }

    /// Save configuration to file
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create config directory")?;
        }

        let toml_string =
            toml::to_string_pretty(self).context("Failed to serialize configuration")?;

        std::fs::write(path, toml_string).context("Failed to write configuration file")?;

        Ok(())
    }

    /// Reset configuration to defaults and save
    pub fn reset(config_path: Option<&Path>) -> Result<()> {
        let path = config_path
            .map(|p| p.to_path_buf())
            .or_else(Self::default_config_path)
            .context("No configuration path available")?;

        let default_config = Self::default();
        default_config.save(&path)?;

        Ok(())
    }

    /// Show current configuration as a formatted string
    pub fn show(&self) -> Result<String> {
        let mut redacted = self.clone();
        if redacted.default_cookies.is_some() {
            redacted.default_cookies = Some("[redacted]".to_string());
        }
        if let Some(proxy) = redacted.default_proxy.as_deref() {
            redacted.default_proxy = Some(platforms_parser::proxy::redacted_url(proxy));
        }
        if redacted.default_proxy_username.is_some() {
            redacted.default_proxy_username = Some("[redacted]".to_string());
        }
        if redacted.default_proxy_password.is_some() {
            redacted.default_proxy_password = Some("[redacted]".to_string());
        }
        toml::to_string_pretty(&redacted).context("Failed to serialize configuration for display")
    }
}
