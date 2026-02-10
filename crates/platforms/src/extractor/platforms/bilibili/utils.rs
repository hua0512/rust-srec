use md5::{Digest, Md5};

/// BiliTV AppKey (云视听小电视) — used for TV QR login and OAuth2 refresh.
pub const TV_APPKEY: &str = "4409e2ce8ffd12b8";
/// BiliTV AppSec — paired with TV_APPKEY.
pub const TV_APPSEC: &str = "59b43e04ad6965f34319062b478f83dd";

/// Android AppKey — used for OAuth2 token validation.
pub const ANDROID_APPKEY: &str = "783bbb7264451d82";
/// Android AppSec — paired with ANDROID_APPKEY.
pub const ANDROID_APPSEC: &str = "2653583c8873dea268ab9386918b1d65";

/// Sign parameters with MD5 for Bilibili APP API requests.
///
/// 1. Sort params alphabetically by key
/// 2. Join as `key=value&...`
/// 3. Append `appsec`
/// 4. MD5 hex digest → `sign`
pub fn sign_params(params: &mut Vec<(&str, String)>, appsec: &str) -> String {
    params.sort_by(|a, b| a.0.cmp(b.0));
    let query: String = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&");
    let to_sign = format!("{}{}", query, appsec);
    let mut hasher = Md5::new();
    hasher.update(to_sign.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Generates a fake BUVID3 identifier for Bilibili API requests.
///
/// BUVID3 is a unique identifier used by Bilibili for tracking and authentication.
/// This function creates a fake one by generating a UUID, removing hyphens,
/// converting to uppercase, and formatting it with the required pattern ending in "infoc".
///
/// # Returns
///
/// A string in the format `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXXinfoc` where X are hexadecimal characters.
pub fn generate_fake_buvid3() -> String {
    let u = uuid::Uuid::new_v4();
    let u_str = u.to_string().to_uppercase().replace('-', "");
    format!(
        "{}-{}-{}-{}-{}infoc",
        &u_str[0..8],
        &u_str[8..12],
        &u_str[12..16],
        &u_str[16..20],
        &u_str[20..]
    )
}
