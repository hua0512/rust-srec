use std::sync::Arc;

use chrono::Utc;
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::downloader::DownloadManagerEvent;
use crate::monitor::MonitorEvent;
use crate::pipeline::PipelineEvent;

use super::NotificationService;
use crate::notification::events::mapping::{
    PipelineNotifications, download_notification, monitor_notification, session_notification,
};

impl NotificationService {
    /// Listen for monitor events.
    pub(super) fn listen_for_monitor_events(
        self: &Arc<Self>,
        mut rx: broadcast::Receiver<MonitorEvent>,
    ) {
        let service = Arc::clone(self);
        let config = service.config.clone();
        let cancellation_token = service.cancellation_token.clone();
        let task_supervisor = service.task_supervisor.clone();

        task_supervisor.spawn("monitor notification listener", async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Monitor event listener shutting down");
                        break;
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                if !config.enabled {
                                    continue;
                                }

                                if !event.should_notify() {
                                    continue;
                                }

                                // StreamOnline/StreamOffline come from
                                // `SessionTransition` via
                                // `listen_for_session_transitions`; the monitor
                                // subscription only produces FatalError alerts.
                                let notification = monitor_notification(event);

                                if let Some(notification) = notification {
                                    service.dispatch_notification(notification);
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Monitor event listener lagged by {} events", n);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                debug!("Monitor event channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Listen for session lifecycle transitions and fan them out as
    /// `NotificationEvent::StreamOnline` / `StreamOffline` payloads.
    ///
    /// `SessionTransition::Started` / `Ended` supply streamer identity,
    /// title, category, and timestamps for notification channel payloads.
    pub(super) fn listen_for_session_transitions(
        self: &Arc<Self>,
        mut rx: broadcast::Receiver<crate::session::SessionTransition>,
    ) {
        let service = Arc::clone(self);
        let config = service.config.clone();
        let cancellation_token = service.cancellation_token.clone();
        let task_supervisor = service.task_supervisor.clone();

        task_supervisor.spawn("session notification listener", async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Session transition listener shutting down");
                        break;
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(transition) => {
                                if !config.enabled {
                                    continue;
                                }

                                let notification = session_notification(transition);

                                if let Some(notification) = notification {
                                    service.dispatch_notification(notification);
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Session transition listener lagged by {} events", n);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                debug!("Session transition channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Listen for download events.
    pub(super) fn listen_for_download_events(
        self: &Arc<Self>,
        mut rx: broadcast::Receiver<DownloadManagerEvent>,
    ) {
        let service = Arc::clone(self);
        let config = service.config.clone();
        let cancellation_token = service.cancellation_token.clone();
        let task_supervisor = service.task_supervisor.clone();

        task_supervisor.spawn("download notification listener", async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Download event listener shutting down");
                        break;
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                if !config.enabled {
                                    continue;
                                }

                                let notification = download_notification(event, Utc::now());

                                if let Some(notification) = notification {
                                    service.dispatch_notification(notification);
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Download event listener lagged {} events", n);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                debug!("Download event channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Listen for pipeline events.
    pub(super) fn listen_for_pipeline_events(
        self: &Arc<Self>,
        mut rx: broadcast::Receiver<PipelineEvent>,
    ) {
        let service = Arc::clone(self);
        let config = service.config.clone();
        let cancellation_token = service.cancellation_token.clone();
        let task_supervisor = service.task_supervisor.clone();

        task_supervisor.spawn("pipeline notification listener", async move {
            let mut mapper = PipelineNotifications::default();
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Pipeline event listener shutting down");
                        break;
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                if !config.enabled {
                                    continue;
                                }

                                let notification = mapper.map(event, Utc::now());

                                if let Some(notification) = notification {
                                    service.dispatch_notification(notification);
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Pipeline event listener lagged by {} events", n);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                debug!("Pipeline event channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }
}
