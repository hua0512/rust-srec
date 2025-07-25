use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
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
                // Use confy for default location
                confy::load("streev-cli", None).context("Failed to load configuration")
            }
        }
    }

    /// Get default configuration file path
    pub fn default_config_path() -> Option<PathBuf> {
        confy::get_configuration_file_path("streev-cli", None).ok()
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
        toml::to_string_pretty(self).context("Failed to serialize configuration for display")
    }
}
