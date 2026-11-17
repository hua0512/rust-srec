//! Live notifications, Bigo's batch detection.
//!
//! BIGO's website has no request that reports many rooms at once. Instead, a
//! signed-in account that follows a streamer, with the streamer's live notices
//! on, receives a push over BIGO's app connection when the streamer starts
//! streaming. With the Bigo platform's `batch_detection` option on and a
//! credential profile chosen as its `notification_account`,
//! [`BigoNotifications`] keeps that connection open and follows every
//! monitored Bigo streamer whose numeric account id (uid) an individual check
//! has stored. Then:
//!
//! - a push makes the scheduler check the streamer at once, and again at the
//!   [`RECHECKS`] offsets while the profile API may still read it offline;
//! - the monitor's batch gate answers a routine offline poll without a request
//!   ([`BigoNotifications::answers_offline`]) while the streamer is subscribed
//!   on the current connection and its latest real check, within
//!   [`SAFETY_CHECK`], found it offline.
//!
//! Stream ends are not pushed, so they still come from checks. While the
//! connection is down nothing is answered offline, and every streamer is
//! polled as without the setting.
//!
//! BIGO keeps one app session per account, so only this listener signs in to
//! BIGO's app service with the account; other native connections use guest
//! tickets.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use platforms_parser::extractor::error::ExtractorError;
use platforms_parser::extractor::factory::ExtractorSelection;
use platforms_parser::extractor::platforms::bigo::metadata_uid;
use platforms_parser::extractor::platforms::bigo::native::listener::{Event, Listener};
use platforms_parser::extractor::platforms::bigo::native::{NativeError, Route, ticket};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, broadcast, watch};
use tokio::time::{Instant, MissedTickBehavior, sleep_until};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::{ConfigService, ConfigUpdateEvent};
use crate::credentials::{CredentialExecutionService, CredentialValidity, OperationDeadline};
use crate::database::repositories::config::SqlxConfigRepository;
use crate::database::repositories::streamer::SqlxStreamerRepository;
use crate::proxies::ResolvedRoute;
use crate::scheduler::feedback::SchedulerFeedback;
use crate::streamer::StreamerManager;
use crate::streamer::platform_metadata::StoredPlatformMetadata;
use crate::utils::task_supervisor::TaskSupervisor;

/// The `platform_name` of the Bigo platform row.
const PLATFORM: &str = "bigo";

/// The longest a streamer goes without a real check while its offline polls
/// are answered from notifications. BIGO may delay or drop a notice; this
/// bounds how late such a start is noticed.
const SAFETY_CHECK: Duration = Duration::from_secs(10 * 60);

/// Offsets after a push at which the streamer is checked. The profile API can
/// read a room offline for a while after BIGO pushed its start, so the first
/// check alone could miss it.
const RECHECKS: [Duration; 5] = [
    Duration::ZERO,
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

/// How long a push keeps the streamer's polls real unless a check reads it
/// live first.
const PENDING_LIMIT: Duration = Duration::from_secs(2 * 60);

/// BIGO announces a start with a push that names no streamer and, in the same
/// second, one that does. The nameless one wakes every subscribed streamer
/// only when no named push comes within this window on either side of it.
const PAIR_WINDOW: Duration = Duration::from_secs(5);

/// Subscriptions go out at five per second after each connect.
const SUBSCRIBE_INTERVAL: Duration = Duration::from_millis(200);

/// Metadata saves publish no event, so the streamer set is also re-read on
/// this period to pick up newly learned uids.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// Reconnect backoff: doubling from the first to the last value.
const FIRST_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A connection that lived this long resets the backoff. Shorter-lived ones
/// keep doubling it, so rust-srec and the BIGO app signed in with the same
/// account do not evict each other every few seconds.
const STABLE_CONNECTION: Duration = Duration::from_secs(60);

/// Retry period after an answer that trying again soon would not change: BIGO
/// still sees the account as signed out after a refresh, or refuses its login.
/// A settings change, a new login included, retries at once.
const SLOW_RETRY: Duration = Duration::from_secs(15 * 60);

/// A check that finds a subscribed streamer live without a push for it this
/// recent counts as a missed push. The profile API can read a pushed start
/// offline through every real check of [`PENDING_LIMIT`], and polls are then
/// answered for up to [`SAFETY_CHECK`], so the window covers both.
const MISSED_PUSH_WINDOW: Duration =
    Duration::from_secs(PENDING_LIMIT.as_secs() + SAFETY_CHECK.as_secs());

/// Bound on refreshing a signed-out account.
const REFRESH_DEADLINE: Duration = Duration::from_secs(60);

/// The shortest gap between two status change notices, so following many
/// streamers after a connect does not make clients refetch at that rate.
const STATUS_PUBLISH_INTERVAL: Duration = Duration::from_secs(1);

/// The listener's connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListenerState {
    /// Notifications are off.
    Disabled,
    /// Connecting or reconnecting.
    Connecting,
    Connected,
    /// Not connected, and waiting before the next attempt or for a settings
    /// change; `reason` and `message` say why.
    Error,
}

/// How a Bigo streamer's start is noticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MonitoringMethod {
    Notifications,
    Polling,
}

/// Why a Bigo streamer is polled. The first five also say why the listener is
/// not connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PollingReason {
    /// The platform's `batch_detection` is off.
    NotificationsOff,
    /// No `notification_account` is chosen.
    NoAccount,
    /// The chosen account is missing, disabled or not a Bigo account.
    AccountUnusable,
    /// BIGO sees the account as signed out.
    SignedOut,
    /// The listener is connecting or waiting to reconnect.
    Disconnected,
    StreamerDisabled,
    /// A template or the streamer turned `batch_detection` off.
    BatchDetectionOff,
    /// The streamer is checked by Streamlink rather than the Bigo extractor.
    StreamlinkExtractor,
    /// No individual check has stored the streamer's uid yet.
    UidUnknown,
    /// The follow and live-notice requests are sent but not answered yet.
    Subscribing,
    /// BIGO refused the follow or the live notices.
    FollowRefused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct StreamerNotificationMethod {
    pub streamer_id: String,
    pub method: MonitoringMethod,
    /// Set exactly when `method` is `polling`.
    pub reason: Option<PollingReason>,
}

/// What `GET /api/config/platforms/bigo/notifications` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct BigoNotificationStatus {
    /// The Bigo platform configuration, absent when there is none.
    pub platform_id: Option<String>,
    pub state: ListenerState,
    /// Why the listener is not connected; absent while connected.
    pub reason: Option<PollingReason>,
    /// The last error, absent while connected.
    pub message: Option<String>,
    /// Streamers whose follow and live notices BIGO confirmed on the current
    /// connection.
    pub subscribed: usize,
    /// Enabled streamers that use notifications unless something stops them.
    pub total: usize,
    /// Every Bigo streamer, disabled ones included.
    pub streamers: Vec<StreamerNotificationMethod>,
    /// Starts that a check found on a subscribed streamer without a push from
    /// BIGO, since rust-srec started. Usually the account's own live
    /// notification setting in BIGO is off.
    pub missed_pushes: u64,
    /// When the latest of those was found.
    pub last_missed_at: Option<DateTime<Utc>>,
}

/// The listener's state as the status reports it.
#[derive(Debug, Clone)]
struct ListenerStatus {
    platform_id: Option<String>,
    state: ListenerState,
    reason: Option<PollingReason>,
    message: Option<String>,
    missed_pushes: u64,
    last_missed_at: Option<DateTime<Utc>>,
}

/// The Bigo platform's notification settings as the listener acts on them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Settings {
    /// Not read yet.
    Unknown,
    /// `batch_detection` is off, or there is no Bigo platform.
    Off,
    NoAccount,
    /// The chosen profile is missing, disabled or not a Bigo account.
    Unusable {
        id: String,
    },
    /// The account's profile id, row version and route. Any edit of the
    /// profile, a new login included, changes the version, and a proxy
    /// change the route; either starts the connection over.
    Account {
        id: String,
        version: i64,
        route: ResolvedRoute,
    },
}

/// A Bigo streamer as the last reconciliation saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    uid: Option<u32>,
    /// Why the streamer is never subscribed.
    excluded: Option<PollingReason>,
}

#[derive(Debug)]
struct Entry {
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
enum Subscription {
    Sent,
    Acknowledged(Instant),
    Refused,
}

/// Real checks a push asked for.
#[derive(Debug)]
struct Pending {
    /// Until then, polls are real checks.
    until: Instant,
    /// Times at which the scheduler is asked for a check, earliest first.
    checks: VecDeque<Instant>,
}

/// Which polls are answered offline and which checks a push asks for. Time
/// is passed in, so the rule is tested without a clock.
#[derive(Debug, Default)]
struct Rule {
    streamers: HashMap<String, Entry>,
    /// Subscriptions on the current connection, by uid.
    subscriptions: HashMap<u32, Subscription>,
    pending: HashMap<String, Pending>,
    /// When a push without a streamer wakes everyone, unless a named push
    /// comes first.
    wake_all_at: Option<Instant>,
    last_named_push: Option<Instant>,
}

impl Rule {
    fn answers_offline(&self, streamer_id: &str, now: Instant) -> bool {
        if self
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
    fn checked(&mut self, streamer_id: &str, started: Instant, live: bool) -> bool {
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
        missed
    }

    fn pushed(&mut self, uid: Option<u32>, now: Instant) {
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

    fn next_due(&self) -> Option<Instant> {
        self.pending
            .values()
            .filter_map(|pending| pending.checks.front().copied())
            .chain(self.wake_all_at)
            .min()
    }

    /// The streamers to check now.
    fn take_due(&mut self, now: Instant) -> Vec<String> {
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

    fn subscribed_ids(&self) -> Vec<String> {
        self.streamers
            .iter()
            .filter(|(_, entry)| subscribed_at(&self.subscriptions, entry).is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Replaces the streamer set, keeping what is known about streamers that
    /// stay unchanged. Returns whether a uid that needs subscribing appeared.
    fn set_streamers(&mut self, members: HashMap<String, Member>) -> bool {
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

    fn wanted_uids(&self) -> HashSet<u32> {
        self.streamers
            .values()
            .filter(|entry| entry.member.excluded.is_none())
            .filter_map(|entry| entry.member.uid)
            .collect()
    }

    /// Ends the connection's subscriptions. Every connection starts after
    /// this, so it starts with none.
    fn disconnected(&mut self) {
        self.subscriptions.clear();
        self.wake_all_at = None;
    }

    /// A wanted uid not subscribed on this connection yet, marked as sent.
    fn next_subscription(&mut self) -> Option<u32> {
        let uid = self
            .wanted_uids()
            .into_iter()
            .find(|uid| !self.subscriptions.contains_key(uid))?;
        self.subscriptions.insert(uid, Subscription::Sent);
        Some(uid)
    }

    fn has_unsent(&self) -> bool {
        self.wanted_uids()
            .iter()
            .any(|uid| !self.subscriptions.contains_key(uid))
    }

    fn subscribed(&mut self, uid: u32, accepted: bool, now: Instant) {
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
    fn methods(
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
                                None
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

/// What the listener needs from the rest of the backend.
#[derive(Clone)]
pub(crate) struct BigoNotificationDeps {
    pub config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    pub credentials: Arc<CredentialExecutionService>,
    pub feedback: Arc<SchedulerFeedback>,
}

/// Live notifications for Bigo streamers; see the module documentation.
pub struct BigoNotifications {
    rule: Mutex<Rule>,
    listener: Mutex<ListenerStatus>,
    /// A check became due sooner than the housekeeping task waits for.
    schedule: Notify,
    /// New uids to subscribe.
    subscriptions: Notify,
    /// The status may have changed.
    status_changed: Notify,
    /// Bumped when the status changed, at most every
    /// [`STATUS_PUBLISH_INTERVAL`].
    changes: watch::Sender<u64>,
}

impl Default for BigoNotifications {
    fn default() -> Self {
        Self {
            rule: Mutex::new(Rule::default()),
            listener: Mutex::new(ListenerStatus {
                platform_id: None,
                state: ListenerState::Disabled,
                reason: Some(PollingReason::NotificationsOff),
                message: None,
                missed_pushes: 0,
                last_missed_at: None,
            }),
            schedule: Notify::new(),
            subscriptions: Notify::new(),
            status_changed: Notify::new(),
            changes: watch::Sender::new(0),
        }
    }
}

impl BigoNotifications {
    /// Whether a routine offline poll of the streamer can be answered without
    /// a request.
    pub(crate) fn answers_offline(&self, streamer_id: &str) -> bool {
        self.rule
            .lock()
            .answers_offline(streamer_id, Instant::now())
    }

    /// Records a successful individual check that started at `started`.
    pub(crate) fn checked(&self, streamer_id: &str, started: Instant, live: bool) {
        if !self.rule.lock().checked(streamer_id, started, live) {
            return;
        }
        warn!(
            streamer_id,
            "A Bigo streamer went live without a notification from BIGO; check that live notifications are on in the notification account's BIGO settings"
        );
        {
            let mut listener = self.listener.lock();
            listener.missed_pushes += 1;
            listener.last_missed_at = Some(Utc::now());
        }
        self.status_changed.notify_one();
    }

    pub fn status(&self) -> BigoNotificationStatus {
        let listener = self.listener.lock().clone();
        let (streamers, subscribed, total) = self.rule.lock().methods(listener.reason);
        BigoNotificationStatus {
            platform_id: listener.platform_id,
            state: listener.state,
            reason: listener.reason,
            message: listener.message,
            subscribed,
            total,
            streamers,
            missed_pushes: listener.missed_pushes,
            last_missed_at: listener.last_missed_at,
        }
    }

    /// Wakes when [`Self::status`] may have changed.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn set_listener(
        &self,
        state: ListenerState,
        reason: Option<PollingReason>,
        message: Option<String>,
    ) {
        {
            let mut listener = self.listener.lock();
            listener.state = state;
            listener.reason = reason;
            listener.message = message;
        }
        self.status_changed.notify_one();
    }

    /// Starts the listener and its housekeeping under `supervisor`.
    pub(crate) fn spawn(
        self: &Arc<Self>,
        deps: BigoNotificationDeps,
        supervisor: &TaskSupervisor,
        cancel: CancellationToken,
    ) {
        let (settings_tx, settings_rx) = watch::channel(Settings::Unknown);
        // Either task ending stops the other.
        let cancel = cancel.child_token();
        let stop = Stop {
            notifications: self.clone(),
            cancel: cancel.clone(),
        };
        let this = self.clone();
        let housekeeping_deps = deps.clone();
        let housekeeping_cancel = cancel.clone();
        let housekeeping_stop = stop.clone();
        supervisor.spawn("bigo notification housekeeping", async move {
            let _stop = housekeeping_stop;
            this.housekeeping(housekeeping_deps, settings_tx, housekeeping_cancel)
                .await;
        });
        let this = self.clone();
        supervisor.spawn("bigo notification listener", async move {
            let _stop = stop;
            this.listen(deps, settings_rx, cancel).await;
        });
    }

    /// Keeps the streamer set and settings current, asks the scheduler for
    /// the checks pushes are due, and publishes status changes.
    async fn housekeeping(
        &self,
        deps: BigoNotificationDeps,
        settings: watch::Sender<Settings>,
        cancel: CancellationToken,
    ) {
        let mut events = deps.config_service.subscribe();
        let mut events_open = true;
        let mut reconcile = tokio::time::interval(RECONCILE_INTERVAL);
        reconcile.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut dirty = true;
        let mut published: Option<BigoNotificationStatus> = None;
        let mut publish_at: Option<Instant> = None;
        let mut last_publish: Option<Instant> = None;
        loop {
            if dirty {
                dirty = false;
                if let Some(current) = self.reconcile(&deps).await {
                    settings.send_if_modified(|settings| {
                        let changed = *settings != current;
                        *settings = current;
                        changed
                    });
                }
            }
            let due = self.rule.lock().next_due();
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                () = sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => {
                    let due = self.rule.lock().take_due(Instant::now());
                    for streamer_id in due {
                        debug!(%streamer_id, "Checking a streamer BIGO reported live");
                        deps.feedback.request_check(&streamer_id);
                    }
                }
                () = self.schedule.notified() => {}
                event = events.recv(), if events_open => match event {
                    Ok(event) => {
                        dirty = self.affects(&deps, &event);
                        // A burst of events costs one reconciliation.
                        while let Ok(event) = events.try_recv() {
                            dirty = dirty || self.affects(&deps, &event);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => dirty = true,
                    Err(broadcast::error::RecvError::Closed) => events_open = false,
                },
                _ = reconcile.tick() => dirty = true,
                () = self.status_changed.notified() => {
                    let now = Instant::now();
                    publish_at.get_or_insert_with(|| {
                        last_publish.map_or(now, |last| (last + STATUS_PUBLISH_INTERVAL).max(now))
                    });
                }
                () = sleep_until(publish_at.unwrap_or_else(Instant::now)), if publish_at.is_some() => {
                    publish_at = None;
                    last_publish = Some(Instant::now());
                    let status = self.status();
                    if published.as_ref() != Some(&status) {
                        published = Some(status);
                        self.changes.send_modify(|generation| *generation = generation.wrapping_add(1));
                    }
                }
            }
        }
    }

    /// Re-reads the Bigo platform and its streamers, and returns the settings
    /// the listener should follow. `None` keeps the current settings: a
    /// failed read must not drop a working connection.
    async fn reconcile(&self, deps: &BigoNotificationDeps) -> Option<Settings> {
        let platform = match deps
            .config_service
            .get_platform_config_by_name(PLATFORM)
            .await
        {
            Ok(platform) => platform,
            Err(crate::Error::NotFound { .. }) => {
                self.listener.lock().platform_id = None;
                self.rule.lock().set_streamers(HashMap::new());
                self.status_changed.notify_one();
                return Some(Settings::Off);
            }
            Err(error) => {
                warn!(%error, "Could not read the Bigo platform configuration");
                return None;
            }
        };
        let (enabled, account) =
            notification_settings(platform.platform_specific_config.as_deref());
        let settings = match (enabled, account) {
            (false, _) => Settings::Off,
            (true, None) => Settings::NoAccount,
            (true, Some(id)) => {
                let repository = deps.credentials.repository();
                let read = async {
                    match repository.get(&id).await {
                        Ok(profile)
                            if profile.enabled && profile.platform_config_id == platform.id =>
                        {
                            let route = repository.account_route(&profile, None).await?;
                            Ok(Some((profile.version, route)))
                        }
                        Ok(_) | Err(crate::Error::NotFound { .. }) => Ok(None),
                        Err(error) => Err(error),
                    }
                };
                match read.await {
                    Ok(Some((version, route))) => Settings::Account { id, version, route },
                    Ok(None) => Settings::Unusable { id },
                    Err(error) => {
                        warn!(profile_id = %id, %error, "Could not read the notification account");
                        return None;
                    }
                }
            }
        };

        let mut members = HashMap::new();
        for streamer in deps.streamer_manager.get_by_platform(&platform.id) {
            // Streamers can only opt out of a setting the platform turns on.
            let opted_out = if enabled {
                match deps
                    .config_service
                    .get_config_for_streamer(&streamer.id)
                    .await
                {
                    Ok(config)
                        if !super::service::batch_detection_enabled(
                            config.platform_extras.as_ref(),
                        ) =>
                    {
                        Some(PollingReason::BatchDetectionOff)
                    }
                    Ok(config) if config.extractor == ExtractorSelection::Streamlink => {
                        Some(PollingReason::StreamlinkExtractor)
                    }
                    Ok(_) => None,
                    Err(error) => {
                        debug!(streamer_id = %streamer.id, %error, "Streamer configuration not readable");
                        None
                    }
                }
            } else {
                None
            };
            let excluded =
                opted_out.or((!streamer.is_active()).then_some(PollingReason::StreamerDisabled));
            members.insert(
                streamer.id.clone(),
                Member {
                    uid: stored_uid(streamer.platform_metadata.as_deref()),
                    excluded,
                },
            );
        }
        if self.rule.lock().set_streamers(members) {
            self.subscriptions.notify_one();
        }
        self.listener.lock().platform_id = Some(platform.id);
        self.status_changed.notify_one();
        Some(settings)
    }

    /// Runs the connection the settings ask for, starting over whenever they
    /// change.
    async fn listen(
        &self,
        deps: BigoNotificationDeps,
        mut settings: watch::Receiver<Settings>,
        cancel: CancellationToken,
    ) {
        let mut backoff = FIRST_BACKOFF;
        let mut last_refresh = None;
        loop {
            let current = settings.borrow_and_update().clone();
            // How long to wait before trying again; `None` waits for a
            // settings change.
            let retry = match current {
                Settings::Unknown => None,
                Settings::Off => {
                    self.set_listener(
                        ListenerState::Disabled,
                        Some(PollingReason::NotificationsOff),
                        None,
                    );
                    None
                }
                Settings::NoAccount => {
                    self.set_listener(ListenerState::Error, Some(PollingReason::NoAccount), None);
                    None
                }
                Settings::Unusable { .. } => {
                    self.set_listener(
                        ListenerState::Error,
                        Some(PollingReason::AccountUnusable),
                        Some("The account is missing, disabled or not a Bigo account".to_owned()),
                    );
                    None
                }
                Settings::Account { id, route, .. } => {
                    let message = self.listener.lock().message.clone();
                    self.set_listener(
                        ListenerState::Connecting,
                        Some(PollingReason::Disconnected),
                        message,
                    );
                    let mut session = Session {
                        profile_id: &id,
                        route: &route,
                        connected_at: None,
                        last_refresh: &mut last_refresh,
                    };
                    let ended = tokio::select! {
                        () = cancel.cancelled() => return,
                        ended = self.session(&deps, &mut session, &mut settings) => ended,
                    };
                    self.rule.lock().disconnected();
                    let lived = session.connected_at.map(|at| at.elapsed());
                    match ended {
                        Ended::SettingsChanged => {
                            backoff = FIRST_BACKOFF;
                            continue;
                        }
                        Ended::NeedsLogin => {
                            warn!(profile_id = %id, "Bigo notification account needs a new login");
                            self.set_listener(
                                ListenerState::Error,
                                Some(PollingReason::SignedOut),
                                Some("The account needs a new login".to_owned()),
                            );
                            None
                        }
                        Ended::SignedOut(message) => {
                            warn!(profile_id = %id, %message, "Bigo notification account is signed out");
                            self.set_listener(
                                ListenerState::Error,
                                Some(PollingReason::SignedOut),
                                Some(message),
                            );
                            backoff = FIRST_BACKOFF;
                            Some(SLOW_RETRY)
                        }
                        Ended::Refused(message) => {
                            warn!(profile_id = %id, %message, retry_in = ?SLOW_RETRY, "BIGO refused the notification login");
                            self.set_listener(
                                ListenerState::Error,
                                Some(PollingReason::Disconnected),
                                Some(message),
                            );
                            backoff = FIRST_BACKOFF;
                            Some(SLOW_RETRY)
                        }
                        Ended::Failed(message) => {
                            if lived.is_some_and(|lived| lived >= STABLE_CONNECTION) {
                                backoff = FIRST_BACKOFF;
                            }
                            warn!(%message, retry_in = ?backoff, "Bigo notification connection failed");
                            self.set_listener(
                                ListenerState::Error,
                                Some(PollingReason::Disconnected),
                                Some(message),
                            );
                            let wait = backoff;
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            Some(wait)
                        }
                    }
                }
            };
            tokio::select! {
                () = cancel.cancelled() => return,
                () = settings_changed(&mut settings) => backoff = FIRST_BACKOFF,
                () = tokio::time::sleep(retry.unwrap_or_default()), if retry.is_some() => {}
            }
        }
    }

    /// One connection: sign in, subscribe, and relay events until it fails or
    /// the settings change.
    async fn session(
        &self,
        deps: &BigoNotificationDeps,
        session: &mut Session<'_>,
        settings: &mut watch::Receiver<Settings>,
    ) -> Ended {
        let profile_id = session.profile_id;
        let connect = async {
            let route = native_route(session.route)?;
            let device_id = device_id(profile_id);
            let cookies = account_cookies(deps, profile_id).await?;
            let ticket = match ticket::account(&route, &cookies, &device_id).await {
                Err(NativeError::SignedOut) => {
                    self.refresh(deps, profile_id, session.last_refresh).await?;
                    let cookies = account_cookies(deps, profile_id).await?;
                    ticket::account(&route, &cookies, &device_id)
                        .await
                        .map_err(|error| match error {
                            NativeError::SignedOut => Ended::SignedOut(
                                "BIGO still sees the account as signed out after a refresh"
                                    .to_owned(),
                            ),
                            error => Ended::Failed(error.to_string()),
                        })?
                }
                ticket => ticket.map_err(|error| Ended::Failed(error.to_string()))?,
            };
            Listener::connect(&route, &ticket)
                .await
                .map_err(|error| match error {
                    NativeError::IpRestricted | NativeError::Rejected { .. } => {
                        Ended::Refused(error.to_string())
                    }
                    error => Ended::Failed(error.to_string()),
                })
        };
        let mut listener = tokio::select! {
            biased;
            () = settings_changed(settings) => return Ended::SettingsChanged,
            listener = connect => match listener {
                Ok(listener) => listener,
                Err(ended) => return ended,
            },
        };
        info!(profile_id, "Bigo notification listener connected");
        session.connected_at = Some(Instant::now());
        self.set_listener(ListenerState::Connected, None, None);

        let mut pace = tokio::time::interval(SUBSCRIBE_INTERVAL);
        pace.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            let unsent = self.rule.lock().has_unsent();
            tokio::select! {
                biased;
                () = settings_changed(settings) => return Ended::SettingsChanged,
                event = listener.next_event() => match event {
                    Ok(Event::Live { uid }) => {
                        debug!(?uid, "BIGO reported a stream start");
                        self.rule.lock().pushed(uid, Instant::now());
                        self.schedule.notify_one();
                    }
                    Ok(Event::Subscribed { uid, result }) => {
                        if let Err(refusal) = result {
                            warn!(uid, %refusal, "BIGO refused a live-notice subscription");
                        }
                        self.rule.lock().subscribed(uid, result.is_ok(), Instant::now());
                        self.status_changed.notify_one();
                    }
                    Err(error) => return Ended::Failed(error.to_string()),
                },
                _ = pace.tick(), if unsent => {
                    let next = self.rule.lock().next_subscription();
                    if let Some(uid) = next {
                        listener.subscribe(uid);
                        self.status_changed.notify_one();
                    }
                }
                () = self.subscriptions.notified() => {}
            }
        }
    }

    /// Refreshes the login of an account BIGO sees as signed out, unless
    /// only a new login can help or the listener refreshed it within
    /// [`SLOW_RETRY`]: a refresh that changes the cookies starts the
    /// connection over, which must not refresh again at once.
    async fn refresh(
        &self,
        deps: &BigoNotificationDeps,
        profile_id: &str,
        last_refresh: &mut Option<Instant>,
    ) -> Result<(), Ended> {
        let health = deps
            .credentials
            .repository()
            .health(profile_id)
            .await
            .map_err(|error| Ended::Failed(error.to_string()))?;
        if health.is_some_and(|health| health.validity == CredentialValidity::Invalid) {
            return Err(Ended::NeedsLogin);
        }
        if last_refresh.is_some_and(|at| at.elapsed() < SLOW_RETRY) {
            return Err(Ended::SignedOut(
                "BIGO still sees the account as signed out after a refresh".to_owned(),
            ));
        }
        *last_refresh = Some(Instant::now());
        deps.credentials
            .refresh_profile(profile_id, OperationDeadline::new(REFRESH_DEADLINE))
            .await
            .map_err(|error| match error {
                // The refresh marked the account as needing a new login,
                // which the account list shows.
                crate::Error::Extractor(ExtractorError::Authentication { .. }) => Ended::NeedsLogin,
                error => Ended::Failed(format!(
                    "BIGO sees the account as signed out, and refreshing it failed: {error}"
                )),
            })?;
        Ok(())
    }

    /// Whether the config event may change the streamer set or the settings.
    /// Streamer events count only for Bigo streamers: every state change of
    /// any streamer publishes one.
    fn affects(&self, deps: &BigoNotificationDeps, event: &ConfigUpdateEvent) -> bool {
        match event {
            ConfigUpdateEvent::EngineUpdated { .. }
            | ConfigUpdateEvent::StreamerFiltersUpdated { .. }
            | ConfigUpdateEvent::StreamerErrorCleared { .. } => false,
            ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id }
            | ConfigUpdateEvent::StreamerDeleted { streamer_id }
            | ConfigUpdateEvent::StreamerStateSyncedFromDb { streamer_id, .. } => {
                if self.rule.lock().streamers.contains_key(streamer_id) {
                    return true;
                }
                let platform_id = self.listener.lock().platform_id.clone();
                platform_id.is_some_and(|platform_id| {
                    deps.streamer_manager
                        .get_streamer_snapshot(streamer_id)
                        .is_some_and(|streamer| streamer.platform_config_id == platform_id)
                })
            }
            ConfigUpdateEvent::CredentialMaterialChanged { .. }
            | ConfigUpdateEvent::GlobalUpdated
            | ConfigUpdateEvent::PlatformUpdated { .. }
            | ConfigUpdateEvent::TemplateUpdated { .. } => true,
        }
    }
}

/// What one connection works with.
struct Session<'a> {
    profile_id: &'a str,
    route: &'a ResolvedRoute,
    /// Set once Linkd accepted the login.
    connected_at: Option<Instant>,
    /// When the listener last refreshed the account, across connections.
    last_refresh: &'a mut Option<Instant>,
}

/// Why a connection ended.
enum Ended {
    SettingsChanged,
    /// BIGO sees the account as signed out, and the account is marked as
    /// needing a new login; waits for a settings change.
    NeedsLogin,
    /// BIGO sees the account as signed out after a refresh.
    SignedOut(String),
    /// BIGO refused the login: it restricts this IP address or rejected the
    /// account.
    Refused(String),
    /// Anything else; retried after the backoff.
    Failed(String),
}

/// Held by both tasks. When either ends, on shutdown or by a panic, it stops
/// the other and drops the connection's subscriptions, so every streamer is
/// polled rather than answered from a connection no task runs any more.
#[derive(Clone)]
struct Stop {
    notifications: Arc<BigoNotifications>,
    cancel: CancellationToken,
}

impl Drop for Stop {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.notifications.rule.lock().disconnected();
        // Only shutdown and a panic end the tasks, and after shutdown no one
        // reads the status.
        self.notifications.set_listener(
            ListenerState::Error,
            Some(PollingReason::Disconnected),
            Some("Live notifications stopped after an internal error; restarting rust-srec resumes them".to_owned()),
        );
    }
}

/// The account's cookies.
async fn account_cookies(deps: &BigoNotificationDeps, profile_id: &str) -> Result<String, Ended> {
    let profile = deps
        .credentials
        .repository()
        .get(profile_id)
        .await
        .map_err(|error| Ended::Failed(error.to_string()))?;
    Ok(profile
        .material()
        .map_err(|error| Ended::Failed(error.to_string()))?
        .cookies)
}

/// The native connection's route for the account's resolved one.
fn native_route(route: &ResolvedRoute) -> Result<Route, Ended> {
    let client = crate::credentials::provider_client(&route.target)
        .map_err(|error| Ended::Failed(error.to_string()))?;
    let proxy = route
        .danmu_proxy(crate::proxies::SystemProxy::current())
        .map_err(|error| Ended::Failed(error.to_string()))?;
    Ok(Route { client, proxy })
}

/// Waits for a new settings value. Never returns once the housekeeping task
/// that sends them has stopped, which only happens on shutdown.
async fn settings_changed(settings: &mut watch::Receiver<Settings>) {
    if settings.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// `batch_detection` and `notification_account` from the platform row's
/// options.
fn notification_settings(platform_specific_config: Option<&str>) -> (bool, Option<String>) {
    let Some(options) = platform_specific_config
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    else {
        return (false, None);
    };
    let enabled = super::service::batch_detection_enabled(Some(&options));
    let account = options
        .get("notification_account")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    (enabled, account)
}

/// The uid a Bigo check stored, however old: a broadcaster's uid does not
/// change, and a wrong one only costs a follow.
fn stored_uid(raw: Option<&str>) -> Option<u32> {
    let stored: StoredPlatformMetadata = serde_json::from_str(raw?).ok()?;
    metadata_uid(&stored.metadata)
}

/// A device id in the website's form, derived from the profile id so it stays
/// the same across connections without being stored.
fn device_id(profile_id: &str) -> String {
    Sha256::digest(profile_id.as_bytes())[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(uid: u32) -> Member {
        Member {
            uid: Some(uid),
            excluded: None,
        }
    }

    /// A rule with streamers `a` (uid 1) and `b` (uid 2), both subscribed
    /// and checked at `now`, so their offline polls are answered.
    fn subscribed(now: Instant) -> Rule {
        let mut rule = Rule::default();
        rule.set_streamers(HashMap::from([
            ("a".to_owned(), member(1)),
            ("b".to_owned(), member(2)),
        ]));
        while let Some(uid) = rule.next_subscription() {
            rule.subscribed(uid, true, now);
        }
        rule.checked("a", now, false);
        rule.checked("b", now, false);
        rule
    }

    fn due(rule: &mut Rule, now: Instant) -> Vec<String> {
        let mut ids = rule.take_due(now);
        ids.sort();
        ids
    }

    #[test]
    fn offline_polls_are_answered_only_while_subscribed_after_a_real_check() {
        let start = Instant::now();
        let mut rule = Rule::default();
        rule.set_streamers(HashMap::from([("a".to_owned(), member(1))]));
        rule.checked("a", start, false);
        assert!(!rule.answers_offline("a", start), "not subscribed");

        assert_eq!(rule.next_subscription(), Some(1));
        let acked = start + Duration::from_secs(1);
        rule.subscribed(1, true, acked);
        // The check before the subscription may have missed a start.
        assert!(!rule.answers_offline("a", acked));

        let checked = acked + Duration::from_secs(1);
        rule.checked("a", checked, false);
        assert!(rule.answers_offline("a", checked));
        assert!(
            !rule.answers_offline("a", checked + SAFETY_CHECK),
            "safety check due"
        );

        rule.disconnected();
        assert!(!rule.answers_offline("a", checked));
        rule.next_subscription();
        let resubscribed = checked + Duration::from_secs(1);
        rule.subscribed(1, true, resubscribed);
        assert!(!rule.answers_offline("a", resubscribed), "reconnected");
        assert!(
            !rule.checked("a", resubscribed, true),
            "the start may predate the subscription, so it is no missed push"
        );
    }

    #[test]
    fn a_streamer_that_was_excluded_is_answered_only_after_a_new_check() {
        let start = Instant::now();
        let mut rule = subscribed(start);
        let streamers = |a| HashMap::from([("a".to_owned(), a), ("b".to_owned(), member(2))]);
        rule.set_streamers(streamers(Member {
            uid: Some(1),
            excluded: Some(PollingReason::StreamerDisabled),
        }));
        rule.set_streamers(streamers(member(1)));
        // A start while it was excluded got no follow-up checks.
        assert!(!rule.answers_offline("a", start));
        assert!(rule.answers_offline("b", start), "b did not change");
        let checked = start + Duration::from_secs(1);
        rule.checked("a", checked, false);
        assert!(rule.answers_offline("a", checked));
    }

    #[test]
    fn a_push_keeps_checks_real_until_one_reads_live_or_the_limit_passes() {
        let start = Instant::now();
        let mut rule = subscribed(start);
        let push = start + Duration::from_secs(10);
        rule.pushed(Some(1), push);
        assert!(!rule.answers_offline("a", push));
        assert!(rule.answers_offline("b", push));

        let mut asked = Vec::new();
        while let Some(at) = rule.next_due() {
            for id in rule.take_due(at) {
                asked.push((id, at - push));
            }
            // A check that still reads offline does not end the push.
            rule.checked("a", at, false);
        }
        let offsets: Vec<_> = RECHECKS
            .iter()
            .map(|offset| ("a".to_owned(), *offset))
            .collect();
        assert_eq!(asked, offsets);
        assert!(!rule.answers_offline("a", push + Duration::from_secs(90)));
        assert!(rule.answers_offline("a", push + PENDING_LIMIT));

        let again = push + PENDING_LIMIT;
        rule.pushed(Some(1), again);
        assert!(!rule.checked("a", again, true), "pushed");
        assert_eq!(rule.next_due(), None, "a live check ends the follow-ups");
        assert!(rule.checked("b", again, true), "b went live without a push");
        assert!(!rule.checked("b", again, true), "b was live already");
        // Ends are not pushed, so polls stay real until one reads offline.
        assert!(!rule.answers_offline("a", again));
        let ended = again + Duration::from_secs(1);
        rule.checked("a", ended, false);
        assert!(rule.answers_offline("a", ended));
    }

    #[test]
    fn a_push_without_a_streamer_wakes_everyone_only_when_unpaired() {
        let start = Instant::now();
        let seconds = |n| start + Duration::from_secs(n);

        // BIGO's pair: the nameless push, then the named one.
        let mut rule = subscribed(start);
        rule.pushed(None, start);
        rule.pushed(Some(1), seconds(1));
        assert_eq!(due(&mut rule, start + PAIR_WINDOW), ["a"]);
        // The other order wakes no one either: only a's follow-ups remain.
        rule.pushed(None, seconds(2));
        assert_eq!(rule.next_due(), Some(seconds(6)));

        let mut rule = subscribed(start);
        rule.pushed(None, start);
        assert_eq!(due(&mut rule, seconds(4)), Vec::<String>::new());
        assert_eq!(due(&mut rule, start + PAIR_WINDOW), ["a", "b"]);
        assert!(!rule.answers_offline("b", start + PAIR_WINDOW));
        // Streamers an earlier push still holds are not woken again.
        rule.pushed(None, seconds(10));
        assert_eq!(due(&mut rule, seconds(20)), Vec::<String>::new());
        assert!(
            !rule.checked("b", seconds(20), true),
            "a nameless push may have been b's"
        );
    }
}
