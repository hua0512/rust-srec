//! Dynamic logging configuration with reloadable filters and real-time streaming.
//!
//! This module provides:
//! - Runtime log level changes via `tracing_subscriber::reload`
//! - Broadcast channel for real-time log streaming to WebSocket clients
//! - Coordinated file byte/count limits and seven-day maximum log age
//! - Local timezone timestamps for logs

use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use jiff::{Timestamp, Zoned};
use parking_lot::RwLock;
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

use crate::utils::fs;
use crate::utils::shared_event::{SharedEvent, SharedEventSender, publish_shared};

pub mod store;
use store::{LogStore, LogWriter};

/// Internal backend crate targets governed by the user-facing `rust_srec` filter prefix.
///
/// Users, persisted database configs, `RUST_LOG`, and the web UI configure `rust_srec=<level>`
/// for the application as a whole. Expanding `rust_srec` to these extracted workspace crates
/// preserves that single-target contract without exposing internal crate boundaries.
const INTERNAL_CRATE_TARGETS: &[&str] = &[
    "srec_core",
    "srec_sqlite",
    "srec_processors",
    "srec_engines",
    "srec_notify",
    "srec_pipeline",
    "srec_recorder",
    "srec_api",
];

/// Module prefixes that moved from root `rust_srec::*` into extracted workspace crates.
/// Ordered longest-prefix-first so nested submodules match before their parent module.
const LEGACY_MODULE_ALIASES: &[(&str, &[&str])] = &[
    (
        "rust_srec::pipeline::processors",
        &["srec_processors::processors"],
    ),
    ("rust_srec::downloader::engine", &["srec_engines"]),
    ("rust_srec::domain", &["srec_core::domain"]),
    ("rust_srec::utils", &["srec_core::utils"]),
    ("rust_srec::database", &["srec_sqlite"]),
    ("rust_srec::pipeline", &["srec_pipeline", "srec_processors"]),
    ("rust_srec::notification", &["srec_notify"]),
    ("rust_srec::config", &["srec_recorder::config"]),
    ("rust_srec::credentials", &["srec_recorder::credentials"]),
    ("rust_srec::danmu", &["srec_recorder::danmu"]),
    (
        "rust_srec::downloader",
        &["srec_recorder::downloader", "srec_engines"],
    ),
    ("rust_srec::monitor", &["srec_recorder::monitor"]),
    ("rust_srec::proxies", &["srec_recorder::proxies"]),
    ("rust_srec::recorder", &["srec_recorder::recorder"]),
    ("rust_srec::session", &["srec_recorder::session"]),
    ("rust_srec::streamer", &["srec_recorder::streamer"]),
    ("rust_srec::api", &["srec_api::api"]),
    ("rust_srec::logging", &["srec_api::logging"]),
    ("rust_srec::mcp", &["srec_api::mcp"]),
    ("rust_srec::metrics", &["srec_api::metrics"]),
];

fn split_directives(directive: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut bracket_depth: usize = 0;
    let mut brace_depth: usize = 0;

    for (idx, ch) in directive.char_indices() {
        match ch {
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if bracket_depth == 0 && brace_depth == 0 => {
                let token = directive[start..idx].trim();
                if !token.is_empty() {
                    parts.push(token);
                }
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }

    let tail = directive[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }

    parts
}

fn split_target_and_rest(token: &str) -> (&str, &str) {
    match token.find(['=', '[']) {
        Some(idx) => (&token[..idx], &token[idx..]),
        None => (token, ""),
    }
}

fn match_legacy_module_alias(target: &str) -> Option<(&'static [&'static str], &str)> {
    LEGACY_MODULE_ALIASES
        .iter()
        .find_map(|(old_prefix, new_prefixes)| {
            target.strip_prefix(old_prefix).and_then(|subpath| {
                if subpath.is_empty() || subpath.starts_with("::") {
                    Some((*new_prefixes, subpath))
                } else {
                    None
                }
            })
        })
}

fn expand_filter_directive(directive: &str) -> String {
    let tokens = split_directives(directive);
    if tokens.is_empty() {
        return directive.to_string();
    }

    // Track internal crates explicitly targeted at crate root (directly or via a
    // crate-root legacy module alias) so umbrella `rust_srec=<level>` expansion
    // never overwrites a more specific directive regardless of order.
    let mut overridden_internal_crates: HashSet<&str> = HashSet::new();
    for &token in &tokens {
        let (target, rest) = split_target_and_rest(token);
        if rest.starts_with('[') {
            continue;
        }
        if INTERNAL_CRATE_TARGETS.contains(&target) {
            overridden_internal_crates.insert(target);
        } else if let Some((new_prefixes, subpath)) = match_legacy_module_alias(target)
            && subpath.is_empty()
        {
            for &new_prefix in new_prefixes {
                if !new_prefix.contains("::") {
                    overridden_internal_crates.insert(new_prefix);
                }
            }
        }
    }

    let mut expanded = Vec::with_capacity(tokens.len() + INTERNAL_CRATE_TARGETS.len());
    for token in tokens {
        let (target, rest) = split_target_and_rest(token);
        if target == "rust_srec" {
            expanded.push(token.to_string());
            for &crate_target in INTERNAL_CRATE_TARGETS {
                if rest.starts_with('[') || !overridden_internal_crates.contains(crate_target) {
                    expanded.push(format!("{crate_target}{rest}"));
                }
            }
        } else if let Some((new_prefixes, subpath)) = match_legacy_module_alias(target) {
            expanded.push(token.to_string());
            for (idx, &new_prefix) in new_prefixes.iter().enumerate() {
                if subpath.is_empty() || idx == 0 {
                    expanded.push(format!("{new_prefix}{subpath}{rest}"));
                }
            }
        } else {
            expanded.push(token.to_string());
        }
    }

    expanded.join(",")
}

fn build_effective_env_filter(
    directive: &str,
) -> std::result::Result<EnvFilter, tracing_subscriber::filter::ParseError> {
    // Validate the user's raw directive first so syntax errors report against their input.
    EnvFilter::try_new(directive)?;
    let expanded = expand_filter_directive(directive);
    EnvFilter::try_new(expanded)
}

/// Validate a tracing `EnvFilter` directive string without applying it.
pub fn validate_filter_directive(directive: &str) -> std::result::Result<(), String> {
    build_effective_env_filter(directive)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Default log filter directive.
pub const DEFAULT_LOG_FILTER: &str = "rust_srec=info,sqlx=warn,mesio_engine=info,flv=info,hls=info";

/// Broadcast channel capacity for log events.
const LOG_BROADCAST_CAPACITY: usize = 1024;

/// Custom timer that uses the local timezone.
///
/// This timer formats timestamps using the server's local timezone
/// instead of UTC, making logs easier to correlate with local time.
#[derive(Debug, Clone, Copy)]
struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        let now = Zoned::now();
        write!(w, "{}", now.strftime("%Y-%m-%dT%H:%M:%S%.3f%:z"))
    }
}

/// Type alias for the reload handle.
pub type FilterHandle = Handle<EnvFilter, tracing_subscriber::Registry>;

/// A single log event for broadcasting to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    pub timestamp: Timestamp,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Logging configuration with reloadable filter and broadcast capability.
pub struct LoggingConfig {
    handle: FilterHandle,
    configured_filter: RwLock<String>,
    log_tx: broadcast::Sender<LogEvent>,
    shared_log_tx: Arc<SharedEventSender<LogEvent>>,
    log_dir: PathBuf,
    store: Arc<LogStore>,
}

impl LoggingConfig {
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_route_tests(
        log_dir: PathBuf,
    ) -> (Self, reload::Layer<EnvFilter, tracing_subscriber::Registry>) {
        let (layer, handle) = reload::Layer::new(EnvFilter::new("info"));
        let (sender, _) = broadcast::channel(LOG_BROADCAST_CAPACITY);
        let store = Arc::new(LogStore::new(log_dir.clone(), Default::default()));
        (
            Self::new(
                handle,
                "info".to_string(),
                sender,
                Arc::new(SharedEventSender::new(LOG_BROADCAST_CAPACITY)),
                log_dir,
                store,
            ),
            layer,
        )
    }

    /// Create a new logging configuration.
    fn new(
        handle: FilterHandle,
        configured_filter: String,
        log_tx: broadcast::Sender<LogEvent>,
        shared_log_tx: Arc<SharedEventSender<LogEvent>>,
        log_dir: PathBuf,
        store: Arc<LogStore>,
    ) -> Self {
        Self {
            handle,
            configured_filter: RwLock::new(configured_filter),
            log_tx,
            shared_log_tx,
            log_dir,
            store,
        }
    }

    /// Get the current user-facing filter directive string.
    pub fn get_filter(&self) -> String {
        self.configured_filter.read().clone()
    }

    /// Set a new filter directive.
    ///
    /// # Arguments
    /// * `directive` - Filter string (e.g., "rust_srec=debug,sqlx=warn")
    ///
    /// # Returns
    /// Error if the directive is invalid.
    pub fn set_filter(&self, directive: &str) -> crate::Result<()> {
        let new_filter = build_effective_env_filter(directive)
            .map_err(|e| crate::Error::Other(format!("Invalid filter directive: {}", e)))?;

        self.handle
            .reload(new_filter)
            .map_err(|e| crate::Error::Other(format!("Failed to reload filter: {}", e)))?;

        *self.configured_filter.write() = directive.to_string();
        info!(directive = %directive, "Log filter updated");
        Ok(())
    }

    /// Subscribe to log events for real-time streaming.
    pub fn subscribe(&self) -> broadcast::Receiver<LogEvent> {
        self.log_tx.subscribe()
    }

    pub(crate) fn subscribe_shared(&self) -> broadcast::Receiver<Arc<SharedEvent<LogEvent>>> {
        self.shared_log_tx.subscribe()
    }

    /// Broadcast a log event to all subscribers.
    pub fn broadcast(&self, event: LogEvent) {
        // Ignore errors - just means no subscribers currently
        publish_shared(&self.log_tx, &self.shared_log_tx, event);
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

    pub async fn run_retention_cleanup(&self, cancel_token: CancellationToken) {
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
                move || store.reconcile(Timestamp::now())
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

    /// Apply a persisted log filter directive if non-empty.
    ///
    /// Logs a warning on invalid directives rather than failing startup.
    pub fn apply_persisted_filter_directive(&self, directive: &str) {
        if directive.is_empty() {
            return;
        }
        match self.set_filter(directive) {
            Ok(()) => info!(filter = %directive, "Applied persisted log filter"),
            Err(e) => warn!("Failed to apply persisted log filter: {}", e),
        }
    }
}

/// Custom layer that broadcasts log events.
struct BroadcastLayer {
    tx: broadcast::Sender<LogEvent>,
    shared_tx: Arc<SharedEventSender<LogEvent>>,
}

impl<S> Layer<S> for BroadcastLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        if self.tx.receiver_count() == 0 && self.shared_tx.receiver_count() == 0 {
            return;
        }
        let metadata = event.metadata();

        // Extract message from the event
        let mut message = String::new();
        let mut visitor = MessageVisitor(&mut message);
        event.record(&mut visitor);

        let log_event = LogEvent {
            timestamp: Timestamp::now(),
            level: metadata.level().to_string(),
            target: metadata.target().to_string(),
            message,
        };

        // Broadcast - ignore errors (no subscribers)
        publish_shared(&self.tx, &self.shared_tx, log_event);
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
    let store = file_appender.store.clone();
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // Create reloadable filter
    let (initial_directive, initial_filter) = std::env::var(EnvFilter::DEFAULT_ENV)
        .ok()
        .and_then(|raw| {
            build_effective_env_filter(&raw)
                .ok()
                .map(|filter| (raw, filter))
        })
        .unwrap_or_else(|| {
            let filter = build_effective_env_filter(DEFAULT_LOG_FILTER)
                .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
            (DEFAULT_LOG_FILTER.to_string(), filter)
        });
    let (filter_layer, filter_handle) = reload::Layer::new(initial_filter);

    // Create broadcast channel for log streaming
    let (log_tx, _) = broadcast::channel(LOG_BROADCAST_CAPACITY);
    let shared_log_tx = Arc::new(SharedEventSender::new(LOG_BROADCAST_CAPACITY));
    let broadcast_layer = BroadcastLayer {
        tx: log_tx.clone(),
        shared_tx: shared_log_tx.clone(),
    };

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

    let config = Arc::new(LoggingConfig::new(
        filter_handle,
        initial_directive,
        log_tx,
        shared_log_tx,
        log_path,
        store,
    ));

    Ok((config, guard))
}

fn build_file_appender(log_path: &Path) -> crate::Result<LogWriter> {
    let store = Arc::new(LogStore::from_env(log_path.to_path_buf())?);
    store
        .initialize()
        .map_err(|error| crate::Error::io_path("opening rotating log file", log_path, error))?;
    Ok(LogWriter::new(store))
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
    fn rust_srec_filter_covers_extracted_crates_and_respects_specific_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let (config, filter_layer) = LoggingConfig::for_route_tests(dir.path().to_owned());
        let mut rx = config.subscribe();
        let drain = |rx: &mut broadcast::Receiver<LogEvent>| {
            while rx.try_recv().is_ok() {}
        };
        let subscriber = tracing_subscriber::registry()
            .with(filter_layer)
            .with(BroadcastLayer {
                tx: config.log_tx.clone(),
                shared_tx: config.shared_log_tx.clone(),
            });

        tracing::subscriber::with_default(subscriber, || {
            config.set_filter(DEFAULT_LOG_FILTER).unwrap();
            assert_eq!(config.get_filter(), DEFAULT_LOG_FILTER);
            drain(&mut rx);

            tracing::info!(target: "srec_api::api::routes::config", "api info");
            tracing::debug!(target: "srec_api::api::routes::config", "api debug filtered");
            tracing::info!(target: "srec_recorder::downloader::manager", "recorder info");

            let first = rx.try_recv().unwrap();
            assert_eq!(first.target, "srec_api::api::routes::config");
            assert_eq!(first.message, "api info");
            let second = rx.try_recv().unwrap();
            assert_eq!(second.target, "srec_recorder::downloader::manager");
            assert_eq!(second.message, "recorder info");
            assert!(rx.try_recv().is_err());

            // Specific internal crate override wins regardless of order against umbrella `rust_srec`.
            config.set_filter("srec_api=debug,rust_srec=warn").unwrap();
            assert_eq!(config.get_filter(), "srec_api=debug,rust_srec=warn");
            drain(&mut rx);
            tracing::debug!(target: "srec_api::api::routes::config", "api debug allowed");
            tracing::info!(target: "srec_recorder::monitor", "recorder info filtered");
            let event = rx.try_recv().unwrap();
            assert_eq!(event.message, "api debug allowed");
            assert!(rx.try_recv().is_err());

            // Legacy `rust_srec::<module>` targets map to extracted crates even when listed first.
            config
                .set_filter("rust_srec::pipeline=debug,rust_srec::api=debug,rust_srec=warn")
                .unwrap();
            assert_eq!(
                config.get_filter(),
                "rust_srec::pipeline=debug,rust_srec::api=debug,rust_srec=warn"
            );
            drain(&mut rx);
            tracing::debug!(target: "srec_pipeline::manager", "pipeline debug");
            tracing::debug!(target: "srec_processors::processors::remux", "processor debug");
            tracing::debug!(target: "srec_api::api::routes::config", "api subtarget debug");
            tracing::info!(target: "srec_api::logging", "sibling logging info filtered");
            assert_eq!(rx.try_recv().unwrap().message, "pipeline debug");
            assert_eq!(rx.try_recv().unwrap().message, "processor debug");
            assert_eq!(rx.try_recv().unwrap().message, "api subtarget debug");
            assert!(rx.try_recv().is_err());
        });
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
        let shared = Arc::new(SharedEventSender::new(LOG_BROADCAST_CAPACITY));
        let subscriber = tracing_subscriber::registry().with(BroadcastLayer {
            tx: tx.clone(),
            shared_tx: shared.clone(),
        });
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(value = ?Observed(&formatted));
            assert_eq!(formatted.load(Ordering::SeqCst), 0);
            let mut rx = tx.subscribe();
            tracing::info!(value = ?Observed(&formatted));
            assert_eq!(formatted.load(Ordering::SeqCst), 1);
            assert_eq!(rx.try_recv().unwrap().message, "value: observed");
            drop(rx);
            let mut shared_rx = shared.subscribe();
            tracing::info!(value = ?Observed(&formatted));
            assert_eq!(formatted.load(Ordering::SeqCst), 2);
            assert_eq!(
                shared_rx.try_recv().unwrap().event.message,
                "value: observed"
            );
            drop(shared_rx);
            tracing::info!(value = ?Observed(&formatted));
            assert_eq!(formatted.load(Ordering::SeqCst), 2);
        });
    }

    #[tokio::test]
    async fn retention_runs_before_first_daily_wait() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("rust-srec.log.2000-01-01");
        let unrelated = dir.path().join("other.log.2000-01-01");
        let today = dir.path().join(format!(
            "rust-srec.log.{}",
            Timestamp::now().strftime("%Y-%m-%d")
        ));
        for path in [&old, &unrelated, &today] {
            tokio::fs::write(path, b"keep unless expired")
                .await
                .unwrap();
        }
        let (_, handle) =
            reload::Layer::<EnvFilter, tracing_subscriber::Registry>::new(EnvFilter::new("info"));
        let (tx, _) = broadcast::channel(4);
        let store = Arc::new(LogStore::new(dir.path().to_owned(), Default::default()));
        let config = LoggingConfig::new(
            handle,
            "info".to_string(),
            tx,
            Arc::new(SharedEventSender::new(LOG_BROADCAST_CAPACITY)),
            dir.path().to_owned(),
            store,
        );
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
}
