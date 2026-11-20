//! A cookie jar that keeps Weibo's sign-in cookies the way a browser does.
//!
//! A Weibo login is a set of cookies per site: `.weibo.com`, `.weibo.cn` and
//! `.sina.com.cn` each get their own `SUB`, with a different value, and some
//! cookies belong to one host only. The jar keeps each cookie with its
//! domain and expiry, sends a host only the cookies a browser would, and
//! drops cookies once they expire, so a stale value is never sent where a
//! browser would send nothing. Paths and the `Secure` flag are not kept:
//! Weibo scopes its sign-in cookies to `/` and serves the sign-in over HTTPS.

use reqwest::header::{HeaderMap, SET_COOKIE};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Cookie {
    /// `.example.com` for a cookie its site's subdomains receive too, or a
    /// bare host for a cookie only that host receives.
    domain: String,
    name: String,
    value: String,
    /// Unix seconds; absent for a cookie that lasts for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires: Option<i64>,
}

impl Cookie {
    fn sent_to(&self, host: &str) -> bool {
        match self.domain.strip_prefix('.') {
            Some(site) => on_site(host, site),
            None => host == self.domain,
        }
    }

    fn alive(&self, now: i64) -> bool {
        self.expires.is_none_or(|expires| expires > now)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct Jar(Vec<Cookie>);

/// Cookie values are credentials, so only their number is shown.
impl std::fmt::Debug for Jar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Jar({} cookies)", self.0.len())
    }
}

impl Jar {
    /// The Cookie header a browser sends to `url` at `now` (Unix seconds).
    pub(super) fn header_for(&self, url: &Url, now: i64) -> Option<String> {
        let host = url.host_str()?;
        let header = self
            .0
            .iter()
            .filter(|cookie| cookie.alive(now) && cookie.sent_to(host))
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ");
        (!header.is_empty()).then_some(header)
    }

    /// Whether a live cookie `name` is kept for exactly `domain`.
    pub(super) fn contains(&self, domain: &str, name: &str, now: i64) -> bool {
        self.0
            .iter()
            .any(|cookie| cookie.domain == domain && cookie.name == name && cookie.alive(now))
    }

    /// Applies the `Set-Cookie` headers of a response from `url`: a cookie
    /// replaces the one with the same domain and name, and an expired or
    /// empty one deletes it. Cookies for a domain other than the host's
    /// own, and headers that are not visible ASCII, are ignored.
    pub(super) fn store(&mut self, url: &Url, headers: &HeaderMap, now: i64) {
        let Some(host) = url.host_str() else {
            return;
        };
        for header in headers.get_all(SET_COOKIE) {
            let Some(cookie) = header
                .to_str()
                .ok()
                .and_then(|header| parse_set_cookie(header, host, now))
            else {
                continue;
            };
            self.0
                .retain(|kept| kept.domain != cookie.domain || kept.name != cookie.name);
            if cookie.alive(now) && !cookie.value.is_empty() {
                self.0.push(cookie);
            }
        }
    }

    /// Forgets the cookies that have expired by `now`.
    pub(super) fn drop_expired(&mut self, now: i64) {
        self.0.retain(|cookie| cookie.alive(now));
    }
}

/// Whether `host` is `site` or one of its subdomains.
pub(super) fn on_site(host: &str, site: &str) -> bool {
    host == site
        || host
            .strip_suffix(site)
            .is_some_and(|subdomain| subdomain.ends_with('.'))
}

fn parse_set_cookie(header: &str, host: &str, now: i64) -> Option<Cookie> {
    let mut parts = header.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let mut domain = host.to_owned();
    let mut expires = None;
    let mut max_age = None;
    for attribute in parts {
        let (key, attribute_value) = attribute.split_once('=').unwrap_or((attribute, ""));
        let attribute_value = attribute_value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "domain" => {
                let site = attribute_value.trim_start_matches('.').to_ascii_lowercase();
                if site.is_empty() {
                    continue;
                }
                if !on_site(host, &site) {
                    return None;
                }
                domain = format!(".{site}");
            }
            "max-age" => max_age = attribute_value.parse::<i64>().ok(),
            "expires" => expires = http_date(attribute_value),
            _ => {}
        }
    }
    Some(Cookie {
        domain,
        name: name.to_owned(),
        value: value.trim().trim_matches('"').to_owned(),
        // `Max-Age` takes precedence over `Expires`, as in a browser.
        expires: max_age.map(|age| now.saturating_add(age)).or(expires),
    })
}

/// Unix seconds of a cookie date such as `Sat, 20-Nov-2027 15:58:26 GMT`.
/// An unreadable date leaves the cookie lasting for the session.
fn http_date(value: &str) -> Option<i64> {
    let date = value
        .split_once(',')
        .map_or(value, |(_, date)| date)
        .trim()
        .replace('-', " ");
    jiff::fmt::rfc2822::DateTimeParser::new()
        .parse_timestamp(&date)
        .ok()
        .map(|timestamp| timestamp.as_second())
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderValue;

    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn set_cookies(values: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(SET_COOKIE, HeaderValue::from_static(value));
        }
        headers
    }

    fn url(address: &str) -> Url {
        Url::parse(address).unwrap()
    }

    #[test]
    fn cookies_are_kept_per_domain_with_their_expiry_like_a_browser() {
        let mut jar = Jar::default();
        jar.store(
            &url("https://passport.weibo.com/sso/v2/login"),
            &set_cookies(&[
                "SUB=site; expires=Sat, 20-Nov-2027 15:58:26 GMT; path=/; domain=.weibo.com",
                "ALC=host-only; Max-Age=60; path=/",
                "OLD=gone; Domain=weibo.com",
                "TMP=gone-too; domain=.weibo.com",
                "FOREIGN=x; domain=.sina.com.cn",
                // Max-Age takes precedence over Expires.
                "SHORT=s; Max-Age=10; expires=Sat, 20-Nov-2027 15:58:26 GMT; domain=.weibo.com",
            ]),
            NOW,
        );
        jar.store(
            &url("https://weibo.com/"),
            &set_cookies(&[
                "OLD=deleted; expires=Thu, 01-Jan-1970 00:00:01 GMT; domain=.weibo.com",
                "TMP=; domain=.weibo.com",
                "SUB=replaced; expires=Sat, 20-Nov-2027 15:58:26 GMT; domain=.weibo.com",
                "WBPSESS=main; Max-Age=86400",
            ]),
            NOW,
        );
        assert_eq!(
            jar.header_for(&url("https://weibo.com/ajax"), NOW)
                .as_deref(),
            Some("SHORT=s; SUB=replaced; WBPSESS=main")
        );
        assert_eq!(
            jar.header_for(&url("https://passport.weibo.com/"), NOW)
                .as_deref(),
            Some("ALC=host-only; SHORT=s; SUB=replaced")
        );
        assert_eq!(
            jar.header_for(&url("https://login.sina.com.cn/"), NOW),
            None
        );
        assert_eq!(jar.header_for(&url("https://notweibo.com/"), NOW), None);
        // An expired cookie is no longer sent.
        assert_eq!(
            jar.header_for(&url("https://weibo.com/"), NOW + 11)
                .as_deref(),
            Some("SUB=replaced; WBPSESS=main")
        );
        let stored = serde_json::to_value(&jar).unwrap();
        assert!(stored.as_array().unwrap().iter().any(|cookie| {
            cookie["domain"] == ".weibo.com"
                && cookie["name"] == "SUB"
                && cookie["expires"] == 1_826_726_306
        }));
        jar.drop_expired(NOW + 61);
        assert_eq!(
            serde_json::to_value(&jar)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(!format!("{jar:?}").contains("replaced"));
    }
}
