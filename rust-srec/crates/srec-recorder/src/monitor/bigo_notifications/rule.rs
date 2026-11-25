//! Pure notification-vs-polling state machine (`Rule`) for Bigo live notices.

use std::collections::{HashMap, HashSet, VecDeque};

use tokio::time::Instant;

use super::{
    MISSED_PUSH_WINDOW, MonitoringMethod, NOT_ARRIVING_AFTER, PAIR_WINDOW, PENDING_LIMIT,
    PollingReason, RECHECKS, SAFETY_CHECK, StreamerNotificationMethod,
};

/// A Bigo streamer as the last reconciliation saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Member {
    pub(super) uid: Option<u32>,
    /// Why the streamer is never subscribed.
    pub(super) excluded: Option<PollingReason>,
}

#[derive(Debug)]
pub(super) struct Entry {
    member: Member,
    /// When the streamer's latest successful individual check started.
    last_check: Option<Instant>,
    /// Whether that check found the stream live, filtered out included.
    /// Ends are not pushed, so such a streamer is checked for real until a
    /// check finds it offline.
    last_live: bool,
    /// When the latest push that may have been this streamer's arrived.
    last_push: Option<Instant>,
}

impl Entry {
    fn new(member: Member) -> Self {
        Self {
            member,
            last_check: None,
            last_live: false,
            last_push: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Subscription {
    Sent,
    Acknowledged(Instant),
    Refused,
}

/// Real checks a push asked for.
#[derive(Debug)]
pub(super) struct Pending {
    /// Until then, polls are real checks.
    until: Instant,
    /// Times at which the scheduler is asked for a check, earliest first.
    checks: VecDeque<Instant>,
}

/// Which polls are answered offline and which checks a push asks for. Time
/// is passed in, so the rule is tested without a clock.
#[derive(Debug, Default)]
pub(super) struct Rule {
    pub(super) streamers: HashMap<String, Entry>,
    /// Subscriptions on the current connection, by uid.
    subscriptions: HashMap<u32, Subscription>,
    pending: HashMap<String, Pending>,
    /// When a push without a streamer wakes everyone, unless a named push
    /// comes first.
    wake_all_at: Option<Instant>,
    last_named_push: Option<Instant>,
    /// Starts found without a push since the last push of any kind, and the
    /// streamers they were found on. They concern the account rather than a
    /// connection, so reconnecting keeps them; choosing another account
    /// clears them.
    missed_in_a_row: u32,
    missed_streamers: HashSet<String>,
}

impl Rule {
    /// Whether BIGO sends the account no pushes: [`NOT_ARRIVING_AFTER`]
    /// misses in a row, on at least two streamers, or on every streamer when
    /// fewer are followed. One streamer whose stream drops and resumes, or
    /// that a check reads offline for a moment, adds a miss at each resume
    /// and so cannot trip it alone.
    pub(super) fn not_arriving(&self) -> bool {
        self.missed_in_a_row >= NOT_ARRIVING_AFTER && {
            let followed = self
                .streamers
                .values()
                .filter(|entry| entry.member.excluded.is_none() && entry.member.uid.is_some())
                .count();
            self.missed_streamers.len() >= followed.min(2)
        }
    }

    /// Forgets the misses: a push arrived, or the account changed.
    pub(super) fn clear_missed(&mut self) {
        self.missed_in_a_row = 0;
        self.missed_streamers.clear();
    }

    pub(super) fn answers_offline(&self, streamer_id: &str, now: Instant) -> bool {
        if self.not_arriving()
            || self
                .pending
                .get(streamer_id)
                .is_some_and(|pending| pending.until > now)
        {
            return false;
        }
        let Some(entry) = self.streamers.get(streamer_id) else {
            return false;
        };
        let Some(subscribed_at) = subscribed_at(&self.subscriptions, entry) else {
            return false;
        };
        // A check from before the subscription may have missed a start that
        // happened while the connection was down.
        !entry.last_live
            && entry.last_check.is_some_and(|checked| {
                checked >= subscribed_at && now.saturating_duration_since(checked) < SAFETY_CHECK
            })
    }

    /// Records a successful individual check. Returns whether it found a
    /// start BIGO did not push: the streamer is subscribed, its previous
    /// check after the subscription read it offline, and no push that may
    /// have been its own came within [`MISSED_PUSH_WINDOW`].
    pub(super) fn checked(&mut self, streamer_id: &str, started: Instant, live: bool) -> bool {
        let mut missed = false;
        if let Some(entry) = self.streamers.get_mut(streamer_id)
            && entry.last_check.is_none_or(|last| started >= last)
        {
            missed = live
                && !entry.last_live
                && subscribed_at(&self.subscriptions, entry)
                    .is_some_and(|at| entry.last_check.is_some_and(|last| last >= at))
                && entry.last_push.is_none_or(|push| {
                    started.saturating_duration_since(push) >= MISSED_PUSH_WINDOW
                });
            entry.last_check = Some(started);
            entry.last_live = live;
        }
        if live {
            self.pending.remove(streamer_id);
        }
        if missed {
            self.missed_in_a_row = self.missed_in_a_row.saturating_add(1);
            self.missed_streamers.insert(streamer_id.to_owned());
        }
        missed
    }

    /// Records a push. Any push, with or without a streamer, shows that BIGO
    /// sends the account notifications.
    pub(super) fn pushed(&mut self, uid: Option<u32>, now: Instant) {
        self.clear_missed();
        let Some(uid) = uid else {
            let paired = self
                .last_named_push
                .is_some_and(|at| now.saturating_duration_since(at) < PAIR_WINDOW);
            if !paired && self.wake_all_at.is_none() {
                self.wake_all_at = Some(now + PAIR_WINDOW);
            }
            return;
        };
        self.last_named_push = Some(now);
        self.wake_all_at = None;
        for (id, entry) in &mut self.streamers {
            if entry.member.uid == Some(uid) && entry.member.excluded.is_none() {
                entry.last_push = Some(now);
                self.pending.insert(
                    id.clone(),
                    Pending {
                        until: now + PENDING_LIMIT,
                        checks: RECHECKS.iter().map(|offset| now + *offset).collect(),
                    },
                );
            }
        }
    }

    pub(super) fn next_due(&self) -> Option<Instant> {
        self.pending
            .values()
            .filter_map(|pending| pending.checks.front().copied())
            .chain(self.wake_all_at)
            .min()
    }

    /// The streamers to check now.
    pub(super) fn take_due(&mut self, now: Instant) -> Vec<String> {
        if self.wake_all_at.is_some_and(|at| at <= now) {
            self.wake_all_at = None;
            // One check each, and none for a streamer an earlier push still
            // holds: its polls are real until that push expires, so a run of
            // nameless pushes wakes each streamer at most once per
            // PENDING_LIMIT. Rechecking everyone on every start would be
            // polling again.
            for id in self.subscribed_ids() {
                if let Some(entry) = self.streamers.get_mut(&id) {
                    entry.last_push = Some(now);
                }
                self.pending.entry(id).or_insert_with(|| Pending {
                    until: now + PENDING_LIMIT,
                    checks: VecDeque::from([now]),
                });
            }
        }
        let mut due = Vec::new();
        self.pending.retain(|id, pending| {
            if pending.checks.front().is_some_and(|&at| at <= now) {
                while pending.checks.front().is_some_and(|&at| at <= now) {
                    pending.checks.pop_front();
                }
                due.push(id.clone());
            }
            pending.until > now
        });
        due
    }

    pub(super) fn subscribed_ids(&self) -> Vec<String> {
        self.streamers
            .iter()
            .filter(|(_, entry)| subscribed_at(&self.subscriptions, entry).is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Replaces the streamer set, keeping what is known about streamers that
    /// stay unchanged. Returns whether a uid that needs subscribing appeared.
    pub(super) fn set_streamers(&mut self, members: HashMap<String, Member>) -> bool {
        let before = self.wanted_uids();
        self.streamers.retain(|id, _| members.contains_key(id));
        self.pending.retain(|id, _| members.contains_key(id));
        for (id, member) in members {
            // A streamer that was excluded got no follow-up checks for a
            // push meanwhile, so its last check says nothing about now.
            if self
                .streamers
                .get(&id)
                .is_none_or(|entry| entry.member != member)
            {
                self.streamers.insert(id, Entry::new(member));
            }
        }
        !self.wanted_uids().is_subset(&before)
    }

    pub(super) fn wanted_uids(&self) -> HashSet<u32> {
        self.streamers
            .values()
            .filter(|entry| entry.member.excluded.is_none())
            .filter_map(|entry| entry.member.uid)
            .collect()
    }

    /// Ends the connection's subscriptions. Every connection starts after
    /// this, so it starts with none.
    pub(super) fn disconnected(&mut self) {
        self.subscriptions.clear();
        self.wake_all_at = None;
    }

    /// A wanted uid not subscribed on this connection yet, marked as sent.
    pub(super) fn next_subscription(&mut self) -> Option<u32> {
        let uid = self
            .wanted_uids()
            .into_iter()
            .find(|uid| !self.subscriptions.contains_key(uid))?;
        self.subscriptions.insert(uid, Subscription::Sent);
        Some(uid)
    }

    pub(super) fn has_unsent(&self) -> bool {
        self.wanted_uids()
            .iter()
            .any(|uid| !self.subscriptions.contains_key(uid))
    }

    pub(super) fn subscribed(&mut self, uid: u32, accepted: bool, now: Instant) {
        self.subscriptions.insert(
            uid,
            if accepted {
                Subscription::Acknowledged(now)
            } else {
                Subscription::Refused
            },
        );
    }

    /// Each streamer's method, and the subscribed and total counts. `listener`
    /// is why the listener is not connected.
    pub(super) fn methods(
        &self,
        listener: Option<PollingReason>,
    ) -> (Vec<StreamerNotificationMethod>, usize, usize) {
        let mut subscribed = 0;
        let mut total = 0;
        let mut methods: Vec<_> = self
            .streamers
            .iter()
            .map(|(id, entry)| {
                let Member { uid, excluded } = entry.member;
                if excluded.is_none() {
                    total += 1;
                }
                // A streamer's own opt-out names what would change its method
                // even with a working listener, so it comes first.
                let reason = match excluded {
                    Some(
                        reason @ (PollingReason::BatchDetectionOff
                        | PollingReason::StreamlinkExtractor),
                    ) => Some(reason),
                    _ => listener.or(excluded).or_else(|| match uid {
                        None => Some(PollingReason::UidUnknown),
                        Some(uid) => match self.subscriptions.get(&uid) {
                            None | Some(Subscription::Sent) => Some(PollingReason::Subscribing),
                            Some(Subscription::Refused) => Some(PollingReason::FollowRefused),
                            Some(Subscription::Acknowledged(_)) => {
                                subscribed += 1;
                                self.not_arriving()
                                    .then_some(PollingReason::NotificationsNotArriving)
                            }
                        },
                    }),
                };
                StreamerNotificationMethod {
                    streamer_id: id.clone(),
                    method: if reason.is_some() {
                        MonitoringMethod::Polling
                    } else {
                        MonitoringMethod::Notifications
                    },
                    reason,
                }
            })
            .collect();
        methods.sort_by(|a, b| a.streamer_id.cmp(&b.streamer_id));
        (methods, subscribed, total)
    }
}

/// When BIGO confirmed the streamer's subscription on the current connection,
/// if it did and the streamer is not excluded.
fn subscribed_at(subscriptions: &HashMap<u32, Subscription>, entry: &Entry) -> Option<Instant> {
    let uid = entry
        .member
        .uid
        .filter(|_| entry.member.excluded.is_none())?;
    match subscriptions.get(&uid) {
        Some(Subscription::Acknowledged(at)) => Some(*at),
        _ => None,
    }
}
