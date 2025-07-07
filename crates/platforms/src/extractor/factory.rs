use super::error::ExtractorError;
use super::platform_extractor::PlatformExtractor;
use regex::Regex;
use reqwest::Client;
use std::sync::Arc;

// A type alias for a thread-safe constructor function.
type ExtractorConstructor = Arc<
    dyn Fn(String, Client, Option<String>, Option<serde_json::Value>) -> Box<dyn PlatformExtractor>
        + Send
        + Sync,
>;

/// A factory for creating platform-specific extractors.
pub struct ExtractorFactory {
    registry: Vec<(Regex, ExtractorConstructor)>,
    client: Client,
}

impl ExtractorFactory {
    /// Creates a new, empty extractor factory.
    pub fn new(client: Client) -> Self {
        Self {
            registry: Vec::new(),
            client,
        }
    }

    /// Registers a new extractor type with the factory.
    ///
    /// # Arguments
    ///
    /// * `regex_str` - The regular expression that identifies URLs for this platform.
    /// * `constructor` - A function that takes a URL and returns a new extractor instance.
    pub fn register(
        &mut self,
        regex_str: &str,
        constructor: ExtractorConstructor,
    ) -> Result<(), regex::Error> {
        let regex = Regex::new(regex_str)?;
        self.registry.push((regex, constructor));
        Ok(())
    }

    /// Creates a platform-specific extractor for the given URL.
    ///
    /// It iterates through the registered platforms and returns the first one
    /// that matches the URL.
    pub fn create_extractor(
        &self,
        url: &str,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Result<Box<dyn PlatformExtractor>, ExtractorError> {
        for (regex, constructor) in &self.registry {
            if regex.is_match(url) {
                return Ok(constructor(
                    url.to_string(),
                    self.client.clone(),
                    cookies,
                    extras,
                ));
            }
        }
        Err(ExtractorError::UnsupportedExtractor)
    }
}
