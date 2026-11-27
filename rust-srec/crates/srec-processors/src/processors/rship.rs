//! `rship` processor for low-memory, crash-resumable multi-cloud transfers.
//!
//! Shells out to the `rship` CLI (`copy`/`copyto`, `move`/`moveto`, `sync`, and
//! `mkdir`) with `--json --json-progress --stats 1s` so live progress is read as
//! structured JSONL on `stderr` and per-file outcomes and typed error classes
//! come from the final JSON summary on `stdout`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use srec_core::CoreResult as Result;
use srec_core::domain::processor::{JobLogEntry, LogLevel, ProgressKind};
use srec_core::domain::proxy::{
    OutputScrubber, ProxyEndpoint, ProxyRoute, ProxyRouteResolver, ProxyTarget,
    set_tool_environment, upload_target,
};
use tempfile::TempPath;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use unicode_normalization::UnicodeNormalization;

use super::traits::{
    Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType, TimeAnchor,
    UploadItemStatus, UploadResultItem,
};
use super::upload_runner::RshipCommandOutput;

/// `rship` transfer operation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RshipOperation {
    /// Copy files without deleting source or destination extras (default).
    #[default]
    Copy,
    /// Move files, deleting each source file only after verified remote commit.
    Move,
    /// Mirror source into destination, deleting extraneous destination files.
    Sync,
}

/// Configuration for the `rship` processor.
///
/// Deserialized from the JSON string in `ProcessorInput::config`. Every field
/// defaults when omitted so saved presets continue to load across schema
/// additions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RshipConfig {
    /// Optional `rship` executable path. Falls back to `RSHIP_PATH`, then
    /// `rship` on `PATH`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<String>,

    /// Base remote or local destination (e.g. `gdrive:{streamer}/%Y-%m` or
    /// `r2:my-bucket/{streamer}`). Supports placeholder expansion for
    /// `{platform}`, `{streamer}`, `{title}`, `{streamer_id}`, `{session_id}`,
    /// and strftime-style time tokens (`%Y`, `%m`, `%d`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_root: Option<String>,

    /// Optional path to `rship` `config.toml` (`--config`). When unset, `rship`
    /// uses `RSHIP_CONFIG` or its default OS config location.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,

    /// Optional directory for `rship` SQLite WAL state and token storage
    /// (`--state-dir`). When unset, `rship` uses `RSHIP_STATE_DIR` or its
    /// default OS data directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_dir: Option<String>,

    /// Transfer operation. Defaults to [`RshipOperation::Copy`].
    pub operation: RshipOperation,

    /// Timestamp source for time placeholder expansion.
    pub time_anchor: TimeAnchor,

    /// Attempts per transfer and per destination `mkdir`, at least one.
    /// Defaults to `3`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Free-form extra CLI arguments appended to transfer invocations.
    pub args: Vec<String>,

    // -------- Throughput, concurrency & memory controls --------
    /// `--bwlimit` sustained bandwidth cap (e.g. `"20M"`, `"500K"`, `"1G"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bwlimit: Option<String>,

    /// `--transfers`: concurrent file workers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfers: Option<u32>,

    /// `--adaptive`: enable Falcon dual-lane (`<= 1 MiB` vs `> 1 MiB`)
    /// adaptive concurrency control.
    pub adaptive: bool,

    /// `--part-concurrency`: concurrent parts per parallel-multipart upload
    /// (S3, Azure Blob, Local).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_concurrency: Option<u32>,

    /// `--chunk-size`: protocol upload chunk size override (e.g. `"64M"`,
    /// `"128M"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_size: Option<String>,

    /// `--max-buffer-mem`: hard cap on engine `BufferPool` memory (e.g. `"16M"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_buffer_mem: Option<String>,

    /// `--tpslimit`: global API requests-per-second ceiling across reads and
    /// writes (`0` or `None` leaves requests uncapped).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tpslimit: Option<f64>,

    /// `--drive-stop-on-upload-limit`: fail fast with `QuotaExceeded` on Google
    /// Drive `403 userRateLimitExceeded` daily 750 GiB upload caps.
    pub drive_stop_on_upload_limit: bool,

    /// `--checksum`: compare files by common hash instead of size + mtime.
    pub checksum: bool,

    /// `--strict-source`: freeze source files into local snapshots before
    /// upload.
    pub strict_source: bool,

    /// How `rship` connects; `inherit`, the default, follows the global route.
    pub proxy_route: ProxyRoute,
}

/// Global flags that consume a following value argument in `rship` (`clap`),
/// accepted by every subcommand including `mkdir`.
const GLOBAL_VALUE_FLAGS: &[&str] = &[
    "--state-dir",
    "--config",
    "--ca-cert",
    "--proxy",
    "--connect-timeout",
    "--stall-timeout",
    "--control-timeout",
    "--tpslimit",
    "--low-level-retries",
    "--log-file",
    "--dump",
];

const GLOBAL_BOOL_FLAGS: &[&str] = &[
    "--no-changes-feed",
    "--drive-stop-on-upload-limit",
    "--non-interactive",
    "--verbose",
];

/// Transfer-subcommand flags in `rship` that consume a following value token
/// when written as `--flag <val>` rather than `--flag=<val>`. Tracking them
/// ensures option values (such as `--exclude -v` or `--exclude --proxy`) are
/// never mistaken for standalone flags.
const TRANSFER_VALUE_FLAGS: &[&str] = &[
    "--bwlimit",
    "--transfers",
    "--part-concurrency",
    "--chunk-size",
    "--max-buffer-mem",
    "--strict-source-max-size",
    "--retries",
    "--baseline",
    "--stats",
    "--fsync",
    "--exclude",
    "--include",
    "--exclude-from",
    "--filter-from",
    "--files-from",
    "--min-size",
    "--max-size",
    "--max-age",
    "--min-age",
    "--max-delete",
    "--backup-dir",
];

fn is_short_verbose_flag(arg: &str) -> bool {
    arg.len() >= 2 && arg.starts_with("-v") && arg.as_bytes()[1..].iter().all(|&b| b == b'v')
}

fn flag_takes_separate_value(arg: &str) -> bool {
    GLOBAL_VALUE_FLAGS.contains(&arg) || TRANSFER_VALUE_FLAGS.contains(&arg)
}

/// True when `args` passes `long` in flag position before any `--` terminator,
/// skipping values belonging to preceding value-taking options so
/// `["--exclude", "--proxy"]` is not misread as `--proxy`.
fn has_flag(args: &[String], long: &str) -> bool {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let s = arg.as_str();
        if s == "--" {
            break;
        }
        if s == long || s.starts_with(&format!("{long}=")) {
            return true;
        }
        if flag_takes_separate_value(s) {
            let _ = iter.next();
        }
    }
    false
}

impl RshipConfig {
    /// Global CLI flags (`--tpslimit` and `--drive-stop-on-upload-limit`) valid
    /// on both `mkdir` and transfer subcommands. Per-HTTP-request low-level
    /// retries stay at `rship`'s own default unless overridden in `extra_args`
    /// or `RSHIP_LOW_LEVEL_RETRIES`.
    fn global_args(&self, extra_args: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(tps) = self.tpslimit.filter(|tps| tps.is_finite() && *tps >= 0.0)
            && !has_flag(extra_args, "--tpslimit")
        {
            out.push("--tpslimit".into());
            out.push(tps.to_string());
        }
        if self.drive_stop_on_upload_limit && !has_flag(extra_args, "--drive-stop-on-upload-limit")
        {
            out.push("--drive-stop-on-upload-limit".into());
        }
        out
    }

    /// Transfer-subcommand CLI flags contributed by structured fields, omitting
    /// any flag explicitly overridden in `extra_args` so `clap` never sees
    /// duplicate arguments.
    fn transfer_args(&self, extra_args: &[String]) -> Vec<String> {
        let mut out = Vec::new();

        if let Some(v) = self
            .bwlimit
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && !has_flag(extra_args, "--bwlimit")
        {
            out.push("--bwlimit".into());
            out.push(v.into());
        }
        if let Some(n) = self.transfers
            && !has_flag(extra_args, "--transfers")
        {
            out.push("--transfers".into());
            out.push(n.max(1).to_string());
        }
        if self.adaptive && !has_flag(extra_args, "--adaptive") {
            out.push("--adaptive".into());
        }
        if let Some(n) = self.part_concurrency
            && !has_flag(extra_args, "--part-concurrency")
        {
            out.push("--part-concurrency".into());
            out.push(n.max(1).to_string());
        }
        if let Some(v) = self
            .chunk_size
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && !has_flag(extra_args, "--chunk-size")
        {
            out.push("--chunk-size".into());
            out.push(v.into());
        }
        if let Some(v) = self
            .max_buffer_mem
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && !has_flag(extra_args, "--max-buffer-mem")
        {
            out.push("--max-buffer-mem".into());
            out.push(v.into());
        }
        if self.checksum && !has_flag(extra_args, "--checksum") {
            out.push("--checksum".into());
        }
        if self.strict_source && !has_flag(extra_args, "--strict-source") {
            out.push("--strict-source".into());
        }

        out
    }
}

/// Sanitize user-supplied extra arguments while respecting option values and `--`:
/// - Strips `--progress` and `--tui` (interactive terminal display modes),
///   returning how many interactive tokens were removed so the job log can warn.
/// - Strips redundant `--json` and `--json-progress` tokens silently because
///   `RshipProcessor` always passes both and `clap` rejects duplicate flags.
fn sanitize_extra_args(args: &[String]) -> (Vec<String>, usize) {
    let mut kept = Vec::with_capacity(args.len());
    let mut stripped_interactive = 0usize;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let s = arg.as_str();
        if s == "--" {
            kept.push(arg.clone());
            kept.extend(iter.cloned());
            break;
        }
        match s {
            "--progress" | "--tui" => stripped_interactive += 1,
            "--json" | "--json-progress" => {}
            _ => {
                kept.push(arg.clone());
                if flag_takes_separate_value(s)
                    && let Some(val) = iter.next()
                {
                    kept.push(val.clone());
                }
            }
        }
    }
    (kept, stripped_interactive)
}

/// Extract only global `rship` flags from `extra_args` so `rship mkdir`
/// receives global transport/timeout overrides (e.g. `--ca-cert`,
/// `--connect-timeout`, `-vvv`) without failing on transfer-only flags (e.g.
/// `--exclude`, `--max-delete`) or leaking values of transfer options.
fn extract_global_extra_args(extra_args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = extra_args.iter();
    while let Some(arg) = iter.next() {
        let s = arg.as_str();
        if s == "--" {
            break;
        }
        if GLOBAL_BOOL_FLAGS.contains(&s) || is_short_verbose_flag(s) {
            out.push(arg.clone());
            continue;
        }
        if GLOBAL_VALUE_FLAGS
            .iter()
            .any(|flag| s.starts_with(&format!("{flag}=")))
        {
            out.push(arg.clone());
            continue;
        }
        if GLOBAL_VALUE_FLAGS.contains(&s) {
            out.push(arg.clone());
            if let Some(val) = iter.next() {
                out.push(val.clone());
            }
            continue;
        }
        if TRANSFER_VALUE_FLAGS.contains(&s) {
            let _ = iter.next();
        }
    }
    out
}

const ENSURED_DIR_TTL: Duration = Duration::from_secs(60);
const DIR_LOCK_WAIT: Duration = Duration::from_secs(600);
const MKDIR_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_ATTEMPTS: u32 = 3;

#[derive(PartialEq, Eq, Hash)]
struct EnsuredDir {
    config_path: Option<String>,
    state_dir: Option<String>,
    global_args: Vec<String>,
    path: String,
}

type EnsuredDirs = HashMap<EnsuredDir, tokio::time::Instant>;

struct RemoteDir {
    remote: String,
    path: String,
}

#[derive(Default)]
struct RemoteDirGate {
    remotes: DashMap<String, Arc<tokio::sync::Mutex<EnsuredDirs>>>,
}

impl RemoteDirGate {
    fn remote(&self, name: &str) -> Arc<tokio::sync::Mutex<EnsuredDirs>> {
        self.remotes.entry(name.to_owned()).or_default().clone()
    }
}

/// `--json` output printed on `rship` stdout (transfer `RunReport` or a fatal
/// CLI error envelope).
#[derive(Debug, Default, Deserialize)]
struct RshipJsonReport {
    #[serde(default)]
    fatal: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_category: Option<String>,
    #[serde(default)]
    error_kind: Option<String>,
    #[serde(default)]
    retry_class: Option<String>,
    #[serde(default)]
    causes: Vec<String>,
    #[serde(default)]
    errors: Vec<String>,
    #[serde(default)]
    error_details: Vec<RshipJsonErrorDetail>,
    #[serde(default)]
    conflicts: Vec<RshipJsonConflict>,
    #[serde(default)]
    high_delete_warning: Option<String>,
    #[serde(default)]
    files: Vec<RshipJsonFileResult>,
    #[serde(default)]
    files_truncated: bool,
    #[serde(default)]
    cancelled: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct RshipJsonErrorDetail {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    retry_class: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RshipJsonConflict {
    #[serde(default, alias = "path")]
    nfc_path: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    detail: String,
}

impl RshipJsonConflict {
    fn summary(&self) -> String {
        let mut msg = if self.nfc_path.is_empty() {
            "sync conflict".to_owned()
        } else {
            format!("conflict on {}", self.nfc_path)
        };
        if !self.kind.is_empty() {
            msg.push_str(&format!(" ({})", self.kind));
        }
        if !self.detail.is_empty() {
            msg.push_str(&format!(": {}", self.detail));
        }
        msg
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RshipJsonFileResult {
    path: String,
    status: String,
    #[serde(default)]
    source_deleted: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_kind: Option<String>,
    #[serde(default)]
    error_category: Option<String>,
    #[serde(default)]
    retry_class: Option<String>,
}

/// Retryability classification for a failed `rship` invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RshipFailureClass {
    /// Network glitches, 5xx responses, lock contention (`lock_busy`), expired
    /// upload sessions, or transient errors worth retrying in-run.
    Transient,
    /// Remote rate limit (`rate_limited`, exit `6`) or upload quota exhaustion
    /// (`quota_exceeded`, exit `8`). Skips remaining in-run retries so the
    /// upload worker is freed while keeping the workflow step retryable.
    RateLimited,
    /// Configuration (`config`, exit `7`), authentication (`auth`, exit `4`),
    /// missing path (`not_found`, exit `3`), conflict/usage (exit `2`), or
    /// non-retryable I/O failures.
    Permanent,
}

fn classify_structured_error(
    error_category: Option<&str>,
    error_kind: Option<&str>,
    retry_class: Option<&str>,
) -> Option<RshipFailureClass> {
    let cat = error_category.unwrap_or_default();
    let kind = error_kind.unwrap_or_default();
    let retry = retry_class.unwrap_or_default();

    if cat == "cancelled" || kind == "cancelled" || retry == "cancelled" {
        return Some(RshipFailureClass::Transient);
    }
    if matches!(cat, "rate_limited" | "quota_exceeded")
        || matches!(kind, "rate_limited" | "quota_exceeded")
        || retry == "throttled"
    {
        return Some(RshipFailureClass::RateLimited);
    }
    if matches!(cat, "transient" | "lock_busy")
        || matches!(
            kind,
            "network"
                | "timeout"
                | "session_expired"
                | "source_mutated"
                | "lock_busy"
                | "integrity"
        )
        || matches!(
            retry,
            "retryable" | "session_expired" | "source_mutated" | "integrity"
        )
    {
        return Some(RshipFailureClass::Transient);
    }
    if matches!(
        cat,
        "config" | "auth" | "not_found" | "conflict" | "io" | "other"
    ) || matches!(
        kind,
        "config"
            | "invalid_path"
            | "invalid_geometry"
            | "not_found"
            | "already_exists"
            | "permission_denied"
            | "precondition_failed"
            | "conflict"
            | "unsupported"
            | "state_store"
    ) || retry == "permanent"
    {
        return Some(RshipFailureClass::Permanent);
    }
    None
}

fn classify_rship_failure(
    exit_code: Option<i32>,
    report: Option<&RshipJsonReport>,
    made_move_progress: bool,
) -> RshipFailureClass {
    if exit_code == Some(130) {
        return RshipFailureClass::Transient;
    }

    if let Some(rep) = report {
        if rep.cancelled || rep.error_category.as_deref() == Some("cancelled") {
            return RshipFailureClass::Transient;
        }
        if rep.fatal
            && let Some(fatal_class) = classify_structured_error(
                rep.error_category.as_deref(),
                rep.error_kind.as_deref(),
                rep.retry_class.as_deref(),
            )
        {
            return match fatal_class {
                RshipFailureClass::Permanent if made_move_progress => RshipFailureClass::Transient,
                other => other,
            };
        }

        // Check per-item errors (`files` and `error_details`) before top-level
        // `error_category` / `exit_code`: `rship`'s `primary_error_category()`
        // ranks `auth`/`not_found` above `transient`, whereas a batch mixing
        // permanent and transient failures must remain retryable for the
        // transient files.
        let mut saw_item_error = false;
        let mut all_items_permanent = true;

        for file in &rep.files {
            if file.status != "failed" {
                continue;
            }
            saw_item_error = true;
            match classify_structured_error(
                file.error_category.as_deref(),
                file.error_kind.as_deref(),
                file.retry_class.as_deref(),
            ) {
                Some(RshipFailureClass::RateLimited) => return RshipFailureClass::RateLimited,
                Some(RshipFailureClass::Permanent) => {}
                Some(RshipFailureClass::Transient) | None => {
                    all_items_permanent = false;
                }
            }
        }
        for detail in &rep.error_details {
            saw_item_error = true;
            match classify_structured_error(
                detail.category.as_deref(),
                detail.kind.as_deref(),
                detail.retry_class.as_deref(),
            ) {
                Some(RshipFailureClass::RateLimited) => return RshipFailureClass::RateLimited,
                Some(RshipFailureClass::Permanent) => {}
                Some(RshipFailureClass::Transient) | None => {
                    all_items_permanent = false;
                }
            }
        }
        if saw_item_error {
            return if all_items_permanent && !made_move_progress {
                RshipFailureClass::Permanent
            } else {
                RshipFailureClass::Transient
            };
        }

        if !rep.conflicts.is_empty() && !made_move_progress {
            return RshipFailureClass::Permanent;
        }
        if let Some(top_class) = classify_structured_error(
            rep.error_category.as_deref(),
            rep.error_kind.as_deref(),
            rep.retry_class.as_deref(),
        ) {
            return match top_class {
                RshipFailureClass::Permanent if made_move_progress => RshipFailureClass::Transient,
                other => other,
            };
        }
    }

    // `rship` exit codes:
    //   2 = conflict / CLI usage, 3 = not_found, 4 = auth, 5 = transient,
    //   6 = rate_limited, 7 = config, 8 = quota_exceeded, 9 = lock_busy, 130 = cancelled.
    match exit_code {
        Some(6 | 8) => RshipFailureClass::RateLimited,
        Some(5 | 9 | 130) => RshipFailureClass::Transient,
        Some(2 | 3 | 4 | 7) if !made_move_progress => RshipFailureClass::Permanent,
        _ => RshipFailureClass::Transient,
    }
}

fn parse_json_report(stdout_json: Option<&str>) -> Option<RshipJsonReport> {
    let raw = stdout_json?.trim();
    if !raw.starts_with('{') {
        return None;
    }
    serde_json::from_str::<RshipJsonReport>(raw).ok()
}

/// Surface `rship`'s `high_delete_warning` (omitted from stderr under
/// `--json-progress`) in the job log sink and captured `logs`.
fn record_high_delete_warning(
    report: Option<&RshipJsonReport>,
    context: &ProcessorContext,
    logs: &mut Vec<JobLogEntry>,
) {
    if let Some(warn_msg) = report
        .and_then(|rep| rep.high_delete_warning.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let entry = JobLogEntry::warn(format!("rship: {warn_msg}"));
        context.log(entry.clone());
        logs.push(entry);
    }
}

/// Mirror any `report` errors, warnings, or conflicts not already streamed on
/// `stderr` into the job log sink and captured `logs`.
fn record_report_diagnostics(
    report: Option<&RshipJsonReport>,
    context: &ProcessorContext,
    logs: &mut Vec<JobLogEntry>,
) {
    let Some(rep) = report else {
        return;
    };
    let had_stderr_error = logs.iter().any(|entry| entry.level == LogLevel::Error);
    if !had_stderr_error {
        if let Some(top_err) = rep
            .error
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let entry = JobLogEntry::error(format!("rship: {top_err}"));
            context.log(entry.clone());
            logs.push(entry);
        } else {
            for err in rep.errors.iter().take(10) {
                let trimmed = err.trim();
                if !trimmed.is_empty() {
                    let entry = JobLogEntry::error(format!("rship: {trimmed}"));
                    context.log(entry.clone());
                    logs.push(entry);
                }
            }
        }
    }
    record_high_delete_warning(Some(rep), context, logs);
    for conflict in rep.conflicts.iter().take(10) {
        let entry = JobLogEntry::warn(format!("rship {}", conflict.summary()));
        context.log(entry.clone());
        logs.push(entry);
    }
}

fn summarize_failure_message(
    output: &super::utils::CommandOutput,
    report: Option<&RshipJsonReport>,
) -> String {
    if let Some(rep) = report {
        if let Some(top_err) = rep
            .error
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if let Some(cause) = rep
                .causes
                .last()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty() && !top_err.contains(*s))
            {
                return format!("{top_err}: {cause}");
            }
            return top_err.to_owned();
        }
        if let Some(detail_msg) = rep
            .error_details
            .first()
            .and_then(|d| d.message.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return detail_msg.to_owned();
        }
        if let Some(first_err) = rep.errors.first().filter(|s| !s.trim().is_empty()) {
            return first_err.trim().to_owned();
        }
        if let Some(file_err) = rep
            .files
            .iter()
            .find(|f| f.status == "failed")
            .and_then(|f| f.error.as_deref())
            .filter(|s| !s.trim().is_empty())
        {
            return file_err.trim().to_owned();
        }
        if let Some(conflict) = rep.conflicts.first() {
            return conflict.summary();
        }
    }

    output
        .effective_error_message()
        .map(str::to_owned)
        .unwrap_or_else(|| "Unknown error".to_owned())
}

/// Processor for interacting with the external `rship` CLI.
pub struct RshipProcessor {
    /// Default `rship` binary path resolved from `RSHIP_PATH` or `"rship"`.
    rship_path: String,
    /// Seam allowing unit tests to observe and stub `rship` invocations without
    /// spawning external processes.
    command_runner: Arc<dyn RshipCommandRunner>,
    /// Serializes destination directory creation across concurrent upload jobs.
    dir_gate: RemoteDirGate,
    /// Resolves each step's proxy route; see [`upload_target`].
    routes: Option<Arc<dyn ProxyRouteResolver>>,
}

#[async_trait]
trait RshipCommandRunner: Send + Sync {
    async fn run(
        &self,
        command: &mut Command,
        context: &ProcessorContext,
        stop: &CancellationToken,
    ) -> Result<RshipCommandOutput>;
}

struct ProcessRshipCommandRunner;

#[async_trait]
impl RshipCommandRunner for ProcessRshipCommandRunner {
    async fn run(
        &self,
        command: &mut Command,
        context: &ProcessorContext,
        stop: &CancellationToken,
    ) -> Result<RshipCommandOutput> {
        super::upload_runner::run_rship_with_progress(
            command,
            &context.progress,
            Some(context.log_sink.clone()),
            stop,
        )
        .await
    }
}

struct RshipExecution<'a> {
    binary_path: &'a str,
    remote_destination: &'a str,
    proxy: &'a ProxyTarget,
    operation: RshipOperation,
    config_path: Option<&'a str>,
    state_dir: Option<&'a str>,
    global_args: &'a [String],
    transfer_args: &'a [String],
    extra_args: &'a [String],
    attempts: u32,
    context: &'a ProcessorContext,
}

impl RshipProcessor {
    /// Create a new `rship` processor resolving its binary from `RSHIP_PATH` or
    /// `"rship"` on `PATH`.
    pub fn new() -> Self {
        Self::with_command_runner(
            srec_core::utils::tools::resolve(None, "RSHIP_PATH", "rship"),
            Arc::new(ProcessRshipCommandRunner),
        )
    }

    fn with_command_runner(
        path: impl Into<String>,
        command_runner: Arc<dyn RshipCommandRunner>,
    ) -> Self {
        Self {
            rship_path: path.into(),
            command_runner,
            dir_gate: RemoteDirGate::default(),
            routes: None,
        }
    }

    /// Resolves each step's proxy route through `routes`.
    pub fn with_routes(mut self, routes: Arc<dyn ProxyRouteResolver>) -> Self {
        self.routes = Some(routes);
        self
    }

    /// Build a base `rship` command with proxy environment, `NO_COLOR`, config
    /// path, state directory, and global flags applied.
    ///
    /// Explicit proxy credentials are passed through `RSHIP_PROXY` rather than
    /// `--proxy` so credentials never appear in process argv. Any route other
    /// than System proxy removes inherited `RSHIP_PROXY` and rejects `--proxy`
    /// in `extra_args` so the step's route cannot be silently overridden.
    fn command(&self, execution: &RshipExecution<'_>) -> Result<Command> {
        if has_flag(execution.extra_args, "--files-from") {
            return Err(srec_core::CoreError::Validation(
                "rship's --files-from argument is managed by RshipProcessor; remove it from extra arguments".to_owned(),
            ));
        }

        let mut cmd = Command::new(execution.binary_path);
        cmd.env("NO_COLOR", "1");
        if *execution.proxy != ProxyTarget::System {
            if has_flag(execution.extra_args, "--proxy") {
                return Err(srec_core::CoreError::Validation(
                    "rship's --proxy argument overrides the step's proxy route; configure Proxy on this step (or RSHIP_PROXY with System proxy) instead".to_owned(),
                ));
            }
            cmd.env_remove("RSHIP_PROXY");
        }
        set_tool_environment(&mut cmd, execution.proxy, "rship")?;
        if let Some(endpoint) = execution.proxy.endpoint() {
            cmd.env("RSHIP_PROXY", endpoint.url_with_login());
        }
        if let Some(cfg) = execution.config_path
            && !has_flag(execution.extra_args, "--config")
        {
            cmd.arg("--config").arg(cfg);
        }
        if let Some(dir) = execution.state_dir
            && !has_flag(execution.extra_args, "--state-dir")
        {
            cmd.arg("--state-dir").arg(dir);
        }
        cmd.args(execution.global_args);
        Ok(cmd)
    }

    /// Append transfer telemetry and retry bounds to `cmd` when not already
    /// supplied in `extra_args`.
    fn append_transfer_telemetry_and_retry_args(cmd: &mut Command, extra_args: &[String]) {
        cmd.args(["--json", "--json-progress"]);
        if !has_flag(extra_args, "--stats") {
            cmd.args(["--stats", "1s"]);
        }
        if !has_flag(extra_args, "--retries") {
            cmd.args(["--retries", "1"]);
        }
    }

    fn find_common_base_dir(paths: &[String]) -> Option<PathBuf> {
        let first_parent = Path::new(paths.first()?).parent()?;
        let mut common = first_parent.to_path_buf();

        for path in paths.iter().skip(1) {
            let parent = Path::new(path).parent()?;
            let common_components: Vec<_> = common.components().collect();
            let path_components: Vec<_> = parent.components().collect();

            let mut new_common = PathBuf::new();
            for (a, b) in common_components.iter().zip(path_components.iter()) {
                if a == b {
                    new_common.push(a.as_os_str());
                } else {
                    break;
                }
            }

            if new_common.as_os_str().is_empty() {
                return None;
            }
            common = new_common;
        }

        Some(common)
    }

    /// Write the `--files-from` manifest with forward-slash paths relative to
    /// `base_dir`, matching `rship`'s `RelPath` normalization across platforms.
    async fn create_files_from_list(
        inputs: &[String],
        base_dir: &Path,
    ) -> std::io::Result<TempPath> {
        let named_file = tempfile::Builder::new()
            .prefix(".rship_files_")
            .suffix(".txt")
            .tempfile()?;
        let (file, temp_path) = named_file.into_parts();
        let mut file = tokio::fs::File::from_std(file);

        for input in inputs {
            let relative = Path::new(input)
                .strip_prefix(base_dir)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            let relative_str = relative.to_string_lossy().replace('\\', "/");
            file.write_all(relative_str.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        file.flush().await?;
        Ok(temp_path)
    }

    async fn take_moved_inputs(pending_inputs: &mut Vec<String>) -> Vec<String> {
        let mut moved_inputs = Vec::new();
        let mut still_pending = Vec::with_capacity(pending_inputs.len());
        for input in pending_inputs.drain(..) {
            if super::inputs::is_confirmed_absent(Path::new(&input)).await {
                moved_inputs.push(input);
            } else {
                still_pending.push(input);
            }
        }
        *pending_inputs = still_pending;
        moved_inputs
    }

    /// Splits an `rship` destination into `(remote_name, remote_path)` using the
    /// exact remote-identifier grammar of `rship`'s `TargetPath::parse`
    /// (`len >= 2` and ASCII `[A-Za-z0-9_-]`), returning `None` for local paths
    /// (including single-letter Windows drive roots like `C:/` and local names
    /// containing dots, spaces, or slashes).
    fn split_remote(destination: &str) -> Option<(&str, &str)> {
        let (name, path) = destination.split_once(':')?;
        if name.len() < 2
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return None;
        }
        Some((name, path))
    }

    fn remote_name(destination: &str) -> Option<&str> {
        Self::split_remote(destination).map(|(name, _)| name)
    }

    fn remote_parent(destination: &str) -> Option<&str> {
        let (name, path) = Self::split_remote(destination)?;
        let slash = path.trim_end_matches('/').rfind('/')?;
        (!path[..slash].trim_matches('/').is_empty())
            .then(|| &destination[..name.len() + 1 + slash])
    }

    /// Relative path normalized to forward slashes and UTF-8 NFC, matching
    /// `rship`'s `RelPath` representation in `RunReport.files`.
    fn relative_key(base_dir: &Path, input: &str) -> Option<String> {
        let relative = Path::new(input).strip_prefix(base_dir).ok()?;
        Some(
            relative
                .to_string_lossy()
                .replace('\\', "/")
                .nfc()
                .collect(),
        )
    }

    /// Trim redundant trailing slashes from a local path while preserving POSIX
    /// root `"/"` and Windows drive roots `"C:/"` / `"C:\"`, because `"C:"` on
    /// Windows refers to the drive's current working directory rather than root.
    fn trim_local_root(root: &str) -> &str {
        if root == "/" || root == "\\" {
            return root;
        }
        let bytes = root.as_bytes();
        if bytes.len() == 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\')
        {
            return root;
        }
        let trimmed = root.trim_end_matches(['/', '\\']);
        if trimmed.is_empty() {
            &root[..1]
        } else if trimmed.len() == 2
            && trimmed.as_bytes()[0].is_ascii_alphabetic()
            && trimmed.as_bytes()[1] == b':'
            && root.len() > 2
        {
            &root[..3]
        } else {
            trimmed
        }
    }

    fn join_local_path(local_root: &str, suffix: &str) -> String {
        let base = Self::trim_local_root(local_root);
        if base.ends_with(['/', '\\']) {
            format!("{base}{suffix}")
        } else {
            format!("{base}/{suffix}")
        }
    }

    fn batch_remote_path(base_dir: &Path, remote_root: &str, input: &str) -> Option<String> {
        let relative = Path::new(input)
            .strip_prefix(base_dir)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/");
        if let Some((remote, path)) = Self::split_remote(remote_root) {
            let trimmed = path.trim_matches('/');
            if trimmed.is_empty() {
                Some(format!("{remote}:{relative}"))
            } else {
                Some(format!("{remote}:{trimmed}/{relative}"))
            }
        } else {
            Some(Self::join_local_path(remote_root, &relative))
        }
    }

    fn single_remote_path(remote_destination: &str, file_name: &str, append_name: bool) -> String {
        if !append_name {
            return remote_destination.to_owned();
        }
        if let Some((remote, path)) = Self::split_remote(remote_destination) {
            let trimmed = path.trim_matches('/');
            if trimmed.is_empty() {
                format!("{remote}:{file_name}")
            } else {
                format!("{remote}:{trimmed}/{file_name}")
            }
        } else {
            Self::join_local_path(remote_destination, file_name)
        }
    }

    async fn mkdir_targets(
        inputs: &[String],
        destination: &str,
        batch_base_dir: Option<&Path>,
        skip_absent_inputs: bool,
    ) -> Vec<RemoteDir> {
        let Some(remote) = Self::remote_name(destination) else {
            return Vec::new();
        };
        let mut targets: Vec<RemoteDir> = Vec::new();
        for input in inputs {
            if skip_absent_inputs && super::inputs::is_confirmed_absent(Path::new(input)).await {
                continue;
            }
            let file_destination = match batch_base_dir {
                Some(base_dir) => Self::batch_remote_path(base_dir, destination, input),
                None => Some(destination.to_owned()),
            };
            if let Some(parent) = file_destination.as_deref().and_then(Self::remote_parent)
                && !targets.iter().any(|target| target.path == parent)
            {
                targets.push(RemoteDir {
                    remote: remote.to_owned(),
                    path: parent.to_owned(),
                });
            }
        }
        targets
    }

    /// Serialize remote directory creation across concurrent `rship` subprocesses
    /// targeting the same remote so backends allowing duplicate sibling folder
    /// names (e.g. Google Drive) resolve a single shared parent folder ID.
    async fn ensure_remote_dirs(
        &self,
        targets: &[RemoteDir],
        execution: &RshipExecution<'_>,
    ) -> Result<()> {
        let global_extra = extract_global_extra_args(execution.extra_args);
        let mut combined_global = execution.global_args.to_vec();
        combined_global.extend(global_extra.iter().cloned());

        for RemoteDir { remote, path } in targets {
            let remote_dirs = self.dir_gate.remote(remote);
            let ensured_dir = EnsuredDir {
                config_path: execution.config_path.map(str::to_owned),
                state_dir: execution.state_dir.map(str::to_owned),
                global_args: combined_global.clone(),
                path: path.clone(),
            };

            let mut attempt = 1;
            loop {
                let mut ensured = tokio::time::timeout(DIR_LOCK_WAIT, remote_dirs.lock())
                    .await
                    .map_err(|_| {
                        srec_core::CoreError::Other(format!(
                            "Timed out waiting to create remote directory {path}"
                        ))
                    })?;
                ensured.retain(|_, ensured_at| ensured_at.elapsed() < ENSURED_DIR_TTL);
                if ensured.contains_key(&ensured_dir) {
                    execution.context.info(format!(
                        "Remote directory {path} was created moments ago; skipping mkdir"
                    ));
                    break;
                }

                match self.run_mkdir(path, &global_extra, execution).await {
                    Ok(()) => {
                        execution
                            .context
                            .info(format!("Ensured remote directory {path}"));
                        ensured.insert(ensured_dir, tokio::time::Instant::now());
                        break;
                    }
                    Err((class, error)) => {
                        drop(ensured);
                        let failure = format!(
                            "rship mkdir attempt {attempt} of {} failed: {error}",
                            execution.attempts
                        );
                        match class {
                            RshipFailureClass::Permanent => {
                                execution.context.warn(format!(
                                    "{failure}; error is non-retryable, aborting retries"
                                ));
                                return Err(error);
                            }
                            RshipFailureClass::RateLimited => {
                                execution.context.warn(format!(
                                    "{failure}; remote rate or quota limit hit, skipping remaining in-run retries"
                                ));
                                return Err(error);
                            }
                            RshipFailureClass::Transient => {}
                        }
                        if attempt >= execution.attempts {
                            execution.context.warn(failure);
                            return Err(error);
                        }
                        super::utils::wait_out_retry(
                            execution.context,
                            ProgressKind::Rship,
                            attempt,
                            &failure,
                        )
                        .await;
                        attempt += 1;
                    }
                }
            }
        }
        Ok(())
    }

    async fn run_mkdir(
        &self,
        target: &str,
        global_extra: &[String],
        execution: &RshipExecution<'_>,
    ) -> std::result::Result<(), (RshipFailureClass, srec_core::CoreError)> {
        let mut cmd = self.command(execution).map_err(|error| {
            let class = if error.is_non_retryable_step_error() {
                RshipFailureClass::Permanent
            } else {
                RshipFailureClass::Transient
            };
            (class, error)
        })?;
        cmd.args(["mkdir", target]);
        cmd.args(global_extra);

        // Drive timeout through a child `CancellationToken` rather than dropping
        // the runner future, so `run_logged_command_until` kills and reaps the
        // `rship` process before returning.
        let mkdir_stop = execution.context.cancellation_token.child_token();
        let timeout_stop = mkdir_stop.clone();
        let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let timed_out_flag = timed_out.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(MKDIR_TIMEOUT).await;
            timed_out_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            timeout_stop.cancel();
        });

        let run_result = self
            .command_runner
            .run(&mut cmd, execution.context, &mkdir_stop)
            .await;
        timer.abort();

        if timed_out.load(std::sync::atomic::Ordering::SeqCst)
            && !execution.context.cancellation_token.is_cancelled()
        {
            return Err((
                RshipFailureClass::Transient,
                srec_core::CoreError::Other(format!(
                    "rship mkdir {target} timed out after {}s",
                    MKDIR_TIMEOUT.as_secs()
                )),
            ));
        }

        let command_output = run_result.map_err(|e| {
            (
                RshipFailureClass::Transient,
                srec_core::CoreError::Other(format!("Failed to execute rship mkdir {target}: {e}")),
            )
        })?;
        if !command_output.output.status.success() {
            let exit_code = command_output.output.status.code();
            let report = parse_json_report(command_output.stdout_json.as_deref());
            let class = classify_rship_failure(exit_code, report.as_ref(), false);
            let message = format!(
                "rship mkdir {target} failed with exit code {}: {}",
                exit_code.unwrap_or(-1),
                summarize_failure_message(&command_output.output, report.as_ref())
            );
            let error = match class {
                RshipFailureClass::Permanent => srec_core::CoreError::NonRetryablePipeline(message),
                RshipFailureClass::Transient | RshipFailureClass::RateLimited => {
                    srec_core::CoreError::Other(message)
                }
            };
            return Err((class, error));
        }
        Ok(())
    }

    async fn process_single(
        &self,
        input_path: &str,
        execution: &RshipExecution<'_>,
    ) -> Result<ProcessorOutput> {
        let RshipExecution {
            remote_destination,
            operation,
            transfer_args,
            extra_args,
            attempts,
            context,
            ..
        } = execution;
        let start = std::time::Instant::now();

        let cmd_op = match *operation {
            RshipOperation::Copy => "copyto",
            RshipOperation::Move => "moveto",
            RshipOperation::Sync => {
                return Err(srec_core::CoreError::Validation(
                    "Sync requires batch inputs".to_owned(),
                ));
            }
        };

        info!("rship {}: {} -> {}", cmd_op, input_path, remote_destination);

        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());
        let mut last_error = None;
        let mut permanent_failure = false;
        let mut attempts_used = 0u32;
        let mut logs = Vec::new();

        let build_success = |logs, item_status: UploadItemStatus| ProcessorOutput {
            outputs: match *operation {
                RshipOperation::Move => vec![],
                _ => vec![input_path.to_string()],
            },
            duration_secs: start.elapsed().as_secs_f64(),
            metadata: None,
            items_produced: vec![],
            input_size_bytes,
            output_size_bytes: None,
            failed_inputs: vec![],
            succeeded_inputs: if item_status == UploadItemStatus::Completed {
                vec![input_path.to_string()]
            } else {
                vec![]
            },
            skipped_inputs: if item_status == UploadItemStatus::Skipped {
                vec![(input_path.to_string(), "unchanged on remote".to_string())]
            } else {
                vec![]
            },
            uploads: vec![UploadResultItem {
                local_path: input_path.to_string(),
                remote_path: Some(remote_destination.to_string()),
                size_bytes: input_size_bytes,
                status: item_status,
                error: None,
            }],
            confirmed_uploads: HashMap::new(),
            logs,
            non_retryable: false,
        };

        for attempt in 0..*attempts {
            if matches!(*operation, RshipOperation::Move)
                && (context.is_retry || attempt > 0)
                && super::inputs::is_confirmed_absent(Path::new(input_path)).await
            {
                info!(
                    input = input_path,
                    "rship move source is already absent; treating it as successfully moved"
                );
                return Ok(build_success(logs, UploadItemStatus::Completed));
            }

            if attempt > 0 {
                info!("Retry attempt {} for rship {}", attempt + 1, cmd_op);
                let error = last_error.as_deref().unwrap_or("unknown error");
                super::utils::wait_to_retry(
                    context,
                    ProgressKind::Rship,
                    attempt,
                    *attempts,
                    &format!("rship {cmd_op} attempt {attempt} of {attempts} failed: {error}"),
                )
                .await;
            }
            attempts_used = attempt + 1;

            let mut cmd = self.command(execution)?;
            cmd.args([cmd_op, input_path, remote_destination]);
            Self::append_transfer_telemetry_and_retry_args(&mut cmd, extra_args);
            cmd.args(*transfer_args);
            cmd.args(*extra_args);

            let run_output = match self
                .command_runner
                .run(&mut cmd, context, &context.cancellation_token)
                .await
            {
                Ok(output) => output,
                Err(e) => {
                    if matches!(*operation, RshipOperation::Move)
                        && super::inputs::is_confirmed_absent(Path::new(input_path)).await
                    {
                        warn!(
                            input = input_path,
                            attempt = attempt + 1,
                            error = %e,
                            "rship reported an execution error after moving the source"
                        );
                        return Ok(build_success(logs, UploadItemStatus::Completed));
                    }
                    last_error = Some(format!("Failed to execute rship: {e}"));
                    continue;
                }
            };

            let report = parse_json_report(run_output.stdout_json.as_deref());
            if run_output.output.status.success() {
                let duration = start.elapsed().as_secs_f64();
                info!("rship {} completed in {:.2}s", cmd_op, duration);
                logs.extend(run_output.output.logs);
                record_high_delete_warning(report.as_ref(), context, &mut logs);
                let is_skipped = !matches!(*operation, RshipOperation::Move)
                    && report
                        .as_ref()
                        .and_then(|rep| rep.files.first())
                        .is_some_and(|file| file.status == "skipped" && !file.source_deleted);
                let item_status = if is_skipped {
                    UploadItemStatus::Skipped
                } else {
                    UploadItemStatus::Completed
                };
                return Ok(build_success(logs, item_status));
            }

            let exit_code = run_output.output.status.code();
            let error_msg = summarize_failure_message(&run_output.output, report.as_ref());
            let mut attempt_logs = run_output.output.logs;
            record_report_diagnostics(report.as_ref(), context, &mut attempt_logs);
            let class = classify_rship_failure(exit_code, report.as_ref(), false);
            logs.extend(attempt_logs);

            if matches!(*operation, RshipOperation::Move)
                && super::inputs::is_confirmed_absent(Path::new(input_path)).await
            {
                warn!(
                    input = input_path,
                    attempt = attempt + 1,
                    "rship exited unsuccessfully after moving the source; treating the input as successful"
                );
                return Ok(build_success(logs, UploadItemStatus::Completed));
            }

            last_error = Some(format!(
                "rship failed with exit code {}: {}",
                exit_code.unwrap_or(-1),
                error_msg
            ));

            match class {
                RshipFailureClass::RateLimited => {
                    if attempt + 1 < *attempts {
                        context.warn(format!(
                            "rship {cmd_op} hit a remote rate or quota limit; skipping remaining in-run retries"
                        ));
                    }
                    break;
                }
                RshipFailureClass::Permanent => {
                    permanent_failure = true;
                    context.warn(format!(
                        "rship {cmd_op} encountered a non-retryable error; aborting retries"
                    ));
                    break;
                }
                RshipFailureClass::Transient => {}
            }
        }

        error!("rship {} failed after {} attempts", cmd_op, attempts_used);
        let error = last_error.unwrap_or_else(|| "rship failed".to_string());
        if permanent_failure {
            Err(srec_core::CoreError::NonRetryablePipeline(error))
        } else {
            Err(srec_core::CoreError::Other(error))
        }
    }

    async fn process_batch(
        &self,
        inputs: &[String],
        execution: &RshipExecution<'_>,
    ) -> Result<ProcessorOutput> {
        let RshipExecution {
            remote_destination,
            operation,
            transfer_args,
            extra_args,
            attempts,
            context,
            ..
        } = execution;
        let start = std::time::Instant::now();

        let cmd_op = match *operation {
            RshipOperation::Copy => "copy",
            RshipOperation::Move => "move",
            RshipOperation::Sync => "sync",
        };

        let base_dir = Self::find_common_base_dir(inputs).ok_or_else(|| {
            srec_core::CoreError::Validation(
                "Could not determine common base directory for batch upload".to_string(),
            )
        })?;
        let base_dir_str = base_dir.to_string_lossy().to_string();
        let (mut pending_inputs, already_moved_inputs) =
            if matches!(*operation, RshipOperation::Move) && context.is_retry {
                super::inputs::partition_absent_inputs(inputs).await
            } else {
                (inputs.to_vec(), Vec::new())
            };
        let resumed_inputs = already_moved_inputs.len();
        let file_sizes = super::inputs::input_size_map(&pending_inputs).await;
        let total_input_size = file_sizes.values().copied().sum::<u64>();

        // Accumulate per-file outcomes across all attempts in this run so an
        // input transferred in attempt 1 is never downgraded to `Skipped` or
        // `Failed` if attempt 2 planning-skips it (omitting it from `files`) or
        // fails before emitting JSON.
        let mut completed_set: HashSet<String> = already_moved_inputs.into_iter().collect();
        let mut skipped_set: HashSet<String> = HashSet::new();
        let mut item_errors: HashMap<String, String> = HashMap::new();

        let build_success =
            |logs, attempts, completed: &HashSet<String>, skipped: &HashSet<String>| {
                let mut succeeded_inputs = Vec::new();
                let mut skipped_inputs = Vec::new();
                let mut uploads = Vec::with_capacity(inputs.len());
                for input in inputs {
                    let item_status = if completed.contains(input)
                        || matches!(*operation, RshipOperation::Move)
                        || !skipped.contains(input)
                    {
                        succeeded_inputs.push(input.clone());
                        UploadItemStatus::Completed
                    } else {
                        skipped_inputs.push((input.clone(), "unchanged on remote".to_string()));
                        UploadItemStatus::Skipped
                    };
                    uploads.push(UploadResultItem {
                        local_path: input.clone(),
                        remote_path: Self::batch_remote_path(&base_dir, remote_destination, input),
                        size_bytes: file_sizes.get(input).copied(),
                        status: item_status,
                        error: None,
                    });
                }

                ProcessorOutput {
                    outputs: match *operation {
                        RshipOperation::Move => vec![],
                        _ => inputs.to_vec(),
                    },
                    duration_secs: start.elapsed().as_secs_f64(),
                    metadata: Some(
                        serde_json::json!({
                            "batch_size": inputs.len(),
                            "base_dir": base_dir_str,
                            "operation": cmd_op,
                            "attempts": attempts,
                            "resumed_inputs": resumed_inputs,
                        })
                        .to_string(),
                    ),
                    items_produced: vec![],
                    input_size_bytes: Some(total_input_size),
                    output_size_bytes: None,
                    failed_inputs: vec![],
                    succeeded_inputs,
                    skipped_inputs,
                    uploads,
                    confirmed_uploads: HashMap::new(),
                    logs,
                    non_retryable: false,
                }
            };

        if resumed_inputs > 0 {
            warn!(
                resumed_inputs,
                pending_inputs = pending_inputs.len(),
                "rship move batch contains missing sources; treating them as previously moved"
            );
            context.info(format!(
                "Resuming rship move with {} pending input(s); {} input(s) were already moved",
                pending_inputs.len(),
                resumed_inputs
            ));
        }

        if pending_inputs.is_empty() {
            info!(
                inputs = inputs.len(),
                "rship move batch is already complete because all sources are absent"
            );
            return Ok(build_success(Vec::new(), 0, &completed_set, &skipped_set));
        }

        info!(
            "rship {} batch: {} pending files ({} total) from {} -> {}",
            cmd_op,
            pending_inputs.len(),
            inputs.len(),
            base_dir_str,
            remote_destination
        );

        let mut last_error = None;
        let mut permanent_failure = false;
        let mut attempts_used = 0u32;
        let mut logs = Vec::new();

        for attempt in 0..*attempts {
            if attempt > 0 {
                info!("Retry attempt {} for rship {} batch", attempt + 1, cmd_op);
                let error = last_error.as_deref().unwrap_or("unknown error");
                super::utils::wait_to_retry(
                    context,
                    ProgressKind::Rship,
                    attempt,
                    *attempts,
                    &format!("rship {cmd_op} attempt {attempt} of {attempts} failed: {error}"),
                )
                .await;
            }
            attempts_used = attempt + 1;

            // `sync` must keep all batch inputs in `--files-from` so destination
            // files transferred in an earlier attempt are not treated as extras
            // and deleted; `copy` and `move` send only remaining pending inputs.
            let manifest_inputs: &[String] = if matches!(*operation, RshipOperation::Sync) {
                inputs
            } else {
                &pending_inputs
            };

            let files_from_path = Self::create_files_from_list(manifest_inputs, &base_dir)
                .await
                .map_err(|e| {
                    srec_core::CoreError::io_path("creating rship files-from list", &base_dir, e)
                })?;
            let files_from_path_str = files_from_path.to_string_lossy().to_string();

            let mut cmd = self.command(execution)?;
            cmd.args([
                cmd_op,
                &base_dir_str,
                remote_destination,
                "--files-from",
                &files_from_path_str,
            ]);
            // Direct-stat only the listed files for `copy` and `move` so sibling
            // live recordings are untouched and remote directories are not listed;
            // `sync` keeps full destination observation so extraneous files are pruned.
            if !matches!(*operation, RshipOperation::Sync) && !has_flag(extra_args, "--no-traverse")
            {
                cmd.arg("--no-traverse");
            }
            Self::append_transfer_telemetry_and_retry_args(&mut cmd, extra_args);
            cmd.args(*transfer_args);
            cmd.args(*extra_args);

            let attempt_inputs_snapshot = manifest_inputs.to_vec();
            let command_result = self
                .command_runner
                .run(&mut cmd, context, &context.cancellation_token)
                .await;
            if let Err(e) = files_from_path.close() {
                warn!(
                    path = %files_from_path_str,
                    error = %e,
                    "Failed to remove rship files-from manifest"
                );
            }

            let mut moved_in_attempt = 0usize;
            if matches!(*operation, RshipOperation::Move) {
                let moved_inputs = Self::take_moved_inputs(&mut pending_inputs).await;
                moved_in_attempt = moved_inputs.len();
                for moved in moved_inputs {
                    item_errors.remove(&moved);
                    completed_set.insert(moved);
                }
                if moved_in_attempt > 0 {
                    info!(
                        attempt = attempt + 1,
                        moved_inputs = moved_in_attempt,
                        pending_inputs = pending_inputs.len(),
                        "Reconciled successfully moved rship inputs"
                    );
                    context.info(format!(
                        "rship move attempt {} completed {} input(s); {} remain pending",
                        attempt + 1,
                        moved_in_attempt,
                        pending_inputs.len()
                    ));
                }
            }

            let record_attempt_report =
                |rep: Option<&RshipJsonReport>,
                 completed_set: &mut HashSet<String>,
                 skipped_set: &mut HashSet<String>,
                 item_errors: &mut HashMap<String, String>,
                 pending_inputs: &mut Vec<String>| {
                    let Some(rep) = rep.filter(|r| !r.files_truncated) else {
                        return;
                    };
                    let by_rel: HashMap<&str, &RshipJsonFileResult> =
                        rep.files.iter().map(|f| (f.path.as_str(), f)).collect();

                    for input in &attempt_inputs_snapshot {
                        let Some(rel) = Self::relative_key(&base_dir, input) else {
                            continue;
                        };
                        match by_rel.get(rel.as_str()).copied() {
                            Some(file)
                                if matches!(
                                    file.status.as_str(),
                                    "transferred" | "server_side_copied"
                                ) || file.source_deleted =>
                            {
                                item_errors.remove(input);
                                skipped_set.remove(input);
                                completed_set.insert(input.clone());
                            }
                            Some(file) if file.status == "skipped" => {
                                item_errors.remove(input);
                                if matches!(*operation, RshipOperation::Move) {
                                    completed_set.insert(input.clone());
                                } else if !completed_set.contains(input) {
                                    skipped_set.insert(input.clone());
                                }
                            }
                            Some(file) if file.status == "failed" => {
                                if let Some(err) = file
                                    .error
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                {
                                    item_errors.insert(input.clone(), err.to_owned());
                                }
                            }
                            _ => {}
                        }
                    }

                    if matches!(*operation, RshipOperation::Copy) {
                        pending_inputs.retain(|input| {
                            !completed_set.contains(input) && !skipped_set.contains(input)
                        });
                    }
                };

            match command_result {
                Ok(run_output) if run_output.output.status.success() => {
                    let report = parse_json_report(run_output.stdout_json.as_deref());
                    record_attempt_report(
                        report.as_ref(),
                        &mut completed_set,
                        &mut skipped_set,
                        &mut item_errors,
                        &mut pending_inputs,
                    );
                    logs.extend(run_output.output.logs);
                    record_high_delete_warning(report.as_ref(), context, &mut logs);
                    let duration = start.elapsed().as_secs_f64();
                    info!(
                        "rship {} batch completed in {:.2}s ({} files)",
                        cmd_op,
                        duration,
                        inputs.len()
                    );
                    return Ok(build_success(
                        logs,
                        attempt + 1,
                        &completed_set,
                        &skipped_set,
                    ));
                }
                Ok(run_output) => {
                    let report = parse_json_report(run_output.stdout_json.as_deref());
                    let prev_done = completed_set.len() + skipped_set.len();
                    record_attempt_report(
                        report.as_ref(),
                        &mut completed_set,
                        &mut skipped_set,
                        &mut item_errors,
                        &mut pending_inputs,
                    );
                    let made_progress = moved_in_attempt > 0
                        || (completed_set.len() + skipped_set.len() > prev_done);

                    let error_msg = summarize_failure_message(&run_output.output, report.as_ref());
                    let exit_code = run_output.output.status.code().unwrap_or(-1);
                    let mut attempt_logs = run_output.output.logs;
                    record_report_diagnostics(report.as_ref(), context, &mut attempt_logs);
                    let class = classify_rship_failure(
                        run_output.output.status.code(),
                        report.as_ref(),
                        made_progress,
                    );
                    logs.extend(attempt_logs);

                    if !matches!(*operation, RshipOperation::Sync) && pending_inputs.is_empty() {
                        warn!(
                            attempt = attempt + 1,
                            exit_code,
                            "rship exited unsuccessfully after completing every pending input; treating the batch as successful"
                        );
                        return Ok(build_success(
                            logs,
                            attempt + 1,
                            &completed_set,
                            &skipped_set,
                        ));
                    }

                    last_error = Some(format!(
                        "rship batch failed with exit code {exit_code}: {error_msg}"
                    ));

                    match class {
                        RshipFailureClass::RateLimited => {
                            if attempt + 1 < *attempts {
                                context.warn(format!(
                                    "rship {cmd_op} batch hit a remote rate or quota limit; skipping remaining in-run retries"
                                ));
                            }
                            break;
                        }
                        RshipFailureClass::Permanent => {
                            permanent_failure = true;
                            context.warn(format!(
                                "rship {cmd_op} batch encountered a non-retryable error; aborting retries"
                            ));
                            break;
                        }
                        RshipFailureClass::Transient => {}
                    }
                }
                Err(e) => {
                    if matches!(*operation, RshipOperation::Move) && pending_inputs.is_empty() {
                        warn!(
                            attempt = attempt + 1,
                            error = %e,
                            "rship reported an execution error after moving every pending input"
                        );
                        return Ok(build_success(
                            logs,
                            attempt + 1,
                            &completed_set,
                            &skipped_set,
                        ));
                    }
                    last_error = Some(format!("Failed to execute rship: {e}"));
                }
            }
        }

        error!(
            pending_inputs = pending_inputs.len(),
            completed_inputs = completed_set.len(),
            skipped_inputs = skipped_set.len(),
            attempts = attempts_used,
            "rship {} batch failed",
            cmd_op
        );
        let fallback_error = last_error.unwrap_or_else(|| "rship batch failed".to_string());

        // Surface per-file outcomes whenever partial progress occurred across any
        // attempt (`completed_set` or `skipped_set` non-empty).
        if !completed_set.is_empty() || !skipped_set.is_empty() {
            let mut succeeded_inputs = Vec::new();
            let mut skipped_inputs = Vec::new();
            let mut failed_inputs = Vec::new();
            let mut uploads = Vec::with_capacity(inputs.len());

            for input in inputs {
                if completed_set.contains(input) {
                    succeeded_inputs.push(input.clone());
                    uploads.push(UploadResultItem {
                        local_path: input.clone(),
                        remote_path: Self::batch_remote_path(&base_dir, remote_destination, input),
                        size_bytes: file_sizes.get(input).copied(),
                        status: UploadItemStatus::Completed,
                        error: None,
                    });
                } else if skipped_set.contains(input) {
                    skipped_inputs.push((input.clone(), "unchanged on remote".to_string()));
                    uploads.push(UploadResultItem {
                        local_path: input.clone(),
                        remote_path: Self::batch_remote_path(&base_dir, remote_destination, input),
                        size_bytes: file_sizes.get(input).copied(),
                        status: UploadItemStatus::Skipped,
                        error: None,
                    });
                } else {
                    let item_error = item_errors
                        .get(input)
                        .cloned()
                        .unwrap_or_else(|| fallback_error.clone());
                    failed_inputs.push((input.clone(), item_error.clone()));
                    uploads.push(UploadResultItem {
                        local_path: input.clone(),
                        remote_path: None,
                        size_bytes: file_sizes.get(input).copied(),
                        status: UploadItemStatus::Failed,
                        error: Some(item_error),
                    });
                }
            }

            if !failed_inputs.is_empty() {
                return Ok(ProcessorOutput {
                    outputs: vec![],
                    duration_secs: start.elapsed().as_secs_f64(),
                    metadata: Some(
                        serde_json::json!({
                            "batch_size": inputs.len(),
                            "base_dir": base_dir_str,
                            "operation": cmd_op,
                            "attempts": attempts_used,
                            "resumed_inputs": resumed_inputs,
                            "succeeded_inputs": succeeded_inputs.len(),
                            "skipped_inputs": skipped_inputs.len(),
                            "failed_inputs": failed_inputs.len(),
                        })
                        .to_string(),
                    ),
                    items_produced: vec![],
                    input_size_bytes: Some(total_input_size),
                    output_size_bytes: None,
                    failed_inputs,
                    succeeded_inputs,
                    skipped_inputs,
                    uploads,
                    confirmed_uploads: HashMap::new(),
                    logs,
                    non_retryable: permanent_failure,
                });
            }
        }

        if permanent_failure {
            Err(srec_core::CoreError::NonRetryablePipeline(fallback_error))
        } else {
            Err(srec_core::CoreError::Other(fallback_error))
        }
    }

    fn normalized_destination_root(config: &RshipConfig) -> Option<&str> {
        config
            .destination_root
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    fn determine_remote_destination(input: &ProcessorInput, config: &RshipConfig) -> String {
        let raw = if let Some(out) = input
            .outputs
            .first()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            out.to_owned()
        } else if let Some(root) = Self::normalized_destination_root(config) {
            if Self::split_remote(root).is_some() {
                root.trim_end_matches('/').to_owned()
            } else {
                Self::trim_local_root(root).to_owned()
            }
        } else {
            String::new()
        };

        let reference_timestamp_ms = config.time_anchor.reference_time(input).as_millisecond();
        input.expand_placeholders(&raw, Some(reference_timestamp_ms))
    }
}

impl Default for RshipProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for RshipProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Upload
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["rship"]
    }

    fn name(&self) -> &'static str {
        "RshipProcessor"
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        if input.inputs.is_empty() {
            return Err(srec_core::CoreError::Validation(
                "No input files provided for RshipProcessor".to_string(),
            ));
        }

        let config: RshipConfig =
            super::utils::parse_config_strict(input.config.as_deref(), "rship")?;

        if !matches!(config.operation, RshipOperation::Move) || !ctx.is_retry {
            super::utils::ensure_inputs_exist(&input.inputs).await?;
        }

        let remote_destination = Self::determine_remote_destination(input, &config);
        if remote_destination.trim().is_empty() {
            return Err(srec_core::CoreError::Validation(
                "Destination root is required for RshipProcessor".to_string(),
            ));
        }

        if matches!(config.operation, RshipOperation::Sync) && input.inputs.len() == 1 {
            return Err(srec_core::CoreError::Validation(
                "Sync operation is not supported for single file uploads. \
                 Use 'copy' or 'move' instead. Sync is designed for directory synchronization \
                 and may delete files at the destination."
                    .to_string(),
            ));
        }

        let binary_path = config
            .binary_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.rship_path);
        let config_path = config
            .config_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let state_dir = config
            .state_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let (extra_args, stripped_interactive) = sanitize_extra_args(&config.args);
        if stripped_interactive > 0 {
            ctx.warn(format!(
                "Removed {stripped_interactive} interactive --progress/--tui argument(s) from rship extra args; \
                 transfer progress is streamed via --json-progress instead"
            ));
        }

        let global_args = config.global_args(&extra_args);
        let transfer_args = config.transfer_args(&extra_args);

        let configured_attempts = config.max_retries.unwrap_or(DEFAULT_ATTEMPTS).max(1);
        let attempts = if ctx.has_step_retry {
            if configured_attempts > 1 {
                info!(
                    job_id = %ctx.job_id,
                    configured_attempts,
                    "Workflow step retry policy is active; deferring rship retries to the DAG scheduler"
                );
            }
            1
        } else {
            configured_attempts
        };

        let proxy = upload_target(self.routes.as_deref(), &config.proxy_route).await?;
        // When `ProxyTarget::System` is active and `RSHIP_PROXY` is set in the
        // host environment with embedded credentials, include it in the output
        // scrubber alongside standard `HTTP_PROXY`/`HTTPS_PROXY` secrets.
        let scrubber_target = match (&proxy, std::env::var("RSHIP_PROXY").ok()) {
            (ProxyTarget::System, Some(env_url)) if !env_url.trim().is_empty() => {
                ProxyTarget::Explicit(ProxyEndpoint::from_url(&env_url))
            }
            _ => proxy.clone(),
        };
        let scrubber = OutputScrubber::for_target(&scrubber_target);
        let ctx = &ctx.scrubbing(&scrubber);
        let result = async {
            let skip_absent_inputs =
                matches!(config.operation, RshipOperation::Move) && ctx.is_retry;

            if input.inputs.len() == 1 {
                let input_path = &input.inputs[0];
                let file_name = Path::new(input_path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                let append_name = remote_destination.ends_with(['/', '\\'])
                    || remote_destination.ends_with(':')
                    || Self::normalized_destination_root(&config).is_some();
                let full_destination =
                    Self::single_remote_path(&remote_destination, &file_name, append_name);

                let execution = RshipExecution {
                    binary_path,
                    remote_destination: &full_destination,
                    proxy: &proxy,
                    operation: config.operation,
                    config_path,
                    state_dir,
                    global_args: &global_args,
                    transfer_args: &transfer_args,
                    extra_args: &extra_args,
                    attempts,
                    context: ctx,
                };
                let targets =
                    Self::mkdir_targets(&input.inputs, &full_destination, None, skip_absent_inputs)
                        .await;
                self.ensure_remote_dirs(&targets, &execution).await?;
                self.process_single(input_path, &execution).await
            } else {
                info!("Using rship batch mode for {} files", input.inputs.len());

                let execution = RshipExecution {
                    binary_path,
                    remote_destination: &remote_destination,
                    proxy: &proxy,
                    operation: config.operation,
                    config_path,
                    state_dir,
                    global_args: &global_args,
                    transfer_args: &transfer_args,
                    extra_args: &extra_args,
                    attempts,
                    context: ctx,
                };
                let targets = match Self::find_common_base_dir(&input.inputs) {
                    Some(base_dir) => {
                        Self::mkdir_targets(
                            &input.inputs,
                            &remote_destination,
                            Some(&base_dir),
                            skip_absent_inputs,
                        )
                        .await
                    }
                    None => Vec::new(),
                };
                self.ensure_remote_dirs(&targets, &execution).await?;
                self.process_batch(&input.inputs, &execution).await
            }
        }
        .await;
        ProcessorOutput::scrubbed(result, &scrubber)
    }
}

#[cfg(test)]
mod tests;
