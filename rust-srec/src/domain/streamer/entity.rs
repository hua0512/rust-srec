//! Streamer entity.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::{Priority, RetryPolicy, DanmuSamplingConfig, StreamerUrl};
use crate::Error;
use super::StreamerState;

/// Error threshold before applying exponential backoff.
const ERROR_THRESHOLD: i32 = 3;

/// Base backoff duration in seconds.
const BASE_BACKOFF_SECS: i64 = 60;

/// Maximum backoff duration in seconds (1 hour).
const MAX_BACKOFF_SECS: i64 = 3600;

/// Streamer entity representing a content creator to be monitored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Streamer {
    pub id: String,
    pub name: String,
    pub url: StreamerUrl,
    pub platform_config_id: String,
    pub template_config_id: Option<String>,
    pub state: StreamerState,
    pub priority: Priority,
    pub last_live_time: Option<DateTime<Utc>>,
    pub streamer_specific_config: Option<serde_json::Value>,
    pub download_retry_policy: Option<RetryPolicy>,
    pub danmu_sampling_config: Option<DanmuSamplingConfig>,
    pub consecutive_error_count: i32,
    pub disabled_until: Option<DateTime<Utc>>,
}

impl Streamer {
    /// Create a new streamer.
    pub fn new(
        name: impl Into<String>,
        url: StreamerUrl,
        platform_config_id: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            url,
            platform_config_id: platform_config_id.into(),
            template_config_id: None,
            state: StreamerState::NotLive,
            priority: Priority::Normal,
            last_live_time: None,
            streamer_specific_config: None,
            download_retry_policy: None,
            danmu_sampling_config: None,
            consecutive_error_count: 0,
            disabled_until: None,
        }
    }

    /// Set the template config.
    pub fn with_template(mut self, template_id: impl Into<String>) -> Self {
        self.template_config_id = Some(template_id.into());
        self
    }

    /// Set the priority.
    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Transition to LIVE state.
    pub fn go_live(&mut self) -> Result<(), Error> {
        self.state = self.state.transition_to(StreamerState::Live)?;
        self.last_live_time = Some(Utc::now());
        self.reset_errors();
        Ok(())
    }

    /// Transition to NOT_LIVE state.
    pub fn go_offline(&mut self) -> Result<(), Error> {
        self.state = self.state.transition_to(StreamerState::NotLive)?;
        Ok(())
    }

    /// Transition to INSPECTING_LIVE state.
    pub fn start_inspection(&mut self) -> Result<(), Error> {
        self.state = self.state.transition_to(StreamerState::InspectingLive)?;
        Ok(())
    }

    /// Set OUT_OF_SCHEDULE state.
    pub fn set_out_of_schedule(&mut self) -> Result<(), Error> {
        self.state = self.state.transition_to(StreamerState::OutOfSchedule)?;
        Ok(())
    }

    /// Set OUT_OF_SPACE state.
    pub fn set_out_of_space(&mut self) -> Result<(), Error> {
        self.state = self.state.transition_to(StreamerState::OutOfSpace)?;
        Ok(())
    }

    /// Enter error state and potentially apply backoff.
    pub fn enter_error_state(&mut self, error: &str) -> Result<(), Error> {
        self.consecutive_error_count += 1;
        
        tracing::warn!(
            streamer_id = %self.id,
            streamer_name = %self.name,
            error_count = self.consecutive_error_count,
            error = %error,
            "Streamer encountered error"
        );

        if self.consecutive_error_count >= ERROR_THRESHOLD {
            // Apply exponential backoff
            let backoff_secs = self.calculate_backoff_duration();
            self.disabled_until = Some(Utc::now() + Duration::seconds(backoff_secs));
            self.state = self.state.transition_to(StreamerState::TemporalDisabled)?;
            
            tracing::info!(
                streamer_id = %self.id,
                disabled_until = ?self.disabled_until,
                "Streamer temporarily disabled due to repeated errors"
            );
        } else {
            self.state = self.state.transition_to(StreamerState::FatalError)?;
        }

        Ok(())
    }

    /// Clear error state and reset error count.
    pub fn clear_error_state(&mut self) -> Result<(), Error> {
        self.state = self.state.transition_to(StreamerState::NotLive)?;
        self.reset_errors();
        Ok(())
    }

    /// Cancel monitoring for this streamer.
    pub fn cancel(&mut self) -> Result<(), Error> {
        self.state = self.state.transition_to(StreamerState::Cancelled)?;
        Ok(())
    }

    /// Mark as not found.
    pub fn mark_not_found(&mut self) -> Result<(), Error> {
        self.state = self.state.transition_to(StreamerState::NotFound)?;
        Ok(())
    }

    /// Reset error count and disabled_until.
    pub fn reset_errors(&mut self) {
        self.consecutive_error_count = 0;
        self.disabled_until = None;
    }

    /// Check if the streamer is currently disabled.
    pub fn is_disabled(&self) -> bool {
        if let Some(until) = self.disabled_until {
            Utc::now() < until
        } else {
            false
        }
    }

    /// Check if the streamer should be monitored.
    pub fn should_monitor(&self) -> bool {
        self.state.is_active() && !self.is_disabled()
    }

    /// Calculate backoff duration based on error count.
    fn calculate_backoff_duration(&self) -> i64 {
        let exponent = (self.consecutive_error_count - ERROR_THRESHOLD).max(0) as u32;
        let backoff = BASE_BACKOFF_SECS * 2_i64.pow(exponent);
        backoff.min(MAX_BACKOFF_SECS)
    }

    /// Update priority.
    pub fn set_priority(&mut self, priority: Priority) {
        self.priority = priority;
    }

    /// Get the platform name from the URL.
    pub fn platform(&self) -> Option<&'static str> {
        self.url.platform()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_streamer() -> Streamer {
        Streamer::new(
            "test_streamer",
            StreamerUrl::from_trusted("https://www.twitch.tv/test"),
            "platform-1",
        )
    }

    #[test]
    fn test_new_streamer() {
        let streamer = create_test_streamer();
        assert_eq!(streamer.name, "test_streamer");
        assert_eq!(streamer.state, StreamerState::NotLive);
        assert_eq!(streamer.priority, Priority::Normal);
        assert_eq!(streamer.consecutive_error_count, 0);
    }

    #[test]
    fn test_go_live() {
        let mut streamer = create_test_streamer();
        streamer.go_live().unwrap();
        assert_eq!(streamer.state, StreamerState::Live);
        assert!(streamer.last_live_time.is_some());
    }

    #[test]
    fn test_go_offline() {
        let mut streamer = create_test_streamer();
        streamer.go_live().unwrap();
        streamer.go_offline().unwrap();
        assert_eq!(streamer.state, StreamerState::NotLive);
    }

    #[test]
    fn test_error_handling() {
        let mut streamer = create_test_streamer();
        
        // First few errors don't disable
        for _ in 0..2 {
            streamer.enter_error_state("test error").unwrap();
        }
        assert_eq!(streamer.state, StreamerState::FatalError);
        assert!(streamer.disabled_until.is_none());
        
        // Third error triggers backoff
        streamer.enter_error_state("test error").unwrap();
        assert_eq!(streamer.state, StreamerState::TemporalDisabled);
        assert!(streamer.disabled_until.is_some());
    }

    #[test]
    fn test_clear_error_state() {
        let mut streamer = create_test_streamer();
        for _ in 0..3 {
            streamer.enter_error_state("test error").unwrap();
        }
        
        streamer.clear_error_state().unwrap();
        assert_eq!(streamer.state, StreamerState::NotLive);
        assert_eq!(streamer.consecutive_error_count, 0);
        assert!(streamer.disabled_until.is_none());
    }

    #[test]
    fn test_backoff_calculation() {
        let mut streamer = create_test_streamer();
        streamer.consecutive_error_count = 3;
        assert_eq!(streamer.calculate_backoff_duration(), 60); // BASE_BACKOFF_SECS
        
        streamer.consecutive_error_count = 4;
        assert_eq!(streamer.calculate_backoff_duration(), 120); // 60 * 2
        
        streamer.consecutive_error_count = 5;
        assert_eq!(streamer.calculate_backoff_duration(), 240); // 60 * 4
    }

    #[test]
    fn test_priority() {
        let mut streamer = create_test_streamer();
        assert_eq!(streamer.priority, Priority::Normal);
        
        streamer.set_priority(Priority::High);
        assert_eq!(streamer.priority, Priority::High);
    }

    #[test]
    fn test_should_monitor() {
        let mut streamer = create_test_streamer();
        assert!(streamer.should_monitor());
        
        streamer.cancel().unwrap();
        assert!(!streamer.should_monitor());
    }
}
