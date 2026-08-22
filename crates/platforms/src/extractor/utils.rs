use std::collections::HashMap;

use regex::Regex;
use serde_json::Value;

use crate::extractor::error::ExtractorError;

#[inline]
pub fn capture_group_1<'a>(re: &Regex, input: &'a str) -> Option<&'a str> {
    re.captures(input)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
}

#[inline]
pub fn capture_group_1_owned(re: &Regex, input: &str) -> Option<String> {
    capture_group_1(re, input).map(ToOwned::to_owned)
}

#[inline]
pub fn capture_group_1_or_invalid_url<'a>(
    re: &Regex,
    input: &'a str,
) -> Result<&'a str, ExtractorError> {
    capture_group_1(re, input).ok_or_else(|| ExtractorError::InvalidUrl(input.to_string()))
}

#[inline]
pub fn extras_get_str<'a>(extras: Option<&'a Value>, key: &str) -> Option<&'a str> {
    extras.and_then(|e| e.get(key)).and_then(|v| v.as_str())
}

#[inline]
pub fn extras_get_bool(extras: Option<&Value>, key: &str) -> Option<bool> {
    extras.and_then(|e| e.get(key)).and_then(|v| {
        if let Some(b) = v.as_bool() {
            Some(b)
        } else if let Some(s) = v.as_str() {
            s.parse::<bool>().ok()
        } else {
            None
        }
    })
}

#[inline]
pub fn extras_get_i64(extras: Option<&Value>, key: &str) -> Option<i64> {
    extras.and_then(|e| e.get(key)).and_then(|v| {
        if let Some(n) = v.as_i64() {
            Some(n)
        } else if let Some(s) = v.as_str() {
            s.parse::<i64>().ok()
        } else {
            None
        }
    })
}

#[inline]
pub fn extras_get_u64(extras: Option<&Value>, key: &str) -> Option<u64> {
    extras.and_then(|e| e.get(key)).and_then(|v| {
        if let Some(n) = v.as_u64() {
            Some(n)
        } else if let Some(s) = v.as_str() {
            s.parse::<u64>().ok()
        } else {
            None
        }
    })
}

pub fn parse_bool_from_extras(extras: Option<&Value>, key: &str, default: bool) -> bool {
    extras_get_bool(extras, key).unwrap_or(default)
}

/// Parse a Cookie header (`a=1; b=2`) into ordered name/value pairs.
///
/// Empty names/values are dropped. Order of first appearance is preserved.
pub fn parse_cookie_header(input: &str) -> Vec<(String, String)> {
    input
        .split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let mut kv = part.splitn(2, '=');
            let name = kv.next()?.trim();
            let value = kv.next()?.trim();
            if name.is_empty() || value.is_empty() {
                return None;
            }
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

/// Merge Cookie headers. Pairs in `extra` override same-named pairs in `base`.
///
/// Order: base cookies first (with overrides applied in place), then any new
/// names from `extra`. Empty inputs are treated as absent.
pub fn merge_cookie_headers(base: Option<&str>, extra: Option<&str>) -> Option<String> {
    let base = base.map(str::trim).filter(|s| !s.is_empty());
    let extra = extra.map(str::trim).filter(|s| !s.is_empty());

    match (base, extra) {
        (None, None) => None,
        (Some(base), None) => Some(base.to_string()),
        (None, Some(extra)) => Some(extra.to_string()),
        (Some(base), Some(extra)) => {
            let mut parts = parse_cookie_header(base);
            let mut index_by_name: HashMap<String, usize> = HashMap::with_capacity(parts.len());
            for (idx, (name, _)) in parts.iter().enumerate() {
                index_by_name.insert(name.clone(), idx);
            }

            for (name, value) in parse_cookie_header(extra) {
                if let Some(&existing_idx) = index_by_name.get(&name) {
                    parts[existing_idx].1 = value;
                } else {
                    let idx = parts.len();
                    parts.push((name.clone(), value));
                    index_by_name.insert(name, idx);
                }
            }

            Some(
                parts
                    .into_iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    }
}

/// Convenience for non-optional cookie strings (empty string = absent).
#[inline]
pub fn merge_cookie_header_strs(base: &str, extra: &str) -> String {
    merge_cookie_headers(Some(base), Some(extra)).unwrap_or_default()
}

#[cfg(test)]
mod cookie_tests {
    use super::*;

    #[test]
    fn merge_overrides_same_name_preserves_base_order() {
        let merged = merge_cookie_headers(
            Some("AuthTicket=stale; other=x"),
            Some("AuthTicket=fresh; UserTicket=u1"),
        )
        .unwrap();
        assert!(merged.contains("AuthTicket=fresh"));
        assert!(!merged.contains("AuthTicket=stale"));
        assert!(merged.contains("other=x"));
        assert!(merged.contains("UserTicket=u1"));
        assert_eq!(merged.matches("AuthTicket=").count(), 1);
        // base keys keep relative order
        assert!(merged.find("AuthTicket=").unwrap() < merged.find("other=").unwrap());
    }

    #[test]
    fn merge_none_none() {
        assert!(merge_cookie_headers(None, None).is_none());
        assert_eq!(
            merge_cookie_headers(Some("a=1"), None).as_deref(),
            Some("a=1")
        );
    }
}
