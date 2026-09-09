//! Bounded configuration resolution and generation/revision fencing.

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures::FutureExt;

use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::warn;

use crate::Result;
use crate::config::ConfigUpdateEvent;
use crate::database::repositories::StreamerRepository;
use crate::streamer::StreamerMetadata;

use super::{Scheduler, StreamerConfig};
use crate::scheduler::feedback::DesiredConfig;

const MAX_CONFIG_WORK: usize = 8;
const CONFIG_RETRY: Duration = Duration::from_secs(5);
const CONFIG_TIMEOUT: Duration = Duration::from_secs(10);

struct PendingConfig {
    revision: u64,
    restart: bool,
    due: Instant,
}

pub(super) enum ConfigResult {
    Global {
        revision: u64,
        result: Result<(u64, u64, u32)>,
    },
    Streamer {
        id: String,
        revision: u64,
        generation: Option<u64>,
        restart: bool,
        result: Result<StreamerConfig>,
    },
}

#[derive(Default)]
pub(super) struct ConfigurationWork {
    pub jobs: JoinSet<ConfigResult>,
    pub(super) failure: Option<String>,
    pending: HashMap<String, PendingConfig>,
    running: HashSet<String>,
    revisions: HashMap<String, u64>,
    restart_ready: HashSet<String>,
    global_revision: u64,
    global_pending: bool,
    global_running: bool,
    global_due: Option<Instant>,
}

impl<R: StreamerRepository + Send + Sync + 'static> Scheduler<R> {
    pub(super) fn queue_configuration(&mut self, event: ConfigUpdateEvent) {
        match event {
            ConfigUpdateEvent::GlobalUpdated => {
                self.configuration.global_revision =
                    self.configuration.global_revision.saturating_add(1);
                self.configuration.global_pending = true;
                self.configuration.global_due = None;
            }
            ConfigUpdateEvent::StreamerDeleted { streamer_id } => {
                self.invalidate_configuration(&streamer_id);
                self.feedback.retire(&streamer_id);
                self.remove_streamer(&streamer_id);
            }
            ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id } => {
                self.queue_streamer_configuration(&streamer_id, false)
            }
            ConfigUpdateEvent::StreamerStateSyncedFromDb { streamer_id, .. } => {
                let active = self
                    .streamer_manager
                    .get_streamer(&streamer_id)
                    .is_some_and(|metadata| metadata.is_active());
                if !active || !self.supervisor.registry().has_streamer(&streamer_id) {
                    self.queue_streamer_configuration(&streamer_id, false);
                }
            }
            ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } => {
                self.feedback.request_check(&streamer_id);
                self.queue_streamer_configuration(&streamer_id, false)
            }
            ConfigUpdateEvent::TemplateUpdated { template_id } => {
                for metadata in self.streamer_manager.get_by_template(&template_id) {
                    self.queue_streamer_configuration(&metadata.id, false);
                }
            }
            ConfigUpdateEvent::PlatformUpdated { platform_id } => {
                for metadata in self.streamer_manager.get_by_platform(&platform_id) {
                    self.queue_streamer_configuration(&metadata.id, false);
                }
                self.retain_platform_configuration(&platform_id);
            }
            ConfigUpdateEvent::EngineUpdated { .. } => self.queue_reconciliation(),
        }
        self.pump_configuration();
    }

    pub(super) fn queue_reconciliation(&mut self) {
        let mut ids: HashSet<String> = self
            .supervisor
            .registry()
            .streamer_handles_map()
            .keys()
            .cloned()
            .collect();
        ids.extend(self.configuration.pending.keys().cloned());
        ids.extend(self.configuration.running.iter().cloned());
        ids.extend(self.feedback.pending_streamers());
        ids.extend(
            self.streamer_manager
                .get_all()
                .into_iter()
                .map(|metadata| metadata.id),
        );
        for id in ids {
            self.queue_streamer_configuration(&id, false);
        }
        let platforms: Vec<_> = self
            .supervisor
            .registry()
            .platform_handles_map()
            .keys()
            .cloned()
            .collect();
        for id in platforms {
            self.retain_platform_configuration(&id);
        }
    }

    pub(super) fn invalidate_configuration(&mut self, id: &str) {
        let revision = self
            .configuration
            .revisions
            .entry(id.to_owned())
            .or_default();
        *revision = revision.saturating_add(1);
        self.configuration.pending.remove(id);
        self.configuration.restart_ready.remove(id);
    }

    fn queue_streamer_configuration(&mut self, id: &str, restart: bool) {
        let Some(metadata) = self
            .streamer_manager
            .get_streamer(id)
            .filter(StreamerMetadata::is_active)
        else {
            self.invalidate_configuration(id);
            self.feedback.retire(id);
            self.remove_streamer(id);
            return;
        };
        self.feedback.activate(id);
        self.platform_mapping
            .register(id, &metadata.platform_config_id);
        if self.is_batch_capable_platform(&metadata.platform_config_id)
            && let Err(error) = self.spawn_platform_actor(&metadata.platform_config_id)
        {
            warn!(%error, "Failed to ensure batch platform actor");
        }
        let revision = self
            .configuration
            .revisions
            .entry(id.to_owned())
            .or_default();
        *revision = revision.saturating_add(1);
        let previous = self.configuration.pending.remove(id);
        self.configuration.pending.insert(
            id.to_owned(),
            PendingConfig {
                revision: *revision,
                restart: restart || previous.as_ref().is_some_and(|old| old.restart),
                due: Instant::now(),
            },
        );
    }

    pub(super) fn queue_due_restarts(&mut self) {
        let due = self.supervisor.due_streamer_restart_ids(Instant::now());
        for id in due {
            self.supervisor.defer_streamer_restart(&id, CONFIG_RETRY);
            self.configuration.restart_ready.insert(id.clone());
            if let Some(pending) = self.configuration.pending.get_mut(&id) {
                pending.due = Instant::now();
                pending.restart = true;
            } else if !self.configuration.running.contains(&id) {
                self.queue_streamer_configuration(&id, true);
            }
        }
        // Platform restarts retain their existing supervisor policy.
        self.supervisor.process_pending_restarts_at(Instant::now());
        self.feedback
            .update_targets(self.supervisor.registry().streamer_handles_map());
        self.pump_configuration();
    }

    pub(super) fn next_configuration_time(&self) -> Option<Instant> {
        if self.configuration.jobs.len() >= MAX_CONFIG_WORK {
            return None;
        }
        let streamer = self
            .configuration
            .pending
            .iter()
            .filter(|(id, _)| !self.configuration.running.contains(*id))
            .map(|(_, item)| item.due)
            .min();
        let global = (self.configuration.global_pending && !self.configuration.global_running)
            .then(|| self.configuration.global_due.unwrap_or_else(Instant::now));
        streamer.into_iter().chain(global).min()
    }

    pub(super) fn pump_configuration(&mut self) {
        if self.configuration.global_pending
            && !self.configuration.global_running
            && self
                .configuration
                .global_due
                .is_none_or(|due| due <= Instant::now())
            && self.configuration.jobs.len() < MAX_CONFIG_WORK
        {
            self.configuration.global_pending = false;
            self.configuration.global_running = true;
            let revision = self.configuration.global_revision;
            let repo = self.config_repo.clone();
            let fallback = (
                self.config.check_interval_ms,
                self.config.offline_check_interval_ms,
                self.config.offline_check_count,
            );
            self.configuration.jobs.spawn(async move {
                let result = match repo {
                    Some(repo) => {
                        match tokio::time::timeout(CONFIG_TIMEOUT, repo.get_global_config()).await {
                            Ok(result) => result.map(|global| {
                                (
                                    global.streamer_check_delay_ms as u64,
                                    global.offline_check_delay_ms as u64,
                                    global.offline_check_count as u32,
                                )
                            }),
                            Err(_) => Err(crate::Error::Other(
                                "global scheduler configuration timed out".to_owned(),
                            )),
                        }
                    }
                    None => Ok(fallback),
                };
                ConfigResult::Global { revision, result }
            });
        }
        let due: Vec<_> = self
            .configuration
            .pending
            .iter()
            .filter(|(id, pending)| {
                pending.due <= Instant::now() && !self.configuration.running.contains(*id)
            })
            .take(MAX_CONFIG_WORK.saturating_sub(self.configuration.jobs.len()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some(pending) = self.configuration.pending.remove(&id) else {
                continue;
            };
            let Some(metadata) = self.streamer_manager.get_streamer(&id) else {
                continue;
            };
            let base = StreamerConfig {
                check_interval_ms: self.config.check_interval_ms,
                offline_check_interval_ms: self.config.offline_check_interval_ms,
                offline_check_count: self.config.offline_check_count,
                priority: metadata.priority,
                batch_capable: self.is_batch_capable_platform(&metadata.platform_config_id),
            };
            let resolver = self.config_resolver.clone();
            let cancel = self.cancellation_token.clone();
            let generation = self
                .supervisor
                .registry()
                .get_streamer(&id)
                .map(|handle| handle.generation());
            self.configuration.running.insert(id.clone());
            self.configuration.jobs.spawn(async move {
                let result = AssertUnwindSafe(async { match resolver {
                    Some(resolver) => tokio::select! {
                        biased;
                        _ = cancel.cancelled() => Err(crate::Error::Other("scheduler configuration cancelled".to_owned())),
                        outcome = tokio::time::timeout(CONFIG_TIMEOUT, resolver.resolve(&id, base)) => match outcome {
                            Ok(result) => result,
                            Err(_) => Err(crate::Error::Other("streamer configuration timed out".to_owned())),
                        }
                    },
                    None => Ok(base),
                } }).catch_unwind().await.unwrap_or_else(|_| Err(crate::Error::Other("scheduler configuration resolver panicked".to_owned())));
                ConfigResult::Streamer { id, revision: pending.revision, generation, restart: pending.restart, result }
            });
        }
    }

    pub(super) fn finish_configuration(
        &mut self,
        result: std::result::Result<ConfigResult, tokio::task::JoinError>,
    ) {
        match result {
            Err(error) => {
                warn!(%error, "Scheduler configuration worker failed; stopping scheduler");
                self.configuration.failure =
                    Some(format!("scheduler configuration worker failed: {error}"));
                self.cancellation_token.cancel();
            }
            Ok(ConfigResult::Global { revision, result }) => {
                self.configuration.global_running = false;
                if revision == self.configuration.global_revision {
                    match result {
                        Ok((check, offline, count)) => {
                            self.config.check_interval_ms = check;
                            self.config.offline_check_interval_ms = offline;
                            self.config.offline_check_count = count;
                            self.queue_reconciliation();
                        }
                        Err(error) => {
                            warn!(%error, "Keeping previous global scheduler timing");
                            self.configuration.global_pending = true;
                            self.configuration.global_due = Some(Instant::now() + CONFIG_RETRY);
                        }
                    }
                }
            }
            Ok(ConfigResult::Streamer {
                id,
                revision,
                generation,
                restart,
                result,
            }) => {
                self.configuration.running.remove(&id);
                if self.configuration.revisions.get(&id) != Some(&revision) {
                    self.pump_configuration();
                    return;
                }
                let current_generation = self
                    .supervisor
                    .registry()
                    .get_streamer(&id)
                    .map(|handle| handle.generation());
                if current_generation != generation {
                    self.queue_streamer_configuration(&id, restart);
                } else if let Some(metadata) = self
                    .streamer_manager
                    .get_streamer(&id)
                    .filter(StreamerMetadata::is_active)
                {
                    match result {
                        Ok(config) => {
                            let restart = restart || self.configuration.restart_ready.remove(&id);
                            let waiting_for_restart =
                                !restart && self.supervisor.has_pending_streamer_restart(&id);
                            if restart {
                                self.supervisor
                                    .update_streamer_restart_config(&id, config.clone());
                                self.supervisor.defer_streamer_restart(&id, Duration::ZERO);
                                self.supervisor.process_pending_restarts_at(Instant::now());
                            }
                            if !waiting_for_restart
                                && !self.supervisor.registry().has_streamer(&id)
                                && let Err(error) =
                                    self.spawn_streamer_resolved(metadata, config.clone())
                            {
                                warn!(%id, %error, "Failed to spawn configured actor");
                            }
                            self.supervisor
                                .update_streamer_restart_config(&id, config.clone());
                            self.feedback
                                .update_targets(self.supervisor.registry().streamer_handles_map());
                            if let Some(handle) = self.supervisor.registry().get_streamer(&id) {
                                self.feedback.configure(
                                    &id,
                                    DesiredConfig {
                                        generation: handle.generation(),
                                        revision,
                                        config,
                                    },
                                );
                            }
                        }
                        Err(error) => {
                            warn!(%id, %error, "Retaining scheduler configuration for retry");
                            self.configuration.pending.insert(
                                id,
                                PendingConfig {
                                    revision,
                                    restart,
                                    due: Instant::now() + CONFIG_RETRY,
                                },
                            );
                        }
                    }
                }
            }
        }
        self.pump_configuration();
    }

    #[cfg(test)]
    pub(super) async fn drain_configuration(&mut self) {
        self.pump_configuration();
        while let Some(result) = self.configuration.jobs.join_next().await {
            self.finish_configuration(result);
        }
        self.feedback.drain_configurations().await;
    }

    fn retain_platform_configuration(&mut self, id: &str) {
        let config = self.create_platform_config(id);
        self.supervisor
            .update_platform_restart_config(id, config.clone());
        if let Some(handle) = self.supervisor.registry().get_platform(id) {
            self.feedback.configure_platform(id, handle.clone(), config);
        }
    }
}
