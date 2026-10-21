//! Registry of available danmu providers.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::danmaku::provider::DanmuProvider;
use crate::extractor::platforms::bigo::create_bigo_danmu_provider;
use crate::extractor::platforms::bilibili::danmu::create_bilibili_danmu_provider;
use crate::extractor::platforms::douyin::create_douyin_danmu_provider;
use crate::extractor::platforms::douyu::create_douyu_danmu_provider;
use crate::extractor::platforms::huya::create_huya_danmu_provider;
use crate::extractor::platforms::soop::create_soop_danmu_provider;
use crate::extractor::platforms::tiktok::create_tiktok_danmu_provider;
use crate::extractor::platforms::twitcasting::create_twitcasting_danmu_provider;
use crate::extractor::platforms::twitch::create_twitch_danmu_provider;

static REGISTERED: RwLock<Vec<Arc<dyn DanmuProvider>>> = RwLock::new(Vec::new());

/// Adds a provider for the rest of the process, for tests that need danmu from an offline
/// platform.
///
/// Every registry consults registered providers after its own, so a registered provider never
/// takes a URL or platform name from a built-in one.
#[doc(hidden)]
pub fn register_provider(provider: Arc<dyn DanmuProvider>) {
    REGISTERED.write().push(provider);
}

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
        registry.register(Arc::new(create_huya_danmu_provider()));
        registry.register(Arc::new(create_bilibili_danmu_provider()));
        registry.register(Arc::new(create_douyu_danmu_provider()));
        registry.register(Arc::new(create_douyin_danmu_provider()));
        registry.register(Arc::new(create_twitch_danmu_provider()));
        registry.register(Arc::new(create_twitcasting_danmu_provider()));
        registry.register(Arc::new(create_soop_danmu_provider()));
        registry.register(Arc::new(create_bigo_danmu_provider()));
        registry.register(Arc::new(create_tiktok_danmu_provider()));
        registry
    }

    /// Register a provider.
    pub fn register(&mut self, provider: Arc<dyn DanmuProvider>) {
        self.providers.push(provider);
    }

    /// Get a provider for the given platform.
    pub fn get_by_platform(&self, platform: &str) -> Option<Arc<dyn DanmuProvider>> {
        self.find(|p| p.platform().eq_ignore_ascii_case(platform))
    }

    /// Get a provider that supports the given URL.
    pub fn get_by_url(&self, url: &str) -> Option<Arc<dyn DanmuProvider>> {
        self.find(|p| p.supports_url(url))
    }

    /// The first matching provider: this registry's own, then registered ones.
    fn find(
        &self,
        matches: impl Fn(&Arc<dyn DanmuProvider>) -> bool,
    ) -> Option<Arc<dyn DanmuProvider>> {
        if let Some(provider) = self.providers.iter().find(|p| matches(p)) {
            return Some(provider.clone());
        }
        REGISTERED.read().iter().find(|p| matches(p)).cloned()
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
        assert!(platforms.contains(&"soop"));
        assert!(platforms.contains(&"bigo"));
        assert!(platforms.contains(&"tiktok"));
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
