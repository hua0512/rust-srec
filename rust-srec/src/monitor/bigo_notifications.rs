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

use parking_lot::Mutex;
use platforms_parser::extractor::error::ExtractorError;
use platforms_parser::extractor::factory::ExtractorSelection;
use platforms_parser::extractor::platforms::bigo::native::listener::{Event, Listener};
use platforms_parser::extractor::platforms::bigo::native::{NativeError, Route, ticket};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, broadcast, watch};
use tokio::time::{Instant, MissedTickBehavior, sleep_until};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::{ConfigService, ConfigUpdateEvent};
use crate::credentials::{CredentialExecutionService, OperationDeadline};
use crate::database::repositories::config::SqlxConfigRepository;
use crate::database::repositories::streamer::SqlxStreamerRepository;
use crate::scheduler::feedback::SchedulerFeedback;
use crate::streamer::StreamerManager;
use crate::streamer::platform_metadata::StoredPlatformMetadata;
use crate::utils::task_supervisor::TaskSupervisor;

/// The `platform_name` of the Bigo platform row, and the platform of the
/// metadata its checks store.
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

/// Retry period while BIGO sees the account as signed out. A new login or any
/// other edit of the profile retries at once.
const SIGNED_OUT_RETRY: Duration = Duration::from_secs(15 * 60);

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
}

/// The listener's state as the status reports it.
#[derive(Debug, Clone)]
struct ListenerStatus {
    platform_id: Option<String>,
    state: ListenerState,
    reason: Option<PollingReason>,
    message: Option<String>,
}

/// The Bigo platform's notification settings as the listener acts on them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Settings {
    /// Not read yet.
    Unknown,
    /// `batch_detection` is off, or there is no Bigo platform.
    Off,
    NoAccount,
    /// The account's profile id and row version, or no version when the
    /// profile is missing, disabled or not a Bigo account. Any edit of the
    /// profile, a new login included, changes the version and starts the
    /// connection over.
    Account {
        id: String,
        version: Option<i64>,
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
    connected: bool,
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
        // Subscriptions belong to the current connection, so none is
        // acknowledged while disconnected.
        let Some(Subscription::Acknowledged(subscribed_at)) = entry
            .member
            .uid
            .filter(|_| entry.member.excluded.is_none())
            .and_then(|uid| self.subscriptions.get(&uid))
        else {
            return false;
        };
        // A check from before the subscription may have missed a start that
        // happened while the connection was down.
        !entry.last_live
            && entry.last_check.is_some_and(|checked| {
                checked >= *subscribed_at && now.saturating_duration_since(checked) < SAFETY_CHECK
            })
    }

    fn checked(&mut self, streamer_id: &str, started: Instant, live: bool) {
        if let Some(entry) = self.streamers.get_mut(streamer_id)
            && entry.last_check.is_none_or(|last| started >= last)
        {
            entry.last_check = Some(started);
            entry.last_live = live;
        }
        if live {
            self.pending.remove(streamer_id);
        }
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
        for (id, entry) in &self.streamers {
            if entry.member.uid == Some(uid) && entry.member.excluded.is_none() {
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
            .filter(|(_, entry)| {
                entry.member.excluded.is_none()
                    && entry.member.uid.is_some_and(|uid| {
                        matches!(
                            self.subscriptions.get(&uid),
                            Some(Subscription::Acknowledged(_))
                        )
                    })
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Replaces the streamer set, keeping what is known about streamers that
    /// stay. Returns whether a uid that needs subscribing appeared.
    fn set_streamers(&mut self, members: HashMap<String, Member>) -> bool {
        let before = self.wanted_uids();
        self.streamers.retain(|id, _| members.contains_key(id));
        self.pending.retain(|id, _| members.contains_key(id));
        for (id, member) in members {
            self.streamers
                .entry(id)
                .and_modify(|entry| entry.member = member.clone())
                .or_insert(Entry {
                    member,
                    last_check: None,
                    last_live: false,
                });
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

    fn connected(&mut self) {
        self.connected = true;
        self.subscriptions.clear();
    }

    fn disconnected(&mut self) {
        self.connected = false;
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
        self.connected
            && self
                .wanted_uids()
                .iter()
                .any(|uid| !self.subscriptions.contains_key(uid))
    }

    fn subscribed(&mut self, uid: u32, accepted: bool, now: Instant) {
        if self.connected {
            self.subscriptions.insert(
                uid,
                if accepted {
                    Subscription::Acknowledged(now)
                } else {
                    Subscription::Refused
                },
            );
        }
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
        self.rule.lock().checked(streamer_id, started, live);
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
        let this = self.clone();
        let housekeeping_deps = deps.clone();
        let housekeeping_cancel = cancel.clone();
        supervisor.spawn("bigo notification housekeeping", async move {
            this.housekeeping(housekeeping_deps, settings_tx, housekeeping_cancel)
                .await;
        });
        let this = self.clone();
        supervisor.spawn("bigo notification listener", async move {
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
                        dirty = affects_notifications(&event);
                        // A burst of events costs one reconciliation.
                        while let Ok(event) = events.try_recv() {
                            dirty |= affects_notifications(&event);
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
                let version = match deps.credentials.repository().get(&id).await {
                    Ok(profile) if profile.enabled && profile.platform_config_id == platform.id => {
                        Some(profile.version)
                    }
                    Ok(_) | Err(crate::Error::NotFound { .. }) => None,
                    Err(error) => {
                        warn!(profile_id = %id, %error, "Could not read the notification account");
                        return None;
                    }
                };
                Settings::Account { id, version }
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
                Settings::Account { version: None, .. } => {
                    self.set_listener(
                        ListenerState::Error,
                        Some(PollingReason::AccountUnusable),
                        Some("The account is missing, disabled or not a Bigo account".to_owned()),
                    );
                    None
                }
                Settings::Account {
                    id,
                    version: Some(_),
                } => {
                    let message = self.listener.lock().message.clone();
                    self.set_listener(
                        ListenerState::Connecting,
                        Some(PollingReason::Disconnected),
                        message,
                    );
                    let mut connected_at = None;
                    let ended = tokio::select! {
                        () = cancel.cancelled() => return,
                        ended = self.session(&deps, &id, &mut settings, &mut connected_at) => ended,
                    };
                    self.rule.lock().disconnected();
                    let lived = connected_at.map(|at: Instant| at.elapsed());
                    match ended {
                        Ended::SettingsChanged => {
                            backoff = FIRST_BACKOFF;
                            continue;
                        }
                        Ended::Unusable(message) => {
                            self.set_listener(
                                ListenerState::Error,
                                Some(PollingReason::AccountUnusable),
                                Some(message),
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
                            Some(SIGNED_OUT_RETRY)
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

    /// One connection with the account `profile_id`: sign in, subscribe, and
    /// relay events until it fails or the settings change. `connected_at` is
    /// set once Linkd accepted the login.
    async fn session(
        &self,
        deps: &BigoNotificationDeps,
        profile_id: &str,
        settings: &mut watch::Receiver<Settings>,
        connected_at: &mut Option<Instant>,
    ) -> Ended {
        let connect = async {
            let (route, cookies) = self.account(deps, profile_id).await?;
            let device_id = device_id(profile_id);
            let ticket = match ticket::account(&route, &cookies, &device_id).await {
                Err(NativeError::SignedOut) => {
                    // The website may only need the login refreshed. A
                    // refresh that finds it signed out marks the account as
                    // needing a new login, which the account list shows.
                    deps.credentials
                        .refresh_profile(profile_id, OperationDeadline::new(REFRESH_DEADLINE))
                        .await
                        .map_err(|error| match error {
                            crate::Error::Extractor(ExtractorError::Authentication { .. }) => {
                                Ended::SignedOut(error.to_string())
                            }
                            error => Ended::Failed(format!(
                                "BIGO sees the account as signed out, and refreshing it failed: {error}"
                            )),
                        })?;
                    let (route, cookies) = self.account(deps, profile_id).await?;
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
                .map_err(|error| Ended::Failed(error.to_string()))
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
        *connected_at = Some(Instant::now());
        self.rule.lock().connected();
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

    /// The account's native route and cookies, checked to be usable.
    async fn account(
        &self,
        deps: &BigoNotificationDeps,
        profile_id: &str,
    ) -> Result<(Route, String), Ended> {
        let repository = deps.credentials.repository();
        let profile = repository
            .get(profile_id)
            .await
            .map_err(|error| Ended::Failed(error.to_string()))?;
        let platform_id = self.listener.lock().platform_id.clone();
        if !profile.enabled || platform_id.as_deref() != Some(profile.platform_config_id.as_str()) {
            return Err(Ended::Unusable(
                "The account is disabled or not a Bigo account".to_owned(),
            ));
        }
        let cookies = profile
            .material()
            .map_err(|error| Ended::Failed(error.to_string()))?
            .cookies;
        let route = repository
            .account_route(&profile, None)
            .await
            .map_err(|error| Ended::Failed(error.to_string()))?;
        let client = crate::credentials::provider_client(&route.target)
            .map_err(|error| Ended::Failed(error.to_string()))?;
        let proxy = route
            .danmu_proxy(crate::proxies::SystemProxy::current())
            .map_err(|error| Ended::Failed(error.to_string()))?;
        Ok((Route { client, proxy }, cookies))
    }
}

/// Why a connection ended.
enum Ended {
    SettingsChanged,
    /// The account cannot be used until its settings change.
    Unusable(String),
    /// BIGO sees the account as signed out, even after a refresh.
    SignedOut(String),
    /// Anything else; retried after the backoff.
    Failed(String),
}

/// Waits for a new settings value. Never returns once the housekeeping task
/// that sends them has stopped, which only happens on shutdown.
async fn settings_changed(settings: &mut watch::Receiver<Settings>) {
    if settings.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Config events after which the streamer set or the settings may differ.
fn affects_notifications(event: &ConfigUpdateEvent) -> bool {
    !matches!(
        event,
        ConfigUpdateEvent::GlobalUpdated
            | ConfigUpdateEvent::EngineUpdated { .. }
            | ConfigUpdateEvent::StreamerFiltersUpdated { .. }
            | ConfigUpdateEvent::StreamerErrorCleared { .. }
    )
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
    let metadata = stored.metadata;
    if metadata.platform != PLATFORM || metadata.schema_version != 1 {
        return None;
    }
    metadata
        .data
        .get("uid")?
        .as_str()?
        .parse()
        .ok()
        .filter(|&uid| uid != 0)
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

    /// A connected rule with streamers `a` (uid 1) and `b` (uid 2), both
    /// subscribed and checked at `now`, so their offline polls are answered.
    fn subscribed(now: Instant) -> Rule {
        let mut rule = Rule::default();
        rule.set_streamers(HashMap::from([
            ("a".to_owned(), member(1)),
            ("b".to_owned(), member(2)),
        ]));
        rule.connected();
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
        assert!(!rule.answers_offline("a", start), "disconnected");

        rule.connected();
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
        rule.connected();
        rule.next_subscription();
        rule.subscribed(1, true, checked + Duration::from_secs(1));
        assert!(
            !rule.answers_offline("a", checked + Duration::from_secs(1)),
            "reconnected"
        );
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
        rule.checked("a", again, true);
        assert_eq!(rule.next_due(), None, "a live check ends the follow-ups");
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
    }
}
