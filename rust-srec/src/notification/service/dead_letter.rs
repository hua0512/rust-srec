use std::sync::atomic::Ordering;

use chrono::{DateTime, Utc};
use dashmap::DashMap;

use crate::Result;

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

    /// Get dead letter entries.
    pub fn get_dead_letters(&self) -> Vec<DeadLetterEntry> {
        self.cleanup_dead_letters();
        self.dead_letters
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Retry a dead letter notification.
    pub async fn retry_dead_letter(&self, id: u64) -> Result<()> {
        if let Some((_, dead_letter)) = self.dead_letters.remove(&id) {
            if let Some(channel_key) = dead_letter.channel_key.clone() {
                self.notify_channel_instance(&channel_key, dead_letter.event)
                    .await
            } else {
                self.notify(dead_letter.event).await
            }
        } else {
            Err(crate::Error::NotFound {
                entity_type: "DeadLetter".to_string(),
                id: id.to_string(),
            })
        }
    }

    /// Clear old dead letters.
    pub fn cleanup_dead_letters(&self) {
        let now = Utc::now();
        self.dead_letter_cleanup_ts
            .store(now.timestamp().max(0) as u64, Ordering::Relaxed);

        if self.config.dead_letter_retention_days == 0 {
            self.dead_letters.clear();
            return;
        }

        let cutoff = now - chrono::Duration::days(self.config.dead_letter_retention_days as i64);
        self.dead_letters
            .retain(|_, entry| entry.dead_lettered_at > cutoff);
    }
}
