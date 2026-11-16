use serde::{Deserialize, Serialize};

/// Platform-owned identity observed during extraction, separate from playback data and secrets.
///
/// Consumers validate the platform and payload version before use and own persistence and
/// freshness. The payload schema belongs to the named platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformMetadata {
    pub platform: String,
    pub schema_version: u32,
    pub data: serde_json::Value,
}
