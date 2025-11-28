//! Event hooks value object.

use serde::{Deserialize, Serialize};

/// Event hooks for streamer lifecycle events.
/// 
/// Allows executing custom commands when certain events occur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EventHooks {
    /// Command to execute when streamer goes online.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_online: Option<String>,
    /// Command to execute when streamer goes offline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_offline: Option<String>,
    /// Command to execute when download starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_download_start: Option<String>,
    /// Command to execute when download completes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_download_complete: Option<String>,
    /// Command to execute when download fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_download_error: Option<String>,
    /// Command to execute when pipeline completes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_pipeline_complete: Option<String>,
}

impl EventHooks {
    /// Create empty event hooks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the on_online hook.
    pub fn with_on_online(mut self, command: impl Into<String>) -> Self {
        self.on_online = Some(command.into());
        self
    }

    /// Set the on_offline hook.
    pub fn with_on_offline(mut self, command: impl Into<String>) -> Self {
        self.on_offline = Some(command.into());
        self
    }

    /// Set the on_download_start hook.
    pub fn with_on_download_start(mut self, command: impl Into<String>) -> Self {
        self.on_download_start = Some(command.into());
        self
    }

    /// Set the on_download_complete hook.
    pub fn with_on_download_complete(mut self, command: impl Into<String>) -> Self {
        self.on_download_complete = Some(command.into());
        self
    }

    /// Set the on_download_error hook.
    pub fn with_on_download_error(mut self, command: impl Into<String>) -> Self {
        self.on_download_error = Some(command.into());
        self
    }

    /// Set the on_pipeline_complete hook.
    pub fn with_on_pipeline_complete(mut self, command: impl Into<String>) -> Self {
        self.on_pipeline_complete = Some(command.into());
        self
    }

    /// Check if any hooks are defined.
    pub fn has_any(&self) -> bool {
        self.on_online.is_some()
            || self.on_offline.is_some()
            || self.on_download_start.is_some()
            || self.on_download_complete.is_some()
            || self.on_download_error.is_some()
            || self.on_pipeline_complete.is_some()
    }

    /// Merge with another EventHooks, with other taking precedence.
    pub fn merge(&self, other: &EventHooks) -> EventHooks {
        EventHooks {
            on_online: other.on_online.clone().or_else(|| self.on_online.clone()),
            on_offline: other.on_offline.clone().or_else(|| self.on_offline.clone()),
            on_download_start: other.on_download_start.clone().or_else(|| self.on_download_start.clone()),
            on_download_complete: other.on_download_complete.clone().or_else(|| self.on_download_complete.clone()),
            on_download_error: other.on_download_error.clone().or_else(|| self.on_download_error.clone()),
            on_pipeline_complete: other.on_pipeline_complete.clone().or_else(|| self.on_pipeline_complete.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_hooks_new() {
        let hooks = EventHooks::new();
        assert!(!hooks.has_any());
    }

    #[test]
    fn test_event_hooks_builder() {
        let hooks = EventHooks::new()
            .with_on_online("echo online")
            .with_on_offline("echo offline");
        
        assert!(hooks.has_any());
        assert_eq!(hooks.on_online, Some("echo online".to_string()));
        assert_eq!(hooks.on_offline, Some("echo offline".to_string()));
    }

    #[test]
    fn test_event_hooks_merge() {
        let base = EventHooks::new()
            .with_on_online("base online")
            .with_on_offline("base offline");
        
        let override_hooks = EventHooks::new()
            .with_on_online("override online");
        
        let merged = base.merge(&override_hooks);
        
        assert_eq!(merged.on_online, Some("override online".to_string()));
        assert_eq!(merged.on_offline, Some("base offline".to_string()));
    }

    #[test]
    fn test_event_hooks_serialization() {
        let hooks = EventHooks::new()
            .with_on_online("echo online");
        
        let json = serde_json::to_string(&hooks).unwrap();
        let parsed: EventHooks = serde_json::from_str(&json).unwrap();
        
        assert_eq!(parsed.on_online, hooks.on_online);
    }
}
