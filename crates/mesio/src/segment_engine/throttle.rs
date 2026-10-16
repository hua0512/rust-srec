//! Bounded rate-limit holds shared by scheduling and ordered output.
//!
//! A hold lasts from a segment's first refusal until its fixed throttling
//! deadline. Revisions travel outside the payload channel so backpressure on
//! that channel cannot hide a hold from a gap or stall timer.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::watch;

use super::descriptor::{MediaTrack, SegmentDescriptor};
use super::identity::{SegmentKey, SegmentKind};

/// A coalescible snapshot of time spent waiting on rate-limited work.
///
/// The fixed deadline bounds an active pause; only elapsed time is retained
/// when work resumes, so a short refusal does not spend its full allowance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThrottleClock {
    paused: Duration,
    active: Option<(Instant, Instant)>,
    last_updated: Option<Instant>,
}

impl ThrottleClock {
    /// Updates the active pause and reports whether the snapshot changed.
    pub fn set_hold(&mut self, now: Instant, until: Option<Instant>) -> bool {
        // Concurrent producers can capture their timestamps before acquiring
        // the shared lock in the opposite order. Never restart an interval
        // before the time already accounted by a newer transition.
        let now = self.last_updated.map_or(now, |previous| previous.max(now));
        self.last_updated = Some(now);
        let until = until.filter(|until| *until > now);
        if self.active.map(|(_, until)| until) == until {
            return false;
        }
        self.paused = self.paused_at(now);
        self.active = until.map(|until| (now, until));
        true
    }

    /// Total actual paused time, including the unexpired active pause.
    /// Retaining the counter makes coalesced start/end updates lossless.
    pub fn paused_at(&self, now: Instant) -> Duration {
        self.active.map_or(self.paused, |(since, until)| {
            self.paused
                .saturating_add(now.min(until).saturating_duration_since(since))
        })
    }

    pub fn active_until(&self) -> Option<Instant> {
        self.active.map(|(_, until)| until)
    }

    /// Resumes the timeout's remaining budget after an active pause.
    /// Callers save `paused_at(since)` when their timeout begins and recompute
    /// this deadline whenever the clock changes, so an early recovery does
    /// not retain the unused part of the throttling allowance.
    pub fn deadline(
        &self,
        since: Instant,
        paused_baseline: Duration,
        timeout: Duration,
        now: Instant,
    ) -> Instant {
        let paused = self.paused_at(now).saturating_sub(paused_baseline);
        let elapsed = now.saturating_duration_since(since).saturating_sub(paused);
        let remaining = timeout.saturating_sub(elapsed);
        let resumes = if remaining.is_zero() {
            now
        } else {
            self.active_until()
                .filter(|until| *until > now)
                .unwrap_or(now)
        };
        resumes.checked_add(remaining).unwrap_or(resumes)
    }
}

/// The union of the two DASH tracks' active pauses, counted once when they
/// overlap. Producers retain elapsed time before notifying the consumer.
#[derive(Debug)]
pub struct ThrottleSession {
    state: Mutex<SessionState>,
    clock: watch::Sender<ThrottleClock>,
}

#[derive(Debug, Default)]
struct SessionState {
    tracks: [Option<Instant>; 2],
    clock: ThrottleClock,
}

impl ThrottleSession {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SessionState::default()),
            clock: watch::channel(ThrottleClock::default()).0,
        }
    }

    pub fn set_hold(&self, track: MediaTrack, now: Instant, until: Option<Instant>) {
        let mut state = self.state.lock();
        let slot = match track {
            MediaTrack::Video => 0,
            MediaTrack::Audio => 1,
        };
        state.tracks[slot] = until;
        let until = state
            .tracks
            .iter()
            .copied()
            .flatten()
            .filter(|until| *until > now)
            .max();
        if state.clock.set_hold(now, until) {
            self.clock.send_replace(state.clock);
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<ThrottleClock> {
        self.clock.subscribe()
    }
}

#[derive(Debug, Default)]
struct Holds {
    media: BTreeMap<u64, MediaHold>,
    inits: HashMap<SegmentKey, Instant>,
    scope: Option<Scope>,
    clock: ThrottleClock,
    session: Option<(Arc<ThrottleSession>, MediaTrack)>,
}

#[derive(Debug)]
struct Scope {
    cursor: u64,
    init: Option<SegmentKey>,
    startup: bool,
}

#[derive(Debug)]
struct MediaHold {
    until: Instant,
    /// A URL change can leave several resource identities at one MSN.
    owners: HashSet<SegmentKey>,
}

#[derive(Debug)]
pub struct RateLimitHolds {
    holds: Mutex<Holds>,
    changed: watch::Sender<()>,
    clock: watch::Sender<ThrottleClock>,
}

impl RateLimitHolds {
    pub fn new() -> Self {
        Self {
            holds: Mutex::new(Holds::default()),
            changed: watch::channel(()).0,
            clock: watch::channel(ThrottleClock::default()).0,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.changed.subscribe()
    }

    pub fn subscribe_clock(&self) -> watch::Receiver<ThrottleClock> {
        self.clock.subscribe()
    }

    pub fn clock(&self) -> ThrottleClock {
        *self.clock.borrow()
    }

    pub fn set_session(&self, session: Arc<ThrottleSession>, track: MediaTrack) {
        let mut holds = self.holds.lock();
        holds.session = Some((session, track));
        self.refresh_clock(&mut holds, Instant::now());
    }

    /// Output identifies the media gap or init dependency that can block its
    /// cursor. Subsequent fetch transitions update this clock immediately.
    pub fn set_scope(
        &self,
        cursor: u64,
        required_init: Option<&SegmentKey>,
        startup_empty: bool,
        now: Instant,
    ) {
        let mut holds = self.holds.lock();
        holds.scope = Some(Scope {
            cursor,
            init: required_init.cloned(),
            startup: startup_empty,
        });
        self.refresh_clock(&mut holds, now);
    }

    /// A stopped track must no longer suspend the session, even if its
    /// cancelled fetches finish and update the registry afterwards.
    pub fn clear_scope(&self, now: Instant) {
        let mut holds = self.holds.lock();
        holds.scope = None;
        self.refresh_clock(&mut holds, now);
    }

    fn refresh_clock(&self, holds: &mut Holds, now: Instant) {
        Self::expire(holds, now);
        let until = holds.scope.as_ref().and_then(|scope| {
            let media = holds.media.get(&scope.cursor).map(|hold| hold.until);
            let init = scope
                .init
                .as_ref()
                .and_then(|key| holds.inits.get(key))
                .copied()
                .or_else(|| {
                    scope
                        .startup
                        .then(|| holds.inits.values().copied().max())
                        .flatten()
                });
            media.into_iter().chain(init).max()
        });
        if holds.clock.set_hold(now, until) {
            self.clock.send_replace(holds.clock);
        }
        if let Some((session, track)) = &holds.session {
            session.set_hold(*track, now, until);
        }
    }

    pub fn hold(&self, descriptor: &SegmentDescriptor, until: Instant, now: Instant) {
        let mut holds = self.holds.lock();
        let changed = match descriptor.key.kind {
            SegmentKind::Media => match holds.media.entry(descriptor.msn) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(MediaHold {
                        until,
                        owners: HashSet::from([descriptor.key.clone()]),
                    });
                    true
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let hold = entry.get_mut();
                    let old_until = hold.until;
                    hold.until = hold.until.min(until);
                    hold.owners.insert(descriptor.key.clone()) || old_until != hold.until
                }
            },
            SegmentKind::Init => match holds.inits.entry(descriptor.key.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(until);
                    true
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if until < *entry.get() {
                        entry.insert(until);
                        true
                    } else {
                        false
                    }
                }
            },
        };
        // Rediscovery and repeated refusals must not restart the allowance.
        self.refresh_clock(&mut holds, now);
        drop(holds);
        if changed {
            self.changed.send_replace(());
        }
    }

    pub fn release(&self, key: &SegmentKey, msn: u64, now: Instant) {
        let mut holds = self.holds.lock();
        let changed = match key.kind {
            SegmentKind::Media => {
                let changed = holds
                    .media
                    .get_mut(&msn)
                    .is_some_and(|hold| hold.owners.remove(key));
                if holds
                    .media
                    .get(&msn)
                    .is_some_and(|hold| hold.owners.is_empty())
                {
                    holds.media.remove(&msn);
                }
                changed
            }
            SegmentKind::Init => holds.inits.remove(key).is_some(),
        };
        self.refresh_clock(&mut holds, now);
        drop(holds);
        if changed {
            self.changed.send_replace(());
        }
    }

    /// Explicit upstream skips are inclusive and override rate-limit holds.
    pub fn skip(&self, from: u64, to: u64, now: Instant) {
        let mut holds = self.holds.lock();
        let before = holds.media.len();
        holds.media.retain(|msn, _| *msn < from || *msn > to);
        let changed = before != holds.media.len();
        self.refresh_clock(&mut holds, now);
        drop(holds);
        if changed {
            self.changed.send_replace(());
        }
    }

    fn expire(holds: &mut Holds, now: Instant) {
        holds.media.retain(|_, hold| hold.until > now);
        holds.inits.retain(|_, until| *until > now);
    }

    /// First held media in the half-open range, and its fixed deadline.
    pub fn first_media_hold(&self, from: u64, to: u64, now: Instant) -> Option<(u64, Instant)> {
        if from >= to {
            return None;
        }
        let mut holds = self.holds.lock();
        self.refresh_clock(&mut holds, now);
        holds
            .media
            .range(from..to)
            .next()
            .map(|(msn, hold)| (*msn, hold.until))
    }

    /// Whether outstanding rate-limited work can still hold up this cursor.
    /// Used to bound admission while output deliberately waits for it.
    pub fn active_until(&self, cursor: u64, now: Instant) -> Option<Instant> {
        let mut holds = self.holds.lock();
        self.refresh_clock(&mut holds, now);
        holds
            .media
            .range(cursor..)
            .map(|(_, hold)| hold.until)
            .chain(holds.inits.values().copied())
            .max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_clock_counts_overlapping_short_holds_once_after_coalescing() {
        let session = ThrottleSession::new();
        let clock = session.subscribe();
        let start = Instant::now();
        session.set_hold(
            MediaTrack::Video,
            start,
            Some(start + Duration::from_secs(300)),
        );
        session.set_hold(
            MediaTrack::Audio,
            start + Duration::from_secs(1),
            Some(start + Duration::from_secs(301)),
        );
        session.set_hold(MediaTrack::Video, start + Duration::from_secs(2), None);
        session.set_hold(MediaTrack::Audio, start + Duration::from_secs(3), None);

        // The consumer sees only the final snapshot. The overlapping waits
        // lasted three seconds, not four or their five-minute allowances.
        let clock = *clock.borrow();
        let now = start + Duration::from_secs(4);
        assert_eq!(clock.paused_at(now), Duration::from_secs(3));
        assert_eq!(clock.active_until(), None);
        assert_eq!(
            clock.deadline(start, Duration::ZERO, Duration::from_secs(10), now),
            start + Duration::from_secs(13)
        );
    }

    #[test]
    fn an_unreleased_hold_stops_counting_at_its_fixed_expiry() {
        let mut clock = ThrottleClock::default();
        let start = Instant::now();
        clock.set_hold(start, Some(start + Duration::from_secs(300)));
        let now = start + Duration::from_secs(305);
        assert_eq!(clock.paused_at(now), Duration::from_secs(300));
        assert_eq!(
            clock.deadline(start, Duration::ZERO, Duration::from_secs(10), now),
            start + Duration::from_secs(310)
        );
    }

    #[test]
    fn a_late_track_end_cannot_manufacture_extra_paused_time() {
        let session = ThrottleSession::new();
        let clock = session.subscribe();
        let start = Instant::now();
        session.set_hold(
            MediaTrack::Video,
            start,
            Some(start + Duration::from_secs(300)),
        );
        session.set_hold(
            MediaTrack::Audio,
            start + Duration::from_secs(10),
            Some(start + Duration::from_secs(400)),
        );
        // An earlier captured teardown timestamp races with a newer hold
        // update while its producer waits for the shared lock.
        session.set_hold(MediaTrack::Audio, start + Duration::from_secs(5), None);
        session.set_hold(MediaTrack::Video, start + Duration::from_secs(20), None);
        assert_eq!(
            clock.borrow().paused_at(start + Duration::from_secs(20)),
            Duration::from_secs(20)
        );
    }
}
