//! Wall-clock shutdown scheduling and enforcement primitives.

use std::fmt;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Timing policy for a bounded shutdown.
///
/// `force_reserve` is held back from the cooperative phase so forced
/// containment can begin before the absolute deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ShutdownPolicy {
    total: Duration,
    force_reserve: Duration,
}

impl ShutdownPolicy {
    pub(crate) fn new(
        total: Duration,
        force_reserve: Duration,
    ) -> Result<Self, ShutdownDeadlineError> {
        if total.is_zero() {
            return Err(ShutdownDeadlineError::ZeroTotal);
        }
        if force_reserve.is_zero() {
            return Err(ShutdownDeadlineError::ZeroForceReserve);
        }
        if force_reserve >= total {
            return Err(ShutdownDeadlineError::ForceReserveNotLessThanTotal);
        }

        Ok(Self {
            total,
            force_reserve,
        })
    }

    pub(crate) fn total(self) -> Duration {
        self.total
    }

    pub(crate) fn force_reserve(self) -> Duration {
        self.force_reserve
    }
}

/// Absolute monotonic instants used by one shutdown attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ShutdownSchedule {
    force_at: Instant,
    deadline: Instant,
}

impl ShutdownSchedule {
    pub(crate) fn from_start(
        started_at: Instant,
        policy: ShutdownPolicy,
    ) -> Result<Self, ShutdownDeadlineError> {
        let deadline = started_at
            .checked_add(policy.total())
            .ok_or(ShutdownDeadlineError::InstantOverflow)?;
        let force_at = started_at
            .checked_add(policy.total() - policy.force_reserve())
            .ok_or(ShutdownDeadlineError::InstantOverflow)?;

        Ok(Self { force_at, deadline })
    }

    pub(crate) fn force_at(self) -> Instant {
        self.force_at
    }

    pub(crate) fn deadline(self) -> Instant {
        self.deadline
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShutdownDeadlineError {
    ZeroTotal,
    ZeroForceReserve,
    ForceReserveNotLessThanTotal,
    InstantOverflow,
}

impl fmt::Display for ShutdownDeadlineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ZeroTotal => "shutdown duration must be greater than zero",
            Self::ZeroForceReserve => "forced-containment reserve must be greater than zero",
            Self::ForceReserveNotLessThanTotal => {
                "forced-containment reserve must be less than the total shutdown duration"
            }
            Self::InstantOverflow => "shutdown schedule exceeds the monotonic clock range",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ShutdownDeadlineError {}

/// Dedicated-thread watchdog for the absolute hard deadline.
///
/// The callback should perform bounded terminal containment. In production it
/// may terminate a process, but that policy stays outside this module. Calling
/// [`Self::disarm`] before the deadline prevents the callback. If firing has
/// already started, disarm waits for it; no callback can outlive disarm or drop.
pub(crate) struct HardDeadlineWatchdog {
    disarm_tx: Option<mpsc::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl HardDeadlineWatchdog {
    pub(crate) fn arm<F>(deadline: Instant, callback: F) -> std::io::Result<Self>
    where
        F: FnOnce() + Send + 'static,
    {
        let (disarm_tx, disarm_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("shutdown-hard-deadline".to_string())
            .spawn(move || {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match disarm_rx.recv_timeout(remaining) {
                    Ok(()) if Instant::now() < deadline => {}
                    Ok(()) => callback(),
                    Err(mpsc::RecvTimeoutError::Disconnected) if Instant::now() < deadline => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => callback(),
                    Err(mpsc::RecvTimeoutError::Timeout) => callback(),
                }
            })?;

        Ok(Self {
            disarm_tx: Some(disarm_tx),
            worker: Some(worker),
        })
    }

    /// Prevent deadline firing on clean completion and join the watchdog thread.
    pub(crate) fn disarm(mut self) -> thread::Result<()> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> thread::Result<()> {
        if let Some(disarm_tx) = self.disarm_tx.take() {
            // A send failure means the worker already fired or exited. Joining
            // below still proves the callback cannot outlive this operation.
            if disarm_tx.send(()).is_err() {
                tracing::debug!("Hard-deadline watchdog was already settled during disarm");
            }
        }

        match self.worker.take() {
            Some(worker) => worker.join(),
            None => Ok(()),
        }
    }
}

impl Drop for HardDeadlineWatchdog {
    fn drop(&mut self) {
        if self.stop_and_join().is_err() {
            tracing::error!("Hard-deadline watchdog callback panicked");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn shutdown_policy_rejects_invalid_durations() {
        assert_eq!(
            ShutdownPolicy::new(Duration::ZERO, Duration::from_secs(1)),
            Err(ShutdownDeadlineError::ZeroTotal)
        );
        assert_eq!(
            ShutdownPolicy::new(Duration::from_secs(2), Duration::ZERO),
            Err(ShutdownDeadlineError::ZeroForceReserve)
        );
        assert_eq!(
            ShutdownPolicy::new(Duration::from_secs(2), Duration::from_secs(2)),
            Err(ShutdownDeadlineError::ForceReserveNotLessThanTotal)
        );
        assert_eq!(
            ShutdownPolicy::new(Duration::from_secs(2), Duration::from_secs(3)),
            Err(ShutdownDeadlineError::ForceReserveNotLessThanTotal)
        );
    }

    #[test]
    fn shutdown_schedule_reserves_time_before_the_deadline() {
        let policy = ShutdownPolicy::new(Duration::from_secs(10), Duration::from_secs(2)).unwrap();
        let started_at = Instant::now();
        let schedule = ShutdownSchedule::from_start(started_at, policy).unwrap();

        assert_eq!(
            schedule.force_at().duration_since(started_at),
            Duration::from_secs(8)
        );
        assert_eq!(
            schedule.deadline().duration_since(schedule.force_at()),
            Duration::from_secs(2)
        );
        assert_eq!(
            schedule.deadline().duration_since(started_at),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn watchdog_fires_at_an_elapsed_deadline() {
        let (fired_tx, fired_rx) = mpsc::channel();
        let watchdog = HardDeadlineWatchdog::arm(Instant::now(), move || {
            fired_tx.send(()).unwrap();
        })
        .unwrap();

        fired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("watchdog callback should fire");
        watchdog.disarm().unwrap();
    }

    #[test]
    fn disarm_cannot_suppress_an_elapsed_deadline() {
        let fired = Arc::new(AtomicBool::new(false));
        let fired_by_callback = Arc::clone(&fired);
        let watchdog =
            HardDeadlineWatchdog::arm(Instant::now() - Duration::from_millis(1), move || {
                fired_by_callback.store(true, Ordering::SeqCst)
            })
            .unwrap();

        watchdog.disarm().unwrap();

        assert!(fired.load(Ordering::SeqCst));
    }

    #[test]
    fn explicit_disarm_prevents_callback() {
        let fired = Arc::new(AtomicBool::new(false));
        let fired_by_callback = Arc::clone(&fired);
        let watchdog =
            HardDeadlineWatchdog::arm(Instant::now() + Duration::from_secs(60), move || {
                fired_by_callback.store(true, Ordering::SeqCst)
            })
            .unwrap();

        watchdog.disarm().unwrap();

        assert!(!fired.load(Ordering::SeqCst));
    }

    #[test]
    fn drop_disarms_and_joins_the_watchdog() {
        let fired = Arc::new(AtomicBool::new(false));
        let fired_by_callback = Arc::clone(&fired);
        let watchdog =
            HardDeadlineWatchdog::arm(Instant::now() + Duration::from_secs(60), move || {
                fired_by_callback.store(true, Ordering::SeqCst)
            })
            .unwrap();

        drop(watchdog);

        assert!(!fired.load(Ordering::SeqCst));
    }
}
