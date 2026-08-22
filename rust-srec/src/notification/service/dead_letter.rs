use std::sync::atomic::Ordering;

use chrono::{DateTime, Utc};
use dashmap::DashMap;

use super::{DEAD_LETTER_CLEANUP_INTERVAL_SECS, DeadLetterEntry, NotificationService};

impl NotificationService {
    pub(super) fn maybe_cleanup_dead_letters_detached(
        dead_letters: &DashMap<u64, DeadLetterEntry>,
        retention_days: u32,
        dead_letter_cleanup_ts: &std::sync::atomic::AtomicU64,
        now: DateTime<Utc>,
    ) {
        let now_ts = now.timestamp().max(0) as u64;
        let last = dead_letter_cleanup_ts.load(Ordering::Relaxed);
        if now_ts.saturating_sub(last) < DEAD_LETTER_CLEANUP_INTERVAL_SECS {
            return;
        }
        if dead_letter_cleanup_ts
            .compare_exchange(last, now_ts, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        if retention_days == 0 {
            dead_letters.clear();
            return;
        }

        let cutoff = now - chrono::Duration::days(retention_days as i64);
        dead_letters.retain(|_, entry| entry.dead_lettered_at > cutoff);
    }
}
