//! Typed events emitted by danmu providers.
//!
//! Danmu providers yield either regular chat messages (DanmuMessage) or control events
//! (DanmuControlEvent). Control events should not be treated as chat messages and are
//! not suitable for writing into danmu XML by default.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Control events produced by the danmu stream that affect session semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DanmuControlEvent {
    /// The live stream ended / was closed by the platform.
    StreamClosed {
        /// Optional human-readable reason/tips provided by the platform.
        message: Option<String>,
        /// Optional platform-specific action code (e.g., Douyin ControlMessage.action).
        action: Option<u64>,
    },
    /// Room info changed (e.g., title/category).
    ///
    /// Bilibili uses `ROOM_CHANGE` for this.
    RoomInfoChanged {
        title: Option<String>,
        category: Option<String>,
        parent_category: Option<String>,
    },
    /// Other platform-specific control event.
    Other {
        kind: String,
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<HashMap<String, serde_json::Value>>,
    },
}

/// A single item in the danmu stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DanmuItem {
    Message(super::message::DanmuMessage),
    Control(DanmuControlEvent),
}
