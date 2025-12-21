//! Registry of available danmu providers.

use crate::danmaku::provider::DanmuProvider;
use crate::extractor::platforms::bilibili::danmu::create_bilibili_danmu_provider;
use crate::extractor::platforms::douyin::create_douyin_danmu_provider;
use crate::extractor::platforms::douyu::create_douyu_danmu_provider;
use crate::extractor::platforms::huya::HuyaDanmuProvider;
use crate::extractor::platforms::twitcasting::create_twitcasting_danmu_provider;
use crate::extractor::platforms::twitch::create_twitch_danmu_provider;
use std::sync::Arc;

/// Registry of available danmu providers.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn DanmuProvider>>,
}

impl ProviderRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Create a registry with default providers.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(HuyaDanmuProvider::new()));
        registry.register(Arc::new(create_bilibili_danmu_provider()));
        registry.register(Arc::new(create_douyu_danmu_provider()));
        registry.register(Arc::new(create_douyin_danmu_provider()));
        registry.register(Arc::new(create_twitch_danmu_provider()));
        registry.register(Arc::new(create_twitcasting_danmu_provider()));
        registry
    }

    /// Register a provider.
    pub fn register(&mut self, provider: Arc<dyn DanmuProvider>) {
        self.providers.push(provider);
    }

    /// Get a provider for the given platform.
    pub fn get_by_platform(&self, platform: &str) -> Option<Arc<dyn DanmuProvider>> {
        self.providers
            .iter()
            .find(|p| p.platform().eq_ignore_ascii_case(platform))
            .cloned()
    }

    /// Get a provider that supports the given URL.
    pub fn get_by_url(&self, url: &str) -> Option<Arc<dyn DanmuProvider>> {
        self.providers.iter().find(|p| p.supports_url(url)).cloned()
    }

    /// List all registered platforms.
    pub fn platforms(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.platform()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_with_defaults() {
        let registry = ProviderRegistry::with_defaults();
        let platforms = registry.platforms();

        assert!(platforms.contains(&"huya"));
        assert!(platforms.contains(&"bilibili"));
        assert!(platforms.contains(&"douyu"));
        assert!(platforms.contains(&"douyin"));
        assert!(platforms.contains(&"twitch"));
        assert!(platforms.contains(&"twitcasting"));
    }

    #[test]
    fn test_get_by_platform() {
        let registry = ProviderRegistry::with_defaults();

        let huya = registry.get_by_platform("huya");
        assert!(huya.is_some());
        assert_eq!(huya.unwrap().platform(), "huya");
    }

    #[test]
    fn test_get_by_url() {
        let registry = ProviderRegistry::with_defaults();

        let huya = registry.get_by_url("https://www.huya.com/12345");
        assert!(huya.is_some());
        assert_eq!(huya.unwrap().platform(), "huya");
    }
}
