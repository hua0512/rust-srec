//! Operational policy for runtime events.

use std::future::Future;
use std::sync::Arc;

use dashmap::DashMap;
use futures::StreamExt;
use tracing::{debug, info, warn};

use crate::config::ConfigService;
use crate::danmu::DanmuService;
use crate::database::repositories::{
    config::SqlxConfigRepository, filter::SqlxFilterRepository, session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
};
use crate::domain::StreamerState;
use crate::downloader::DownloadManager;
use crate::monitor::{MonitorEvent, StreamMonitor};
use crate::pipeline::PipelineManager;
use crate::scheduler::SchedulerHandle;
use crate::session::{SessionLifecycle, SessionTransition, TerminalCause};
use crate::streamer::StreamerManager;
use crate::utils::task_supervisor::TaskSupervisor;

use super::session_cancels::SessionCancelTokens;

mod download_pipeline;
mod retirement;

use download_pipeline::{PipelineExit, StreamerLivePayload, run_live_download_pipeline};

pub(crate) use retirement::{INTERACTIVE_RETIREMENT, OBSERVE_RETIREMENT};

const MAX_CONCURRENT_CONFIG_REFRESHES: usize = 16;

/// Signed media URLs from a diagnostic stay usable only briefly.
const RECOVERED_MEDIA_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Media a successful credential diagnostic extracted with the binding it
/// committed. The restarted attempt uses it instead of extracting again.
pub(super) struct RecoveredMedia {
    pub(super) binding: crate::credentials::CredentialBinding,
    pub(super) snapshot: Arc<crate::credentials::CredentialSnapshot>,
    pub(super) streams: Vec<crate::monitor::StreamInfo>,
    pub(super) media_headers: Option<std::collections::HashMap<String, String>>,
    pub(super) media_extras: Option<std::collections::HashMap<String, String>>,
    extracted_at: std::time::Instant,
}

/// Only independent owners run concurrently; callers await the whole batch before
/// handling the next configuration event, retaining event order and bounded fan-out.
async fn run_config_refreshes<F, Fut>(ids: impl IntoIterator<Item = String>, refresh: F)
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    futures::stream::iter(ids)
        .map(refresh)
        .buffer_unordered(MAX_CONCURRENT_CONFIG_REFRESHES)
        .for_each(|()| async {})
        .await;
}

type RuntimeConfigService = ConfigService<SqlxConfigRepository, SqlxStreamerRepository>;
type RuntimeStreamMonitor = StreamMonitor<
    SqlxStreamerRepository,
    SqlxFilterRepository,
    SqlxSessionRepository,
    SqlxConfigRepository,
>;

/// Whether [`RuntimeCoordinator::stop_streamer_work`] returns as soon as the
/// stops are requested or only once every download attempt has published its
/// terminal outcome.
#[derive(Clone, Copy)]
enum StopWait {
    /// For a disable, where no caller decides anything on the outcome and the
    /// stop must not block the loop it runs on.
    Requested,
    /// For a retirement, where the caller is about to delete the `streamers`
    /// row and needs the attempts finalized first. Blocks for as long as the
    /// engine's graceful stop takes, up to `bound`.
    Finalized { bound: std::time::Duration },
}

/// What [`RuntimeCoordinator::stop_streamer_work`] left behind.
#[derive(Default)]
struct StreamerWorkStopped {
    /// Owners that could not be stopped, phrased for a caller's log line. A
    /// download that finalized on its own between the snapshot and the stop is
    /// not one of them.
    failures: Vec<String>,
    /// The session `SessionLifecycle::end_for_disable` resolved, if any.
    ///
    /// `Some` means the streamer's session is still in `SessionLifecycle`'s
    /// in-memory map — either this call ended it, or it ended within
    /// `session::lifecycle::ENDED_RETENTION_DEFAULT` and `schedule_ended_eviction` has not
    /// dropped it yet. Both are windows in which the
    /// `SessionTransition::Ended` broadcast may not have reached
    /// `PipelineCoordinator`, so `RuntimeCoordinator::retire_streamer` treats it
    /// as "no conclusion available from `PipelineManager::drain_for_session`".
    /// `None` means the end is settled and the drain answers for it.
    session_id: Option<String>,
}

/// Coordinates required side effects for configuration, monitor, and session events.
pub(crate) struct RuntimeCoordinator {
    credential_diagnostics: DashMap<String, std::collections::HashSet<String>>,
    /// Keyed by session; at most one bundle, consumed by the next startup.
    recovered_media: DashMap<String, RecoveredMedia>,
    #[cfg(test)]
    freshness_check: Option<Arc<dyn contract_tests::FreshnessCheck>>,
    #[cfg(test)]
    recovery_check: Option<Arc<dyn contract_tests::FreshnessCheck>>,
    download_manager: Arc<DownloadManager>,
    streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    config_service: Arc<RuntimeConfigService>,
    danmu_service: Arc<DanmuService>,
    stream_monitor: Arc<RuntimeStreamMonitor>,
    session_repository: Arc<SqlxSessionRepository>,
    session_cancels: Arc<SessionCancelTokens>,
    /// Streamer ids with a `run_live_download_pipeline` task in flight.
    /// The pipeline inserts its streamer id before its first await (the
    /// per-streamer dedup that stops two concurrent `StreamerLive` events
    /// from both reaching `start_with_slot`) and removes it via
    /// `PipelineReservationGuard` on every exit path. Keyed by streamer
    /// id, unlike `session_cancels`, which is keyed by session id.
    pending_pipelines: Arc<DashMap<String, ()>>,
    pipeline_manager: Arc<PipelineManager>,
    session_lifecycle: Arc<SessionLifecycle>,
    task_supervisor: Arc<TaskSupervisor>,
    /// Reaches the scheduler's event loop for `RemoveStreamerAwaitable`, the
    /// only way to observe a `StreamerActor`'s task actually leaving the runtime
    /// from off that loop.
    scheduler_handle: SchedulerHandle,
}

pub(super) struct RuntimeCoordinatorDependencies {
    pub download_manager: Arc<DownloadManager>,
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    pub config_service: Arc<RuntimeConfigService>,
    pub danmu_service: Arc<DanmuService>,
    pub stream_monitor: Arc<RuntimeStreamMonitor>,
    pub session_repository: Arc<SqlxSessionRepository>,
    pub session_cancels: Arc<SessionCancelTokens>,
    pub pending_pipelines: Arc<DashMap<String, ()>>,
    pub pipeline_manager: Arc<PipelineManager>,
    pub session_lifecycle: Arc<SessionLifecycle>,
    pub task_supervisor: Arc<TaskSupervisor>,
    pub scheduler_handle: SchedulerHandle,
}

impl RuntimeCoordinator {
    pub(super) fn new(dependencies: RuntimeCoordinatorDependencies) -> Self {
        let RuntimeCoordinatorDependencies {
            download_manager,
            streamer_manager,
            config_service,
            danmu_service,
            stream_monitor,
            session_repository,
            session_cancels,
            pending_pipelines,
            pipeline_manager,
            session_lifecycle,
            task_supervisor,
            scheduler_handle,
        } = dependencies;
        Self {
            credential_diagnostics: DashMap::new(),
            recovered_media: DashMap::new(),
            #[cfg(test)]
            freshness_check: None,
            #[cfg(test)]
            recovery_check: None,
            download_manager,
            streamer_manager,
            config_service,
            danmu_service,
            stream_monitor,
            session_repository,
            session_cancels,
            pending_pipelines,
            pipeline_manager,
            session_lifecycle,
            task_supervisor,
            scheduler_handle,
        }
    }

    async fn check_startup_freshness(
        &self,
        metadata: &crate::streamer::StreamerMetadata,
    ) -> crate::Result<crate::monitor::LiveStatus> {
        #[cfg(test)]
        if let Some(checker) = &self.freshness_check {
            return checker.check(metadata).await;
        }
        self.stream_monitor
            .check_streamer_for(metadata, crate::monitor::CredentialCheckPurpose::QueueStart)
            .await
    }

    async fn check_recovery(
        &self,
        metadata: &crate::streamer::StreamerMetadata,
    ) -> crate::Result<crate::monitor::LiveStatus> {
        #[cfg(test)]
        if let Some(checker) = &self.recovery_check {
            return checker.check(metadata).await;
        }
        self.stream_monitor
            .check_streamer_for(metadata, crate::monitor::CredentialCheckPurpose::Recovery)
            .await
    }

    /// Hands a diagnostic's bundle to the startup that carries the same binding.
    pub(super) fn take_recovered_media(
        &self,
        session_id: &str,
        binding: Option<&crate::credentials::CredentialBinding>,
    ) -> Option<RecoveredMedia> {
        let (_, media) = self.recovered_media.remove(session_id)?;
        (binding == Some(&media.binding) && media.extracted_at.elapsed() < RECOVERED_MEDIA_TTL)
            .then_some(media)
    }

    pub(crate) async fn diagnose_credential_attempt(
        &self,
        terminal: crate::downloader::DownloadTerminalEvent,
    ) -> Option<crate::downloader::DownloadFailureKind> {
        use crate::downloader::DownloadFailureKind;
        let crate::downloader::DownloadTerminalEvent::Failed {
            download_id,
            streamer_id,
            session_id,
            kind,
            engine_type,
            ..
        } = terminal
        else {
            return None;
        };
        if !kind.requests_credential_diagnostic(engine_type)
            || !self.session_lifecycle.is_session_active(&session_id)
        {
            return None;
        }
        let config = self
            .config_service
            .get_config_for_streamer(&streamer_id)
            .await
            .ok()?;
        config.credential_policy.as_ref()?;
        let attempt = {
            let mut attempts = self
                .credential_diagnostics
                .entry(session_id.clone())
                .or_default();
            if attempts.contains(&download_id)
                || attempts.len() >= config.download_retry_policy.max_retries as usize
            {
                return None;
            }
            let attempt = attempts.len() as u32;
            attempts.insert(download_id);
            attempt
        };
        tokio::time::sleep(config.download_retry_policy.delay_for_attempt(attempt)).await;
        let metadata = self.streamer_manager.get_streamer(&streamer_id)?;
        if !metadata.is_active() || metadata.is_disabled() {
            return None;
        }
        match self.check_recovery(&metadata).await {
            Ok(crate::monitor::LiveStatus::Live {
                credential_binding: Some(binding),
                credential_snapshot,
                streams,
                media_headers,
                media_extras,
                ..
            }) => {
                match self
                    .session_lifecycle
                    .commit_credential_binding(&session_id, &streamer_id, *binding)
                    .await
                {
                    Ok(binding) => {
                        if let Some(snapshot) = credential_snapshot
                            && !streams.is_empty()
                        {
                            self.recovered_media.insert(
                                session_id,
                                RecoveredMedia {
                                    binding,
                                    snapshot,
                                    streams,
                                    media_headers,
                                    media_extras: media_extras.map(|extras| *extras),
                                    extracted_at: std::time::Instant::now(),
                                },
                            );
                        }
                        Some(DownloadFailureKind::CredentialRecovery)
                    }
                    Err(error) => {
                        debug!(session_id, %error, "Credential recovery handoff changed");
                        None
                    }
                }
            }
            Err(crate::Error::CredentialUnavailable(_)) => {
                Some(DownloadFailureKind::CredentialUnavailable)
            }
            _ => None,
        }
    }

    /// Restarts a recording session whose managed download is waiting for usable
    /// credentials. Sessions with a running or starting download are left alone.
    pub(crate) async fn resume_pending_credentials(self: &Arc<Self>, streamer_id: &str) {
        self.resume_pending(streamer_id, true).await;
    }

    /// A waiting recording keeps its streamer Live, so other streamers are
    /// skipped without a session lookup.
    pub(crate) async fn resume_pending_credentials_for(
        self: &Arc<Self>,
        streamer_ids: impl IntoIterator<Item = String>,
    ) {
        let live = streamer_ids.into_iter().filter(|id| {
            self.streamer_manager
                .get_streamer(id)
                .is_some_and(|metadata| metadata.state == StreamerState::Live)
        });
        run_config_refreshes(live, |id| async move {
            self.resume_pending_credentials(&id).await;
        })
        .await;
    }

    /// A bound check that succeeds after a credential change reports Live to
    /// Live and emits no event, so a recording waiting for the changed owner's
    /// accounts is restarted here.
    pub(crate) async fn resume_pending_credentials_for_owner(
        self: &Arc<Self>,
        owner: &crate::credentials::CredentialOwner,
    ) {
        use crate::credentials::CredentialOwner;
        let affected: Vec<String> = self
            .streamer_manager
            .get_all()
            .into_iter()
            .filter(|metadata| match owner {
                CredentialOwner::Platform { platform_id } => {
                    metadata.platform_config_id == *platform_id
                }
                CredentialOwner::Template { template_id } => {
                    metadata.template_config_id.as_deref() == Some(template_id.as_str())
                }
                CredentialOwner::Streamer { streamer_id } => metadata.id == *streamer_id,
            })
            .map(|metadata| metadata.id)
            .collect();
        self.resume_pending_credentials_for(affected).await;
    }

    async fn resume_pending(self: &Arc<Self>, streamer_id: &str, retry_changed: bool) {
        use crate::database::repositories::SessionRepository;
        if self.download_manager.has_active_download(streamer_id)
            || self.pending_pipelines.contains_key(streamer_id)
        {
            return;
        }
        let Some(metadata) = self
            .streamer_manager
            .get_streamer(streamer_id)
            .filter(|metadata| metadata.is_active() && !metadata.is_disabled())
        else {
            return;
        };
        let session_id = match self
            .session_lifecycle
            .current_session_id_for_streamer(streamer_id)
        {
            // This hands off straight to download startup. A session in
            // hysteresis resumes only through live detection, which cancels its
            // quiet-period timer; starting an engine here would let that timer
            // end the session underneath the new recording.
            Some(session_id)
                if self
                    .session_lifecycle
                    .session_snapshot(&session_id)
                    .is_some_and(|state| state.is_recording()) =>
            {
                session_id
            }
            Some(_) => return,
            None => match self
                .session_repository
                .get_active_session_for_streamer(streamer_id)
                .await
            {
                Ok(Some(session)) => session.id,
                _ => return,
            },
        };
        let title = self
            .session_repository
            .get_session(&session_id)
            .await
            .ok()
            .and_then(|session| session.titles)
            .and_then(|raw| {
                serde_json::from_str::<Vec<crate::database::models::TitleEntry>>(&raw).ok()
            })
            .and_then(|mut titles| titles.pop())
            .map_or_else(String::new, |entry| entry.title);
        match self
            .stream_monitor
            .session_credential_binding(&session_id)
            .await
        {
            Ok(Some(binding)) => {
                debug!(
                    streamer_id,
                    session_id, "Resuming a recording that awaited credentials"
                );
                self.spawn_live_pipeline(
                    StreamerLivePayload {
                        runtime_instance: Some(crate::monitor::runtime_instance_id().to_owned()),
                        credential_binding: Some(binding),
                        streamer_id: streamer_id.to_owned(),
                        session_id,
                        streamer_name: metadata.name,
                        title,
                        streams: Vec::new(),
                        streamer_url: metadata.url,
                        media_headers: None,
                        media_extras: None,
                    },
                    false,
                    retry_changed,
                );
            }
            Ok(None) => {}
            Err(error) => {
                debug!(streamer_id, %error, "Pending credential recovery could not load its binding")
            }
        }
    }

    /// `retry_changed` allows one fresh bound extraction when the account
    /// changes between extraction and engine start. A repeat waits for the
    /// next check or credential change instead of looping.
    fn spawn_live_pipeline(
        self: &Arc<Self>,
        payload: StreamerLivePayload,
        from_hysteresis_resume: bool,
        retry_changed: bool,
    ) {
        let coordinator = self.clone();
        self.task_supervisor
            .spawn("live download pipeline", async move {
                let streamer_id = payload.streamer_id.clone();
                let exit = run_live_download_pipeline(
                    coordinator.clone(),
                    payload,
                    from_hysteresis_resume,
                )
                .await;
                if exit == PipelineExit::CredentialsChanged && retry_changed {
                    coordinator.resume_pending(&streamer_id, false).await;
                }
            });
    }

    pub(crate) async fn refresh_metadata_offline_checks(
        &self,
        streamer_ids: impl IntoIterator<Item = String>,
    ) {
        run_config_refreshes(streamer_ids, |id| async move {
            self.refresh_metadata_offline_check(&id).await;
        })
        .await;
    }

    pub(crate) async fn refresh_metadata_offline_check(&self, streamer_id: &str) {
        match self
            .config_service
            .get_config_for_streamer(streamer_id)
            .await
        {
            Ok(merged) => self
                .streamer_manager
                .apply_resolved_config(streamer_id, &merged),
            Err(error) => debug!(
                streamer_id,
                error = %error,
                "Skipping resolved scheduler configuration refresh"
            ),
        }
    }

    pub(crate) async fn handle_streamer_disabled(&self, streamer_id: &str) {
        // `StreamerManager` is the only source of the name here, and there is
        // no useful session-row fallback behind it. It drops a cache entry
        // only once the `streamers` row is gone (`reap_deleted`, or
        // `reload_from_repo` seeing `NotFound`), and by then the
        // `ON DELETE SET NULL` foreign key has cleared
        // `live_sessions.streamer_id` and `trg_live_session_orphan_ends` has
        // stamped `end_time`. `SessionLifecycle::end_for_disable` reads this
        // name only on the branch where the session is still active, which is
        // exactly the branch on which this lookup hits.
        let streamer_name = self
            .streamer_manager
            .get_streamer(streamer_id)
            .map(|metadata| metadata.name)
            .unwrap_or_default();

        // Every failure is already logged; a disable has no caller waiting to
        // decide anything on the outcome.
        let _ = self
            .stop_streamer_work(streamer_id, &streamer_name, StopWait::Requested)
            .await;
    }

    /// Import retires authentication separately from the streamer row. Settle
    /// current work before its profile can be physically reaped.
    pub(crate) async fn settle_credential_retirement(&self, streamer_id: &str) {
        let (session_id, binding) = match self
            .stream_monitor
            .retiring_credential_session(streamer_id)
            .await
        {
            Ok(Some(session)) => session,
            Ok(None) => return,
            Err(error) => {
                warn!(streamer_id, %error, "Could not inspect credential retirement");
                return;
            }
        };
        let streamer_name = self
            .streamer_manager
            .get_streamer(streamer_id)
            .map(|metadata| metadata.name)
            .unwrap_or_default();
        let downloads: Vec<_> = self
            .download_manager
            .get_active_downloads()
            .into_iter()
            .filter(|download| download.session_id == session_id)
            .collect();
        self.session_cancels.cancel(&session_id);
        for download in downloads {
            match tokio::time::timeout(
                std::time::Duration::from_secs(20),
                self.download_manager.stop_download_with_reason(
                    &download.id,
                    crate::downloader::DownloadStopCause::StreamerDisabled,
                ),
            )
            .await
            {
                Ok(Ok(())) | Ok(Err(crate::Error::NotFound { .. })) => {}
                _ => {
                    warn!(
                        streamer_id,
                        session_id, "Credential retirement awaits the captured engine attempt"
                    );
                    return;
                }
            }
        }
        if self.danmu_service.is_collecting(&session_id)
            && let Err(error) = self.danmu_service.stop_collection(&session_id).await
        {
            warn!(session_id, %error, "Credential retirement awaits danmu completion");
            return;
        }
        if let Err(error) = self
            .session_lifecycle
            .end_for_credential_retirement(&session_id, streamer_id, &streamer_name, binding)
            .await
        {
            warn!(session_id, %error, "Credential retirement could not finalize its captured session");
        }
    }

    /// Stop everything bound to `streamer_id`: its queued and running download
    /// attempts, its danmu collection and its open session.
    ///
    /// Ordering matters. The session cancel tokens go first, because a
    /// `run_live_download_pipeline` task past `SessionCancelTokens::register`
    /// but not yet queued appears in neither `DownloadManager::snapshot_pending`
    /// nor `get_active_downloads`, and its `cancel.is_cancelled()` checks are
    /// the only thing that stops it before `acquire_slot`. The session end runs
    /// last, so `SessionLifecycle::end_for_disable` writes `end_time` after the
    /// attempts that would otherwise reopen it have finalized.
    ///
    /// Ending through `SessionLifecycle` rather than letting the reap's
    /// `ON DELETE SET NULL` and `trg_live_session_orphan_ends` stamp `end_time`
    /// is what keeps the `session_ended` row, the `SessionTransition::Ended`
    /// broadcast, the in-memory `Recording` -> `Ended` transition and
    /// `schedule_ended_eviction`.
    async fn stop_streamer_work(
        &self,
        streamer_id: &str,
        streamer_name: &str,
        wait: StopWait,
    ) -> StreamerWorkStopped {
        let mut stopped = StreamerWorkStopped::default();

        if let Some(session_id) = self
            .session_lifecycle
            .current_session_id_for_streamer(streamer_id)
        {
            self.session_cancels.cancel(&session_id);
            debug!(
                streamer_id,
                session_id, "Cancelled current session token for stopped streamer"
            );
        }

        // A queued attempt waiting on a download slot is not in
        // `get_active_downloads`, and would otherwise start after the caller has
        // stood the streamer down.
        for pending in self.download_manager.snapshot_pending() {
            if pending.streamer_id == streamer_id {
                self.session_cancels.cancel(&pending.session_id);
                info!(
                    streamer_id,
                    session_id = %pending.session_id,
                    "Cancelled queued download for stopped streamer"
                );
            }
        }

        let downloads: Vec<_> = self
            .download_manager
            .get_active_downloads()
            .into_iter()
            .filter(|download| download.streamer_id == streamer_id)
            .collect();

        for download in downloads {
            let (result, outcome) = match wait {
                StopWait::Requested => (
                    self.download_manager.request_stop_download(
                        &download.id,
                        crate::downloader::DownloadStopCause::StreamerDisabled,
                    ),
                    "Requested download stop for stopped streamer",
                ),
                StopWait::Finalized { bound } => (
                    match tokio::time::timeout(
                        bound,
                        self.download_manager.stop_download_with_reason(
                            &download.id,
                            crate::downloader::DownloadStopCause::StreamerDisabled,
                        ),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err(crate::Error::Other(format!(
                            "download did not finalize within {} seconds",
                            bound.as_secs()
                        ))),
                    },
                    "Finalized download for retired streamer",
                ),
            };
            match result {
                Ok(()) => info!(
                    download_id = %download.id,
                    streamer_id,
                    "{outcome}"
                ),
                // The attempt finalized between `get_active_downloads` and the
                // stop, so it holds nothing.
                Err(crate::Error::NotFound { .. }) => debug!(
                    download_id = %download.id,
                    streamer_id,
                    "Download already finalized before the stop request"
                ),
                Err(error) => {
                    warn!(
                        download_id = %download.id,
                        streamer_id,
                        error = %error,
                        "Failed to stop download for stopped streamer"
                    );
                    stopped
                        .failures
                        .push(format!("download {}: {error}", download.id));
                }
            }
        }

        if let Some(session_id) = self.danmu_service.get_session_by_streamer(streamer_id) {
            match self.danmu_service.stop_collection(&session_id).await {
                Ok(stats) => info!(
                    streamer_id,
                    session_id,
                    messages = stats.total_count,
                    "Stopped danmu collection for stopped streamer"
                ),
                // A collector that released its `collections` entry between
                // `get_session_by_streamer` and the stop reports an error but
                // holds nothing, so it cannot keep writing danmu rows.
                Err(_) if !self.danmu_service.is_collecting(&session_id) => debug!(
                    streamer_id,
                    session_id, "Danmu collection already released its slot before the stop"
                ),
                Err(error) => {
                    warn!(
                        streamer_id,
                        session_id,
                        error = %error,
                        "Failed to stop danmu collection for stopped streamer"
                    );
                    stopped
                        .failures
                        .push(format!("danmu collection {session_id}: {error}"));
                }
            }
        }

        match self
            .session_lifecycle
            .end_for_disable(streamer_id, streamer_name)
            .await
        {
            Ok(session_id) => stopped.session_id = session_id,
            Err(error) => {
                warn!(
                    streamer_id,
                    error = %error,
                    "Failed to end stopped streamer's session"
                );
                stopped.failures.push(format!("session: {error}"));
            }
        }

        stopped
    }

    pub(crate) async fn handle_monitor_event(
        self: &Arc<Self>,
        event: MonitorEvent,
        from_hysteresis_resume: bool,
    ) {
        match event {
            MonitorEvent::StreamerLive {
                runtime_instance,
                credential_binding,
                streamer_id,
                session_id,
                streamer_name,
                title,
                streams,
                streamer_url,
                media_headers,
                media_extras,
                ..
            } => {
                info!(
                    streamer_id,
                    streamer_name,
                    title,
                    stream_count = streams.len(),
                    media_header_count = media_headers.as_ref().map_or(0, |value| value.len()),
                    media_extra_count = media_extras.as_ref().map_or(0, |value| value.len()),
                    "Streamer went live"
                );

                self.spawn_live_pipeline(
                    StreamerLivePayload {
                        runtime_instance,
                        credential_binding: credential_binding.map(|binding| *binding),

                        streamer_id,
                        session_id,
                        streamer_name,
                        title,
                        streams,
                        streamer_url,
                        media_headers,
                        media_extras: media_extras.map(|extras| *extras),
                    },
                    from_hysteresis_resume,
                    true,
                );
            }
            MonitorEvent::StreamerOffline {
                streamer_id,
                streamer_name,
                session_id,
                ..
            } => {
                info!(streamer_id, streamer_name, "Streamer went offline");

                if let Some(session_id) = session_id.as_deref() {
                    self.session_cancels.cancel(session_id);
                }

                // An explicitly identified old session must never fall back to
                // a successor that now happens to own this streamer.
                let danmu_session_id = match session_id.as_ref() {
                    Some(id) => self.danmu_service.is_collecting(id).then(|| id.clone()),
                    None => self.danmu_service.get_session_by_streamer(&streamer_id),
                };
                if let Some(session_id) = danmu_session_id
                    && let Err(error) = self.danmu_service.stop_collection(&session_id).await
                {
                    warn!(
                        session_id,
                        error = %error,
                        "Failed to stop danmu collection for offline streamer"
                    );
                }

                for download in self
                    .download_manager
                    .get_active_downloads()
                    .into_iter()
                    .filter(|download| {
                        download.streamer_id == streamer_id
                            && session_id
                                .as_ref()
                                .is_none_or(|id| download.session_id == *id)
                    })
                {
                    if let Err(error) = self.download_manager.request_stop_download(
                        &download.id,
                        crate::downloader::DownloadStopCause::StreamerOffline,
                    ) {
                        warn!(
                        streamer_id,
                        download_id = %download.id,
                        error = %error,
                        "Failed to stop download for offline streamer"
                        );
                    }
                }
            }
            MonitorEvent::StateChanged {
                streamer_id,
                streamer_name,
                new_state: StreamerState::OutOfSchedule,
                reason,
                ..
            } if reason.as_deref() == Some("out_of_schedule") => {
                info!(
                    streamer_id,
                    streamer_name, "Streamer left its schedule window; stopping active work"
                );

                // A pipeline resolving config or preflight owns its token before
                // it appears in either the queue or active-download snapshots.
                if let Some(session_id) = self
                    .session_lifecycle
                    .current_session_id_for_streamer(&streamer_id)
                {
                    self.session_cancels.cancel(&session_id);
                }

                for pending in self.download_manager.snapshot_pending() {
                    if pending.streamer_id == streamer_id {
                        self.session_cancels.cancel(&pending.session_id);
                    }
                }

                if let Some(session_id) = self.danmu_service.get_session_by_streamer(&streamer_id)
                    && let Err(error) = self.danmu_service.stop_collection(&session_id).await
                {
                    warn!(
                        session_id,
                        error = %error,
                        "Failed to stop out-of-schedule danmu collection"
                    );
                }

                if let Some(download) = self.download_manager.get_download_by_streamer(&streamer_id)
                    && let Err(error) = self.download_manager.request_stop_download(
                        &download.id,
                        crate::downloader::DownloadStopCause::OutOfSchedule,
                    )
                {
                    warn!(
                        streamer_id,
                        download_id = %download.id,
                        error = %error,
                        "Failed to stop out-of-schedule download"
                    );
                }
            }
            _ => {}
        }
    }

    pub(crate) async fn handle_session_transition(self: &Arc<Self>, transition: SessionTransition) {
        if let SessionTransition::Ended { session_id, .. } = &transition {
            self.credential_diagnostics.remove(session_id);
            self.recovered_media.remove(session_id);
            self.download_manager
                .clear_session_segment_index(session_id);
        }

        if let SessionTransition::Ended {
            session_id,
            cause: TerminalCause::Failed { .. },
            ..
        } = &transition
            && self.danmu_service.is_collecting(session_id)
            && let Err(error) = self.danmu_service.stop_collection(session_id).await
        {
            warn!(
                session_id,
                error = %error,
                "Failed to stop danmu collection after download failure"
            );
        }

        if let SessionTransition::Started {
            from_hysteresis: true,
            download_start: Some(payload),
            session_id,
            streamer_id,
            streamer_name,
            title,
            category,
            started_at,
            ..
        } = &transition
        {
            if self.session_lifecycle.is_session_active(session_id) {
                info!(
                    streamer_id,
                    session_id,
                    streamer_name,
                    "Session resumed from hysteresis; restarting download"
                );
                self.handle_monitor_event(
                    MonitorEvent::StreamerLive {
                        runtime_instance: Some(crate::monitor::runtime_instance_id().to_owned()),
                        credential_binding: payload.credential_binding.clone().map(Box::new),

                        streamer_id: streamer_id.clone(),
                        session_id: session_id.clone(),
                        streamer_name: streamer_name.clone(),
                        streamer_url: payload.streamer_url.clone(),
                        title: title.clone(),
                        category: category.clone(),
                        streams: payload.streams.clone(),
                        media_headers: payload.media_headers.clone(),
                        media_extras: payload.media_extras.clone().map(Box::new),
                        timestamp: started_at.to_owned(),
                    },
                    true,
                )
                .await;
            } else {
                debug!(
                    session_id,
                    streamer_id, "Session no longer active; skipping resumed download"
                );
            }
        }

        self.pipeline_manager
            .handle_session_transition(transition)
            .await;
    }
}

#[cfg(test)]
mod config_refresh_tests;

#[cfg(test)]
mod contract_tests;
