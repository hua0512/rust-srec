//! Execution log persistence, live progress aggregation, and upload/long-job WebSocket events.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use dashmap::DashMap;
use futures::FutureExt;
use futures::future::{BoxFuture, OptionFuture};
use jiff::Timestamp;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::{
    ActiveJobInfo, AttemptToken, EXECUTION_INFO_MAX_LOGS, HEARTBEAT_INTERVAL, Job,
    JobExecutionInfo, JobLogEntry, JobQueue, LOG_PERSISTENCE_LOCK_SHARDS, LogCursorCleanup,
    LogLevel, LogPersistence, LongJobTrack, MAX_PERSISTED_LOG_ROWS_PER_JOB,
    PROGRESS_FLUSH_INTERVAL_MS, UPLOAD_QUEUE_POLL_INTERVAL, db_model_to_job, non_empty,
    upload_kind_for_job_type,
};
use crate::Result;
use crate::database::models::job::LogEntry as DbLogEntry;
use crate::database::models::{JobExecutionLogDbModel, JobStatus, Pagination, UploadRecordDbModel};
use crate::database::repositories::JobRepository;
use crate::pipeline::processors::{UploadItemStatus, UploadResultItem, utils as processor_utils};
use crate::pipeline::progress::{
    JobProgressSnapshot, JobProgressUpdate, ProgressKind, ProgressReporter,
};
use crate::pipeline::upload_events::{
    UploadStatusBroadcaster, UploadStatusEvent, UploadTerminalStatus,
};
use crate::utils::json::{self, JsonContext};

/// `UploadStatusEvent::Started` for `job`, or `None` when it is not an
/// upload job.
pub(super) fn upload_started_event(job: &Job) -> Option<UploadStatusEvent> {
    let uploader = upload_kind_for_job_type(&job.job_type)?;
    Some(UploadStatusEvent::Started {
        job_id: job.id.clone(),
        streamer_id: non_empty(&job.streamer_id),
        streamer_name: job.streamer_name.clone(),
        session_id: non_empty(&job.session_id),
        uploader,
        files_total: job.inputs.len() as u32,
        started_at_ms: job
            .started_at
            .unwrap_or_else(jiff::Timestamp::now)
            .as_millisecond(),
    })
}

/// True when `job` is not an upload and has been processing for at least
/// `long_job_after` (see [`super::JobQueueConfig::long_job_after`]) at `now`. Only
/// such jobs reach the WebSocket, which keeps per-segment thumbnails and quick
/// renames off it.
pub(super) fn is_long_job(job: &Job, now: Timestamp, long_job_after: std::time::Duration) -> bool {
    upload_kind_for_job_type(&job.job_type).is_none()
        && job.started_at.is_some_and(|started| {
            std::time::Duration::try_from(now.duration_since(started))
                .is_ok_and(|elapsed| elapsed >= long_job_after)
        })
}

pub(super) fn long_job_started_event(job: &Job) -> UploadStatusEvent {
    UploadStatusEvent::JobStarted {
        job_id: job.id.clone(),
        streamer_id: non_empty(&job.streamer_id),
        streamer_name: job.streamer_name.clone(),
        job_type: job.job_type.clone(),
        started_at_ms: job
            .started_at
            .unwrap_or_else(jiff::Timestamp::now)
            .as_millisecond(),
    }
}

/// `JobProgress` for a long job, `Progress` for an upload.
pub(super) fn progress_event(
    long_job: bool,
    job_id: String,
    streamer_id: Option<String>,
    snapshot: JobProgressSnapshot,
) -> UploadStatusEvent {
    if long_job {
        UploadStatusEvent::JobProgress {
            job_id,
            streamer_id,
            snapshot,
        }
    } else {
        UploadStatusEvent::Progress {
            job_id,
            streamer_id,
            snapshot,
        }
    }
}

/// Re-announces every live upload job and long job (see [`is_long_job`]) at
/// `now`. Uploaders can go silent for minutes (BaiduPCS-Go prints nothing
/// while it prepares a file), and clients drop jobs they have not heard
/// about for a while so that a lost terminal event cannot leave one on
/// screen forever. Repeating the latest progress, or the start event before
/// there is any, keeps a silent job alive without inventing new numbers.
pub(super) fn heartbeat_events(
    jobs_cache: &DashMap<String, Job>,
    cancellation_tokens: &DashMap<String, AttemptToken>,
    long_job_tracking: &DashMap<String, LongJobTrack>,
    progress_cache: &DashMap<String, JobProgressSnapshot>,
) -> Vec<UploadStatusEvent> {
    // Collected before touching jobs_cache: cancel_pipeline locks jobs_cache
    // and then cancellation_tokens, so holding a token guard here while
    // locking jobs_cache could deadlock against it.
    let live_job_ids: Vec<String> = cancellation_tokens
        .iter()
        .filter(|entry| !entry.value().is_cancelled())
        .map(|entry| entry.key().clone())
        .collect();
    let mut events = Vec::new();
    for job_id in live_job_ids {
        let announced_long_job = long_job_tracking
            .get(&job_id)
            .is_some_and(|track| track.announced);
        let Some(job) = jobs_cache.get(&job_id) else {
            continue;
        };
        if job.status != JobStatus::Processing {
            continue;
        }
        let started = match upload_started_event(&job) {
            Some(started) => started,
            None if announced_long_job => long_job_started_event(&job),
            None => continue,
        };
        let long_job = upload_kind_for_job_type(&job.job_type).is_none() && announced_long_job;
        match progress_cache.get(&job.id) {
            Some(snapshot) => events.push(progress_event(
                long_job,
                job.id.clone(),
                non_empty(&job.streamer_id),
                snapshot.clone(),
            )),
            None => events.push(started),
        }
    }
    events
}

/// Upload jobs waiting for a worker. Uses the same job-type rule as
/// [`upload_kind_for_job_type`], which SQL cannot express, so the database
/// only groups pending jobs by type.
pub(super) async fn count_pending_uploads(repo: &dyn JobRepository) -> Result<u32> {
    let count: u64 = repo
        .count_pending_jobs_by_type()
        .await?
        .into_iter()
        .filter(|(job_type, _)| upload_kind_for_job_type(job_type).is_some())
        .map(|(_, count)| count)
        .sum();
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// Oldest rows to delete so `already + incoming` stays within `cap`.
pub(super) fn excess_log_rows(already: usize, incoming: usize, cap: usize) -> usize {
    already.saturating_add(incoming).saturating_sub(cap)
}

pub(super) fn extend_logs_capped(exec_info: &mut JobExecutionInfo, new_logs: &[JobLogEntry]) {
    if new_logs.is_empty() {
        return;
    }

    let tail = if new_logs.len() > EXECUTION_INFO_MAX_LOGS {
        &new_logs[new_logs.len() - EXECUTION_INFO_MAX_LOGS..]
    } else {
        new_logs
    };

    exec_info.logs.extend(tail.iter().cloned());
    while exec_info.logs.len() > EXECUTION_INFO_MAX_LOGS {
        exec_info.logs.pop_front();
    }
}

pub(super) fn cap_logs_in_place(logs: &mut VecDeque<JobLogEntry>, cap: usize) {
    // VecDeque::pop_front is O(1) vs Vec::drain which is O(n)
    while logs.len() > cap {
        logs.pop_front();
    }
}

pub(super) fn update_log_summary(exec_info: &mut JobExecutionInfo, new_logs: &[JobLogEntry]) {
    exec_info.log_lines_total = exec_info
        .log_lines_total
        .saturating_add(new_logs.len() as u64);

    for log in new_logs {
        match log.level {
            LogLevel::Warn => {
                exec_info.log_warn_count = exec_info.log_warn_count.saturating_add(1);
            }
            LogLevel::Error => {
                exec_info.log_error_count = exec_info.log_error_count.saturating_add(1);
            }
            _ => {}
        }
    }
}

impl JobQueue {
    pub(crate) async fn stop_progress_aggregator(&self) {
        self.progress_shutdown.cancel();
        let task = self.progress_aggregator.lock().take();
        if let Some(task) = task
            && let Err(error) = task.await
        {
            warn!(%error, "Job progress aggregator failed while stopping");
        }
    }

    /// Cancel and abort the background progress aggregator, reaping it up to `deadline`.
    pub(crate) async fn abort_progress_aggregator(&self, deadline: tokio::time::Instant) -> usize {
        self.progress_shutdown.cancel();
        let Some(task) = self.progress_aggregator.lock().take() else {
            return 0;
        };
        let was_running = usize::from(!task.is_finished());
        task.abort();
        if tokio::time::timeout_at(deadline, task).await.is_err() {
            warn!("Aborted job progress aggregator did not settle before the reap deadline");
        }
        was_running
    }

    #[cfg(test)]
    pub(crate) fn progress_aggregator_is_running(&self) -> bool {
        self.progress_aggregator
            .lock()
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }

    pub(super) fn emit_upload_event(&self, event: UploadStatusEvent) {
        if let Some(broadcaster) = self.upload_broadcaster.get() {
            broadcaster.send(event);
        }
    }

    /// Publish `Started` for an upload job. Any other job is registered for
    /// `JobStarted` emission on the progress aggregator's flush tick once
    /// [`JobQueueConfig::long_job_after`] elapses while `attempt` is still running.
    pub(super) fn announce_started(&self, job: &Job, attempt: u64) {
        if let Some(event) = upload_started_event(job) {
            self.emit_upload_event(event);
            return;
        }
        let started_at_ms = job
            .started_at
            .unwrap_or_else(Timestamp::now)
            .as_millisecond();
        self.long_job_tracking.insert(
            job.id.clone(),
            LongJobTrack {
                attempt,
                started_at_ms,
                announced: false,
            },
        );
    }

    /// Publish `Terminal { Cancelled }` when the job is an upload job.
    /// Cancellation produces no per-file records (the job's CANCELLED status
    /// is the durable truth), so the file counters are zero by construction.
    pub(super) fn emit_upload_cancelled(&self, job_id: &str, job_type: &str, streamer_id: &str) {
        if upload_kind_for_job_type(job_type).is_none() {
            return;
        }
        self.emit_upload_event(UploadStatusEvent::Terminal {
            job_id: job_id.to_string(),
            streamer_id: non_empty(streamer_id),
            status: UploadTerminalStatus::Cancelled,
            files_succeeded: 0,
            files_failed: 0,
            files_skipped: 0,
            error: None,
        });
    }

    /// Remove long-job tracking for `job_id`, publishing `JobEnded` if and only
    /// if `emit_ended` is true and `JobStarted` was already broadcast.
    pub(super) fn finish_long_job_tracking(&self, job_id: &str, emit_ended: bool) {
        if let Some((_, track)) = self.long_job_tracking.remove(job_id)
            && emit_ended
            && track.announced
        {
            self.emit_upload_event(UploadStatusEvent::JobEnded {
                job_id: job_id.to_string(),
                started_at_ms: track.started_at_ms,
            });
        }
    }

    /// Persist per-file upload results to `upload_records`.
    ///
    /// Best-effort, same policy as `persist_thumbnail_output`: a bookkeeping
    /// failure is logged and swallowed so it can never fail the job whose
    /// transfer already succeeded. No-op for non-upload job types and when
    /// no upload record repository is installed.
    pub(crate) async fn persist_upload_records(
        &self,
        job_id: &str,
        job_type: &str,
        streamer_id: Option<&str>,
        session_id: Option<&str>,
        items: &[UploadResultItem],
    ) {
        let Some(uploader) = upload_kind_for_job_type(job_type) else {
            return;
        };
        if items.is_empty() {
            return;
        }
        let Some(repo) = self.upload_repo.get() else {
            return;
        };

        let now = crate::database::time::now_ms();
        let records: Vec<UploadRecordDbModel> = items
            .iter()
            .map(|item| UploadRecordDbModel {
                id: uuid::Uuid::new_v4().to_string(),
                job_id: Some(job_id.to_string()),
                streamer_id: streamer_id.filter(|s| !s.is_empty()).map(str::to_string),
                session_id: session_id.filter(|s| !s.is_empty()).map(str::to_string),
                uploader: uploader.to_string(),
                local_path: item.local_path.clone(),
                remote_path: item.remote_path.clone(),
                status: item.status.as_str().to_string(),
                size_bytes: item.size_bytes.map(|s| s as i64),
                error: item.error.clone(),
                created_at: now,
                updated_at: now,
                completed_at: matches!(item.status, UploadItemStatus::Completed).then_some(now),
            })
            .collect();

        if let Err(e) = repo.upsert_records(&records).await {
            warn!(job_id, error = %e, "Failed to persist upload records");
        }
    }

    /// In-flight upload jobs, and other jobs processing for
    /// [`JobQueueConfig::long_job_after`] or more, with their latest progress
    /// snapshots. Feeds the WebSocket
    /// `DownloadSnapshot`.
    pub async fn list_active_jobs(&self) -> Result<Vec<ActiveJobInfo>> {
        let now = Timestamp::now();
        let mut infos = Vec::new();
        for db_job in self
            .job_repository
            .list_jobs_by_status(JobStatus::Processing)
            .await?
        {
            infos.extend(self.active_job_info(&db_model_to_job(&db_job), now));
        }
        Ok(infos)
    }

    /// Upload jobs waiting for a worker.
    pub async fn count_pending_uploads(&self) -> Result<u32> {
        count_pending_uploads(self.job_repository.as_ref()).await
    }

    fn active_job_info(&self, job: &Job, now: Timestamp) -> Option<ActiveJobInfo> {
        let uploader = upload_kind_for_job_type(&job.job_type);
        if uploader.is_none() && !is_long_job(job, now, self.config.long_job_after) {
            return None;
        }
        Some(ActiveJobInfo {
            job_id: job.id.clone(),
            streamer_id: non_empty(&job.streamer_id),
            streamer_name: job.streamer_name.clone(),
            session_id: non_empty(&job.session_id),
            uploader,
            job_type: job.job_type.clone(),
            files_total: job.inputs.len() as u32,
            started_at: job.started_at,
            progress: self.progress_cache.get(&job.id).map(|s| s.clone()),
        })
    }

    /// Persist thumbnail output to media_outputs table.
    pub(super) async fn persist_thumbnail_output(&self, session_id: &str, output_path: &str) {
        let Some(port) = self.session_port.get() else {
            return;
        };

        // Avoid inserting obviously-non-thumbnail outputs.
        // The ThumbnailProcessor may return passthrough outputs for unsupported inputs.
        let Some(ext) = processor_utils::get_extension(output_path) else {
            return;
        };
        if !processor_utils::is_image(&ext) {
            return;
        }

        // Get file size
        let size_bytes = tokio::fs::metadata(output_path)
            .await
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        match port
            .record_thumbnail_output(session_id, output_path, size_bytes)
            .await
        {
            Ok(true) => {
                info!(
                    "Persisted thumbnail output for session {}: {}",
                    session_id, output_path
                );
            }
            Ok(false) => {}
            Err(e) => {
                warn!(
                    "Failed to persist thumbnail output for session {}: {}",
                    session_id, e
                );
            }
        }
    }

    /// Persist a disjoint batch, including repeated entries. Callers must not resubmit a
    /// successful batch; after an error the same batch can be retried.
    ///
    /// The batch is also counted into the job's log summary, because the result snapshot
    /// skips entries already persisted here. A failed summary write is logged rather than
    /// returned: the rows are stored, and a retry would insert them again.
    pub async fn append_log_entry(&self, job_id: &str, logs: &[JobLogEntry]) -> Result<()> {
        let new_logs = self
            .persist_logs_to_db(job_id, logs, LogPersistence::Append)
            .await?;
        if !new_logs.is_empty()
            && let Err(error) = self.add_to_log_summary(job_id, &new_logs).await
        {
            warn!(job_id, %error, "Failed to update the job's log summary");
        }
        Ok(())
    }

    async fn add_to_log_summary(&self, job_id: &str, logs: &[JobLogEntry]) -> Result<()> {
        let repo = &self.job_repository;
        let stored = repo.get_job_execution_info(job_id).await?;
        let mut exec_info: JobExecutionInfo = json::parse_optional_or_default(
            stored.as_deref(),
            JsonContext::JobField {
                job_id,
                field: "execution_info",
            },
            "Invalid execution_info JSON; resetting to defaults",
        );
        update_log_summary(&mut exec_info, logs);
        repo.update_job_execution_info(job_id, &serde_json::to_string(&exec_info)?)
            .await
    }

    /// Persist the logs a processor returned with its result and fold them into
    /// the job's execution summary. Entries the streaming collector already wrote
    /// are not duplicated. Used for a result the pool turns into a failure, where
    /// `complete_attempt` never runs.
    pub(crate) async fn record_result_logs(
        &self,
        job_id: &str,
        logs: &[JobLogEntry],
    ) -> Result<()> {
        let repo = &self.job_repository;
        let new_logs = self
            .persist_logs_to_db(job_id, logs, LogPersistence::Snapshot)
            .await?;
        if new_logs.is_empty() {
            return Ok(());
        }
        let stored = repo.get_job_execution_info(job_id).await?;
        let mut exec_info: JobExecutionInfo = json::parse_optional_or_default(
            stored.as_deref(),
            JsonContext::JobField {
                job_id,
                field: "execution_info",
            },
            "Invalid execution_info JSON; resetting to defaults",
        );
        update_log_summary(&mut exec_info, &new_logs);
        extend_logs_capped(&mut exec_info, &new_logs);
        let execution_info = serde_json::to_string(&exec_info)?;
        repo.update_job_execution_info(job_id, &execution_info)
            .await
    }

    pub(super) fn log_persistence_lock(&self, job_id: &str) -> &tokio::sync::Mutex<()> {
        let mut hasher = DefaultHasher::new();
        job_id.hash(&mut hasher);
        &self.log_persistence_locks[hasher.finish() as usize % LOG_PERSISTENCE_LOCK_SHARDS]
    }

    pub(super) async fn persist_logs_to_db(
        &self,
        job_id: &str,
        logs: &[JobLogEntry],
        mode: LogPersistence,
    ) -> Result<Vec<JobLogEntry>> {
        let repo = &self.job_repository;
        if logs.is_empty() {
            return Ok(vec![]);
        }

        // Serialize snapshot deduplication and the row cap across database IO. Concurrent
        // snapshots can otherwise both observe an unpersisted occurrence before insertion.
        // The lock must survive removal of the cursor by cancellation or terminal cleanup.
        let _guard = self.log_persistence_lock(job_id).lock().await;
        let _cleanup = LogCursorCleanup {
            queue: self,
            job_id,
        };
        let previous_rows = self
            .persisted_log_cursor
            .entry(job_id.to_owned())
            .or_default()
            .persisted_rows;
        let already_persisted = match previous_rows {
            Some(rows) => rows,
            None => {
                let (_, rows) = repo
                    .list_execution_logs(job_id, &Pagination::new(0, 0))
                    .await?;
                let rows = usize::try_from(rows).unwrap_or(usize::MAX);
                if let Some(mut cursor) = self.persisted_log_cursor.get_mut(job_id) {
                    cursor.persisted_rows = Some(rows);
                }
                rows
            }
        };

        let new_logs: Vec<JobLogEntry> = logs
            .iter()
            .filter(|entry| {
                matches!(mode, LogPersistence::Append)
                    || !entry.persisted_for.lock().contains(job_id)
            })
            .cloned()
            .collect();
        let mut persisted_rows = already_persisted;
        if !new_logs.is_empty() {
            let db_logs: Vec<JobExecutionLogDbModel> = new_logs
                .iter()
                .map(|entry| JobExecutionLogDbModel {
                    id: uuid::Uuid::new_v4().to_string(),
                    job_id: job_id.to_string(),
                    entry: json::to_string_or_fallback(
                        entry,
                        r#"{"level":"error","message":"log serialize failed"}"#,
                        JsonContext::JobField {
                            job_id,
                            field: "execution_log_entry",
                        },
                        "Failed to serialize execution log entry; using fallback",
                    ),
                    created_at: entry.timestamp.as_millisecond(),
                    level: Some(entry.level.as_str().to_string()),
                    message: Some(entry.message.clone()),
                })
                .collect();
            repo.add_execution_logs(&db_logs).await?;
            persisted_rows = persisted_rows.saturating_add(new_logs.len());
            let owner: Arc<str> = Arc::from(job_id);
            for entry in &new_logs {
                entry.mark_persisted(owner.clone());
            }

            // Insert first, trim after: the newest lines (a failing run's
            // tail) always land, and the oldest rows leave once the run
            // exceeds MAX_PERSISTED_LOG_ROWS_PER_JOB. Trim failure is
            // logged, not propagated — the log write itself succeeded.
            let excess = excess_log_rows(
                already_persisted,
                new_logs.len(),
                MAX_PERSISTED_LOG_ROWS_PER_JOB,
            );
            if excess > 0 {
                match repo.trim_execution_logs(job_id, excess).await {
                    Ok(()) => persisted_rows = persisted_rows.saturating_sub(excess),
                    Err(error) => {
                        warn!(job_id, %error, "Failed to trim oldest execution log rows");
                    }
                }
            }
        }

        // Keep excess rows in the count after failed trimming, so the next write retries
        // all outstanding excess. Do not resurrect a cursor removed during the IO above.
        if let Some(mut cursor) = self.persisted_log_cursor.get_mut(job_id) {
            cursor.persisted_rows = Some(persisted_rows);
        }
        Ok(new_logs)
    }

    /// Create a progress reporter for a job.
    pub fn progress_reporter(&self, job_id: &str) -> ProgressReporter {
        ProgressReporter::new(job_id.to_string(), self.progress_tx.clone())
    }

    /// The latest progress snapshot of a running job.
    ///
    /// Snapshots are kept in memory only: a job has none until it first
    /// reports, including after a restart, and loses it when it ends.
    pub fn get_job_progress(&self, job_id: &str) -> Option<JobProgressSnapshot> {
        self.progress_cache
            .get(job_id)
            .map(|snapshot| snapshot.clone())
    }

    /// [`Self::get_job_progress`] with `raw` set to null.
    ///
    /// Only running jobs have an entry, but a failing job keeps its entry for a
    /// moment after FAILED is written, so callers should only ask about jobs
    /// whose stored status is PROCESSING. `raw` is dropped because it is too
    /// large to repeat for every workflow step.
    pub fn cached_progress(&self, job_id: &str) -> Option<JobProgressSnapshot> {
        let snapshot = self.progress_cache.get(job_id)?;
        Some(JobProgressSnapshot {
            current_file: snapshot.current_file.clone(),
            raw: serde_json::Value::Null,
            ..*snapshot
        })
    }

    /// List job execution logs (paged) from `job_execution_logs`, falling back to
    /// the logs that legacy rows kept only in `execution_info`.
    pub async fn list_job_logs(
        &self,
        job_id: &str,
        pagination: &Pagination,
    ) -> Result<(Vec<JobLogEntry>, u64)> {
        let repo = &self.job_repository;
        let (rows, total) = repo.list_execution_logs(job_id, pagination).await?;

        let logs = rows
            .into_iter()
            .map(|row| {
                let timestamp = crate::database::time::ms_to_datetime(row.created_at);

                if row.level.is_some() || row.message.is_some() {
                    let level = row
                        .level
                        .as_deref()
                        .map(|l| match l.to_ascii_uppercase().as_str() {
                            "DEBUG" => LogLevel::Debug,
                            "WARN" | "WARNING" => LogLevel::Warn,
                            "ERROR" => LogLevel::Error,
                            _ => LogLevel::Info,
                        })
                        .unwrap_or(LogLevel::Info);

                    return JobLogEntry {
                        timestamp,
                        level,
                        message: row.message.unwrap_or(row.entry),
                        persisted_for: Arc::default(),
                    };
                }

                if let Ok(entry) = serde_json::from_str::<JobLogEntry>(&row.entry) {
                    return entry;
                }

                if let Ok(entry) = serde_json::from_str::<DbLogEntry>(&row.entry) {
                    let level = match entry.level.to_ascii_uppercase().as_str() {
                        "DEBUG" => LogLevel::Debug,
                        "WARN" | "WARNING" => LogLevel::Warn,
                        "ERROR" => LogLevel::Error,
                        _ => LogLevel::Info,
                    };

                    return JobLogEntry {
                        timestamp,
                        level,
                        message: entry.message,
                        persisted_for: Arc::default(),
                    };
                }

                JobLogEntry {
                    timestamp,
                    level: LogLevel::Info,
                    message: row.entry,
                    persisted_for: Arc::default(),
                }
            })
            .inspect(|entry| entry.mark_persisted(Arc::from(job_id)))
            .collect();

        if total > 0 {
            return Ok((logs, total));
        }

        // Fallback for legacy rows that only stored logs in `job.execution_info`.
        let exec_info = repo.get_job_execution_info(job_id).await?;
        if let Some(exec_info) = exec_info
            && let Ok(parsed) = serde_json::from_str::<JobExecutionInfo>(&exec_info)
        {
            let total = parsed.logs.len() as u64;
            let start = pagination.offset as usize;
            let limit = pagination.limit as usize;
            let page: Vec<_> = parsed
                .logs
                .iter()
                .skip(start)
                .take(limit)
                .cloned()
                .collect();
            return Ok((page, total));
        }

        Ok((vec![], 0))
    }

    /// Record produced artifacts for observability. These paths do not convey
    /// temporary-file ownership and must never authorize generic failure cleanup.
    pub async fn record_produced_items(&self, job_id: &str, outputs: &[String]) -> Result<()> {
        let repo = &self.job_repository;
        let exec_info_str = repo.get_job_execution_info(job_id).await?;
        let mut exec_info: JobExecutionInfo = json::parse_optional_or_default(
            exec_info_str.as_deref(),
            JsonContext::JobField {
                job_id,
                field: "execution_info",
            },
            "Invalid execution_info JSON; resetting to defaults",
        );

        // Retain produced artifacts across attempts.
        exec_info.items_produced.extend(outputs.iter().cloned());

        let exec_info_json = serde_json::to_string(&exec_info)?;
        repo.update_job_execution_info(job_id, &exec_info_json)
            .await?;

        // Update cache
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            let exec_info = job
                .execution_info
                .get_or_insert_with(JobExecutionInfo::default);
            exec_info.items_produced.extend(outputs.iter().cloned());
        }

        Ok(())
    }

    /// Patch the current processor before its log collector starts. Preserve all
    /// stored metadata, including unknown fields, without replaying embedded logs.
    pub async fn begin_execution(&self, job_id: &str, processor: &str) -> Result<()> {
        let repo = &self.job_repository;
        let stored = repo.get_job_execution_info(job_id).await?;
        let mut info = match stored {
            Some(raw) if !raw.is_empty() => serde_json::from_str::<JobExecutionInfo>(&raw)?,
            // Legacy writers also used an empty string for absent metadata.
            // Nonempty malformed documents remain errors rather than losing history.
            _ => JobExecutionInfo::default(),
        };
        info.current_processor = Some(processor.to_owned());
        repo.update_job_execution_info(job_id, &serde_json::to_string(&info)?)
            .await?;
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            job.execution_info
                .get_or_insert_with(JobExecutionInfo::default)
                .current_processor = Some(processor.to_owned());
        }
        Ok(())
    }

    /// Replace execution info with an explicit snapshot, persisting new log entries.
    pub async fn update_execution_info(
        &self,
        job_id: &str,
        exec_info: JobExecutionInfo,
    ) -> Result<()> {
        let mut exec_info = exec_info;

        if !exec_info.logs.is_empty() {
            // make_contiguous() allows VecDeque to be used as a slice
            let new_logs = self
                .persist_logs_to_db(
                    job_id,
                    exec_info.logs.make_contiguous(),
                    LogPersistence::Snapshot,
                )
                .await?;
            update_log_summary(&mut exec_info, &new_logs);
            cap_logs_in_place(&mut exec_info.logs, EXECUTION_INFO_MAX_LOGS);
        }

        let exec_info_json = serde_json::to_string(&exec_info)?;
        self.job_repository
            .update_job_execution_info(job_id, &exec_info_json)
            .await?;

        // Update cache
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            job.execution_info = Some(exec_info);
        }

        Ok(())
    }
}

/// Announces any non-upload job attempts whose `long_job_after` delay has
/// elapsed while still actively processing, emitting `JobStarted` followed
/// immediately by the latest cached `JobProgress` snapshot.
#[allow(clippy::too_many_arguments)]
fn announce_due_long_jobs(
    long_job_tracking: &DashMap<String, LongJobTrack>,
    jobs_cache: &DashMap<String, Job>,
    cancellation_tokens: &DashMap<String, AttemptToken>,
    progress_cache: &DashMap<String, JobProgressSnapshot>,
    upload_broadcaster: &std::sync::OnceLock<UploadStatusBroadcaster>,
    pending: &mut HashMap<String, JobProgressSnapshot>,
    now: Timestamp,
    long_job_after: std::time::Duration,
) {
    let candidates: Vec<(String, u64)> = long_job_tracking
        .iter()
        .filter(|entry| !entry.value().announced)
        .map(|entry| (entry.key().clone(), entry.value().attempt))
        .collect();
    if candidates.is_empty() {
        return;
    }
    for (job_id, attempt) in candidates {
        // Release the token guard before locking `jobs_cache` to preserve lock ordering.
        let live = cancellation_tokens
            .get(&job_id)
            .is_some_and(|token| token.id() == attempt && !token.is_cancelled());
        if !live {
            long_job_tracking.remove_if(&job_id, |_, current| {
                current.attempt == attempt && !current.announced
            });
            continue;
        }
        let Some(due_payload) = jobs_cache
            .get(&job_id)
            .filter(|job| job.status == JobStatus::Processing)
            .map(|job| {
                is_long_job(&job, now, long_job_after)
                    .then(|| (long_job_started_event(&job), non_empty(&job.streamer_id)))
            })
        else {
            long_job_tracking.remove_if(&job_id, |_, current| {
                current.attempt == attempt && !current.announced
            });
            continue;
        };
        let Some((started, streamer_id)) = due_payload else {
            continue;
        };
        let snapshot = progress_cache.get(&job_id).map(|s| s.clone());
        let Some(mut track) = long_job_tracking.get_mut(&job_id) else {
            continue;
        };
        if track.attempt != attempt || track.announced {
            continue;
        }
        track.announced = true;
        pending.remove(&job_id);
        if let Some(broadcaster) = upload_broadcaster.get()
            && broadcaster.has_subscribers()
        {
            broadcaster.send(started);
            if let Some(snapshot) = snapshot {
                broadcaster.send(UploadStatusEvent::JobProgress {
                    job_id,
                    streamer_id,
                    snapshot,
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_progress_aggregator(
    repo: Arc<dyn JobRepository>,
    long_job_after: std::time::Duration,
    mut rx: tokio::sync::mpsc::Receiver<JobProgressUpdate>,
    jobs_cache: Arc<DashMap<String, Job>>,
    cancellation_tokens: Arc<DashMap<String, AttemptToken>>,
    long_job_tracking: Arc<DashMap<String, LongJobTrack>>,
    progress_cache: Arc<DashMap<String, JobProgressSnapshot>>,
    upload_broadcaster: Arc<std::sync::OnceLock<UploadStatusBroadcaster>>,
    shutdown: CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
    if tokio::runtime::Handle::try_current().is_err() {
        // Some unit tests construct JobQueue outside a Tokio runtime; they run
        // without progress tracking.
        return None;
    }

    Some(tokio::spawn(async move {
        let mut pending: HashMap<String, JobProgressSnapshot> = HashMap::new();
        let flush_every = std::time::Duration::from_millis(PROGRESS_FLUSH_INTERVAL_MS);
        let mut tick = tokio::time::interval(flush_every);
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + HEARTBEAT_INTERVAL,
            HEARTBEAT_INTERVAL,
        );
        let mut queue_poll = tokio::time::interval(UPLOAD_QUEUE_POLL_INTERVAL);
        // Last count broadcast; `None` forces a send on the first poll that
        // has a subscriber.
        let mut last_pending_uploads: Option<u32> = None;
        // The count can wait on the database for a connection, so it runs as
        // its own branch rather than inside a handler: meanwhile the loop
        // keeps draining progress, flushing and heartbeating, and shutdown
        // still stops it.
        let mut queue_count: Option<BoxFuture<'_, Result<u32>>> = None;

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                _ = heartbeat.tick() => {
                    if let Some(broadcaster) = upload_broadcaster.get()
                        && broadcaster.has_subscribers()
                    {
                        for event in heartbeat_events(
                            &jobs_cache,
                            &cancellation_tokens,
                            &long_job_tracking,
                            &progress_cache,
                        ) {
                            broadcaster.send(event);
                        }
                    }
                }
                _ = queue_poll.tick() => {
                    let Some(broadcaster) = upload_broadcaster.get() else {
                        continue;
                    };
                    if !broadcaster.has_subscribers() {
                        // New subscribers get the count from their snapshot;
                        // re-sync on the next poll that has one. A count still
                        // in flight would set the remembered value for nobody.
                        last_pending_uploads = None;
                        queue_count = None;
                        continue;
                    }
                    if queue_count.is_none() {
                        queue_count =
                            Some(count_pending_uploads(repo.as_ref()).boxed());
                    }
                }
                Some(result) = OptionFuture::from(queue_count.as_mut()) => {
                    queue_count = None;
                    match result {
                        Ok(pending) if last_pending_uploads != Some(pending) => {
                            last_pending_uploads = Some(pending);
                            if let Some(broadcaster) = upload_broadcaster.get() {
                                broadcaster.send(UploadStatusEvent::Queue { pending });
                            }
                        }
                        Ok(_) => {}
                        Err(error) => warn!(%error, "Failed to count pending uploads"),
                    }
                }
                _ = tick.tick() => {
                    let now = Timestamp::now();
                    announce_due_long_jobs(
                        &long_job_tracking,
                        &jobs_cache,
                        &cancellation_tokens,
                        &progress_cache,
                        &upload_broadcaster,
                        &mut pending,
                        now,
                        long_job_after,
                    );
                    if pending.is_empty() {
                        continue;
                    }
                    let Some(broadcaster) = upload_broadcaster.get() else {
                        continue;
                    };
                    for (job_id, snapshot) in pending.drain() {
                        // A job can reach a terminal state between receive and
                        // this flush tick, and its terminal WS event is already
                        // out, so a progress event now would outlive it.
                        // cancel_job cancels the token before the worker
                        // finalizes, hence the is_cancelled check on top of
                        // plain presence.
                        let job_is_live = cancellation_tokens
                            .get(&job_id)
                            .is_some_and(|token| !token.is_cancelled());
                        if !job_is_live {
                            continue;
                        }

                        // Snapshots of upload processors and announced long jobs
                        // double as live WS events. Publishing on the flush tick
                        // reuses this loop's coalescing instead of adding a second throttle.
                        let upload = matches!(
                            snapshot.kind,
                            ProgressKind::Rclone | ProgressKind::BaiduPcs
                        );
                        let announced_long_job = !upload
                            && long_job_tracking
                                .get(&job_id)
                                .is_some_and(|track| track.announced);
                        let streamer_id = jobs_cache
                            .get(&job_id)
                            .and_then(|job| non_empty(&job.streamer_id));
                        if upload || announced_long_job {
                            broadcaster.send(progress_event(
                                announced_long_job,
                                job_id,
                                streamer_id,
                                snapshot,
                            ));
                        }
                    }
                }
                update = rx.recv() => {
                    let Some(update) = update else { break; };
                    // Only retain progress for actively-processing jobs. This prevents
                    // late progress messages (after completion/cancellation) from reintroducing
                    // entries into the in-memory cache. The token is also checked for
                    // cancellation: cancel_job cancels it but leaves it in the map until the
                    // worker calls finalize_attempt.
                    let job_is_live = cancellation_tokens
                        .get(&update.job_id)
                        .is_some_and(|token| !token.is_cancelled());
                    if !job_is_live {
                        continue;
                    }
                    // pending only feeds the WS broadcast, so skip the clone
                    // while nobody is listening.
                    if upload_broadcaster
                        .get()
                        .is_some_and(|broadcaster| broadcaster.has_subscribers())
                    {
                        pending.insert(update.job_id.clone(), update.snapshot.clone());
                    }
                    progress_cache.insert(update.job_id, update.snapshot);
                }
            }
        }
    }))
}
