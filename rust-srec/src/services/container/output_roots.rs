//! Output-root discovery and recovery helpers.

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::downloader::{LAST_ERROR_GATE_PREFIX, RecoveryHook};
use crate::streamer::StreamerManager;
use crate::utils::task_supervisor::TaskSupervisor;

/// Build the recovery hook closure for the output-root write gate.
///
/// The closure iterates the streamer metadata store, finds every streamer
/// whose `last_error` starts with [`LAST_ERROR_GATE_PREFIX`] (i.e., was
/// placed in backoff by `InfraBlockReason::OutputRootUnavailable`), and
/// clears their error state via `StreamerManager::clear_error_state` so
/// they immediately re-enter the live-check rotation.
///
/// Invoked by [`crate::downloader::OutputRootGate::mark_healthy`] on every `Degraded → Healthy`
/// transition. The synchronous portion only snapshots IDs; database writes
/// are handed to the application task supervisor.
pub(super) fn build_output_root_gate_recovery_hook<R>(
    streamer_manager: Arc<StreamerManager<R>>,
    task_supervisor: Arc<TaskSupervisor>,
) -> RecoveryHook
where
    R: crate::database::repositories::streamer::StreamerRepository + Send + Sync + 'static,
    StreamerManager<R>: Send + Sync + 'static,
{
    Arc::new(move |root: &std::path::Path| {
        // Build the exact prefix this root's streamers would carry in
        // `last_error`. `set_infra_blocked` writes
        //     "output-root blocked: {root.display()} ({io_kind})"
        // so we filter by "output-root blocked: {root.display()} " (with
        // trailing space) to discriminate between streamers blocked on THIS
        // root vs a different Degraded root. Without the trailing space,
        // "/rec" would also match "/rec/huya" — which would wipe streamers
        // that are still legitimately blocked.
        let root_marker = format!("{} {} ", LAST_ERROR_GATE_PREFIX, root.display());

        // Snapshot affected streamer IDs first so we don't hold a DashMap
        // iterator across await points. `metadata_store()` returns an
        // `Arc<DashMap<_>>`; iteration holds per-bucket read locks.
        let affected_ids: Vec<String> = streamer_manager
            .metadata_store()
            .iter()
            .filter(|entry| {
                entry
                    .last_error
                    .as_deref()
                    .is_some_and(|s| s.starts_with(&root_marker))
            })
            .map(|entry| entry.key().clone())
            .collect();

        if affected_ids.is_empty() {
            debug!(
                root = %root.display(),
                "Output-root gate recovery hook fired but no affected streamers found"
            );
            return;
        }

        info!(
            root = %root.display(),
            count = affected_ids.len(),
            "Output-root gate recovered; clearing error state for affected streamers"
        );

        // Keep database writes outside the synchronous gate callback and
        // serialize them to avoid a write burst during fleet recovery.
        let sm = streamer_manager.clone();
        task_supervisor.spawn("output-root recovery", async move {
            for id in affected_ids {
                if let Err(e) = sm.clear_error_state(&id).await {
                    warn!(
                        streamer_id = %id,
                        error = %e,
                        "Failed to clear error state during gate recovery (non-fatal)"
                    );
                }
            }
        });
    })
}

/// Extract the static root-prefix from a user-configured `output_folder`
/// template (e.g. `"/rec/{platform}/{streamer}/%Y%m%d"`), used by the
/// startup probe to derive a mount root from a template without
/// evaluating its placeholders.
///
/// Algorithm:
/// 1. Truncate at the first `{` (curly-brace variable) or `%` (strftime
///    placeholder) — everything after is streamer/date-dependent and not
///    part of the mount.
/// 2. Trim to end at the last `/` so we don't emit a partial directory
///    name (e.g. `/recordings-` from `/recordings-{streamer}/files`).
/// 3. Return `None` for relative, empty, or root-only prefixes that
///    carry no useful probe signal (relative templates would anchor to
///    the container's CWD, which is unpredictable).
///
/// Examples:
///   `"/rec/{platform}/{streamer}"` → `Some("/rec/")`
///   `"/home/{user}/recordings/"` → `Some("/home/")`
///   `"/app/output"` (no placeholder) → `Some("/app/")`
///   `"/recordings-{streamer}/files"` → `None` (last-complete-segment is `/`)
///   `"{streamer}/files"` (no root) → `None`
///   `"recordings/{streamer}"` (relative) → `None`
pub(super) fn static_root_prefix(template: &str) -> Option<String> {
    if !template.starts_with('/') {
        return None;
    }
    let cut = template.find(['{', '%']).unwrap_or(template.len());
    let prefix = &template[..cut];
    let last_slash = prefix.rfind('/')?;
    let result = &prefix[..=last_slash];
    if result.is_empty() || result == "/" {
        None
    } else {
        Some(result.to_string())
    }
}

/// Read `RUST_SREC_OUTPUT_ROOTS` from the environment and parse it into a
/// list of absolute paths. The value is comma-separated; empty entries are
/// skipped. Relative paths are rejected with a warning (they would anchor
/// to the current working directory, which is unpredictable inside Docker).
pub(super) fn parse_output_roots_env() -> Vec<std::path::PathBuf> {
    let Ok(raw) = std::env::var("RUST_SREC_OUTPUT_ROOTS") else {
        return Vec::new();
    };
    raw.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let p = std::path::PathBuf::from(s);
            if p.is_absolute() {
                Some(p)
            } else {
                warn!(
                    entry = %s,
                    "Ignoring non-absolute entry in RUST_SREC_OUTPUT_ROOTS"
                );
                None
            }
        })
        .collect()
}

pub(super) fn sqlite_file_path_from_url(url: &str) -> Option<std::path::PathBuf> {
    let url = url.strip_prefix("sqlite:")?;
    let path_part = url.split('?').next().unwrap_or(url);

    if path_part.is_empty() || path_part == ":memory:" || path_part.starts_with(":memory:") {
        return None;
    }

    let normalized = path_part.strip_prefix("///").unwrap_or(path_part);
    Some(std::path::PathBuf::from(normalized))
}
