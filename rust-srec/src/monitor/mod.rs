use crate::{
    config::ConfigService,
    danmu::DanmuService,
    database::{repositories::StreamerRepository, DatabaseService},
    domain::{streamer::Streamer, types::StreamerState},
    notification::events::SystemEvent,
};
use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use std::sync::Arc;
use tokio::{
    sync::broadcast,
    time::{self, Duration},
};
use tracing::{error, info, warn};

const ERROR_THRESHOLD: u32 = 3;
const INITIAL_BACKOFF_DELAY_SECS: i64 = 60;

pub struct StreamMonitor {
    streamer_id: String,
    config_service: Arc<ConfigService>,
    streamers_repo: Arc<dyn StreamerRepository>,
    db_service: Arc<DatabaseService>,
    danmu_service: Arc<DanmuService>,
    event_sender: broadcast::Sender<SystemEvent>,
}

impl StreamMonitor {
    pub async fn new(
        streamer_id: String,
        config_service: Arc<ConfigService>,
        streamers_repo: Arc<dyn StreamerRepository>,
        db_service: Arc<DatabaseService>,
        event_sender: broadcast::Sender<SystemEvent>,
    ) -> Result<Self> {
        let streamer = streamers_repo.get_streamer(&streamer_id).await?;
        let danmu_service = Arc::new(DanmuService::new(&streamer)?);
        Ok(Self {
            streamer_id,
            config_service,
            streamers_repo,
            db_service,
            danmu_service,
            event_sender,
        })
    }

    pub async fn run(self) -> Result<()> {
        info!("Starting monitor for streamer {}", self.streamer_id);

        let merged_config = self
            .config_service
            .get_merged_config(&self.streamer_id)
            .await?;

        let check_interval = Duration::from_millis(merged_config.streamer_check_delay_ms as u64);
        let mut interval = time::interval(check_interval);

        loop {
            interval.tick().await;
            info!("Checking status for streamer {}", self.streamer_id);

            let streamer = match self.streamers_repo.get_streamer(&self.streamer_id).await {
                Ok(s) => s,
                Err(e) => {
                    error!(
                        "Failed to get streamer {} from DB, stopping monitor: {}",
                        self.streamer_id, e
                    );
                    break;
                }
            };

            // Fetch the latest merged configuration for the streamer
            let latest_merged_config =
                match self.config_service.get_merged_config(&self.streamer_id).await {
                    Ok(config) => config,
                    Err(e) => {
                        warn!(
                            "Failed to get merged config for streamer {}: {}",
                            self.streamer_id, e
                        );
                        continue;
                    }
                };

            // Use the platforms crate to check the live status of streamers
            let extractor =
                new_extractor(&streamer.url.0, latest_merged_config.cookies.clone())?;
            let live_status = extractor.get_live_status().await;

            match live_status {
                Ok(status) => {
                    self.handle_monitoring_success().await?;
                    // Apply filters (e.g., time, title)
                    info!("Streamer {} status: {:?}", self.streamer_id, status);
                    // Placeholder for filter logic
                    let should_record = true;

                    if should_record {
                        // Start danmu collection
                        self.danmu_service.start_collection();

                        // Update the streamer's state in the Database Service
                        // Placeholder for state update
                        info!("Streamer {} should be recorded", self.streamer_id);

                        // Placeholder for initiating downloads
                        info!("Initiating download for streamer {}", self.streamer_id);
                    } else {
                        // Stop danmu collection
                        if let Err(e) = self.danmu_service.stop_collection() {
                            warn!(
                                "Failed to stop danmu collection for streamer {}: {}",
                                self.streamer_id, e
                            );
                        }
                    }
                }
                Err(e) => {
                    self.handle_monitoring_error(e).await?;
                }
            }
        }
        Ok(())
    }

    async fn handle_monitoring_success(&self) -> Result<()> {
        let mut streamer = self.streamers_repo.get_streamer(&self.streamer_id).await?;
        if streamer.consecutive_error_count > 0 {
            info!(
                "Streamer {} is back online, resetting error count.",
                self.streamer_id
            );
            streamer.consecutive_error_count = 0;
            // If it was temporarily disabled, set it back to NotLive
            if streamer.state == StreamerState::TemporalDisabled {
                streamer.state = StreamerState::NotLive;
            }
            self.streamers_repo.update(streamer).await?;
        }
        Ok(())
    }

    async fn handle_monitoring_error(&self, e: anyhow::Error) -> Result<()> {
        let error_message = format!(
            "Failed to get live status for streamer {}: {}",
            self.streamer_id, e
        );
        warn!("{}", error_message);

        let mut streamer = self.streamers_repo.find_by_id(&self.streamer_id).await??;
        streamer.consecutive_error_count += 1;

        if streamer.consecutive_error_count >= ERROR_THRESHOLD {
            let backoff_duration_secs = INITIAL_BACKOFF_DELAY_SECS
                * 2i64.pow(streamer.consecutive_error_count - ERROR_THRESHOLD);
            let disabled_until = Utc::now() + ChronoDuration::seconds(backoff_duration_secs);

            streamer.state = StreamerState::TemporalDisabled;
            streamer.disabled_until = Some(disabled_until);

            warn!(
                "Streamer {} has failed {} consecutive checks. Temporarily disabling until {}.",
                self.streamer_id, streamer.consecutive_error_count, disabled_until
            );
        } else {
            info!(
                "Streamer {} failed check. Consecutive errors: {}",
                self.streamer_id, streamer.consecutive_error_count
            );
        }

        self.streamers_repo.update(streamer).await?;

        if self
            .event_sender
            .send(SystemEvent::FatalError(error_message))
            .is_err()
        {
            error!("Failed to send FatalError event");
        }
        Ok(())
    }
}