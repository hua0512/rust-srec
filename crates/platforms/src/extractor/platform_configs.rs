//! Platform-specific configuration types and utilities.
//!
//! This module provides typed configuration structs for each platform's
//! extractor options, along with utility functions for merging configs
//! through the 4-layer hierarchy (Global → Platform → Template → Streamer).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Huya platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HuyaConfig {
    /// Use WUP protocol for extraction (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_wup: Option<bool>,
    /// Use WUP v2 (computed query params) (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_wup_v2: Option<bool>,
    /// Force origin quality stream (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_origin_quality: Option<bool>,
}

/// Douyin platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
}

/// Bilibili platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BilibiliConfig {
    /// Quality level (0=lowest, 30000=dolby vision) (default: 30000)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<u64>,
}

/// Douyu platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DouyuConfig {
    /// CDN type selection (default: "ws-h5")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdn: Option<String>,
    /// Treat interactive games as offline (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_interactive_game: Option<bool>,
    /// Quality rate, 0 = original quality (default: 0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<i64>,
    /// Force Huoshan CDN URL construction (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_hs: Option<bool>,
    /// API request retry count (default: 3)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_retries: Option<u32>,
}

/// Twitch platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TwitchConfig {
    /// OAuth token for authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_token: Option<String>,
}

/// TikTok platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TikTokConfig {
    /// Force origin quality stream
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_origin_quality: Option<bool>,
}

/// Twitcasting platform-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TwitcastingConfig {
    /// Password for protected streams
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
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
/// let base = Some(json!({"use_wup": true, "force_origin_quality": false}));
/// let overlay = Some(json!({"force_origin_quality": true}));
/// let merged = merge_platform_extras(base, overlay);
/// // Result: {"use_wup": true, "force_origin_quality": true}
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
        let base = json!({"use_wup": true});
        let result = merge_platform_extras(Some(base), None);
        assert_eq!(result, Some(json!({"use_wup": true})));
    }

    #[test]
    fn test_merge_overlay_only() {
        let overlay = json!({"force_origin_quality": true});
        let result = merge_platform_extras(None, Some(overlay));
        assert_eq!(result, Some(json!({"force_origin_quality": true})));
    }

    #[test]
    fn test_merge_overlay_wins() {
        let base = json!({"use_wup": true, "force_origin_quality": false});
        let overlay = json!({"force_origin_quality": true});
        let result = merge_platform_extras(Some(base), Some(overlay));
        assert_eq!(
            result,
            Some(json!({"use_wup": true, "force_origin_quality": true}))
        );
    }

    #[test]
    fn test_merge_null_values_ignored() {
        let base = json!({"use_wup": true, "force_origin_quality": false});
        let overlay = json!({"force_origin_quality": null, "use_wup_v2": true});
        let result = merge_platform_extras(Some(base), Some(overlay));
        assert_eq!(
            result,
            Some(json!({"use_wup": true, "force_origin_quality": false, "use_wup_v2": true}))
        );
    }

    #[test]
    fn test_huya_config_deserialize() {
        let json = json!({"use_wup": false, "force_origin_quality": true});
        let config: HuyaConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.use_wup, Some(false));
        assert_eq!(config.force_origin_quality, Some(true));
        assert_eq!(config.use_wup_v2, None);
    }

    #[test]
    fn test_douyin_config_deserialize() {
        let json = json!({
            "force_origin_quality": true,
            "ttwid_management_mode": "per_extractor"
        });
        let config: DouyinConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.force_origin_quality, Some(true));
        assert_eq!(
            config.ttwid_management_mode,
            Some("per_extractor".to_string())
        );
        assert_eq!(config.double_screen, None);
    }

    #[test]
    fn test_douyu_config_deserialize() {
        let json = json!({"cdn": "hs-h5", "rate": 0, "request_retries": 5});
        let config: DouyuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.cdn, Some("hs-h5".to_string()));
        assert_eq!(config.rate, Some(0));
        assert_eq!(config.request_retries, Some(5));
    }
}
