//! Per-engine circuit breakers for download resilience.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use tracing::{debug, info, warn};

use super::engine::EngineType;

/// Key for circuit breaker isolation.
///
/// Combines engine type, optional config ID, and optional streamer ID to support
/// per-instance and per-streamer tracking:
/// - `config_id: None` → global default engine
/// - `config_id: Some(id)` → custom engine config or override (isolated breaker)
/// - `streamer_id: None` → shared across all streamers (legacy/global)
/// - `streamer_id: Some(id)` → isolated per-streamer, so one streamer's CDN issues
///   don't trip the breaker for unrelated streamers on the same engine
///
/// ## Equality Cases
///
/// | Scenario | EngineKey | Shared? |
/// |----------|-----------|---------|
/// | Global FFMPEG | `{ FFMPEG, None, None }` | Yes - all global FFMPEG users |
/// | Custom "my-ffmpeg" | `{ FFMPEG, Some("my-ffmpeg"), None }` | Only same config ID |
/// | Per-streamer | `{ MESIO, Some("default"), Some("abc") }` | Only that streamer |
/// | Global + override | `{ FFMPEG, Some("ffmpeg#hash"), None }` | No - ephemeral |
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineKey {
    /// The engine type (FFMPEG, MESIO, STREAMLINK).
    pub engine_type: EngineType,
    /// Custom config ID, or None for the global default instance.
    pub config_id: Option<String>,
    /// Streamer ID for per-streamer isolation, or None for shared/global breakers.
    pub streamer_id: Option<String>,
}

impl EngineKey {
    /// Create a key for the global default engine of a type.
    pub fn global(engine_type: EngineType) -> Self {
        Self {
            engine_type,
            config_id: None,
            streamer_id: None,
        }
    }

    /// Create a key for a custom engine configuration.
    pub fn custom(engine_type: EngineType, config_id: impl Into<String>) -> Self {
        Self {
            engine_type,
            config_id: Some(config_id.into()),
            streamer_id: None,
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
            streamer_id: None,
        }
    }

    /// Scope this key to a specific streamer for per-streamer circuit breaker isolation.
    pub fn for_streamer(mut self, streamer_id: impl Into<String>) -> Self {
        self.streamer_id = Some(streamer_id.into());
        self
    }

    pub fn is_override(&self) -> bool {
        self.config_id.as_ref().is_some_and(|id| id.contains('#'))
    }

    /// Whether this key is scoped to a specific streamer.
    pub fn is_per_streamer(&self) -> bool {
        self.streamer_id.is_some()
    }
}

impl std::fmt::Display for EngineKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.config_id {
            Some(id) => write!(f, "{}:{}", self.engine_type, id)?,
            None => write!(f, "{}", self.engine_type)?,
        }
        if let Some(sid) = &self.streamer_id {
            // Show truncated streamer ID for readability (up to the first 8 UTF-8 bytes).
            let short = &sid[..sid.floor_char_boundary(8)];
            write!(f, "[{}]", short)?;
        }
        Ok(())
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

#[derive(Debug)]
struct BreakerInner {
    state: CircuitState,
    failure_count: u32,
    opened_at: Option<Instant>,
    half_open_successes: u32,
    half_open_failures: u32,
}

impl BreakerInner {
    fn advance_cooldown(&mut self, cooldown_duration: Duration) {
        if self.state == CircuitState::Open
            && let Some(opened_at) = self.opened_at
            && opened_at.elapsed() >= cooldown_duration
        {
            self.state = CircuitState::HalfOpen;
            self.opened_at = None;
            self.half_open_successes = 0;
            self.half_open_failures = 0;
            debug!("Circuit breaker transitioned to half-open state");
        }
    }
}

/// Circuit breaker for protecting against cascading failures.
pub struct CircuitBreaker {
    /// Mutable state and counters guarded atomically to prevent torn transitions.
    inner: Mutex<BreakerInner>,
    /// Failure threshold to open circuit.
    failure_threshold: u32,
    /// Cooldown duration before trying again.
    cooldown_duration: Duration,
    /// Required successes to close circuit.
    success_threshold: u32,
    /// Required failures in half-open state to reopen circuit.
    /// This allows for transient errors without immediately reopening.
    half_open_failure_threshold: u32,
    /// Last time this breaker was used (unix epoch seconds).
    last_used_unix: AtomicU64,
}

impl CircuitBreaker {
    fn unix_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
    }

    fn touch(&self) {
        self.last_used_unix
            .store(Self::unix_now(), Ordering::Relaxed);
    }

    pub fn last_used_unix(&self) -> u64 {
        self.last_used_unix.load(Ordering::Relaxed)
    }

    /// Create a new circuit breaker.
    pub fn new(failure_threshold: u32, cooldown_secs: u64) -> Self {
        Self::with_half_open(failure_threshold, cooldown_secs, 2, 2)
    }

    /// Create a circuit breaker with explicit half-open thresholds.
    pub fn with_half_open(
        failure_threshold: u32,
        cooldown_secs: u64,
        half_open_success_threshold: u32,
        half_open_failure_threshold: u32,
    ) -> Self {
        Self {
            inner: Mutex::new(BreakerInner {
                state: CircuitState::Closed,
                failure_count: 0,
                opened_at: None,
                half_open_successes: 0,
                half_open_failures: 0,
            }),
            failure_threshold,
            cooldown_duration: Duration::from_secs(cooldown_secs),
            success_threshold: half_open_success_threshold,
            half_open_failure_threshold,
            last_used_unix: AtomicU64::new(Self::unix_now()),
        }
    }

    /// Get the current state.
    pub fn state(&self) -> CircuitState {
        self.touch();
        let mut inner = self.inner.lock();
        inner.advance_cooldown(self.cooldown_duration);
        inner.state
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
        self.touch();
        let mut inner = self.inner.lock();
        inner.advance_cooldown(self.cooldown_duration);

        match inner.state {
            CircuitState::Closed => {
                inner.failure_count = 0;
            }
            CircuitState::HalfOpen => {
                inner.half_open_successes += 1;
                if inner.half_open_successes >= self.success_threshold {
                    inner.state = CircuitState::Closed;
                    inner.opened_at = None;
                    inner.failure_count = 0;
                    inner.half_open_successes = 0;
                    inner.half_open_failures = 0;
                    info!("Circuit breaker closed after successful recovery");
                }
            }
            CircuitState::Open => {}
        }
    }

    /// Record a failed operation.
    pub fn record_failure(&self) {
        self.touch();
        let mut inner = self.inner.lock();
        inner.advance_cooldown(self.cooldown_duration);

        match inner.state {
            CircuitState::Closed => {
                inner.failure_count += 1;
                if inner.failure_count >= self.failure_threshold {
                    let failures = inner.failure_count;
                    inner.state = CircuitState::Open;
                    inner.opened_at = Some(Instant::now());
                    warn!(
                        "Circuit breaker opened after {} consecutive failures",
                        failures
                    );
                }
            }
            CircuitState::HalfOpen => {
                inner.half_open_failures += 1;
                if inner.half_open_failures >= self.half_open_failure_threshold {
                    let failures = inner.half_open_failures;
                    inner.state = CircuitState::Open;
                    inner.opened_at = Some(Instant::now());
                    inner.half_open_successes = 0;
                    inner.half_open_failures = 0;
                    warn!(
                        "Circuit breaker reopened after {} failures in half-open state",
                        failures
                    );
                } else {
                    debug!(
                        "Circuit breaker half-open failure {}/{}, staying half-open",
                        inner.half_open_failures, self.half_open_failure_threshold
                    );
                }
            }
            CircuitState::Open => {}
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
/// Keys circuit breakers by `EngineKey` (type + optional config ID + optional streamer ID)
/// to support per-instance and per-streamer isolation.
pub struct CircuitBreakerManager {
    breakers: RwLock<HashMap<EngineKey, Arc<CircuitBreaker>>>,
    failure_threshold: u32,
    cooldown_secs: u64,
    half_open_success_threshold: u32,
    half_open_failure_threshold: u32,
    last_cleanup_unix: AtomicU64,
}

impl CircuitBreakerManager {
    const EPHEMERAL_BREAKER_MAX: usize = 256;
    const EPHEMERAL_BREAKER_TTL_SECS: u64 = 6 * 60 * 60;
    const CLEANUP_INTERVAL_SECS: u64 = 5 * 60;

    /// Create a new circuit breaker manager.
    pub fn new(failure_threshold: u32, cooldown_secs: u64) -> Self {
        Self::with_half_open(failure_threshold, cooldown_secs, 2, 2)
    }

    /// Create a manager with explicit half-open thresholds.
    pub fn with_half_open(
        failure_threshold: u32,
        cooldown_secs: u64,
        half_open_success_threshold: u32,
        half_open_failure_threshold: u32,
    ) -> Self {
        Self {
            breakers: RwLock::new(HashMap::new()),
            failure_threshold,
            cooldown_secs,
            half_open_success_threshold,
            half_open_failure_threshold,
            last_cleanup_unix: AtomicU64::new(0),
        }
    }

    /// Whether a key represents an ephemeral (auto-created) breaker that should
    /// be subject to TTL cleanup and capacity limits.
    fn is_ephemeral(key: &EngineKey) -> bool {
        key.is_override() || key.is_per_streamer()
    }

    fn maybe_cleanup_ephemeral_breakers_locked(
        &self,
        breakers: &mut HashMap<EngineKey, Arc<CircuitBreaker>>,
    ) {
        let now = CircuitBreaker::unix_now();
        let last = self.last_cleanup_unix.load(Ordering::Relaxed);
        if now.saturating_sub(last) < Self::CLEANUP_INTERVAL_SECS {
            return;
        }

        if self
            .last_cleanup_unix
            .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        // Remove stale ephemeral breakers (overrides + per-streamer).
        let cutoff = now.saturating_sub(Self::EPHEMERAL_BREAKER_TTL_SECS);
        breakers.retain(|key, breaker| {
            if !Self::is_ephemeral(key) {
                return true;
            }
            breaker.last_used_unix() >= cutoff
        });

        // Cap the number of ephemeral breakers (best-effort LRU by last_used_unix).
        let mut ephemerals: Vec<(u64, EngineKey)> = breakers
            .iter()
            .filter_map(|(key, breaker)| {
                Self::is_ephemeral(key).then_some((breaker.last_used_unix(), key.clone()))
            })
            .collect();

        if ephemerals.len() <= Self::EPHEMERAL_BREAKER_MAX {
            return;
        }

        ephemerals.sort_by_key(|(last_used, _)| *last_used);
        let remove_count = ephemerals.len().saturating_sub(Self::EPHEMERAL_BREAKER_MAX);
        for (_, key) in ephemerals.into_iter().take(remove_count) {
            breakers.remove(&key);
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
        self.maybe_cleanup_ephemeral_breakers_locked(&mut breakers);
        breakers
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(CircuitBreaker::with_half_open(
                    self.failure_threshold,
                    self.cooldown_secs,
                    self.half_open_success_threshold,
                    self.half_open_failure_threshold,
                ))
            })
            .clone()
    }

    /// Check if an engine is allowed (circuit not open).
    pub fn is_allowed(&self, key: &EngineKey) -> bool {
        self.get(key).is_allowed()
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
    fn half_open_close_clears_partial_failures_and_reopen_enforces_fresh_cooldown() {
        let breaker = CircuitBreaker::with_half_open(1, 60, 2, 2);

        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Open);

        // Simulate cooldown expiry into HalfOpen, accumulate one partial failure, then recover.
        breaker.inner.lock().opened_at = Some(Instant::now() - Duration::from_secs(61));
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
        breaker.record_success();
        breaker.record_success();
        assert_eq!(breaker.state(), CircuitState::Closed);
        assert_eq!(breaker.inner.lock().half_open_failures, 0);

        // Next half-open cycle must tolerate a single transient failure again before reopening,
        // and once reopened must stay Open for a full fresh cooldown window.
        breaker.record_failure();
        breaker.inner.lock().opened_at = Some(Instant::now() - Duration::from_secs(61));
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Open);
        assert!(!breaker.is_allowed());
    }
}
