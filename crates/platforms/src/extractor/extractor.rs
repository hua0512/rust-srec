use crate::media::{StreamInfo, stream_info};

use super::{super::media::media_info::MediaInfo, error::ExtractorError};
use async_trait::async_trait;
use regex::Regex;
use reqwest::{Client, Method, RequestBuilder};
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct Extractor {
    // url to extract from, e.g., "https://www.huya.com/123456"
    pub url: String,
    // name of the platform, e.g., "Huya", "Douyin"...
    pub platform_name: String,
    // The reqwest client
    pub client: Client,
    // optional regex to match the platform URL
    pub platform_regex: Option<Regex>,
    // platform-specific headers and parameters
    pub platform_headers: HashMap<String, String>,
    pub platform_params: HashMap<String, String>,
}

impl Extractor {
    pub fn new(platform_name: String, platform_url: String, client: Client) -> Self {
        Self {
            platform_name,
            url: platform_url,
            client,
            platform_regex: None,
            platform_headers: HashMap::new(),
            platform_params: HashMap::new(),
        }
    }

    pub fn add_header(&mut self, key: String, value: String) {
        self.platform_headers.insert(key, value);
    }

    pub fn add_param(&mut self, key: String, value: String) {
        self.platform_params.insert(key, value);
    }

    pub fn set_regex(&mut self, regex_str: &str) -> Result<(), regex::Error> {
        self.platform_regex = Some(Regex::new(regex_str)?);
        Ok(())
    }

    pub fn is_url_valid(&self) -> bool {
        if let Some(re) = &self.platform_regex {
            re.is_match(&self.url)
        } else {
            // If no regex is provided, assume the URL is valid
            true
        }
    }

    pub fn get(&self, url: &str) -> RequestBuilder {
        self.request(Method::GET, url)
    }

    pub fn post(&self, url: &str) -> RequestBuilder {
        self.request(Method::POST, url)
    }

    pub fn post_bytes(&self, url: &str, body: &[u8]) -> RequestBuilder {
        self.request(Method::POST, url).body(body.to_vec())
    }

    pub fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let mut headers = reqwest::header::HeaderMap::new();
        for (key, value) in &self.platform_headers {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_str(key),
                reqwest::header::HeaderValue::from_str(value),
            ) {
                headers.insert(name, val);
            }
        }

        self.client
            .request(method, url)
            .headers(headers)
            .query(&self.platform_params)
    }
}

#[async_trait]
pub trait PlatformExtractor: Send + Sync {
    fn get_extractor(&self) -> &Extractor;

    async fn extract(&self) -> Result<MediaInfo, ExtractorError>;

    async fn get_url(
        &self,
        stream_info: stream_info::StreamInfo,
    ) -> Result<StreamInfo, ExtractorError> {
        // Default implementation, can be overridden by specific extractors
        Ok(stream_info)
    }
}
