//! Shared monitor configuration, infrastructure backoff reasons, and option helpers.

use std::time::Duration;

use crate::domain::StreamerState;

/// Default rate-limit and HTTP pooling configuration shared by platform admission
/// and [`crate::recorder::PlatformExtractorRunner`].
#[derive(Debug, Clone)]
pub struct StreamMonitorConfig {
    /// Default rate limit (requests per second).
    pub default_rate_limit: f64,
    /// Platform-specific rate limits.
    pub platform_rate_limits: Vec<(String, f64)>,
    /// HTTP client timeout.
    pub request_timeout: Duration,
    /// Maximum concurrent requests / idle connections per host.
    pub max_concurrent_requests: usize,
}

impl Default for StreamMonitorConfig {
    fn default() -> Self {
        Self {
            default_rate_limit: 1.0,
            platform_rate_limits: vec![("twitch".to_string(), 2.0)],
            request_timeout: Duration::ZERO,
            max_concurrent_requests: 10,
        }
    }
}

/// Reason a streamer is placed into infrastructure-level backoff.
///
/// The reason determines the target [`StreamerState`], the retry window, and
/// whether `last_error` is rewritten.
#[derive(Debug, Clone)]
pub enum InfraBlockReason {
    /// Engine circuit breaker is open for the engine this streamer is trying
    /// to use. Preserves whatever `last_error` is already on the streamer.
    CircuitBreaker { retry_after_secs: u64 },
    /// Output-root write gate has the target filesystem in a Degraded state.
    /// Transitions the streamer to [`StreamerState::OutOfSpace`] and writes a
    /// distinctive `last_error` with the `"output-root blocked: "` prefix so
    /// the gate's recovery hook can filter affected streamers when the
    /// filesystem comes back.
    OutputRootUnavailable {
        path: std::path::PathBuf,
        io_kind: crate::downloader::IoErrorKindSer,
        retry_after_secs: u64,
    },
}

impl InfraBlockReason {
    pub(crate) fn retry_after_secs(&self) -> u64 {
        match self {
            Self::CircuitBreaker { retry_after_secs }
            | Self::OutputRootUnavailable {
                retry_after_secs, ..
            } => *retry_after_secs,
        }
    }

    pub(crate) fn target_state(&self) -> StreamerState {
        match self {
            Self::CircuitBreaker { .. } => StreamerState::TemporalDisabled,
            Self::OutputRootUnavailable { .. } => StreamerState::OutOfSpace,
        }
    }

    pub(crate) fn last_error_override(&self) -> Option<String> {
        match self {
            Self::CircuitBreaker { .. } => None,
            Self::OutputRootUnavailable { path, io_kind, .. } => {
                // This string format is load-bearing: the gate recovery hook
                // filters streamers by matching the `LAST_ERROR_GATE_PREFIX`
                // prefix.
                Some(format!(
                    "{} {} ({})",
                    crate::downloader::LAST_ERROR_GATE_PREFIX,
                    path.display(),
                    io_kind.as_str()
                ))
            }
        }
    }
}

/// The `batch_detection` platform option, off unless a configuration layer sets it to `true`.
pub(crate) fn batch_detection_enabled(platform_extras: Option<&serde_json::Value>) -> bool {
    platform_extras
        .and_then(|extras| extras.get("batch_detection"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Process-lifetime identifier stamped onto outbox rows written by
/// [`crate::database::repositories::SessionLifecycleRepository`].
pub(crate) fn runtime_instance_id() -> &'static str {
    static INSTANCE_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    INSTANCE_ID
        .get_or_init(|| uuid::Uuid::new_v4().to_string())
        .as_str()
}
