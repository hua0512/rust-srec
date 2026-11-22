//! Platform-specific configuration types and utilities.
//!
//! This module provides typed configuration structs for each platform's
//! extractor options, along with utility functions for merging configs
//! through the 4-layer hierarchy (Global → Platform → Template → Streamer).

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn redact_opt(opt: &Option<String>) -> Option<&'static str> {
    opt.as_ref().map(|_| "[redacted]")
}

/// Huya platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HuyaConfig {
    /// Extraction API mode (WEB, MP, WUP) (default: "WEB")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_mode: Option<String>,
    /// Force origin quality stream (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_origin_quality: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_stream_on_danmu_stream_closed: Option<bool>,
}

/// Douyin platform-specific configuration
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct DouyinConfig {
    /// Force origin quality stream (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_origin_quality: Option<bool>,
    /// Use double screen stream data (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_screen: Option<bool>,
    /// ttwid management mode: "global" or "per_extractor" (default: "global")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttwid_management_mode: Option<String>,
    /// Specific ttwid cookie value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttwid: Option<String>,
    /// Force use mobile API for stream extraction (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_mobile_api: Option<bool>,
    /// Skip interactive game streams (互动玩法), treat as offline (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_interactive_games: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_stream_on_danmu_stream_closed: Option<bool>,
}

impl fmt::Debug for DouyinConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DouyinConfig")
            .field("force_origin_quality", &self.force_origin_quality)
            .field("double_screen", &self.double_screen)
            .field("ttwid_management_mode", &self.ttwid_management_mode)
            .field("ttwid", &redact_opt(&self.ttwid))
            .field("force_mobile_api", &self.force_mobile_api)
            .field("skip_interactive_games", &self.skip_interactive_games)
            .field(
                "end_stream_on_danmu_stream_closed",
                &self.end_stream_on_danmu_stream_closed,
            )
            .finish()
    }
}

/// Bilibili platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BilibiliConfig {
    /// Quality level (0=lowest, 30000=dolby vision) (default: 30000)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_stream_on_danmu_stream_closed: Option<bool>,
}

/// Douyu playback API. The web method is retained for compatibility but deprecated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DouyuApiMode {
    #[default]
    App,
    Web,
}

/// Preferred video codec; HEVC falls back to AVC when unavailable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DouyuCodec {
    #[default]
    Avc,
    Hevc,
}

/// How an anonymous Android device ID is obtained when no explicit ID/cookie is supplied.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DouyuDeviceIdMode {
    #[default]
    Local,
    Server,
    Default,
}

/// Douyu platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DouyuConfig {
    /// Playback method (default: app; web is deprecated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_mode: Option<DouyuApiMode>,
    /// Preferred video codec (default: avc).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<DouyuCodec>,
    /// Android model; unset generates an ABC-DE12-style model per extractor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    /// Android version used in the generated User-Agent (default: 14).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    /// Explicit Android DID, taking precedence over acf_did and device_id_mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id_mode: Option<DouyuDeviceIdMode>,
    /// CDN type selection (default: "hw" for app, "ws-h5" for web).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdn: Option<String>,
    /// Treat interactive games as offline (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_interactive_game: Option<bool>,
    /// Audio-only extraction uses the deprecated web API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_audio: Option<bool>,
    /// Quality rate, 0 = original quality (default: 0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<i64>,
    /// API request retry count (default: 3)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_retries: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_stream_on_danmu_stream_closed: Option<bool>,
}

/// Twitch platform-specific configuration
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct TwitchConfig {
    /// OAuth token for authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_token: Option<String>,
    /// Codecs announced to Twitch (`h264`, `h265`, `av1`); unset means H.264 only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_codecs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_stream_on_danmu_stream_closed: Option<bool>,
}

impl fmt::Debug for TwitchConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TwitchConfig")
            .field("oauth_token", &redact_opt(&self.oauth_token))
            .field("supported_codecs", &self.supported_codecs)
            .field(
                "end_stream_on_danmu_stream_closed",
                &self.end_stream_on_danmu_stream_closed,
            )
            .finish()
    }
}

/// TikTok platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TikTokConfig {
    /// Room lookup mode: "auto", "web" (api-live JSON), or "html" (page scrape) (default: "auto")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_mode: Option<String>,
    /// Force origin quality stream
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_origin_quality: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_stream_on_danmu_stream_closed: Option<bool>,
}

/// Twitcasting platform-specific configuration
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct TwitcastingConfig {
    /// Password for protected streams
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_stream_on_danmu_stream_closed: Option<bool>,
}

impl fmt::Debug for TwitcastingConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TwitcastingConfig")
            .field("password", &redact_opt(&self.password))
            .field(
                "end_stream_on_danmu_stream_closed",
                &self.end_stream_on_danmu_stream_closed,
            )
            .finish()
    }
}

/// SOOP (formerly AfreecaTV) platform-specific configuration
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct SoopConfig {
    /// SOOP account username (login-required broadcasts, e.g. 19+)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// SOOP account password
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Password for password-protected rooms
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_password: Option<String>,
}

impl fmt::Debug for SoopConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoopConfig")
            .field("username", &self.username)
            .field("password", &redact_opt(&self.password))
            .field("stream_password", &redact_opt(&self.stream_password))
            .finish()
    }
}

/// Bigo Live platform-specific configuration
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct BigoConfig {
    /// Password for password-protected rooms
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_password: Option<String>,
    /// Record the broadcaster's original stream from BIGO's app servers,
    /// falling back to the web HLS stream (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_stream: Option<bool>,
}

impl fmt::Debug for BigoConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BigoConfig")
            .field("stream_password", &redact_opt(&self.stream_password))
            .field("native_stream", &self.native_stream)
            .finish()
    }
}

/// Streamlink *extractor* configuration.
///
/// Distinct from `StreamlinkEngineConfig`, which configures the streamlink download engine.
/// This one configures stream-URL resolution and lives under a nested `streamlink` key in the
/// extras blob, matching what `StreamlinkExtractor::from_extras` reads. It applies whenever the
/// streamlink extractor runs: as the fallback for a URL no built-in platform claims, or because
/// `ExtractorSelection::Streamlink` forced it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamlinkExtractorConfig {
    /// Path to the `streamlink` binary. Falls back to `$STREAMLINK_PATH`, then `streamlink`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<String>,
    /// Quality passed to streamlink (default: "best")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// Extra CLI arguments appended to every streamlink invocation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_args: Option<Vec<String>>,
}

/// Merge two JSON objects, with overlay taking precedence.
///
/// This function performs a shallow merge of JSON objects. For nested objects,
/// the overlay completely replaces the base value (no deep merge).
///
/// # Arguments
/// * `base` - The base configuration (from lower priority layer), consumed
/// * `overlay` - The overlay configuration (from higher priority layer), consumed
///
/// # Returns
/// The merged configuration, or None if both inputs are None
///
/// # Example
/// ```
/// use serde_json::json;
/// use platforms_parser::extractor::platform_configs::merge_platform_extras;
///
/// let base = Some(json!({"api_mode": "WUP", "force_origin_quality": false}));
/// let overlay = Some(json!({"force_origin_quality": true}));
/// let merged = merge_platform_extras(base, overlay);
/// // Result: {"api_mode": "WUP", "force_origin_quality": true}
/// ```
pub fn merge_platform_extras(base: Option<Value>, overlay: Option<Value>) -> Option<Value> {
    match (base, overlay) {
        (None, None) => None,
        (Some(b), None) => Some(b),
        (None, Some(o)) => Some(o),
        (Some(Value::Object(mut base_map)), Some(Value::Object(overlay_map))) => {
            for (k, v) in overlay_map {
                // Skip null values - they don't override
                if !v.is_null() {
                    base_map.insert(k, v);
                }
            }
            Some(Value::Object(base_map))
        }
        // If either is not an object, overlay wins
        (_, Some(o)) => Some(o),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_merge_both_none() {
        let result = merge_platform_extras(None, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_merge_base_only() {
        let base = json!({"api_mode": "WUP"});
        let result = merge_platform_extras(Some(base), None);
        assert_eq!(result, Some(json!({"api_mode": "WUP"})));
    }

    #[test]
    fn test_merge_overlay_only() {
        let overlay = json!({"force_origin_quality": true});
        let result = merge_platform_extras(None, Some(overlay));
        assert_eq!(result, Some(json!({"force_origin_quality": true})));
    }

    #[test]
    fn test_merge_overlay_wins() {
        let base = json!({"api_mode": "WUP", "force_origin_quality": false});
        let overlay = json!({"force_origin_quality": true});
        let result = merge_platform_extras(Some(base), Some(overlay));
        assert_eq!(
            result,
            Some(json!({"api_mode": "WUP", "force_origin_quality": true}))
        );
    }

    #[test]
    fn test_merge_null_values_ignored() {
        let base = json!({"api_mode": "WUP", "force_origin_quality": false});
        let overlay = json!({"force_origin_quality": null, "api_mode": "MP"});
        let result = merge_platform_extras(Some(base), Some(overlay));
        assert_eq!(
            result,
            Some(json!({"api_mode": "MP", "force_origin_quality": false}))
        );
    }

    #[test]
    fn secret_bearing_structs_redact_credentials_in_debug_output() {
        use crate::extractor::{
            default::default_client,
            platforms::{bigo::account::AccountSession, bilibili::token_refresh::RefreshedTokens},
            streamlink_extractor::StreamlinkExtractor,
        };

        let douyin = DouyinConfig {
            ttwid: Some("secret_ttwid".to_string()),
            ..Default::default()
        };
        let twitch = TwitchConfig {
            oauth_token: Some("secret_oauth".to_string()),
            ..Default::default()
        };
        let twitcasting = TwitcastingConfig {
            password: Some("secret_tc_pw".to_string()),
            ..Default::default()
        };
        let soop = SoopConfig {
            username: Some("soop_user".to_string()),
            password: Some("secret_soop_pw".to_string()),
            stream_password: Some("secret_soop_room_pw".to_string()),
        };
        let bigo = BigoConfig {
            stream_password: Some("secret_bigo_pw".to_string()),
            native_stream: Some(true),
        };
        let streamlink = StreamlinkExtractor::new(
            "https://example.com/live".to_string(),
            default_client(),
            Some("SESS=secret_sl_cookie".to_string()),
            None,
        );
        let refreshed = RefreshedTokens {
            cookies: "SESSDATA=secret_bili_cookie".to_string(),
            access_token: "secret_bili_access".to_string(),
            refresh_token: "secret_bili_refresh".to_string(),
            mid: 12345,
            expires_in: 3600,
        };
        let bigo_sess = AccountSession::SignedIn {
            cookies: "COOK=secret_bigo_sess".to_string(),
        };

        let combined = format!(
            "{douyin:?}\n{twitch:?}\n{twitcasting:?}\n{soop:?}\n{bigo:?}\n{streamlink:?}\n{refreshed:?}\n{bigo_sess:?}"
        );
        for secret in [
            "secret_ttwid",
            "secret_oauth",
            "secret_tc_pw",
            "secret_soop_pw",
            "secret_soop_room_pw",
            "secret_bigo_pw",
            "secret_sl_cookie",
            "secret_bili_cookie",
            "secret_bili_access",
            "secret_bili_refresh",
            "secret_bigo_sess",
        ] {
            assert!(
                !combined.contains(secret),
                "leaked {secret} in:\n{combined}"
            );
        }
        assert!(combined.contains("soop_user"));
        assert!(combined.contains("12345"));
    }
}
