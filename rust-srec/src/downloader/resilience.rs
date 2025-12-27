//! Retry logic and circuit breaker for download resilience.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::engine::EngineType;

/// Key for circuit breaker isolation.
///
/// Combines engine type with optional config ID to support per-instance tracking:
/// - `config_id: None` → global default engine (shared by all users of that type)
/// - `config_id: Some(id)` → custom engine config or override (isolated breaker)
///
/// ## Equality Cases
///
/// | Scenario | EngineKey | Shared? |
/// |----------|-----------|---------|
/// | Global FFMPEG | `{ FFMPEG, None }` | Yes - all global FFMPEG users |
/// | Custom "my-ffmpeg" | `{ FFMPEG, Some("my-ffmpeg") }` | Only same config ID |
/// | Global + override | `{ FFMPEG, Some("ffmpeg#hash") }` | No - ephemeral |
/// | Fallback default | `{ default_type, None }` | Yes - all defaults |
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineKey {
    /// The engine type (FFMPEG, MESIO, STREAMLINK).
    pub engine_type: EngineType,
    /// Custom config ID, or None for the global default instance.
    pub config_id: Option<String>,
}

impl EngineKey {
    /// Create a key for the global default engine of a type.
    pub fn global(engine_type: EngineType) -> Self {
        Self {
            engine_type,
            config_id: None,
        }
    }

    /// Create a key for a custom engine configuration.
    pub fn custom(engine_type: EngineType, config_id: impl Into<String>) -> Self {
        Self {
            engine_type,
            config_id: Some(config_id.into()),
        }
    }

    /// Create a key for an engine with an override applied.
    /// The hash ensures different overrides get different breakers.
    pub fn with_override(
        engine_type: EngineType,
        base_id: Option<&str>,
        override_hash: u64,
    ) -> Self {
        let config_id = match base_id {
            Some(id) => format!("{}#{:x}", id, override_hash),
            None => format!("{}#{:x}", engine_type.as_str(), override_hash),
        };
        Self {
            engine_type,
            config_id: Some(config_id),
        }
    }
}

impl std::fmt::Display for EngineKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.config_id {
            Some(id) => write!(f, "{}:{}", self.engine_type, id),
            None => write!(f, "{}", self.engine_type),
        }
    }
}

/// Configuration for retry behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Initial delay between retries in milliseconds.
    pub initial_delay_ms: u64,
    /// Maximum delay between retries in milliseconds.
    pub max_delay_ms: u64,
    /// Multiplier for exponential backoff.
    pub backoff_multiplier: f64,
    /// Whether to add jitter to delays.
    pub use_jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            use_jitter: true,
        }
    }
}

impl RetryConfig {
    /// Calculate the delay for a given attempt number.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }

        let base_delay = self.initial_delay_ms as f64
            * self
                .backoff_multiplier
                .powi(attempt.saturating_sub(1) as i32);

        let delay_ms = base_delay.min(self.max_delay_ms as f64) as u64;

        let final_delay = if self.use_jitter {
            // Add up to 25% jitter
            let jitter = (delay_ms as f64 * 0.25 * rand::random::<f64>()) as u64;
            delay_ms + jitter
        } else {
            delay_ms
        };

        Duration::from_millis(final_delay)
    }

    /// Check if another retry should be attempted.
    pub fn should_retry(&self, attempt: u32) -> bool {
        attempt < self.max_retries
    }
}

/// State of a circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed (normal operation).
    Closed,
    /// Circuit is open (failing, rejecting requests).
    Open,
    /// Circuit is half-open (testing if service recovered).
    HalfOpen,
}

/// Circuit breaker for protecting against cascading failures.
pub struct CircuitBreaker {
    /// Current state.
    state: RwLock<CircuitState>,
    /// Consecutive failure count.
    failure_count: AtomicU32,
    /// Failure threshold to open circuit.
    failure_threshold: u32,
    /// Time when circuit was opened.
    opened_at: RwLock<Option<Instant>>,
    /// Cooldown duration before trying again.
    cooldown_duration: Duration,
    /// Success count in half-open state.
    half_open_successes: AtomicU32,
    /// Failure count in half-open state.
    half_open_failures: AtomicU32,
    /// Required successes to close circuit.
    success_threshold: u32,
    /// Required failures in half-open state to reopen circuit.
    /// This allows for transient errors without immediately reopening.
    half_open_failure_threshold: u32,
}

impl CircuitBreaker {
    /// Create a new circuit breaker.
    pub fn new(failure_threshold: u32, cooldown_secs: u64) -> Self {
        Self {
            state: RwLock::new(CircuitState::Closed),
            failure_count: AtomicU32::new(0),
            failure_threshold,
            opened_at: RwLock::new(None),
            cooldown_duration: Duration::from_secs(cooldown_secs),
            half_open_successes: AtomicU32::new(0),
            half_open_failures: AtomicU32::new(0),
            success_threshold: 2,
            // Allow 2 failures in half-open state before reopening
            // This prevents transient network errors from immediately blocking recovery
            half_open_failure_threshold: 2,
        }
    }

    /// Get the current state.
    pub fn state(&self) -> CircuitState {
        self.check_state_transition();
        *self.state.read()
    }

    /// Check if the circuit allows requests.
    pub fn is_allowed(&self) -> bool {
        match self.state() {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true, // Allow limited requests
            CircuitState::Open => false,
        }
    }

    /// Record a successful operation.
    pub fn record_success(&self) {
        let state = *self.state.read();

        match state {
            CircuitState::Closed => {
                // Reset failure count on success
                self.failure_count.store(0, Ordering::SeqCst);
            }
            CircuitState::HalfOpen => {
                let successes = self.half_open_successes.fetch_add(1, Ordering::SeqCst) + 1;
                if successes >= self.success_threshold {
                    // Close the circuit
                    *self.state.write() = CircuitState::Closed;
                    self.failure_count.store(0, Ordering::SeqCst);
                    self.half_open_successes.store(0, Ordering::SeqCst);
                    info!("Circuit breaker closed after successful recovery");
                }
            }
            CircuitState::Open => {
                // Shouldn't happen, but reset if it does
                self.check_state_transition();
            }
        }
    }

    /// Record a failed operation.
    pub fn record_failure(&self) {
        let state = *self.state.read();

        match state {
            CircuitState::Closed => {
                let failures = self.failure_count.fetch_add(1, Ordering::SeqCst) + 1;
                if failures >= self.failure_threshold {
                    // Open the circuit
                    *self.state.write() = CircuitState::Open;
                    *self.opened_at.write() = Some(Instant::now());
                    warn!(
                        "Circuit breaker opened after {} consecutive failures",
                        failures
                    );
                }
            }
            CircuitState::HalfOpen => {
                // Count failures in half-open state and only reopen after threshold
                let failures = self.half_open_failures.fetch_add(1, Ordering::SeqCst) + 1;
                if failures >= self.half_open_failure_threshold {
                    // Exceeded half-open failure threshold, reopen the circuit
                    *self.state.write() = CircuitState::Open;
                    *self.opened_at.write() = Some(Instant::now());
                    self.half_open_successes.store(0, Ordering::SeqCst);
                    self.half_open_failures.store(0, Ordering::SeqCst);
                    warn!(
                        "Circuit breaker reopened after {} failures in half-open state",
                        failures
                    );
                } else {
                    debug!(
                        "Circuit breaker half-open failure {}/{}, staying half-open",
                        failures, self.half_open_failure_threshold
                    );
                }
            }
            CircuitState::Open => {
                // Already open, nothing to do
            }
        }
    }

    /// Reset the circuit breaker to closed state.
    pub fn reset(&self) {
        *self.state.write() = CircuitState::Closed;
        self.failure_count.store(0, Ordering::SeqCst);
        self.half_open_successes.store(0, Ordering::SeqCst);
        self.half_open_failures.store(0, Ordering::SeqCst);
        *self.opened_at.write() = None;
        debug!("Circuit breaker reset to closed state");
    }

    /// Check if state should transition (open -> half-open after cooldown).
    fn check_state_transition(&self) {
        let state = *self.state.read();

        if state == CircuitState::Open
            && let Some(opened_at) = *self.opened_at.read()
            && opened_at.elapsed() >= self.cooldown_duration
        {
            *self.state.write() = CircuitState::HalfOpen;
            self.half_open_successes.store(0, Ordering::SeqCst);
            self.half_open_failures.store(0, Ordering::SeqCst);
            debug!("Circuit breaker transitioned to half-open state");
        }
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(5, 60) // 5 failures, 60 second cooldown
    }
}

/// Manager for circuit breakers per engine key.
///
/// Keys circuit breakers by `EngineKey` (type + optional config ID) to support
/// per-instance isolation. Different custom configs get their own breakers.
pub struct CircuitBreakerManager {
    breakers: RwLock<HashMap<EngineKey, Arc<CircuitBreaker>>>,
    failure_threshold: u32,
    cooldown_secs: u64,
}

impl CircuitBreakerManager {
    /// Create a new circuit breaker manager.
    pub fn new(failure_threshold: u32, cooldown_secs: u64) -> Self {
        Self {
            breakers: RwLock::new(HashMap::new()),
            failure_threshold,
            cooldown_secs,
        }
    }

    /// Get or create a circuit breaker for an engine key.
    pub fn get(&self, key: &EngineKey) -> Arc<CircuitBreaker> {
        {
            let breakers = self.breakers.read();
            if let Some(breaker) = breakers.get(key) {
                return breaker.clone();
            }
        }

        let mut breakers = self.breakers.write();
        breakers
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(CircuitBreaker::new(
                    self.failure_threshold,
                    self.cooldown_secs,
                ))
            })
            .clone()
    }

    /// Check if an engine is allowed (circuit not open).
    pub fn is_allowed(&self, key: &EngineKey) -> bool {
        self.get(key).is_allowed()
    }

    /// Record success for an engine.
    #[allow(dead_code)]
    pub fn record_success(&self, key: &EngineKey) {
        self.get(key).record_success();
    }

    /// Record failure for an engine.
    #[allow(dead_code)]
    pub fn record_failure(&self, key: &EngineKey) {
        self.get(key).record_failure();
    }
}

impl Default for CircuitBreakerManager {
    fn default() -> Self {
        Self::new(5, 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert!(config.should_retry(0));
        assert!(config.should_retry(2));
        assert!(!config.should_retry(3));
    }

    #[test]
    fn test_retry_delay_calculation() {
        let config = RetryConfig {
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 10000,
            backoff_multiplier: 2.0,
            use_jitter: false,
        };

        assert_eq!(config.delay_for_attempt(0), Duration::ZERO);
        assert_eq!(config.delay_for_attempt(1), Duration::from_millis(1000));
        assert_eq!(config.delay_for_attempt(2), Duration::from_millis(2000));
        assert_eq!(config.delay_for_attempt(3), Duration::from_millis(4000));
        assert_eq!(config.delay_for_attempt(10), Duration::from_millis(10000)); // Capped at max
    }

    #[test]
    fn test_circuit_breaker_closed() {
        let breaker = CircuitBreaker::new(3, 60);
        assert_eq!(breaker.state(), CircuitState::Closed);
        assert!(breaker.is_allowed());
    }

    #[test]
    fn test_circuit_breaker_opens_on_failures() {
        let breaker = CircuitBreaker::new(3, 60);

        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Closed);

        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Closed);

        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Open);
        assert!(!breaker.is_allowed());
    }

    #[test]
    fn test_circuit_breaker_success_resets_failures() {
        let breaker = CircuitBreaker::new(3, 60);

        breaker.record_failure();
        breaker.record_failure();
        breaker.record_success();

        // Failure count should be reset
        breaker.record_failure();
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let breaker = CircuitBreaker::new(3, 60);

        breaker.record_failure();
        breaker.record_failure();
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Open);

        breaker.reset();
        assert_eq!(breaker.state(), CircuitState::Closed);
        assert!(breaker.is_allowed());
    }

    #[test]
    fn test_circuit_breaker_manager_global() {
        let manager = CircuitBreakerManager::new(3, 60);
        let key_ffmpeg = EngineKey::global(EngineType::Ffmpeg);
        let key_streamlink = EngineKey::global(EngineType::Streamlink);

        assert!(manager.is_allowed(&key_ffmpeg));

        manager.record_failure(&key_ffmpeg);
        manager.record_failure(&key_ffmpeg);
        manager.record_failure(&key_ffmpeg);

        assert!(!manager.is_allowed(&key_ffmpeg));
        assert!(manager.is_allowed(&key_streamlink)); // Different engine type
    }

    #[test]
    fn test_circuit_breaker_manager_custom_isolation() {
        let manager = CircuitBreakerManager::new(3, 60);

        // Global FFMPEG key
        let key_global = EngineKey::global(EngineType::Ffmpeg);
        // Custom config key
        let key_custom = EngineKey::custom(EngineType::Ffmpeg, "my-custom-ffmpeg");
        // Another custom config key
        let key_custom2 = EngineKey::custom(EngineType::Ffmpeg, "alt-ffmpeg");

        // Trip the custom config breaker
        manager.record_failure(&key_custom);
        manager.record_failure(&key_custom);
        manager.record_failure(&key_custom);

        // Custom config is blocked
        assert!(!manager.is_allowed(&key_custom));
        // Global FFMPEG is NOT blocked - isolated!
        assert!(manager.is_allowed(&key_global));
        // Other custom config is NOT blocked
        assert!(manager.is_allowed(&key_custom2));
    }
}
