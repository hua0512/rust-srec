use super::types::ProxyConfig;

#[derive(Debug, Clone)]
pub struct PlatformConfig {
    pub id: String,
    pub platform_name: String,
    pub fetch_delay_ms: u64,
    pub download_delay_ms: u64,
    pub cookies: Option<String>,
    pub platform_specific_config: Option<serde_json::Value>,
    pub proxy_config: Option<ProxyConfig>,
    pub record_danmu: Option<bool>,
}

impl From<crate::database::models::PlatformConfig> for PlatformConfig {
    fn from(model: crate::database::models::PlatformConfig) -> Self {
        Self {
            id: model.id,
            platform_name: model.platform_name,
            fetch_delay_ms: model.fetch_delay_ms as u64,
            download_delay_ms: model.download_delay_ms as u64,
            cookies: model.cookies,
            platform_specific_config: model
                .platform_specific_config
                .and_then(|config| serde_json::from_str(&config).ok()),
            proxy_config: model
                .proxy_config
                .and_then(|config| serde_json::from_str(&config).ok()),
            record_danmu: model.record_danmu,
        }
    }
}
