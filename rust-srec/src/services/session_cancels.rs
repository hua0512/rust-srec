//! Per-session cancellation registry.
//!
//! The download pipeline runs in `tokio::spawn`'d tasks, one per
//! `StreamerLive` event. Those tasks may park for a long time waiting
//! for a queue slot, then continue to engine bring-up and danmu
//! collection. While they're parked, the streamer can go offline,
//! get disabled, or be restarted under a fresh session — and the
//! pipeline must abort cleanly.
//!
//! `SessionCancelTokens` gives every running pipeline a
//! [`tokio_util::sync::CancellationToken`] keyed by `session_id`.
//! When a `StreamerOffline` (or any other terminal monitor event)
//! arrives, the corresponding handler calls
//! [`SessionCancelTokens::cancel`] and the pipeline observes the
//! token via `tokio::select!` on whichever await it is currently
//! parked on.
//!
//! ## Lifecycle
//!
//! - The pipeline calls [`SessionCancelTokens::register`] at the
//!   start of its task. If a token already exists for that
//!   `session_id`, it is returned (this is the dedup case for
//!   hysteresis-resume `StreamerLive` re-emits with the same
//!   session_id).
//! - On terminal exit (success, error, cancellation), the returned
//!   [`SessionCancelHandle`] drops and removes the entry only if it
//!   still owns the same token.
//! - On `StreamerOffline`, the monitor handler calls
//!   [`SessionCancelTokens::cancel`] which both fires the token (so
//!   any in-progress await wakes) and removes the entry.
//!
//! Tokens are not persisted; a process restart drops them and
//! pipelines that survived restart will re-register naturally on
//! the next live event.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use tokio_util::sync::CancellationToken;

/// Registry of per-session cancellation tokens.
///
/// Cheap to clone — internally `Arc<DashMap>`-backed.
#[derive(Debug, Clone, Default)]
pub struct SessionCancelTokens {
    inner: Arc<DashMap<String, TokenEntry>>,
    next_id: Arc<AtomicU64>,
}

#[derive(Debug, Clone)]
struct TokenEntry {
    id: u64,
    token: CancellationToken,
}

/// Scoped registration for a per-session cancellation token.
///
/// Dropping the handle forgets the session only if the registry still
/// points at this exact entry id. That identity check keeps an older
/// pipeline from accidentally removing a fresh token that was
/// registered for the same session id after a cancel/restart race.
#[derive(Debug)]
pub struct SessionCancelHandle {
    registry: Arc<DashMap<String, TokenEntry>>,
    session_id: String,
    id: u64,
    token: CancellationToken,
}

impl SessionCancelHandle {
    /// Cancellation token observed by the pipeline.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Drop for SessionCancelHandle {
    fn drop(&mut self) {
        if let Some(entry) = self.registry.get(&self.session_id)
            && entry.value().id == self.id
        {
            drop(entry);
            self.registry.remove(&self.session_id);
        }
    }
}

impl SessionCancelTokens {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Register a running pipeline and get its cancellation token.
    ///
    /// If the session already has a token, returns the existing one
    /// (so two concurrent paths cooperatively share cancellation).
    /// Otherwise inserts and returns a fresh token. The returned
    /// handle removes the entry on drop if it still owns the same
    /// token.
    pub fn register(&self, session_id: &str) -> SessionCancelHandle {
        let entry = match self.inner.entry(session_id.to_string()) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let token_entry = TokenEntry {
                    id: self.next_id.fetch_add(1, Ordering::Relaxed),
                    token: CancellationToken::new(),
                };
                entry.insert(token_entry.clone());
                token_entry
            }
        };

        SessionCancelHandle {
            registry: self.inner.clone(),
            session_id: session_id.to_string(),
            id: entry.id,
            token: entry.token,
        }
    }

    /// Cancel the token for `session_id` and remove the entry.
    ///
    /// No-op if the session has no registered token. After this,
    /// any future [`Self::register`] for the same `session_id`
    /// returns a fresh, uncancelled token (useful when a session
    /// transitions Offline → Live with the same id, although in
    /// practice a fresh `StreamerLive` carries a new session_id).
    pub fn cancel(&self, session_id: &str) {
        if let Some((_, entry)) = self.inner.remove(session_id) {
            entry.token.cancel();
        }
    }

    /// Number of currently-tracked sessions. Test-only.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True when no sessions are tracked.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_for_returns_existing_on_duplicate() {
        let reg = SessionCancelTokens::new();
        let a = reg.register("s1");
        let b = reg.register("s1");
        // Same underlying token: cancelling one cancels the other.
        a.token().cancel();
        assert!(b.token().is_cancelled());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn cancel_fires_and_removes() {
        let reg = SessionCancelTokens::new();
        let handle = reg.register("s1");
        let token = handle.token();
        assert!(!token.is_cancelled());
        reg.cancel("s1");
        assert!(token.is_cancelled());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn cancel_unknown_is_noop() {
        let reg = SessionCancelTokens::new();
        reg.cancel("ghost");
        assert!(reg.is_empty());
    }

    #[test]
    fn handle_drop_removes_without_firing() {
        let reg = SessionCancelTokens::new();
        let handle = reg.register("s1");
        let token = handle.token();
        drop(handle);
        assert!(!token.is_cancelled());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn token_for_after_cancel_is_fresh() {
        let reg = SessionCancelTokens::new();
        let handle = reg.register("s1");
        let t1 = handle.token();
        reg.cancel("s1");
        assert!(t1.is_cancelled());

        let t2 = reg.register("s1").token();
        assert!(!t2.is_cancelled());
    }

    #[test]
    fn old_handle_does_not_remove_new_token() {
        let reg = SessionCancelTokens::new();
        let old = reg.register("s1");
        reg.cancel("s1");

        let new = reg.register("s1");
        let new_token = new.token();
        drop(old);

        assert_eq!(reg.len(), 1);
        assert!(!new_token.is_cancelled());
        drop(new);
        assert_eq!(reg.len(), 0);
    }
}
