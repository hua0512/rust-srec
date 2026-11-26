use std::sync::Arc;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::{ServiceContainer, autoscale_concurrency_limit};
use crate::config::{ConfigService, ConfigUpdateEvent};
use crate::database::repositories::{
    config::SqlxConfigRepository, streamer::SqlxStreamerRepository,
};
use crate::downloader::DownloadManager;
use crate::pipeline::PipelineManager;
use crate::recorder::ActorRegistry;
use crate::streamer::StreamerManager;

type RuntimeConfigService = ConfigService<SqlxConfigRepository, SqlxStreamerRepository>;

fn broadcast_error_is_recoverable(
    subscriber: &'static str,
    error: broadcast::error::RecvError,
) -> bool {
    match error {
        broadcast::error::RecvError::Lagged(skipped) => {
            warn!(
                subscriber,
                skipped, "Broadcast subscriber lagged; continuing from the newest available event"
            );
            true
        }
        broadcast::error::RecvError::Closed => {
            debug!(subscriber, "Broadcast channel closed");
            false
        }
    }
}

impl ServiceContainer {
    /// Set up config event subscriptions between services.
    pub(super) fn setup_config_event_subscriptions(&self) {
        let handler = ConfigEventHandler {
            streamer_manager: self.streamer_manager.clone(),
            config_service: self.config_service.clone(),
            download_manager: self.download_manager.clone(),
            pipeline_manager: self.pipeline_manager.clone(),
            actor_registry: self.actor_registry.clone(),
            gpu_health_monitor: self.gpu_health_monitor.get().cloned(),
            logging_config: self.logging_config.clone(),
            credential_blocks: self.credential_blocks().clone(),
        };
        let receiver = self.event_broadcaster.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        self.task_supervisor.spawn(
            "config event handler",
            handler.run(receiver, cancellation_token),
        );
    }

    /// Apply one [`ConfigUpdateEvent`] on the caller's task, through the same
    /// handler [`Self::setup_config_event_subscriptions`] spawns.
    ///
    /// Lets a test observe what the handler does to a single event instead of
    /// racing the spawned task for it.
    #[cfg(test)]
    pub(super) async fn apply_config_event_for_test(&self, event: ConfigUpdateEvent) {
        ConfigEventHandler {
            streamer_manager: self.streamer_manager.clone(),
            config_service: self.config_service.clone(),
            download_manager: self.download_manager.clone(),
            pipeline_manager: self.pipeline_manager.clone(),
            actor_registry: self.actor_registry.clone(),
            gpu_health_monitor: self.gpu_health_monitor.get().cloned(),
            logging_config: self.logging_config.clone(),
            credential_blocks: self.credential_blocks().clone(),
        }
        .handle_event(event)
        .await;
    }
}

/// Owned service handles for the `config event handler` task, cloned out of
/// [`ServiceContainer`] by [`ServiceContainer::setup_config_event_subscriptions`]
/// so the spawned future is `'static`.
struct ConfigEventHandler {
    streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    config_service: Arc<RuntimeConfigService>,
    download_manager: Arc<DownloadManager>,
    pipeline_manager: Arc<PipelineManager>,
    actor_registry: Arc<ActorRegistry>,
    gpu_health_monitor: Option<Arc<crate::metrics::GpuHealthMonitor>>,
    logging_config: Arc<std::sync::OnceLock<Arc<crate::logging::LoggingConfig>>>,
    /// Lifted here for streamers that stop being monitored: no later check
    /// would lift it.
    credential_blocks: Arc<crate::credentials::CredentialBlocks>,
}

impl ConfigEventHandler {
    /// Receive loop for [`ConfigUpdateEvent`]s. A lagged receiver
    /// compensates for the skipped events by handling a synthetic
    /// `GlobalUpdated`, the broadest refresh; a closed channel or
    /// cancellation ends the task.
    async fn run(
        self,
        mut receiver: broadcast::Receiver<ConfigUpdateEvent>,
        cancellation_token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("Config event handler shutting down");
                    break;
                }
                result = receiver.recv() => {
                    let event = match result {
                        Ok(event) => Some(event),
                        Err(error) => {
                            if broadcast_error_is_recoverable("config", error) {
                                Some(ConfigUpdateEvent::GlobalUpdated)
                            } else {
                                None
                            }
                        }
                    };
                    let Some(event) = event else {
                        break;
                    };
                    self.handle_event(event).await;
                }
            }
        }
    }

    async fn handle_event(&self, event: ConfigUpdateEvent) {
        match &event {
            ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id } => {
                self.config_service.invalidate_streamer(streamer_id);
                self.actor_registry
                    .refresh_metadata_offline_check(streamer_id)
                    .await;
                let inactive_or_missing = self
                    .streamer_manager
                    .get_streamer(streamer_id)
                    .is_none_or(|metadata| !metadata.is_active());
                if inactive_or_missing {
                    self.credential_blocks.clear(streamer_id);
                }
                self.actor_registry.handle_config_event(&event);
            }
            ConfigUpdateEvent::PlatformUpdated { platform_id } => {
                debug!("Received platform config update event: {}", platform_id);
                let affected: Vec<String> = self
                    .streamer_manager
                    .get_all()
                    .into_iter()
                    .filter(|m| m.platform_config_id == *platform_id)
                    .map(|m| m.id)
                    .collect();
                self.actor_registry
                    .refresh_metadata_offline_checks(affected)
                    .await;
                self.actor_registry.handle_config_event(&event);
            }
            ConfigUpdateEvent::TemplateUpdated { template_id } => {
                debug!("Received template config update event: {}", template_id);
                let affected: Vec<String> = self
                    .streamer_manager
                    .get_all()
                    .into_iter()
                    .filter(|m| m.template_config_id.as_deref() == Some(template_id.as_str()))
                    .map(|m| m.id)
                    .collect();
                self.actor_registry
                    .refresh_metadata_offline_checks(affected)
                    .await;
                self.actor_registry.handle_config_event(&event);
            }
            ConfigUpdateEvent::CredentialMaterialChanged { .. }
            | ConfigUpdateEvent::StreamerErrorCleared { .. } => {
                self.actor_registry.handle_config_event(&event);
            }
            ConfigUpdateEvent::GlobalUpdated => {
                debug!("Received global config update event");
                self.config_service.invalidate_all_filter_snapshots();
                let all_ids: Vec<String> = self
                    .streamer_manager
                    .get_all()
                    .into_iter()
                    .map(|m| m.id)
                    .collect();
                self.actor_registry
                    .refresh_metadata_offline_checks(all_ids)
                    .await;
                match self.config_service.get_global_config().await {
                    Ok(global) => {
                        let new_limit = (global.max_concurrent_downloads as i64).max(1) as usize;
                        let old_limit = self.download_manager.max_concurrent_downloads();
                        if new_limit != old_limit {
                            self.download_manager
                                .set_max_concurrent_downloads(new_limit);
                            info!(
                                "Updated download concurrency: max_concurrent_downloads {} -> {}",
                                old_limit, new_limit
                            );
                        }

                        let old_freshness = self.download_manager.queue_freshness_threshold_ms();
                        let new_freshness = self
                            .download_manager
                            .set_queue_freshness_threshold_ms(global.queue_freshness_threshold_ms);
                        if new_freshness != old_freshness {
                            info!(
                                "Updated queue-wait freshness threshold: {} ms -> {} ms",
                                old_freshness, new_freshness
                            );
                        }

                        if let Some(monitor) = self.gpu_health_monitor.as_ref() {
                            monitor
                                .set_interval(global.gpu_health_probe_interval_secs.max(0) as u64);
                        }

                        if let Some(logging) = self.logging_config.get()
                            && !global.log_filter_directive.is_empty()
                            && logging.get_filter() != global.log_filter_directive
                            && let Err(error) = logging.set_filter(&global.log_filter_directive)
                        {
                            warn!(
                                directive = %global.log_filter_directive,
                                %error,
                                "Failed to apply updated log filter directive"
                            );
                        }

                        let cpu_jobs = autoscale_concurrency_limit(global.max_concurrent_cpu_jobs);
                        let io_jobs = autoscale_concurrency_limit(global.max_concurrent_io_jobs);
                        let uploads = autoscale_concurrency_limit(global.max_concurrent_uploads);
                        self.pipeline_manager
                            .set_worker_concurrency(cpu_jobs, io_jobs, uploads);
                        self.actor_registry
                            .set_check_interval_ms(global.streamer_check_delay_ms as u64);
                    }
                    Err(e) => {
                        warn!(
                            "Failed to reload global config for download concurrency: {}",
                            e
                        );
                    }
                }
                self.actor_registry.handle_config_event(&event);
            }
            ConfigUpdateEvent::StreamerDeleted { streamer_id } => {
                self.config_service.invalidate_streamer(streamer_id);
                self.config_service.invalidate_filter_snapshots(streamer_id);
                self.credential_blocks.clear(streamer_id);
                self.actor_registry.handle_config_event(&event);
            }
            ConfigUpdateEvent::EngineUpdated { engine_id } => {
                debug!("Received engine config update event: {}", engine_id);
            }
            ConfigUpdateEvent::StreamerStateSyncedFromDb {
                streamer_id,
                is_active,
            } => {
                if !is_active {
                    self.credential_blocks.clear(streamer_id);
                }
                self.actor_registry.handle_config_event(&event);
            }
            ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } => {
                self.config_service.invalidate_streamer(streamer_id);
                self.config_service.invalidate_filter_snapshots(streamer_id);
                self.actor_registry.handle_config_event(&event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    mod credential_blocks;
    mod filter_snapshots;
}
