//! Dynamic logging configuration with reloadable filters and real-time streaming.
//!
//! This module provides:
//! - Runtime log level changes via `tracing_subscriber::reload`
//! - Broadcast channel for real-time log streaming to WebSocket clients
//! - Coordinated file byte/count limits and seven-day maximum log age
//! - Local timezone timestamps for logs

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{Event, Subscriber, debug, info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::Writer, time::FormatTime},
    layer::SubscriberExt,
    reload::{self, Handle},
    util::SubscriberInitExt,
};

use crate::database::repositories::{ConfigRepository, StreamerRepository};
use crate::utils::fs;

pub(crate) mod store;
use store::{LogStore, LogWriter};

/// Default log filter directive.
pub const DEFAULT_LOG_FILTER: &str = "rust_srec=info,sqlx=warn,mesio_engine=info,flv=info,hls=info";

/// Broadcast channel capacity for log events.
const LOG_BROADCAST_CAPACITY: usize = 1024;

/// Custom timer that uses the local timezone via chrono.
///
/// This timer formats timestamps using the server's local timezone
/// instead of UTC, making logs easier to correlate with local time.
#[derive(Debug, Clone, Copy)]
struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        let now = Local::now();
        write!(w, "{}", now.format("%Y-%m-%dT%H:%M:%S%.3f%:z"))
    }
}

/// Type alias for the reload handle.
pub type FilterHandle = Handle<EnvFilter, tracing_subscriber::Registry>;

/// A single log event for broadcasting to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Logging configuration with reloadable filter and broadcast capability.
pub struct LoggingConfig {
    handle: FilterHandle,
    log_tx: broadcast::Sender<LogEvent>,
    log_dir: PathBuf,
    store: Arc<LogStore>,
}

impl LoggingConfig {
    #[cfg(test)]
    pub(crate) fn for_route_tests(
        log_dir: PathBuf,
    ) -> (Self, reload::Layer<EnvFilter, tracing_subscriber::Registry>) {
        let (layer, handle) = reload::Layer::new(EnvFilter::new("info"));
        let (sender, _) = broadcast::channel(LOG_BROADCAST_CAPACITY);
        let store = Arc::new(LogStore::new(log_dir.clone(), Default::default()));
        (Self::new(handle, sender, log_dir, store), layer)
    }

    /// Create a new logging configuration.
    fn new(
        handle: FilterHandle,
        log_tx: broadcast::Sender<LogEvent>,
        log_dir: PathBuf,
        store: Arc<LogStore>,
    ) -> Self {
        Self {
            handle,
            log_tx,
            log_dir,
            store,
        }
    }

    /// Get the current filter directive string.
    pub fn get_filter(&self) -> String {
        self.handle
            .with_current(|filter| filter.to_string())
            .unwrap_or_default()
    }

    /// Set a new filter directive.
    ///
    /// # Arguments
    /// * `directive` - Filter string (e.g., "rust_srec=debug,sqlx=warn")
    ///
    /// # Returns
    /// Error if the directive is invalid.
    pub fn set_filter(&self, directive: &str) -> crate::Result<()> {
        let new_filter = EnvFilter::try_new(directive)
            .map_err(|e| crate::Error::Other(format!("Invalid filter directive: {}", e)))?;

        self.handle
            .reload(new_filter)
            .map_err(|e| crate::Error::Other(format!("Failed to reload filter: {}", e)))?;

        info!(directive = %directive, "Log filter updated");
        Ok(())
    }

    /// Subscribe to log events for real-time streaming.
    pub fn subscribe(&self) -> broadcast::Receiver<LogEvent> {
        self.log_tx.subscribe()
    }

    /// Broadcast a log event to all subscribers.
    pub fn broadcast(&self, event: LogEvent) {
        // Ignore errors - just means no subscribers currently
        let _ = self.log_tx.send(event);
    }

    /// Get the log directory path.
    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// Start the log retention cleanup task.
    ///
    /// Runs daily and deletes log files older than 7 days.
    pub fn start_retention_cleanup(self: &Arc<Self>, cancel_token: CancellationToken) {
        let config = Arc::clone(self);

        tokio::spawn(async move {
            config.run_retention_cleanup(cancel_token).await;
        });
    }

    pub(crate) async fn run_retention_cleanup(&self, cancel_token: CancellationToken) {
        let cleanup_interval = Duration::from_secs(24 * 60 * 60);

        loop {
            if cancel_token.is_cancelled() {
                break;
            }
            // Cleanup also runs on startup so frequent restarts cannot prevent
            // retention from ever taking effect.
            // Reconciliation can prune files. Once admitted, keep its blocking
            // work owned through cancellation instead of leaving deletion running
            // after the supervised cleanup task reports that it has stopped.
            match tokio::task::spawn_blocking({
                let store = self.store.clone();
                move || store.reconcile(Utc::now())
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("Failed to maintain log retention: {error}"),
                Err(error) => eprintln!("Log retention task failed: {error}"),
            }
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    debug!("Log retention cleanup task shutting down");
                    break;
                }
                _ = tokio::time::sleep(cleanup_interval) => {}
            }
        }
    }

    /// Apply persisted log filter from the config service.
    ///
    /// Loads the `log_filter_directive` from the database and applies it.
    /// Logs warnings on errors but doesn't fail.
    pub async fn apply_persisted_filter<C, S>(
        &self,
        config_service: &Arc<crate::config::ConfigService<C, S>>,
    ) where
        C: ConfigRepository + Send + Sync + 'static,
        S: StreamerRepository + Send + Sync + 'static,
    {
        match config_service.get_global_config().await {
            Ok(global_config) if !global_config.log_filter_directive.is_empty() => {
                match self.set_filter(&global_config.log_filter_directive) {
                    Ok(()) => {
                        info!(filter = %global_config.log_filter_directive, "Applied persisted log filter")
                    }
                    Err(e) => warn!("Failed to apply persisted log filter: {}", e),
                }
            }
            Ok(_) => {} // Empty filter directive, use default
            Err(e) => warn!("Failed to load persisted log filter: {}", e),
        }
    }
}

/// Custom layer that broadcasts log events.
struct BroadcastLayer {
    tx: broadcast::Sender<LogEvent>,
}

impl<S> Layer<S> for BroadcastLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        if self.tx.receiver_count() == 0 {
            return;
        }
        let metadata = event.metadata();

        // Extract message from the event
        let mut message = String::new();
        let mut visitor = MessageVisitor(&mut message);
        event.record(&mut visitor);

        let log_event = LogEvent {
            timestamp: Utc::now(),
            level: metadata.level().to_string(),
            target: metadata.target().to_string(),
            message,
        };

        // Broadcast - ignore errors (no subscribers)
        let _ = self.tx.send(log_event);
    }
}

/// Visitor to extract the message field from a tracing event.
struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            *self.0 = format!("{:?}", value);
        } else if self.0.is_empty() {
            // Fallback: use any field if no message field
            *self.0 = format!("{}: {:?}", field.name(), value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" || self.0.is_empty() {
            *self.0 = value.to_string();
        }
    }
}

/// Initialize logging with reloadable filter and broadcast capability.
///
/// # Arguments
/// * `log_dir` - Directory for log files
///
/// # Returns
/// Tuple of (LoggingConfig, WorkerGuard) - keep the guard alive for the app lifetime
pub fn init_logging(log_dir: &str) -> crate::Result<(Arc<LoggingConfig>, WorkerGuard)> {
    let log_path = PathBuf::from(log_dir);

    // Create log directory if it doesn't exist
    fs::ensure_dir_all_sync_with_op("creating log directory", &log_path)?;

    // File coordination and rotation run on the nonblocking writer's worker.
    let file_appender = build_file_appender(&log_path)?;
    let store = file_appender.0.clone();
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // Create reloadable filter
    let initial_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    let (filter_layer, filter_handle) = reload::Layer::new(initial_filter);

    // Create broadcast channel for log streaming
    let (log_tx, _) = broadcast::channel(LOG_BROADCAST_CAPACITY);
    let broadcast_layer = BroadcastLayer { tx: log_tx.clone() };

    // Build and initialize the subscriber with local timezone timestamps
    tracing_subscriber::registry()
        .with(filter_layer)
        .with(
            fmt::layer()
                .with_ansi(std::io::stdout().is_terminal())
                .with_timer(LocalTimer),
        )
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_timer(LocalTimer),
        ) // File output with local time
        .with(broadcast_layer)
        .try_init()
        .map_err(|e| {
            crate::Error::Other(format!("Failed to set global default subscriber: {}", e))
        })?;

    let config = Arc::new(LoggingConfig::new(filter_handle, log_tx, log_path, store));

    Ok((config, guard))
}

fn build_file_appender(log_path: &Path) -> crate::Result<LogWriter> {
    let store = Arc::new(LogStore::from_env(log_path.to_path_buf())?);
    store
        .initialize()
        .map_err(|error| crate::Error::io_path("opening rotating log file", log_path, error))?;
    Ok(LogWriter(store))
}

/// Available logging modules for documentation/API responses.
pub fn available_modules() -> Vec<(&'static str, &'static str)> {
    vec![
        ("rust_srec", "Main application"),
        ("mesio_engine", "Download engine (mesio)"),
        ("flv", "FLV parser"),
        ("flv_fix", "FLV stream fixing pipeline"),
        ("hls", "HLS parser"),
        ("hls_fix", "HLS stream fixing pipeline"),
        ("platforms_parser", "Platform URL extractors"),
        ("pipeline_common", "Shared pipeline utilities"),
        ("sqlx", "Database queries"),
        ("reqwest", "HTTP requests"),
        ("tower_http", "HTTP middleware"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appender_open_failure_returns_path_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".rust-srec.log.lock");
        std::fs::create_dir(&path).unwrap();
        let result = init_logging(dir.path().to_str().unwrap());
        assert!(matches!(
            result,
            Err(crate::Error::IoPath {
                op: "opening rotating log file",
                ..
            })
        ));
        assert!(path.is_dir());
    }

    #[test]
    fn broadcast_formats_only_for_connected_receivers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Observed<'a>(&'a AtomicUsize);
        impl std::fmt::Debug for Observed<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fetch_add(1, Ordering::SeqCst);
                f.write_str("observed")
            }
        }
        let formatted = AtomicUsize::new(0);
        let (tx, rx) = broadcast::channel(4);
        drop(rx);
        let subscriber = tracing_subscriber::registry().with(BroadcastLayer { tx: tx.clone() });
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(value = ?Observed(&formatted));
            assert_eq!(formatted.load(Ordering::SeqCst), 0);
            let mut rx = tx.subscribe();
            tracing::info!(value = ?Observed(&formatted));
            assert_eq!(formatted.load(Ordering::SeqCst), 1);
            assert_eq!(rx.try_recv().unwrap().message, "value: observed");
            drop(rx);
            tracing::info!(value = ?Observed(&formatted));
            assert_eq!(formatted.load(Ordering::SeqCst), 1);
        });
    }

    #[tokio::test]
    async fn retention_runs_before_first_daily_wait() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("rust-srec.log.2000-01-01");
        let unrelated = dir.path().join("other.log.2000-01-01");
        let today = dir
            .path()
            .join(format!("rust-srec.log.{}", Utc::now().format("%Y-%m-%d")));
        for path in [&old, &unrelated, &today] {
            tokio::fs::write(path, b"keep unless expired")
                .await
                .unwrap();
        }
        let (_, handle) =
            reload::Layer::<EnvFilter, tracing_subscriber::Registry>::new(EnvFilter::new("info"));
        let (tx, _) = broadcast::channel(4);
        let store = Arc::new(LogStore::new(dir.path().to_owned(), Default::default()));
        let config = LoggingConfig::new(handle, tx, dir.path().to_owned(), store);
        let cancel = CancellationToken::new();
        let cleanup = config.run_retention_cleanup(cancel.clone());
        let verify = async {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    // Windows can reject metadata queries while deletion is pending.
                    // Observe namespace removal without reopening the deleting file.
                    let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
                    let mut found = false;
                    while let Some(entry) = entries.next_entry().await.unwrap() {
                        found |= entry.file_name() == old.file_name().unwrap();
                    }
                    if !found {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            cancel.cancel();
        };
        tokio::join!(cleanup, verify);
        assert!(unrelated.exists());
        assert!(today.exists());
    }

    #[test]
    fn test_default_filter() {
        assert!(DEFAULT_LOG_FILTER.contains("rust_srec=info"));
        assert!(DEFAULT_LOG_FILTER.contains("sqlx=warn"));
    }

    #[test]
    fn test_log_event_serialization() {
        let event = LogEvent {
            timestamp: Utc::now(),
            level: "INFO".to_string(),
            target: "rust_srec::api".to_string(),
            message: "Test message".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("INFO"));
        assert!(json.contains("Test message"));
    }

    #[test]
    fn test_available_modules() {
        let modules = available_modules();
        assert!(!modules.is_empty());
        assert!(modules.iter().any(|(name, _)| *name == "rust_srec"));
    }
}
