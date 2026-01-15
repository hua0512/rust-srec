//! Bilibili cookie and credential utilities.
//!
//! This module provides helper functions for working with Bilibili cookies
//! and the cookie refresh process.

use std::collections::HashMap;

/// Extract a specific cookie value from a cookie string.
///
/// # Example
/// ```
/// use platforms_parser::extractor::platforms::bilibili::cookie_utils::extract_cookie_value;
///
/// let cookies = "SESSDATA=abc123; bili_jct=xyz789";
/// assert_eq!(extract_cookie_value(cookies, "SESSDATA"), Some("abc123".to_string()));
/// ```
pub fn extract_cookie_value(cookies: &str, name: &str) -> Option<String> {
    for cookie in cookies.split(';') {
        let parts: Vec<&str> = cookie.trim().splitn(2, '=').collect();
        if parts.len() == 2 && parts[0] == name {
            return Some(parts[1].to_string());
        }
    }
    None
}

/// Extract refresh_csrf from the Bilibili correspond HTML page.
///
/// Looks for: `<div id="1-name">...</div>` and extracts the content.
pub fn extract_refresh_csrf(html: &str) -> Option<String> {
    let start_marker = r#"<div id="1-name">"#;
    let end_marker = "</div>";

    let start = html.find(start_marker)? + start_marker.len();
    let remaining = &html[start..];
    let end = remaining.find(end_marker)?;

    Some(remaining[..end].trim().to_string())
}

/// Parse cookies from Set-Cookie response headers.
pub fn parse_set_cookies(headers: &reqwest::header::HeaderMap) -> HashMap<String, String> {
    let mut cookies = HashMap::new();

    for value in headers.get_all(reqwest::header::SET_COOKIE) {
        if let Ok(cookie_str) = value.to_str() {
            // Parse "name=value; Path=...; ..."
            if let Some(kv) = cookie_str.split(';').next() {
                let parts: Vec<&str> = kv.splitn(2, '=').collect();
                if parts.len() == 2 {
                    cookies.insert(parts[0].to_string(), parts[1].to_string());
                }
            }
        }
    }

    cookies
}

/// Rebuild cookie string with updated values while preserving priority ordering.
///
/// Priority cookies (SESSDATA, bili_jct, DedeUserID, DedeUserID__ckMd5) come first.
pub fn rebuild_cookies(original: &str, updates: &HashMap<String, String>) -> String {
    let mut cookie_map: HashMap<String, String> = HashMap::new();

    // Parse original cookies
    for cookie in original.split(';') {
        let parts: Vec<&str> = cookie.trim().splitn(2, '=').collect();
        if parts.len() == 2 {
            cookie_map.insert(parts[0].to_string(), parts[1].to_string());
        }
    }

    // Apply updates
    for (key, value) in updates {
        cookie_map.insert(key.clone(), value.clone());
    }

    // Rebuild string - ensure important cookies come first
    let priority_cookies = ["SESSDATA", "bili_jct", "DedeUserID", "DedeUserID__ckMd5"];
    let mut result = Vec::new();

    for key in priority_cookies {
        if let Some(value) = cookie_map.remove(key) {
            result.push(format!("{}={}", key, value));
        }
    }

    // Add remaining cookies
    for (key, value) in cookie_map {
        result.push(format!("{}={}", key, value));
    }

    result.join("; ")
}

/// Name used for storing refresh_token in cookie string.
pub const REFRESH_TOKEN_KEY: &str = "refresh_token";

/// Extract refresh_token from a cookie string.
///
/// The refresh_token may be stored as a pseudo-cookie with the key "refresh_token".
/// This allows storing refresh_token alongside cookies in a single string.
///
/// # Example
/// ```
/// use platforms_parser::extractor::platforms::bilibili::cookie_utils::extract_refresh_token;
///
/// let cookies = "SESSDATA=abc; bili_jct=xyz; refresh_token=my_token";
/// assert_eq!(extract_refresh_token(cookies), Some("my_token".to_string()));
/// ```
pub fn extract_refresh_token(cookies: &str) -> Option<String> {
    extract_cookie_value(cookies, REFRESH_TOKEN_KEY)
}

/// Embed refresh_token into a cookie string.
///
/// This allows storing refresh_token alongside cookies for simpler storage.
/// If refresh_token already exists, it will be updated.
///
/// # Example
/// ```
/// use platforms_parser::extractor::platforms::bilibili::cookie_utils::embed_refresh_token;
///
/// let cookies = "SESSDATA=abc; bili_jct=xyz";
/// let with_token = embed_refresh_token(cookies, "my_token");
/// assert!(with_token.contains("refresh_token=my_token"));
/// ```
pub fn embed_refresh_token(cookies: &str, refresh_token: &str) -> String {
    let mut cookie_map: HashMap<String, String> = HashMap::new();

    // Parse existing cookies
    for cookie in cookies.split(';') {
        let parts: Vec<&str> = cookie.trim().splitn(2, '=').collect();
        if parts.len() == 2 {
            cookie_map.insert(parts[0].to_string(), parts[1].to_string());
        }
    }

    // Add/update refresh_token
    cookie_map.insert(REFRESH_TOKEN_KEY.to_string(), refresh_token.to_string());

    // Rebuild with priority ordering
    let priority_cookies = [
        "SESSDATA",
        "bili_jct",
        "DedeUserID",
        "DedeUserID__ckMd5",
        REFRESH_TOKEN_KEY,
    ];
    let mut result = Vec::new();

    for key in priority_cookies {
        if let Some(value) = cookie_map.remove(key) {
            result.push(format!("{}={}", key, value));
        }
    }

    // Add remaining cookies
    for (key, value) in cookie_map {
        result.push(format!("{}={}", key, value));
    }

    result.join("; ")
}

/// Remove refresh_token from a cookie string (for sending to API).
///
/// Since refresh_token is not a real HTTP cookie, it should be removed
/// before using the cookie string for API requests.
pub fn strip_refresh_token(cookies: &str) -> String {
    cookies
        .split(';')
        .filter_map(|part| {
            let parts: Vec<&str> = part.trim().splitn(2, '=').collect();
            if parts.len() == 2 && parts[0] != REFRESH_TOKEN_KEY {
                Some(format!("{}={}", parts[0], parts[1]))
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// API URLs for Bilibili cookie management.
pub mod urls {
    /// Cookie info check URL
    pub const COOKIE_INFO: &str = "https://passport.bilibili.com/x/passport-login/web/cookie/info";
    /// Correspond URL template for refresh_csrf
    pub const CORRESPOND: &str = "https://www.bilibili.com/correspond/1/";
    /// Cookie refresh URL
    pub const REFRESH: &str = "https://passport.bilibili.com/x/passport-login/web/cookie/refresh";
    /// Confirm refresh URL
    pub const CONFIRM: &str = "https://passport.bilibili.com/x/passport-login/web/confirm/refresh";
    /// NAV API for validation
    pub const NAV: &str = "https://api.bilibili.com/x/web-interface/nav";       
    /// Live user info API (requires authenticated cookies)
    pub const USER_INFO: &str =
        "https://api.live.bilibili.com/xlive/web-ucenter/user/get_user_info";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_cookie_value() {
        let cookies = "SESSDATA=abc123; bili_jct=xyz789; DedeUserID=12345";

        assert_eq!(
            extract_cookie_value(cookies, "SESSDATA"),
            Some("abc123".to_string())
        );
        assert_eq!(
            extract_cookie_value(cookies, "bili_jct"),
            Some("xyz789".to_string())
        );
        assert_eq!(
            extract_cookie_value(cookies, "DedeUserID"),
            Some("12345".to_string())
        );
        assert_eq!(extract_cookie_value(cookies, "nonexistent"), None);
    }

    #[test]
    fn test_extract_refresh_csrf() {
        let html = r#"
        <html>
        <body>
        <div id="1-name">abcdef123456csrf</div>
        </body>
        </html>
        "#;

        let result = extract_refresh_csrf(html);
        assert_eq!(result, Some("abcdef123456csrf".to_string()));
    }

    #[test]
    fn test_extract_refresh_csrf_not_found() {
        let html = "<html><body>No csrf here</body></html>";
        let result = extract_refresh_csrf(html);
        assert_eq!(result, None);
    }

    #[test]
    fn test_rebuild_cookies() {
        let original = "SESSDATA=old; bili_jct=old; other=value";
        let mut updates = HashMap::new();
        updates.insert("SESSDATA".to_string(), "new".to_string());
        updates.insert("bili_jct".to_string(), "new".to_string());

        let result = rebuild_cookies(original, &updates);

        // Should contain new values
        assert!(result.contains("SESSDATA=new"));
        assert!(result.contains("bili_jct=new"));
        assert!(result.contains("other=value"));
    }

    #[test]
    fn test_extract_refresh_token() {
        let cookies = "SESSDATA=abc; bili_jct=xyz; refresh_token=my_token";
        assert_eq!(extract_refresh_token(cookies), Some("my_token".to_string()));

        let cookies_without = "SESSDATA=abc; bili_jct=xyz";
        assert_eq!(extract_refresh_token(cookies_without), None);
    }

    #[test]
    fn test_embed_refresh_token() {
        let cookies = "SESSDATA=abc; bili_jct=xyz";
        let result = embed_refresh_token(cookies, "my_token");
        assert!(result.contains("refresh_token=my_token"));
        assert!(result.contains("SESSDATA=abc"));
    }

    #[test]
    fn test_strip_refresh_token() {
        let cookies = "SESSDATA=abc; bili_jct=xyz; refresh_token=my_token";
        let result = strip_refresh_token(cookies);
        assert!(!result.contains("refresh_token"));
        assert!(result.contains("SESSDATA=abc"));
        assert!(result.contains("bili_jct=xyz"));
    }
}
