//! Stateless platform extraction runner and stateful recorder prober.
//!
//! [`PlatformExtractorRunner`] owns the shared HTTP client cache and proxy/tunnel
//! extractor factory setup so both [`RecorderProber`] and `/api/parse` reuse the
//! same connection pools and proxy rules. [`RecorderProber`] adds in-flight
//! deduplication keyed by `CredentialCheckKey`, batch precheck, Bigo live
//! notification accounting, credential block tracking, CAS platform metadata
//! persistence, and non-blocking check-history recording.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use jiff::{SignedDuration, Timestamp};
use platforms_parser::extractor::factory::{
    ExtractorFactory, ExtractorSelection, batch_probe_for_url, platform_name_for_url,
};
use tokio::sync::{OnceCell, watch};
use tracing::{debug, trace, warn};

use crate::config::ConfigService;
use crate::credentials::{
    CredentialBinding, CredentialBlocks, CredentialExecutionService, CredentialValidity,
    OperationDeadline, PlatformAdmission, ProfileError, ResolvedCredentialPolicy,
    UnavailableReason,
};
use crate::monitor::{
    BatchAnswer, BatchPrecheck, BigoNotifications, CheckContext, CheckHistoryWriter,
    DetectionResult, FilterReason, GroupKey, LiveStatus, Poll, StreamDetector,
    batch_detection_enabled,
};
use crate::proxies::ProxyTarget;
use crate::recorder::ports::{
    LiveProbePayload, ProbeFailure, ProbePurpose, ProbeRequest, ProbeStatus,
};
use crate::recorder::store::RecorderStore;
use crate::streamer::platform_metadata::StoredPlatformMetadata;
use crate::streamer::{StreamerManager, StreamerMetadata};
use crate::{Error, Result};
use srec_core::CoreError;
use srec_core::domain::StreamerState;
use srec_core::domain::filter::Filter;
use srec_core::domain::streamer::{
    CheckOutcome, CheckRecord, CheckSource, FilterCause, SelectedStreamSummary,
};
use srec_sqlite::filter_store::FilterStore;
use srec_sqlite::repositories::config::SqlxConfigRepository;
use srec_sqlite::repositories::streamer::SqlxStreamerRepository;

/// Hard upper bound for a single streamer status probe to prevent stuck
/// in-flight deduplication entries when upstream requests hang.
const STREAM_CHECK_HARD_TIMEOUT: Duration = Duration::from_secs(300);

/// Upper bound for one batch status request.
const BATCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum time a status probe waits to persist newly observed platform metadata.
const PLATFORM_METADATA_SAVE_TIMEOUT: Duration = Duration::from_secs(2);

/// Stateless shared extractor runner owning the HTTP client pool and proxy/tunnel
/// factory setup for both background probing and `/api/parse` playback requests.
pub struct PlatformExtractorRunner {
    detector: StreamDetector,
}

impl Default for PlatformExtractorRunner {
    fn default() -> Self {
        Self::new(Duration::ZERO, 0)
    }
}

impl PlatformExtractorRunner {
    /// Construct a runner with the specified per-request timeout and idle pool size.
    pub fn new(request_timeout: Duration, pool_max_idle_per_host: usize) -> Self {
        Self {
            detector: StreamDetector::with_http_config(request_timeout, pool_max_idle_per_host),
        }
    }

    /// Return a pooled HTTP client configured for `proxy`.
    pub fn client_for_proxy(&self, proxy: &ProxyTarget) -> Result<reqwest::Client> {
        self.detector.client_for_proxy(proxy)
    }

    /// Build a platform [`ExtractorFactory`] reusing the pooled client and
    /// route tunnel for `proxy`.
    pub fn extractor_factory_for_proxy(
        &self,
        proxy: &ProxyTarget,
    ) -> std::result::Result<ExtractorFactory, crate::proxies::ProxyError> {
        let client = self
            .detector
            .client_for_proxy(proxy)
            .map_err(|error| match error {
                Error::Core(CoreError::Proxy(proxy_error)) => proxy_error,
                other => crate::proxies::ProxyError::invalid(other.to_string()),
            })?;
        let mut factory = ExtractorFactory::new(client).with_proxy(proxy.clone());
        match crate::proxies::danmu_proxy_for(crate::proxies::SystemProxy::current(), proxy) {
            Ok(tunnel) => factory = factory.with_tunnel(tunnel),
            Err(error) => debug!(%error, "no tunnel for raw connections on this route"),
        }
        Ok(factory)
    }

    /// Run platform extraction, stream selection, URL resolution, and filter
    /// evaluation for `streamer`.
    pub(crate) async fn detect_with_filters(
        &self,
        streamer: &StreamerMetadata,
        filters: &[Filter],
        context: CheckContext<'_>,
    ) -> Result<DetectionResult> {
        self.detector
            .check_status_with_filters(streamer, filters, context)
            .await
    }

    /// Install or clear a test extraction override on the underlying detector.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_test_extraction(&self, extraction: Option<crate::monitor::TestExtraction>) {
        self.detector.set_test_extraction(extraction);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CredentialCheckKey {
    streamer_id: String,
    purpose: ProbePurpose,
    generation: Option<String>,
    session_id: Option<String>,
    epoch: Option<u64>,
    revision: Option<u64>,
}

impl CredentialCheckKey {
    /// Only equivalent operations share one in-flight result. A discovery while
    /// a session exists becomes that session's bound poll; queue-start and
    /// recovery may switch accounts and therefore keep distinct keys.
    fn new(
        streamer_id: &str,
        requested: ProbePurpose,
        policy: Option<&ResolvedCredentialPolicy>,
        session_id: Option<String>,
        binding: Option<&CredentialBinding>,
    ) -> Self {
        let purpose = if requested == ProbePurpose::Discovery && session_id.is_some() {
            ProbePurpose::BoundPoll
        } else {
            requested
        };
        Self {
            streamer_id: streamer_id.to_owned(),
            purpose,
            generation: policy.map(|policy| policy.generation.clone()),
            session_id,
            epoch: binding.map(|binding| binding.epoch),
            revision: binding.map(|binding| binding.revision),
        }
    }
}

#[derive(Clone)]
struct CheckedProbe {
    status: LiveStatus,
    source: CheckSource,
    live_hint: Option<watch::Receiver<bool>>,
}

#[derive(Default)]
struct SharedCheck {
    result: Arc<OnceCell<CheckedProbe>>,
    callers: usize,
}

type InFlightChecks = DashMap<CredentialCheckKey, SharedCheck>;

struct CheckLease<'a> {
    checks: &'a InFlightChecks,
    key: &'a CredentialCheckKey,
    result: Arc<OnceCell<CheckedProbe>>,
}

impl<'a> CheckLease<'a> {
    fn join(checks: &'a InFlightChecks, key: &'a CredentialCheckKey) -> Self {
        let mut check = checks.entry(key.clone()).or_default();
        check.callers += 1;
        Self {
            checks,
            key,
            result: check.result.clone(),
        }
    }

    fn finish(&self) {
        self.checks.remove_if(self.key, |_, check| {
            Arc::ptr_eq(&check.result, &self.result)
        });
    }
}

impl Drop for CheckLease<'_> {
    fn drop(&mut self) {
        self.checks.remove_if_mut(self.key, |_, check| {
            if !Arc::ptr_eq(&check.result, &self.result) {
                return false;
            }
            check.callers -= 1;
            check.callers == 0
        });
    }
}

struct StatusCheck<'a> {
    streamer: &'a StreamerMetadata,
    config: &'a crate::config::MergedConfig,
    filters: &'a [Filter],
    deadline: OperationDeadline,
}

/// Dependencies required to build a [`RecorderProber`].
pub struct RecorderProberDependencies {
    pub extractor_runner: Arc<PlatformExtractorRunner>,
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    pub config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    pub filter_store: Arc<FilterStore>,
    pub store: Arc<RecorderStore>,
    pub admission: Arc<PlatformAdmission>,
    pub execution_service: Arc<CredentialExecutionService>,
    pub bigo_notifications: Arc<BigoNotifications>,
    pub check_history_writer: Option<CheckHistoryWriter>,
    pub request_timeout: Duration,
}

/// Stateful recorder prober performing deduplicated status, freshness, and
/// credential-recovery checks for `StreamerActor`.
pub struct RecorderProber {
    extractor_runner: Arc<PlatformExtractorRunner>,
    streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    filter_store: Arc<FilterStore>,
    store: Arc<RecorderStore>,
    admission: Arc<PlatformAdmission>,
    execution_service: Arc<CredentialExecutionService>,
    credential_blocks: Arc<CredentialBlocks>,
    batch_precheck: BatchPrecheck,
    bigo_notifications: Arc<BigoNotifications>,
    check_history_writer: Option<CheckHistoryWriter>,
    in_flight: InFlightChecks,
    request_timeout: Duration,
}

impl RecorderProber {
    /// Construct a new [`RecorderProber`].
    pub fn new(deps: RecorderProberDependencies) -> Self {
        Self {
            extractor_runner: deps.extractor_runner,
            streamer_manager: deps.streamer_manager,
            config_service: deps.config_service,
            filter_store: deps.filter_store,
            store: deps.store,
            admission: deps.admission,
            execution_service: deps.execution_service,
            credential_blocks: Arc::new(CredentialBlocks::new()),
            batch_precheck: BatchPrecheck::default(),
            bigo_notifications: deps.bigo_notifications,
            check_history_writer: deps.check_history_writer,
            in_flight: InFlightChecks::new(),
            request_timeout: deps.request_timeout,
        }
    }

    /// Access the shared [`CredentialBlocks`] map surfaced on the streamer API.
    pub fn credential_blocks(&self) -> &Arc<CredentialBlocks> {
        &self.credential_blocks
    }

    /// Access the underlying stateless [`PlatformExtractorRunner`].
    pub fn extractor_runner(&self) -> &Arc<PlatformExtractorRunner> {
        &self.extractor_runner
    }

    /// Validate that a session's committed credential binding is still valid
    /// after acquiring a download slot and before starting the engine.
    pub async fn validate_download_binding(
        &self,
        session_id: &str,
        streamer_id: &str,
        binding: &CredentialBinding,
    ) -> Result<()> {
        let current = self
            .store
            .active_session_credential_binding(session_id)
            .await?
            .ok_or(ProfileError::SourceChanged)?;
        if current != *binding {
            return Err(ProfileError::SourceChanged.into());
        }
        let config = self
            .config_service
            .get_config_for_streamer(streamer_id)
            .await?;
        if config
            .credential_policy
            .as_ref()
            .map(|policy| &policy.generation)
            != Some(&binding.policy.generation)
        {
            return Err(ProfileError::SourceChanged.into());
        }
        self.execution_service
            .repository()
            .validate_selection(&binding.policy)
            .await?;
        if let crate::credentials::CredentialIdentity::Profile { profile_id } = &binding.identity {
            let profile = self.execution_service.repository().get(profile_id).await?;
            if !profile.enabled || profile.revision as u64 != binding.revision {
                return Err(ProfileError::SourceChanged.into());
            }
            let health = self
                .execution_service
                .repository()
                .health(profile_id)
                .await?;
            if health.is_some_and(|health| health.validity == CredentialValidity::Invalid) {
                return Err(Error::Core(CoreError::CredentialUnavailable(
                    crate::credentials::CredentialUnavailable {
                        reason: UnavailableReason::BoundProfileUnavailable,
                        policy_generation: binding.policy.generation.clone(),
                    },
                )));
            }
        }
        Ok(())
    }

    /// Execute a probe and return both the FSM-level status/failure and any
    /// batch precheck live-hint watch receiver to observe while idle.
    pub async fn probe_with_hint(
        &self,
        request: ProbeRequest,
    ) -> Result<(
        std::result::Result<ProbeStatus, ProbeFailure>,
        Option<watch::Receiver<bool>>,
    )> {
        let started = Instant::now();
        let checked_at = Timestamp::now();
        let record_history = matches!(
            request.purpose,
            ProbePurpose::Discovery | ProbePurpose::BoundPoll
        );

        let Some(streamer) = self
            .streamer_manager
            .get_streamer_snapshot(&request.streamer_id)
        else {
            return Ok((
                Ok(ProbeStatus::Offline {
                    batch_live_hint_armed: false,
                }),
                None,
            ));
        };

        if streamer.state == StreamerState::Disabled || streamer.is_deleted() {
            debug!(
                streamer_id = %streamer.id,
                "streamer disabled or deleted; skipping probe"
            );
            return Ok((
                Ok(ProbeStatus::Offline {
                    batch_live_hint_armed: false,
                }),
                None,
            ));
        }

        let outcome = self.run_deduped_probe(&streamer, request.purpose).await;
        let duration = SignedDuration::try_from(started.elapsed()).unwrap_or(SignedDuration::MAX);

        if record_history && let Some(writer) = &self.check_history_writer {
            let record = match &outcome {
                Ok(checked) => check_record_from_live_status(
                    &streamer.id,
                    checked_at,
                    duration,
                    &checked.status,
                    checked.source,
                ),
                Err(error) => {
                    CheckRecord::from_error(&streamer.id, checked_at, duration, &error.to_string())
                }
            };
            writer.record(record);
        }

        match outcome {
            Ok(mut checked) => {
                let live_hint = if matches!(checked.status, LiveStatus::Offline) {
                    if checked.source == CheckSource::Individual
                        && let Some(hint) = &mut checked.live_hint
                    {
                        hint.mark_unchanged();
                    }
                    checked.live_hint
                } else {
                    None
                };
                let status = live_status_to_probe_status(checked.status, live_hint.is_some());
                Ok((Ok(status), live_hint))
            }
            Err(error) => Ok((Err(map_probe_failure(error)), None)),
        }
    }

    async fn run_deduped_probe(
        &self,
        streamer: &StreamerMetadata,
        requested_purpose: ProbePurpose,
    ) -> Result<CheckedProbe> {
        trace!(
            streamer_id = %streamer.id,
            streamer_name = %streamer.name,
            streamer_url = %streamer.url,
            purpose = ?requested_purpose,
            "recorder status probe"
        );

        let hard_timeout = std::cmp::max(STREAM_CHECK_HARD_TIMEOUT, self.request_timeout);
        let deadline = OperationDeadline::new(hard_timeout);

        let (config, active) = tokio::time::timeout_at(deadline.instant(), async {
            let config = self
                .config_service
                .get_config_for_streamer(&streamer.id)
                .await?;
            let active = self
                .store
                .active_streamer_session_binding(&streamer.id)
                .await?;
            Ok::<_, Error>((config, active))
        })
        .await
        .map_err(|_| Error::Monitor("Credential context deadline exceeded".into()))??;

        let (active_session_id, binding) = active.unzip();
        let binding = binding.flatten();
        let key = CredentialCheckKey::new(
            &streamer.id,
            requested_purpose,
            config.credential_policy.as_ref(),
            active_session_id,
            binding.as_ref(),
        );
        let purpose = key.purpose;
        let lease = CheckLease::join(&self.in_flight, &key);

        let result = tokio::time::timeout_at(
            deadline.instant(),
            lease.result.get_or_try_init(|| async {
                let check = async {
                    let batch = self
                        .batch_precheck(streamer, &config, purpose, deadline)
                        .await;
                    if batch.offline {
                        return Ok(CheckedProbe {
                            status: LiveStatus::Offline,
                            source: CheckSource::Batch,
                            live_hint: batch.live_hint,
                        });
                    }
                    let filters = self.filter_store.get(&streamer.id).await?;
                    let check = StatusCheck {
                        streamer,
                        config: &config,
                        filters: &filters,
                        deadline,
                    };
                    let verified_at_ms = srec_sqlite::time::now_ms();
                    let started = tokio::time::Instant::now();
                    let observation = match &config.credential_policy {
                        Some(policy) => {
                            self.check_with_credentials(check, policy, binding.as_ref(), purpose)
                                .await?
                        }
                        None => self.check_anonymous(check).await?,
                    };
                    self.bigo_notifications.checked(
                        &streamer.id,
                        started,
                        matches!(
                            observation.status,
                            LiveStatus::Live { .. } | LiveStatus::Filtered { .. }
                        ),
                    );
                    if let Some(metadata) = observation.platform_metadata {
                        self.save_platform_metadata(streamer, metadata, verified_at_ms)
                            .await;
                    }
                    Ok(CheckedProbe {
                        status: observation.status,
                        source: CheckSource::Individual,
                        live_hint: batch.live_hint,
                    })
                };

                let result = tokio::time::timeout_at(deadline.instant(), check)
                    .await
                    .map_err(|_| {
                        Error::Monitor(format!(
                            "Stream probe timed out after {:?} (streamer_id={})",
                            hard_timeout, streamer.id
                        ))
                    })?;
                if result.is_ok() {
                    lease.finish();
                }
                result
            }),
        )
        .await
        .map_err(|_| {
            Error::Monitor("Stream probe deadline exceeded during admission or check".into())
        })
        .and_then(|result| result);

        result.cloned()
    }

    async fn save_platform_metadata(
        &self,
        streamer: &StreamerMetadata,
        metadata: platforms_parser::media::PlatformMetadata,
        verified_at_ms: i64,
    ) {
        let save =
            self.streamer_manager
                .observe_platform_metadata(streamer, metadata, verified_at_ms);
        match tokio::time::timeout(PLATFORM_METADATA_SAVE_TIMEOUT, save).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                warn!(streamer_id = %streamer.id, %error, "Could not save platform metadata");
            }
            Err(_) => warn!(streamer_id = %streamer.id, "Saving platform metadata timed out"),
        }
    }

    async fn batch_precheck(
        &self,
        streamer: &StreamerMetadata,
        config: &crate::config::MergedConfig,
        purpose: ProbePurpose,
        deadline: OperationDeadline,
    ) -> BatchAnswer {
        let platform_id = &streamer.platform_config_id;
        let route = &config.proxy_route;
        let current = self.streamer_manager.get_streamer_snapshot(&streamer.id);
        let current = current.as_deref().unwrap_or(streamer);
        if purpose != ProbePurpose::Discovery
            || !batch_detection_enabled(config.platform_extras.as_ref())
            || config.extractor == ExtractorSelection::Streamlink
            || self.credential_blocks.get(&streamer.id).is_some()
            || current.consecutive_error_count != 0
            || current.disabled_until.is_some()
        {
            return BatchAnswer::default();
        }
        let Some(platform) = platform_name_for_url(&streamer.url) else {
            return BatchAnswer::default();
        };
        if current.url != streamer.url || current.platform_config_id != streamer.platform_config_id
        {
            return BatchAnswer::default();
        }
        if platform.eq_ignore_ascii_case("bigo") {
            return BatchAnswer {
                offline: self.bigo_notifications.answers_offline(&streamer.id),
                live_hint: None,
            };
        }
        let Some(probe) = batch_probe_for_url(&streamer.url) else {
            return BatchAnswer::default();
        };
        let metadata = StoredPlatformMetadata::fresh(
            current.platform_metadata.as_deref(),
            platform,
            srec_sqlite::time::now_ms(),
        );
        let Some(key) = probe.key_for_url(&streamer.url, metadata.as_ref()) else {
            return BatchAnswer::default();
        };
        let interval = match self.config_service.get_cached_global_config().await {
            Ok(global) => Duration::from_millis(
                u64::try_from(global.streamer_check_delay_ms).unwrap_or_default(),
            ),
            Err(error) => {
                debug!(
                    streamer_id = %streamer.id,
                    %error,
                    "Global configuration unavailable; skipping batch status"
                );
                return BatchAnswer::default();
            }
        };
        let client = match self.extractor_runner.client_for_proxy(&route.target) {
            Ok(client) => client,
            Err(error) => {
                debug!(
                    streamer_id = %streamer.id,
                    %error,
                    "No HTTP client for the route; skipping batch status"
                );
                return BatchAnswer::default();
            }
        };
        let poll = Poll {
            group: GroupKey {
                platform,
                platform_id: platform_id.clone(),
                route: route.key.clone(),
            },
            streamer_id: &streamer.id,
            key,
            max_keys: probe.max_keys(),
            interval,
        };
        self.batch_precheck
            .check(poll, |keys| async move {
                self.admission
                    .admit(platform_id, &route.key, deadline)
                    .await
                    .map_err(|error| Error::Monitor(error.to_string()))?;
                tokio::time::timeout(BATCH_REQUEST_TIMEOUT, probe.probe(&client, &keys))
                    .await
                    .map_err(|_| {
                        Error::Monitor(format!(
                            "Batch status request timed out after {BATCH_REQUEST_TIMEOUT:?}"
                        ))
                    })?
                    .map_err(Error::from)
            })
            .await
    }

    async fn check_with_credentials(
        &self,
        check: StatusCheck<'_>,
        policy: &ResolvedCredentialPolicy,
        binding: Option<&CredentialBinding>,
        purpose: ProbePurpose,
    ) -> Result<DetectionResult> {
        let StatusCheck {
            streamer,
            config,
            filters,
            deadline,
        } = check;
        let runner = &self.extractor_runner;
        let result = self
            .execution_service
            .execute(
                policy,
                binding,
                purpose != ProbePurpose::BoundPoll,
                deadline,
                &config.proxy_route,
                |snapshot| {
                    let extras = crate::credentials::managed_authentication_extras(
                        streamer.platform(),
                        config.platform_extras.clone(),
                        &snapshot.material,
                    );
                    let proxy = snapshot.route.target.clone();
                    async move {
                        let status = runner
                            .detect_with_filters(
                                streamer,
                                filters,
                                CheckContext {
                                    cookies: Some(snapshot.material.cookies),
                                    selection_config: Some(&config.stream_selection),
                                    platform_extras: extras,
                                    proxy: &proxy,
                                    extractor: config.extractor,
                                },
                            )
                            .await?;
                        let session_cookies = match &status.status {
                            LiveStatus::Live { media_extras, .. } => media_extras
                                .as_ref()
                                .and_then(|extras| extras.get("session_cookies"))
                                .filter(|cookies| !cookies.is_empty())
                                .cloned(),
                            _ => None,
                        };
                        Ok(crate::credentials::Extracted {
                            preserve_health: !status.status.is_live(),
                            value: status,
                            session_cookies,
                        })
                    }
                },
            )
            .await;

        match &result {
            Err(Error::Core(CoreError::CredentialUnavailable(unavailable))) => {
                if self
                    .streamer_manager
                    .get_streamer(&streamer.id)
                    .is_some_and(|current| current.is_active())
                {
                    self.credential_blocks.block(
                        &streamer.id,
                        &policy.platform_id,
                        unavailable.reason,
                    );
                }
            }
            Err(Error::Core(CoreError::CredentialProfile(ProfileError::SourceChanged))) => {}
            _ => self.credential_blocks.clear(&streamer.id),
        }

        let result = result?;
        let current = self
            .config_service
            .get_config_for_streamer(&streamer.id)
            .await?;
        if current
            .credential_policy
            .as_ref()
            .map(|policy| &policy.generation)
            != Some(&policy.generation)
        {
            return Err(ProfileError::SourceChanged.into());
        }

        let mut status = result.value;
        if let LiveStatus::Live {
            credential_binding,
            credential_snapshot,
            media_extras,
            ..
        } = &mut status.status
        {
            *credential_binding = Some(Box::new(result.snapshot.binding.clone()));
            *credential_snapshot = Some(Arc::new(result.snapshot));
            if let Some(extras) = media_extras {
                extras.remove("session_cookies");
            }
        }
        Ok(status)
    }

    async fn check_anonymous(&self, check: StatusCheck<'_>) -> Result<DetectionResult> {
        let StatusCheck {
            streamer,
            config,
            filters,
            deadline,
        } = check;
        let platform_id = &streamer.platform_config_id;
        self.credential_blocks.clear(&streamer.id);
        self.admission
            .admit(platform_id, &config.proxy_route.key, deadline)
            .await
            .map_err(|error| Error::Monitor(error.to_string()))?;
        let mut status = self
            .extractor_runner
            .detect_with_filters(
                streamer,
                filters,
                CheckContext {
                    cookies: None,
                    selection_config: Some(&config.stream_selection),
                    platform_extras: config.platform_extras.clone(),
                    proxy: &config.proxy_route.target,
                    extractor: config.extractor,
                },
            )
            .await
            .inspect_err(|error| {
                if let Error::Extractor(error) = error {
                    self.admission
                        .observe(platform_id, &config.proxy_route, error);
                }
            })?;
        if let LiveStatus::Live {
            media_extras: Some(extras),
            ..
        } = &mut status.status
        {
            extras.remove("session_cookies");
        }
        Ok(status)
    }
}

fn live_status_to_probe_status(status: LiveStatus, batch_live_hint_armed: bool) -> ProbeStatus {
    if let Some(kind) = status.fatal_kind() {
        let message = status
            .fatal_error_description()
            .unwrap_or("Fatal platform error")
            .to_owned();
        return ProbeStatus::Fatal { kind, message };
    }

    match status {
        LiveStatus::Live {
            credential_binding,
            credential_snapshot,
            title,
            category,
            avatar,
            streams,
            media_headers,
            media_extras,
            next_check_hint,
            ..
        } => ProbeStatus::Live(Box::new(LiveProbePayload {
            title,
            category,
            avatar,
            streams,
            media_headers,
            media_extras: media_extras.map(|extras| *extras),
            credential_binding: credential_binding.map(|binding| *binding),
            credential_snapshot,
            next_check_hint,
        })),
        LiveStatus::Offline => ProbeStatus::Offline {
            batch_live_hint_armed,
        },
        LiveStatus::Filtered {
            reason,
            title,
            category,
        } => ProbeStatus::Filtered {
            reason,
            title,
            category,
        },
        LiveStatus::NotFound
        | LiveStatus::Banned
        | LiveStatus::AgeRestricted
        | LiveStatus::RegionLocked
        | LiveStatus::Private
        | LiveStatus::UnsupportedPlatform => ProbeStatus::Fatal {
            kind: srec_core::domain::streamer::FatalErrorType::NotFound,
            message: "Fatal platform error".to_owned(),
        },
    }
}

pub(crate) fn map_probe_failure(error: Error) -> ProbeFailure {
    match error {
        Error::Core(CoreError::CredentialUnavailable(unavailable)) => {
            ProbeFailure::CredentialUnavailable {
                reason: unavailable.reason,
                policy_generation: unavailable.policy_generation,
            }
        }
        Error::Core(CoreError::CredentialProfile(ProfileError::SourceChanged)) => {
            ProbeFailure::CredentialSourceChanged
        }
        other => ProbeFailure::Transient {
            message: other.to_string(),
        },
    }
}

fn check_record_from_live_status(
    streamer_id: &str,
    checked_at: Timestamp,
    duration: SignedDuration,
    status: &LiveStatus,
    source: CheckSource,
) -> CheckRecord {
    let outcome = if let Some(kind) = status.fatal_kind() {
        CheckOutcome::FatalError { kind }
    } else {
        match status {
            LiveStatus::Live {
                title,
                category,
                viewer_count,
                streams,
                candidates,
                ..
            } => CheckOutcome::Live {
                title: title.clone(),
                category: category.clone(),
                viewer_count: *viewer_count,
                candidates: candidates.iter().map(selected_stream_summary).collect(),
                selected_stream: streams.first().map(selected_stream_summary),
            },
            LiveStatus::Filtered {
                reason,
                title,
                category,
            } => CheckOutcome::Filtered {
                reason: match reason {
                    FilterReason::OutOfSchedule { .. } => FilterCause::OutOfSchedule,
                    FilterReason::TitleMismatch => FilterCause::TitleMismatch,
                    FilterReason::CategoryMismatch => FilterCause::CategoryMismatch,
                },
                title: title.clone(),
                category: category.clone(),
            },
            LiveStatus::Offline
            | LiveStatus::NotFound
            | LiveStatus::Banned
            | LiveStatus::AgeRestricted
            | LiveStatus::RegionLocked
            | LiveStatus::Private
            | LiveStatus::UnsupportedPlatform => CheckOutcome::Offline,
        }
    };

    CheckRecord::new(streamer_id, checked_at, duration, outcome, source)
}

fn selected_stream_summary(stream: &platforms_parser::media::StreamInfo) -> SelectedStreamSummary {
    SelectedStreamSummary {
        quality: stream.quality.clone(),
        stream_format: format!("{:?}", stream.stream_format),
        media_format: format!("{:?}", stream.media_format),
        bitrate: stream.bitrate,
        codec: stream.codec.clone(),
        fps: stream.fps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_a_check_releases_only_its_own_entry() {
        let checks = InFlightChecks::new();
        let key = CredentialCheckKey::new("streamer", ProbePurpose::Discovery, None, None, None);
        let owner = CheckLease::join(&checks, &key);
        let waiter = CheckLease::join(&checks, &key);
        drop(owner);
        assert!(checks.contains_key(&key));

        waiter.finish();
        let replacement = CheckLease::join(&checks, &key);
        drop(waiter);
        assert!(checks.contains_key(&key));
        drop(replacement);
        assert!(checks.is_empty());
    }
}
