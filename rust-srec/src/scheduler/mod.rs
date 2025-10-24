use crate::{
    config::ConfigService, database::DatabaseService, monitor::StreamMonitor,
    notification::events::SystemEvent,
};
use anyhow::Result;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::{
    sync::broadcast,
    task::JoinHandle,
    time::{interval, Duration},
};
use tracing::{error, info};

const REFRESH_INTERVAL_SECS: u64 = 60;

pub struct Scheduler {
    config_service: Arc<ConfigService>,
    db_service: Arc<DatabaseService>,
    monitors: Arc<DashMap<String, JoinHandle<()>>>,
    event_sender: broadcast::Sender<SystemEvent>,
}

impl Scheduler {
    pub async fn new(
        config_service: Arc<ConfigService>,
        db_service: Arc<DatabaseService>,
        event_sender: broadcast::Sender<SystemEvent>,
    ) -> Result<Self> {
        let monitors = Arc::new(DashMap::new());
        let scheduler = Self {
            config_service,
            db_service,
            monitors,
            event_sender,
        };
        scheduler.spawn_refresh_task();
        Ok(scheduler)
    }

    fn spawn_refresh_task(&self) {
        let config_service = self.config_service.clone();
        let db_service = self.db_service.clone();
        let monitors = self.monitors.clone();
        let event_sender = self.event_sender.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(REFRESH_INTERVAL_SECS));
            loop {
                interval.tick().await;
                info!("Refreshing streamer monitors...");
                if let Err(e) =
                    refresh_monitors(config_service.clone(), db_service.clone(), monitors.clone(), event_sender.clone()).await
                {
                    error!("Failed to refresh monitors: {}", e);
                }
            }
        });
    }
}

async fn refresh_monitors(
    config_service: Arc<ConfigService>,
    db_service: Arc<DatabaseService>,
    monitors: Arc<DashMap<String, JoinHandle<()>>>,
    event_sender: broadcast::Sender<SystemEvent>,
) -> Result<()> {
    let streamers = db_service.get_all_streamers().await?;

    // Stop monitors for disabled or deleted streamers
    monitors.retain(|streamer_id, monitor| {
        if let Some(streamer) = streamers.iter().find(|s| &s.id == streamer_id) {
            if streamer.is_enabled() {
                true // Keep the monitor
            } else {
                monitor.abort();
                false // Remove the monitor
            }
        } else {
            monitor.abort();
            false // Streamer deleted, remove the monitor
        }
    });

    // Start monitors for new or re-enabled streamers
    for streamer in streamers {
        if streamer.is_enabled() && !monitors.contains_key(&streamer.id) {
            let streamer_id = streamer.id.clone();
            let monitor_result = StreamMonitor::new(
                streamer_id.clone(),
                config_service.clone(),
                db_service.clone(),
                event_sender.clone(),
            )
            .await;

            match monitor_result {
                Ok(monitor) => {
                    let monitor_handle = tokio::spawn(async move {
                        if let Err(e) = monitor.run().await {
                            error!("Monitor for streamer {} failed: {}", streamer_id, e);
                        }
                    });
                    monitors.insert(streamer.id.clone(), monitor_handle);
                }
                Err(e) => {
                    error!("Failed to create monitor for streamer {}: {}", streamer.id, e);
                }
            }
        }
    }

    Ok(())
}