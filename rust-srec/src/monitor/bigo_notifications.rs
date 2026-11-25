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
//! polled as without the setting. The same holds while BIGO connects and
//! confirms the subscriptions but sends no pushes: after
//! [`NOT_ARRIVING_AFTER`] starts in a row that real checks found without a
//! push, polls are real until the next push of any kind arrives.
//!
//! The notification channels hear once when pushes stop arriving and once
//! when the listener stays disconnected for [`DOWN_ALERT_AFTER`], and once
//! again when either recovers.
//!
//! BIGO keeps one app session per account, so only this listener signs in to
//! BIGO's app service with the account; other native connections use guest
//! tickets.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use jiff::Timestamp;
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
use crate::domain::notification::{BigoNotificationProblem, RuntimeAlert, RuntimeAlertSink};
use crate::proxies::ResolvedRoute;
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

/// The backoff's cap while BIGO refuses the login. A refusal that follows an
/// address change can pass within minutes, so it is retried well before
/// [`SLOW_RETRY`], but a lasting one at most this often.
const MAX_REFUSED_BACKOFF: Duration = Duration::from_secs(5 * 60);

/// A connection that lived this long resets the backoff and ends an outage.
/// Shorter-lived ones keep doubling the backoff, so rust-srec and the BIGO app
/// signed in with the same account do not evict each other every few seconds,
/// and keep the outage running, so such a cycle still alerts.
const STABLE_CONNECTION: Duration = Duration::from_secs(60);

/// Retry period after an answer that trying again soon would not change: BIGO
/// still sees the account as signed out after a refresh. A settings change, a
/// new login included, or a manual reconnect retries at once.
const SLOW_RETRY: Duration = Duration::from_secs(15 * 60);

/// A check that finds a subscribed streamer live without a push for it this
/// recent counts as a missed push. The profile API can read a pushed start
/// offline through every real check of [`PENDING_LIMIT`], and polls are then
/// answered for up to [`SAFETY_CHECK`], so the window covers both.
const MISSED_PUSH_WINDOW: Duration =
    Duration::from_secs(PENDING_LIMIT.as_secs() + SAFETY_CHECK.as_secs());

/// Starts in a row that real checks found on subscribed streamers without a
/// push, after which pushes count as not arriving. BIGO may drop a single
/// notice; several in a row with none in between mean the account receives
/// none, usually because it is also signed in to the BIGO app or its live
/// notifications are off.
const NOT_ARRIVING_AFTER: u32 = 3;

/// How long the listener may stay disconnected before the notification
/// channels hear of it. Reconnecting after a dropped connection takes
/// seconds, so only an outage that the backoff or [`SLOW_RETRY`] keeps going
/// alerts. The alert's text in `locales/` and the status's `down_since`
/// documentation name this duration.
const DOWN_ALERT_AFTER: Duration = Duration::from_secs(5 * 60);

/// Bound on refreshing a signed-out account.
const REFRESH_DEADLINE: Duration = Duration::from_secs(60);

/// The shortest gap between two status change notices, so following many
/// streamers after a connect does not make clients refetch at that rate.
const STATUS_PUBLISH_INTERVAL: Duration = Duration::from_secs(1);

/// The listener's connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitoringMethod {
    Notifications,
    Polling,
}

/// Why a Bigo streamer is polled. The first five also say why the listener is
/// not connected; `NotificationsNotArriving` why a connected one's streamers
/// are polled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
    /// BIGO confirmed the subscription but sent no push for the latest
    /// starts that checks found; polls are real until a push arrives.
    NotificationsNotArriving,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamerNotificationMethod {
    pub streamer_id: String,
    pub method: MonitoringMethod,
    /// Set exactly when `method` is `polling`.
    pub reason: Option<PollingReason>,
}

/// What `GET /api/config/platforms/bigo/notifications` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BigoNotificationStatus {
    /// The Bigo platform configuration, absent when there is none.
    pub platform_id: Option<String>,
    pub state: ListenerState,
    /// Why the listener is not connected, or `notifications_not_arriving`
    /// while it is connected but BIGO sends no pushes; absent otherwise.
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
    pub last_missed_at: Option<Timestamp>,
    /// When the listener lost its connection, or began connecting, once it
    /// has been disconnected for more than 5 minutes; absent before then and
    /// while connected.
    pub down_since: Option<Timestamp>,
}

/// The listener's state as the status reports it.
#[derive(Debug, Clone)]
struct ListenerStatus {
    platform_id: Option<String>,
    state: ListenerState,
    reason: Option<PollingReason>,
    message: Option<String>,
    missed_pushes: u64,
    last_missed_at: Option<Timestamp>,
    /// Set while the settings name an account and the listener is not
    /// connected.
    outage: Option<Outage>,
}

/// A stretch of time an account's listener spends disconnected.
#[derive(Debug, Clone, Copy)]
struct Outage {
    started: Instant,
    since: Timestamp,
    /// Whether the notification channels heard of it.
    alerted: bool,
}

impl ListenerStatus {
    /// Starts an outage unless one is running.
    fn down(&mut self, now: Instant) {
        self.outage.get_or_insert(Outage {
            started: now,
            since: Timestamp::now(),
            alerted: false,
        });
    }

    /// When the running outage is due an alert, unless it had one.
    fn alert_at(&self) -> Option<Instant> {
        self.outage
            .filter(|outage| !outage.alerted)
            .map(|outage| outage.started + DOWN_ALERT_AFTER)
    }

    /// Marks the running outage as alerted if it lasted [`DOWN_ALERT_AFTER`]
    /// and had no alert yet. Returns whether it did.
    fn take_alert(&mut self, now: Instant) -> bool {
        match &mut self.outage {
            Some(outage)
                if !outage.alerted
                    && now.saturating_duration_since(outage.started) >= DOWN_ALERT_AFTER =>
            {
                outage.alerted = true;
                true
            }
            _ => false,
        }
    }

    /// Ends the running outage. Returns whether it had an alert, which its
    /// end then answers.
    fn up(&mut self) -> bool {
        self.outage.take().is_some_and(|outage| outage.alerted)
    }
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

/// Callback used by [`BigoNotifications`] to request an immediate check for a
/// streamer when a live push arrives.
pub(crate) trait BigoCheckTrigger: Send + Sync {
    fn request_check(&self, streamer_id: &str);
}

/// What the listener needs from the rest of the backend.
#[derive(Clone)]
pub(crate) struct BigoNotificationDeps {
    pub config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    pub credentials: Arc<CredentialExecutionService>,
    pub feedback: Arc<dyn BigoCheckTrigger>,
    pub notifications: Arc<dyn RuntimeAlertSink>,
}

/// Live notifications for Bigo streamers; see the module documentation.
pub struct BigoNotifications {
    rule: Mutex<Rule>,
    listener: Mutex<ListenerStatus>,
    /// A check became due sooner than the housekeeping task waits for.
    schedule: Notify,
    /// New uids to subscribe.
    subscriptions: Notify,
    /// A manual request to stop waiting between connection attempts. Only a
    /// listener waiting at that moment hears it.
    reconnect: Notify,
    /// The status may have changed.
    status_changed: Notify,
    /// Bumped when the status changed, at most every
    /// [`STATUS_PUBLISH_INTERVAL`].
    changes: watch::Sender<u64>,
    /// Where alerts go; set when the listener starts.
    alerts: OnceLock<Arc<dyn RuntimeAlertSink>>,
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
                outage: None,
            }),
            schedule: Notify::new(),
            subscriptions: Notify::new(),
            reconnect: Notify::new(),
            status_changed: Notify::new(),
            changes: watch::Sender::new(0),
            alerts: OnceLock::new(),
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
        let (missed, was_arriving, arriving) = {
            let mut rule = self.rule.lock();
            let was_arriving = !rule.not_arriving();
            let missed = rule.checked(streamer_id, started, live);
            (missed, was_arriving, !rule.not_arriving())
        };
        if !missed {
            return;
        }
        if was_arriving && !arriving {
            warn!(
                streamer_id,
                missed = NOT_ARRIVING_AFTER,
                "BIGO sent no live notification for the latest Bigo stream starts, so Bigo streamers are polled until one arrives; check that the notification account is not signed in to the BIGO app and that its live notifications are on"
            );
            self.alert(RuntimeAlert::BigoNotificationsFailing {
                problem: BigoNotificationProblem::NotArriving,
                message: None,
                timestamp: Timestamp::now(),
            });
        } else if arriving {
            warn!(
                streamer_id,
                "A Bigo streamer went live without a notification from BIGO; check that live notifications are on in the notification account's BIGO settings"
            );
        } else {
            // Reported once, when polling took over.
            debug!(
                streamer_id,
                "A Bigo streamer went live without a notification from BIGO"
            );
        }
        {
            let mut listener = self.listener.lock();
            listener.missed_pushes += 1;
            listener.last_missed_at = Some(Timestamp::now());
        }
        self.status_changed.notify_one();
    }

    pub fn status(&self) -> BigoNotificationStatus {
        let listener = self.listener.lock().clone();
        let (streamers, subscribed, total, not_arriving) = {
            let rule = self.rule.lock();
            let (streamers, subscribed, total) = rule.methods(listener.reason);
            (streamers, subscribed, total, rule.not_arriving())
        };
        BigoNotificationStatus {
            platform_id: listener.platform_id,
            state: listener.state,
            reason: listener
                .reason
                .or((listener.state == ListenerState::Connected && not_arriving)
                    .then_some(PollingReason::NotificationsNotArriving)),
            message: listener.message,
            subscribed,
            total,
            streamers,
            missed_pushes: listener.missed_pushes,
            last_missed_at: listener.last_missed_at,
            // A connection younger than STABLE_CONNECTION keeps the outage
            // running but is not reported as down.
            down_since: listener
                .outage
                .filter(|outage| outage.alerted && listener.state != ListenerState::Connected)
                .map(|outage| outage.since),
        }
    }

    /// Sends `alert` to the runtime alert sink.
    fn alert(&self, alert: RuntimeAlert) {
        if let Some(sink) = self.alerts.get() {
            sink.clone().emit(alert);
        }
    }

    /// Alerts about the running outage once it lasted [`DOWN_ALERT_AFTER`].
    fn alert_outage(&self) {
        let message = {
            let mut listener = self.listener.lock();
            if !listener.take_alert(Instant::now()) {
                return;
            }
            listener.message.clone()
        };
        warn!(
            after = ?DOWN_ALERT_AFTER,
            message = message.as_deref().unwrap_or_default(),
            "Bigo live notifications have been disconnected for a while; Bigo streamers are polled meanwhile"
        );
        self.alert(RuntimeAlert::BigoNotificationsFailing {
            problem: BigoNotificationProblem::Disconnected,
            message,
            timestamp: Timestamp::now(),
        });
        self.status_changed.notify_one();
    }

    /// Ends the running outage, if any. Only an alerted outage that ends
    /// because a connection lasted [`STABLE_CONNECTION`], rather than because
    /// the settings turned notifications off, sends an alert that it ended.
    fn end_outage(&self, connected: bool) {
        let alerted = self.listener.lock().up();
        if alerted && connected {
            info!("Bigo live notifications reconnected");
            self.alert(RuntimeAlert::BigoNotificationsRestored {
                problem: BigoNotificationProblem::Disconnected,
                timestamp: Timestamp::now(),
            });
        }
        self.status_changed.notify_one();
    }

    /// Makes a listener that waits between connection attempts try again
    /// now, with the backoff reset. A connected or connecting one is left as
    /// it is.
    pub fn reconnect(&self) {
        self.reconnect.notify_waiters();
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
        self.alerts.get_or_init(|| deps.notifications.clone());
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
            let alert_at = self.listener.lock().alert_at();
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                () = sleep_until(alert_at.unwrap_or_else(Instant::now)), if alert_at.is_some() => {
                    self.alert_outage();
                }
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
                        if !super::batch_detection_enabled(config.platform_extras.as_ref()) =>
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
        let mut account = None;
        loop {
            let current = settings.borrow_and_update().clone();
            // Whether pushes arrive is a property of the account, so only
            // choosing another one, or none, starts its count over.
            let current_account = match &current {
                Settings::Account { id, .. } | Settings::Unusable { id } => Some(id.clone()),
                Settings::Unknown | Settings::Off | Settings::NoAccount => None,
            };
            if current_account != account {
                account = current_account;
                self.rule.lock().clear_missed();
            }
            if !matches!(current, Settings::Account { .. } | Settings::Unknown) {
                self.end_outage(false);
            }
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
                    let message = {
                        let mut listener = self.listener.lock();
                        listener.down(Instant::now());
                        listener.message.clone()
                    };
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
                    // The outage starts when the connection is lost, not
                    // after the wait before the next attempt.
                    self.listener.lock().down(Instant::now());
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
                            let wait = backoff;
                            backoff = (backoff * 2).min(MAX_REFUSED_BACKOFF);
                            warn!(profile_id = %id, %message, retry_in = ?wait, "BIGO refused the notification login");
                            self.set_listener(
                                ListenerState::Error,
                                Some(PollingReason::Disconnected),
                                Some(message),
                            );
                            Some(wait)
                        }
                        Ended::Failed(message) => {
                            if lived.is_some_and(|lived| lived >= STABLE_CONNECTION) {
                                backoff = FIRST_BACKOFF;
                            }
                            // A refusal before may have raised the backoff
                            // past this cap.
                            let wait = backoff.min(MAX_BACKOFF);
                            backoff = (wait * 2).min(MAX_BACKOFF);
                            warn!(%message, retry_in = ?wait, "Bigo notification connection failed");
                            self.set_listener(
                                ListenerState::Error,
                                Some(PollingReason::Disconnected),
                                Some(message),
                            );
                            Some(wait)
                        }
                    }
                }
            };
            tokio::select! {
                () = cancel.cancelled() => return,
                () = settings_changed(&mut settings) => backoff = FIRST_BACKOFF,
                () = self.reconnect.notified() => backoff = FIRST_BACKOFF,
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
                    // BIGO allows one app session per account, so for an
                    // account the refusal can also come from a sign-in
                    // elsewhere.
                    NativeError::IpRestricted => Ended::Refused(format!(
                        "{error}, or the account being signed in elsewhere, such as in the BIGO app"
                    )),
                    NativeError::Rejected { .. } => Ended::Refused(error.to_string()),
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

        let stable_at = Instant::now() + STABLE_CONNECTION;
        let mut stable = false;
        let mut pace = tokio::time::interval(SUBSCRIBE_INTERVAL);
        pace.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            let unsent = self.rule.lock().has_unsent();
            tokio::select! {
                biased;
                () = settings_changed(settings) => return Ended::SettingsChanged,
                () = sleep_until(stable_at), if !stable => {
                    stable = true;
                    self.end_outage(true);
                }
                event = listener.next_event() => match event {
                    Ok(Event::Live { uid }) => {
                        debug!(?uid, "BIGO reported a stream start");
                        let restored = {
                            let mut rule = self.rule.lock();
                            let not_arriving = rule.not_arriving();
                            rule.pushed(uid, Instant::now());
                            not_arriving
                        };
                        self.schedule.notify_one();
                        if restored {
                            info!("BIGO sends live notifications again, so Bigo streamers use them again");
                            self.alert(RuntimeAlert::BigoNotificationsRestored {
                                problem: BigoNotificationProblem::NotArriving,
                                timestamp: Timestamp::now(),
                            });
                            self.status_changed.notify_one();
                        }
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
    /// BIGO refused the login, as it does after an address change or a
    /// sign-in elsewhere, or rejected the account.
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
    let proxy =
        crate::proxies::danmu_proxy_for_route(route, crate::proxies::SystemProxy::current())
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
    let enabled = super::batch_detection_enabled(Some(&options));
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

mod rule;
#[cfg(test)]
mod tests;
use rule::*;
