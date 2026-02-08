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
