use crate::extractor::default::DEFAULT_UA;
use crate::media::{StreamInfo, stream_info};

use super::{super::media::media_info::MediaInfo, error::ExtractorError};
use async_trait::async_trait;
use regex::Regex;
use reqwest::{Client, Method, RequestBuilder};
use std::collections::HashMap;
use std::str::FromStr;
use tracing::debug;

/// Base extractor with comprehensive cookie management support.
///
/// Each extractor instance maintains its own cookie store, allowing for
/// platform-specific session management and authentication.
///
/// # Cookie Features
///
/// - **Automatic cookie inclusion**: Cookies are automatically added to all HTTP requests
/// - **Response cookie parsing**: Cookies from server responses are automatically stored
/// - **Flexible cookie management**: Add individual cookies, bulk cookies, or parse cookie strings
/// - **Platform isolation**: Each extractor instance has its own cookie store
///
/// # Example Usage
///
/// ```rust
/// use reqwest::Client;
/// use crate::extractor::extractor::Extractor;
///
/// let mut extractor = Extractor::new("Platform".to_string(), "https://example.com".to_string(), Client::new());
///
/// // Add individual cookies
/// extractor.add_cookie("session_id".to_string(), "abc123".to_string());
///
/// // Parse cookie string from browser/external source
/// extractor.set_cookies_from_string("token=xyz789; user_id=12345; theme=dark");
///
/// // Cookies are automatically included in all requests
/// let response = extractor.get("https://api.example.com/data").send().await?;
///
/// // Response cookies are automatically parsed and stored
/// ```
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
    /// Cookie storage for the extractor. Each extractor instance maintains
    /// its own cookies for platform-specific session management.
    pub cookies: HashMap<String, String>,
}

impl Extractor {
    pub fn new(platform_name: String, platform_url: String, client: Client) -> Self {
        let mut default_headers = HashMap::new();
        default_headers.insert(
            reqwest::header::USER_AGENT.to_string(),
            DEFAULT_UA.to_string(),
        );
        default_headers.insert(
            reqwest::header::ACCEPT.to_string(),
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".to_string(),
        );
        default_headers.insert(
            reqwest::header::ACCEPT_LANGUAGE.to_string(),
            "zh-CN,zh;q=0.8,en-US;q=0.5,en;q=0.3".to_string(),
        );
        default_headers.insert(
            reqwest::header::ACCEPT_ENCODING.to_string(),
            "gzip, deflate, br".to_string(),
        );

        Self {
            platform_name,
            url: platform_url,
            client,
            platform_regex: None,
            platform_headers: default_headers,
            platform_params: HashMap::new(),
            cookies: HashMap::new(),
        }
    }

    pub fn add_header(&mut self, key: String, value: String) {
        self.platform_headers.insert(key, value);
    }

    pub fn insert_header(&mut self, key: String, value: String) {
        self.platform_headers.insert(key, value);
    }

    pub fn add_param(&mut self, key: String, value: String) {
        self.platform_params.insert(key, value);
    }

    pub fn get_param(&self, key: &str) -> Option<&String> {
        self.platform_params.get(key)
    }

    pub fn update_param(&mut self, key: String, value: String) {
        self.platform_params.insert(key, value);
    }

    pub fn update_cookie(&mut self, key: String, value: String) {
        self.cookies.insert(key, value);
    }

    /// Add a single cookie to the extractor's cookie store.
    ///
    /// # Arguments
    ///
    /// * `name` - Cookie name
    /// * `value` - Cookie value
    ///
    /// # Example
    ///
    /// ```rust
    /// extractor.add_cookie("session_token".to_string(), "abc123def456".to_string());
    /// ```
    pub fn add_cookie(&mut self, name: String, value: String) {
        self.cookies.insert(name, value);
    }

    /// Add multiple cookies from a HashMap.
    ///
    /// # Arguments
    ///
    /// * `cookies` - HashMap containing cookie name-value pairs
    ///
    /// # Example
    ///
    /// ```rust
    /// let mut cookies = HashMap::new();
    /// cookies.insert("token".to_string(), "xyz789".to_string());
    /// cookies.insert("user_id".to_string(), "12345".to_string());
    /// extractor.add_cookies(cookies);
    /// ```
    pub fn add_cookies(&mut self, cookies: HashMap<String, String>) {
        self.cookies.extend(cookies);
    }

    /// Set cookies from a cookie string (format: "name1=value1; name2=value2").
    /// This is useful for importing cookies from browsers or external sources.
    ///
    /// # Arguments
    ///
    /// * `cookie_string` - Cookie string in standard format
    ///
    /// # Example
    ///
    /// ```rust
    /// extractor.set_cookies_from_string("sessionid=abc123; csrftoken=def456; theme=dark");
    /// ```
    pub fn set_cookies_from_string(&mut self, cookie_string: &str) {
        for cookie in cookie_string.split(';') {
            let cookie = cookie.trim();
            if let Some((name, value)) = cookie.split_once('=') {
                self.cookies
                    .insert(name.trim().to_string(), value.trim().to_string());
            }
        }
    }

    /// Clear all cookies from the extractor's cookie store.
    pub fn clear_cookies(&mut self) {
        self.cookies.clear();
    }

    /// Get all cookies as a reference to the internal HashMap.
    ///
    /// # Returns
    ///
    /// Reference to the cookie HashMap
    pub fn get_cookies(&self) -> &HashMap<String, String> {
        &self.cookies
    }

    /// Remove a specific cookie and return its value if it existed.
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the cookie to remove
    ///
    /// # Returns
    ///
    /// `Some(value)` if the cookie existed, `None` otherwise
    pub fn remove_cookie(&mut self, name: &str) -> Option<String> {
        self.cookies.remove(name)
    }

    /// Check if a specific cookie exists in the store.
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the cookie to check
    ///
    /// # Returns
    ///
    /// `true` if the cookie exists, `false` otherwise
    pub fn has_cookie(&self, name: &str) -> bool {
        self.cookies.contains_key(name)
    }

    /// Get the value of a specific cookie.
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the cookie to retrieve
    ///
    /// # Returns
    ///
    /// `Some(&value)` if the cookie exists, `None` otherwise
    pub fn get_cookie(&self, name: &str) -> Option<&String> {
        self.cookies.get(name)
    }

    /// Convert stored cookies to a Cookie header value string.
    /// This is used internally to add cookies to HTTP requests.
    ///
    /// # Returns
    ///
    /// `Some(cookie_header)` if cookies exist, `None` if no cookies are stored
    fn build_cookie_header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            return None;
        }

        let cookie_string = self
            .cookies
            .iter()
            .map(|(name, value)| format!("{}={}", name, value))
            .collect::<Vec<_>>()
            .join("; ");

        Some(cookie_string)
    }

    /// Parse and store cookies from HTTP response headers.
    /// This method is automatically called after each request to capture
    /// any new cookies sent by the server.
    ///
    /// # Arguments
    ///
    /// * `headers` - HTTP response headers to parse for cookies
    pub fn parse_and_store_cookies(&mut self, headers: &reqwest::header::HeaderMap) {
        for value in headers.get_all("set-cookie").iter() {
            if let Ok(cookie_str) = value.to_str() {
                // Parse "name=value; other_attributes" format
                if let Some(cookie_part) = cookie_str.split(';').next() {
                    if let Some((name, value)) = cookie_part.split_once('=') {
                        let name = name.trim().to_string();
                        let value = value.trim().to_string();
                        debug!("Auto-storing cookie: {}={}", name, value);
                        self.cookies.insert(name, value);
                    }
                }
            }
        }
    }

    pub fn set_regex_str(&mut self, regex_str: &str) -> Result<(), regex::Error> {
        self.platform_regex = Some(Regex::new(regex_str)?);
        Ok(())
    }

    pub fn set_regex(&mut self, regex: Regex) {
        self.platform_regex = Some(regex);
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

    /// Create an HTTP request with automatic cookie inclusion.
    /// All stored cookies are automatically added to the request headers.
    ///
    /// # Arguments
    ///
    /// * `method` - HTTP method (GET, POST, etc.)
    /// * `url` - Target URL for the request
    ///
    /// # Returns
    ///
    /// RequestBuilder with cookies and platform headers pre-configured
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

        // Automatically add cookies to headers if any exist
        if let Some(cookie_header) = self.build_cookie_header() {
            if let Ok(cookie_value) = reqwest::header::HeaderValue::from_str(&cookie_header) {
                headers.insert(reqwest::header::COOKIE, cookie_value);
                debug!("Adding cookies to request: {}", cookie_header);
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

    fn get_platform_headers(&self) -> &HashMap<String, String> {
        &self.get_extractor().platform_headers
    }

    fn get_platform_params(&self) -> &HashMap<String, String> {
        &self.get_extractor().platform_params
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError>;

    async fn get_url(
        &self,
        stream_info: stream_info::StreamInfo,
    ) -> Result<StreamInfo, ExtractorError> {
        // Default implementation, can be overridden by specific extractors
        Ok(stream_info)
    }
}
