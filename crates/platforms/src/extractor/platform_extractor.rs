use crate::extractor::default::DEFAULT_UA;
use crate::media::StreamInfo;

use super::{super::media::media_info::MediaInfo, error::ExtractorError};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, RequestBuilder};
use rustc_hash::FxHashMap;
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
/// ```rust,ignore
/// # use reqwest::Client;
/// # use platforms_parser::extractor::platform_extractor::Extractor;
/// #
/// # async fn doc_test() -> Result<(), Box<dyn std::error::Error>> {
/// let mut extractor = Extractor::new("Platform".to_string(), "https://example.com".to_string(), Client::new());
///
/// // Add individual cookies
/// extractor.add_cookie("session_id", "abc123");
///
/// // Parse cookie string from browser/external source
/// extractor.set_cookies_from_string("token=xyz789; user_id=12345; theme=dark");
///
/// // Cookies are automatically included in all requests
/// let response = extractor.get("https://api.example.com/data").send().await?;
///
/// // Response cookies are automatically parsed and stored
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Extractor {
    // url to extract from, e.g., "https://www.huya.com/123456"
    pub url: String,
    // name of the platform, e.g., "Huya", "Douyin"...
    pub platform_name: String,
    // The reqwest client
    pub client: Client,
    // platform-specific headers and parameters
    platform_headers: HeaderMap,
    pub platform_params: FxHashMap<String, String>,
    /// Cookie storage for the extractor. Each extractor instance maintains
    /// its own cookies for platform-specific session management.
    pub cookies: FxHashMap<String, String>,
}

impl Extractor {
    pub fn new<S1: Into<String>, S2: Into<String>>(
        platform_name: S1,
        platform_url: S2,
        client: Client,
    ) -> Self {
        let mut default_headers = HeaderMap::new();
        default_headers.insert(
            reqwest::header::USER_AGENT,
            HeaderValue::from_static(DEFAULT_UA),
        );
        default_headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );
        default_headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("zh-CN,zh;q=0.8,en-US;q=0.5,en;q=0.3"),
        );
        // Do not set `Accept-Encoding` here.
        // Reqwest auto-adds it (and auto-decompresses) when the corresponding
        // crate features are enabled, as long as we don't override the header.

        Self {
            platform_name: platform_name.into(),
            url: platform_url.into(),
            client,
            platform_headers: default_headers,
            platform_params: FxHashMap::default(),
            cookies: FxHashMap::default(),
        }
    }

    #[inline]
    pub fn set_origin_static(&mut self, origin: &'static str) {
        self.add_header_owned(reqwest::header::ORIGIN, HeaderValue::from_static(origin));
    }

    #[inline]
    pub fn set_referer_static(&mut self, referer: &'static str) {
        self.add_header_owned(reqwest::header::REFERER, HeaderValue::from_static(referer));
    }

    #[inline]
    pub fn set_origin_and_referer_static(&mut self, base_url: &'static str) {
        let v = HeaderValue::from_static(base_url);
        self.add_header_owned(reqwest::header::ORIGIN, v.clone());
        self.add_header_owned(reqwest::header::REFERER, v);
    }

    /// Insert an arbitrary header.
    ///
    /// Prefer `add_header_typed` / `add_header_owned` for better type safety.
    pub fn add_header<K: Into<String>, V: Into<String>>(&mut self, key: K, value: V) {
        match HeaderName::from_str(&key.into()) {
            Ok(name) => match HeaderValue::from_str(&value.into()) {
                Ok(value) => {
                    self.platform_headers.insert(name, value);
                }
                Err(e) => {
                    debug!(error = %e, "Invalid header value; skipping");
                }
            },
            Err(e) => {
                debug!(error = %e, "Invalid header name; skipping");
            }
        }
    }

    pub fn add_header_str<K: AsRef<str>, V: AsRef<str>>(&mut self, key: K, value: V) {
        match HeaderName::from_str(key.as_ref()) {
            Ok(name) => match HeaderValue::from_str(value.as_ref()) {
                Ok(value) => {
                    self.platform_headers.insert(name, value);
                }
                Err(e) => {
                    debug!(error = %e, "Invalid header value; skipping");
                }
            },
            Err(e) => {
                debug!(error = %e, "Invalid header name; skipping");
            }
        }
    }

    pub fn add_header_name<K: Into<HeaderName>, V: Into<HeaderValue>>(&mut self, key: K, value: V) {
        self.platform_headers.insert(key.into(), value.into());
    }

    pub fn add_header_owned<K: Into<HeaderName>, V: Into<HeaderValue>>(
        &mut self,
        key: K,
        value: V,
    ) {
        self.platform_headers.insert(key.into(), value.into());
    }

    pub fn add_header_typed<K: Into<HeaderName>, V: AsRef<str>>(&mut self, key: K, value: V) {
        match HeaderValue::from_str(value.as_ref()) {
            Ok(value) => {
                self.platform_headers.insert(key.into(), value);
            }
            Err(e) => {
                debug!(error = %e, "Invalid header value; skipping");
            }
        }
    }

    pub fn add_param<K: Into<String>, V: Into<String>>(&mut self, key: K, value: V) {
        self.platform_params.insert(key.into(), value.into());
    }

    pub fn get_param(&self, key: &str) -> Option<&String> {
        self.platform_params.get(key)
    }

    pub fn update_param<K: Into<String>, V: Into<String>>(&mut self, key: K, value: V) {
        self.platform_params.insert(key.into(), value.into());
    }

    pub fn update_param_by_key(&mut self, key: &str, value: &str) {
        self.platform_params
            .insert(key.to_string(), value.to_string());
    }

    pub fn update_cookie<K: Into<String>, V: Into<String>>(&mut self, key: K, value: V) {
        self.cookies.insert(key.into(), value.into());
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
    /// ```rust,no_run
    /// # use reqwest::Client;
    /// # use platforms_parser::extractor::platform_extractor::Extractor;
    /// # let mut extractor = Extractor::new("Platform".to_string(), "https://example.com".to_string(), Client::new());
    /// extractor.add_cookie("session_token", "abc123def456");
    /// ```
    pub fn add_cookie<N: Into<String>, V: Into<String>>(&mut self, name: N, value: V) {
        self.cookies.insert(name.into(), value.into());
    }

    /// Add multiple cookies from a HashMap.
    ///
    /// # Arguments
    ///
    /// * `cookies` - HashMap containing cookie name-value pairs
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use rustc_hash::FxHashMap;
    /// # use reqwest::Client;
    /// # use platforms_parser::extractor::platform_extractor::Extractor;
    /// # let mut extractor = Extractor::new("Platform".to_string(), "https://example.com".to_string(), Client::new());
    /// let mut cookies = FxHashMap::default();
    /// cookies.insert("token".to_string(), "xyz789".to_string());
    /// cookies.insert("user_id".to_string(), "12345".to_string());
    /// extractor.add_cookies(cookies);
    /// ```
    pub fn add_cookies(&mut self, cookies: FxHashMap<String, String>) {
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
    /// ```rust,no_run
    /// # use reqwest::Client;
    /// # use platforms_parser::extractor::platform_extractor::Extractor;
    /// # let mut extractor = Extractor::new("Platform".to_string(), "https://example.com".to_string(), Client::new());
    /// extractor.set_cookies_from_string("sessionid=abc123; csrftoken=def456; theme=dark");
    /// ```
    pub fn set_cookies_from_string(&mut self, cookie_string: &str) {
        // Accept common separators: ';' from Cookie headers and '\n' from copy/paste.
        for part in cookie_string.split(&[';', '\n'][..]).map(str::trim) {
            if part.is_empty() {
                continue;
            }

            let Some((name, value)) = part.split_once('=') else {
                continue;
            };
            let name = name.trim();
            let value = value.trim();
            if name.is_empty() || value.is_empty() {
                continue;
            }

            self.cookies.insert(name.to_owned(), value.to_owned());
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
    pub fn get_cookies(&self) -> &FxHashMap<String, String> {
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

        // Rough capacity estimate to avoid repeated growth.
        let mut cookie_string = String::with_capacity(
            self.cookies
                .iter()
                .map(|(k, v)| k.len() + 1 + v.len() + 2)
                .sum(),
        );

        for (name, value) in &self.cookies {
            if !cookie_string.is_empty() {
                cookie_string.push_str("; ");
            }
            cookie_string.push_str(name);
            cookie_string.push('=');
            cookie_string.push_str(value);
        }

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
            if let Ok(cookie_str) = value.to_str()
                && let Some(cookie_part) = cookie_str.split(';').next()
                && let Some((name, value)) = cookie_part.split_once('=')
            {
                let name = name.trim();
                let value = value.trim();
                if name.is_empty() || value.is_empty() {
                    continue;
                }
                debug!("Auto-storing cookie: {}={}", name, value);
                self.cookies.insert(name.to_owned(), value.to_owned());
            }
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
        let mut headers = self.platform_headers.clone();

        if !self.cookies.is_empty()
            && let Some(cookie_header) = self.build_cookie_header()
        {
            match reqwest::header::HeaderValue::from_str(&cookie_header) {
                Ok(value) => {
                    debug!("Adding cookies to request: {:?}", value);
                    headers.insert(reqwest::header::COOKIE, value);
                }
                Err(e) => {
                    // If cookies are malformed, skip the Cookie header instead of sending
                    // an empty/invalid value.
                    debug!(error = %e, "Failed to build Cookie header");
                }
            }
        }

        self.client
            .request(method, url)
            .headers(headers)
            .query(&self.platform_params)
    }

    pub fn get_platform_headers(&self) -> &HeaderMap {
        &self.platform_headers
    }

    pub fn get_platform_headers_map(&self) -> FxHashMap<String, String> {
        // Headers are consumed by callers (MediaInfo stores owned Strings), so we must allocate.
        // Pre-size to avoid rehashing on repeated calls.
        let mut headers_map =
            FxHashMap::with_capacity_and_hasher(self.platform_headers.len(), Default::default());

        for (key, value) in &self.platform_headers {
            if let Ok(value) = value.to_str() {
                headers_map.insert(key.as_str().to_owned(), value.to_owned());
            }
        }

        headers_map
    }
}

#[async_trait]
pub trait PlatformExtractor: Send + Sync {
    fn get_extractor(&self) -> &Extractor;

    fn get_platform_headers(&self) -> &HeaderMap {
        &self.get_extractor().platform_headers
    }

    fn get_platform_params(&self) -> &FxHashMap<String, String> {
        &self.get_extractor().platform_params
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError>;

    #[allow(unused_variables)]
    async fn get_url(&self, stream_info: &mut StreamInfo) -> Result<(), ExtractorError> {
        // Default implementation, can be overridden by specific extractors
        Ok(())
    }
}
