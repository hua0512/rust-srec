//! Streamers whose latest credential acquisition found no usable account.
//!
//! An unavailable account selection backs a streamer off without counting an
//! error, so its stored state stays whatever it was (usually offline) while
//! nothing can be recorded. This record is what tells the two apart. It is
//! runtime state: the first check after a restart establishes it again.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::broadcast;

use super::UnavailableReason;

/// Undelivered changes a slow subscriber may fall behind by. Changes are
/// transitions, not per-check repeats, so this covers every streamer turning
/// blocked at once on a large installation.
const CHANGE_CAPACITY: usize = 256;

/// Why a streamer's checks found no usable account, and since when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CredentialBlock {
    pub reason: UnavailableReason,
    /// The platform whose accounts the streamer selects from.
    pub platform_id: String,
    /// The first check of the current blocked run; a change of reason on the
    /// same platform keeps it.
    pub since: DateTime<Utc>,
}

/// A streamer's block appeared, changed or was lifted (`block` is `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialBlockChange {
    pub streamer_id: String,
    pub block: Option<CredentialBlock>,
}

/// Per-streamer blocks, with a change feed for live clients.
pub struct CredentialBlocks {
    blocks: Mutex<HashMap<String, CredentialBlock>>,
    changes: broadcast::Sender<CredentialBlockChange>,
}

impl Default for CredentialBlocks {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialBlocks {
    pub fn new() -> Self {
        let (changes, _) = broadcast::channel(CHANGE_CAPACITY);
        Self {
            blocks: Mutex::new(HashMap::new()),
            changes,
        }
    }

    pub fn get(&self, streamer_id: &str) -> Option<CredentialBlock> {
        self.blocks.lock().get(streamer_id).cloned()
    }

    /// Records that the streamer's acquisition found no usable account.
    /// Repeating the current block publishes nothing.
    pub fn block(&self, streamer_id: &str, platform_id: &str, reason: UnavailableReason) {
        let mut blocks = self.blocks.lock();
        let since = match blocks.get(streamer_id) {
            Some(current) if current.platform_id == platform_id => {
                if current.reason == reason {
                    return;
                }
                current.since
            }
            _ => Utc::now(),
        };
        let block = CredentialBlock {
            reason,
            platform_id: platform_id.to_owned(),
            since,
        };
        blocks.insert(streamer_id.to_owned(), block.clone());
        self.publish(streamer_id, Some(block));
    }

    /// Lifts the streamer's block, if it has one.
    pub fn clear(&self, streamer_id: &str) {
        let mut blocks = self.blocks.lock();
        if blocks.remove(streamer_id).is_some() {
            self.publish(streamer_id, None);
        }
    }

    /// Called with the map locked, so subscribers see changes in the order
    /// they were applied.
    fn publish(&self, streamer_id: &str, block: Option<CredentialBlock>) {
        let change = CredentialBlockChange {
            streamer_id: streamer_id.to_owned(),
            block,
        };
        // Fails only without subscribers, the normal state with no client
        // connected; the map stays the source of truth either way.
        if self.changes.send(change).is_err() {
            tracing::trace!(streamer_id, "No live client for a credential block change");
        }
    }

    /// Changes applied after this call; the current blocks come from [`Self::get`].
    pub fn subscribe(&self) -> broadcast::Receiver<CredentialBlockChange> {
        self.changes.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_keep_the_start_and_publish_only_transitions() {
        let blocks = CredentialBlocks::new();
        let mut changes = blocks.subscribe();
        blocks.block("s", "platform-a", UnavailableReason::LoginRequired);
        let first = blocks.get("s").unwrap();
        blocks.block("s", "platform-a", UnavailableReason::LoginRequired);
        blocks.block("s", "platform-a", UnavailableReason::ProfilesDisabled);
        let changed = blocks.get("s").unwrap();
        assert_eq!(changed.reason, UnavailableReason::ProfilesDisabled);
        assert_eq!(changed.since, first.since);
        blocks.clear("s");
        blocks.clear("s");
        assert!(blocks.get("s").is_none());

        let received: Vec<_> = std::iter::from_fn(|| changes.try_recv().ok()).collect();
        assert_eq!(
            received,
            vec![
                CredentialBlockChange {
                    streamer_id: "s".into(),
                    block: Some(first),
                },
                CredentialBlockChange {
                    streamer_id: "s".into(),
                    block: Some(changed),
                },
                CredentialBlockChange {
                    streamer_id: "s".into(),
                    block: None,
                },
            ]
        );
    }

    #[test]
    fn moving_to_another_platform_starts_a_new_block() {
        let blocks = CredentialBlocks::new();
        blocks.block("s", "platform-a", UnavailableReason::LoginRequired);
        let first = blocks.get("s").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        blocks.block("s", "platform-b", UnavailableReason::LoginRequired);
        let moved = blocks.get("s").unwrap();
        assert_eq!(moved.platform_id, "platform-b");
        assert!(moved.since > first.since);
    }
}
