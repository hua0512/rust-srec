//! Processor trait and related types.

use async_trait::async_trait;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

use crate::Result;
use crate::pipeline::job_queue::JobLogEntry;
use crate::pipeline::manifest::PipelineInputManifest;
use crate::pipeline::progress::ProgressReporter;
use crate::pipeline::staged_outputs::StagedOutputRecorder;

/// Type of processor (determines which worker pool handles it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProcessorType {
    /// CPU-bound processor (remux, thumbnail).
    Cpu,
    /// IO-bound processor (file operations).
    Io,
    /// Remote upload processor. Its jobs run on the upload pool, whose size is
    /// the `max_concurrent_uploads` setting, so that setting caps every upload
    /// tool together and uploads do not count against the IO job limit.
    Upload,
}

/// Timestamp source for processor time placeholder expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimeAnchor {
    /// Expand time tokens with `ProcessorInput::created_at`.
    #[default]
    JobCreated,
    /// Expand time tokens with `ProcessorInput::session_start`, falling back to created_at.
    SessionStart,
}

impl TimeAnchor {
    /// Resolve the timestamp this anchor selects from `input`.
    ///
    /// `SessionStart` falls back to `input.created_at` when the job carries
    /// no session start (empty/synthetic `session_id`, or the session row
    /// was gone when the job's metadata was resolved).
    pub fn reference_time(self, input: &ProcessorInput) -> Timestamp {
        match self {
            TimeAnchor::JobCreated => input.created_at,
            TimeAnchor::SessionStart => input.session_start.unwrap_or_else(|| {
                tracing::debug!(
                    session_id = %input.session_id,
                    "time_anchor=session_start but job has no session start; falling back to created_at"
                );
                input.created_at
            }),
        }
    }
}

/// Input for a processor.
#[derive(Debug, Clone)]
pub struct ProcessorInput {
    /// Input file paths.
    pub inputs: Vec<String>,
    /// Output file paths (if known/required upfront).
    pub outputs: Vec<String>,
    /// Additional configuration as JSON.
    pub config: Option<String>,
    /// Streamer ID.
    pub streamer_id: String,
    /// Session ID.
    pub session_id: String,
    /// Human-readable streamer name.
    pub streamer_name: Option<String>,
    /// Session/stream title.
    pub session_title: Option<String>,
    /// Platform name (e.g., "Twitch", "Huya").
    pub platform: Option<String>,
    /// live_sessions.start_time of the session this job belongs to.
    pub session_start: Option<Timestamp>,
    /// When the job was originally created.
    /// Used for time-based placeholder expansion to ensure consistency across retries.
    pub created_at: Timestamp,
    /// Per-segment video/danmu pairing of a paired-segment or session-complete
    /// pipeline. `None` for segment, thumbnail and API-created DAGs and for
    /// standalone jobs.
    pub manifest: Option<Arc<PipelineInputManifest>>,
}

impl Default for ProcessorInput {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            config: None,
            streamer_id: String::new(),
            session_id: String::new(),
            streamer_name: None,
            session_title: None,
            platform: None,
            session_start: None,
            created_at: Timestamp::now(),
            manifest: None,
        }
    }
}

impl ProcessorInput {
    /// Create a new ProcessorInput with required fields.
    pub fn new(
        inputs: Vec<String>,
        outputs: Vec<String>,
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            inputs,
            outputs,
            config: None,
            streamer_id: streamer_id.into(),
            session_id: session_id.into(),
            streamer_name: None,
            session_title: None,
            platform: None,
            session_start: None,
            created_at: Timestamp::now(),
            manifest: None,
        }
    }

    /// Expand `template` with this job's streamer and session metadata. Time
    /// tokens use `reference_timestamp_ms` (Unix epoch milliseconds), or the
    /// current local time when it is `None`.
    pub fn expand_placeholders(
        &self,
        template: &str,
        reference_timestamp_ms: Option<i64>,
    ) -> String {
        crate::utils::filename::expand_placeholders_at(
            template,
            &self.streamer_id,
            &self.session_id,
            self.streamer_name.as_deref(),
            self.session_title.as_deref(),
            self.platform.as_deref(),
            reference_timestamp_ms,
        )
    }

    /// Set the configuration.
    pub fn with_config(mut self, config: impl Into<String>) -> Self {
        self.config = Some(config.into());
        self
    }

    /// Set the streamer name.
    pub fn with_streamer_name(mut self, name: impl Into<String>) -> Self {
        self.streamer_name = Some(name.into());
        self
    }

    /// Set the session title.
    pub fn with_session_title(mut self, title: impl Into<String>) -> Self {
        self.session_title = Some(title.into());
        self
    }

    /// Set the platform.
    pub fn with_platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }

    /// Set the created_at timestamp.
    pub fn with_created_at(mut self, created_at: Timestamp) -> Self {
        self.created_at = created_at;
        self
    }

    /// Set the session start timestamp.
    pub fn with_session_start(mut self, session_start: Timestamp) -> Self {
        self.session_start = Some(session_start);
        self
    }
}

/// Processor context for emitting progress and other side-channel data.
#[derive(Clone)]
pub struct ProcessorContext {
    pub job_id: String,
    pub progress: ProgressReporter,
    pub log_sink: JobLogSink,
    pub cancellation_token: CancellationToken,
    pub is_retry: bool,
    /// True when the owning workflow step carries an automatic retry policy
    /// (`max_attempts > 1`). Upload processors defer transfer-level retries to
    /// the workflow step so retries do not multiply and workers are freed
    /// between attempts.
    pub has_step_retry: bool,
    /// The workflow step's own timeout, when it sets one. It replaces both the
    /// worker pool's job timeout and any processor-specific default, such as
    /// the `execute` command timeout.
    pub step_timeout: Option<std::time::Duration>,
    /// Records staged temporary files so a restart can list what an
    /// interrupted attempt left behind.
    pub(crate) staged_outputs: StagedOutputRecorder,
    /// Per-file `local_path -> ConfirmedUpload` outcomes already confirmed as
    /// `COMPLETED` by an earlier attempt of this job and bound to the
    /// destination account. Upload processors use this on retries so
    /// previously uploaded files still on disk are neither re-uploaded nor
    /// downgraded to `Skipped`, and remain eligible for source cleanup after
    /// full batch success only when the active account still matches.
    pub prior_completed_uploads: std::collections::HashMap<String, ConfirmedUpload>,
}

#[derive(Clone)]
pub struct JobLogSink {
    tx: tokio::sync::mpsc::Sender<JobLogEntry>,
    dropped: Arc<AtomicUsize>,
    /// Applied to every entry sent; see [`ProcessorContext::scrubbing`].
    scrubber: crate::proxies::OutputScrubber,
}

impl JobLogSink {
    pub fn new(tx: tokio::sync::mpsc::Sender<JobLogEntry>, dropped: Arc<AtomicUsize>) -> Self {
        Self {
            tx,
            dropped,
            scrubber: crate::proxies::OutputScrubber::default(),
        }
    }

    pub fn try_send(&self, mut entry: JobLogEntry) {
        entry.message = self.scrubber.scrub(&entry.message);
        if self.tx.try_send(entry).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped_count(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl ProcessorContext {
    pub fn noop(job_id: impl Into<String>) -> Self {
        let job_id = job_id.into();
        let (log_tx, _) = tokio::sync::mpsc::channel(100);
        let dropped = Arc::new(AtomicUsize::new(0));
        Self {
            job_id: job_id.clone(),
            progress: ProgressReporter::noop(job_id),
            log_sink: JobLogSink::new(log_tx, dropped),
            cancellation_token: CancellationToken::new(),
            is_retry: false,
            has_step_retry: false,
            step_timeout: None,
            staged_outputs: StagedOutputRecorder::default(),
            prior_completed_uploads: std::collections::HashMap::new(),
        }
    }

    pub fn new(
        job_id: impl Into<String>,
        progress: ProgressReporter,
        log_sink: JobLogSink,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            job_id: job_id.into(),
            progress,
            log_sink,
            cancellation_token,
            is_retry: false,
            has_step_retry: false,
            step_timeout: None,
            staged_outputs: StagedOutputRecorder::default(),
            prior_completed_uploads: std::collections::HashMap::new(),
        }
    }

    /// Mark this execution as a retry of a previously persisted job.
    pub fn with_retry(mut self, is_retry: bool) -> Self {
        self.is_retry = is_retry;
        self
    }

    /// Mark whether this job's workflow step has its own automatic retry policy.
    pub fn with_step_retry(mut self, has_step_retry: bool) -> Self {
        self.has_step_retry = has_step_retry;
        self
    }

    /// Set the workflow step's own timeout.
    pub fn with_step_timeout(mut self, step_timeout: Option<std::time::Duration>) -> Self {
        self.step_timeout = step_timeout;
        self
    }

    /// Record this attempt's staged temporary files through `recorder`.
    pub(crate) fn with_staged_outputs(mut self, recorder: StagedOutputRecorder) -> Self {
        self.staged_outputs = recorder;
        self
    }

    /// Attach per-file uploads confirmed by earlier attempts of this job,
    /// bound to the destination account that received them.
    pub fn with_prior_completed_uploads(
        mut self,
        uploads: std::collections::HashMap<String, ConfirmedUpload>,
    ) -> Self {
        self.prior_completed_uploads = uploads;
        self
    }

    /// A context for a command whose output must not reach the job log, such
    /// as a probe that prints account details. Progress, cancellation and the
    /// retry flag are shared with `self`; entries sent to the returned
    /// context's log sink are discarded without counting as dropped for the job.
    /// This context with `scrubber` applied to the job log, for a tool
    /// that may print a proxy login.
    pub fn scrubbing(&self, scrubber: &crate::proxies::OutputScrubber) -> Self {
        let mut context = self.clone();
        context.log_sink.scrubber = scrubber.clone();
        context
    }

    pub fn without_job_log(&self) -> Self {
        let (log_tx, _discarded) = tokio::sync::mpsc::channel(1);
        Self {
            job_id: self.job_id.clone(),
            progress: self.progress.clone(),
            log_sink: JobLogSink::new(log_tx, Arc::new(AtomicUsize::new(0))),
            cancellation_token: self.cancellation_token.clone(),
            is_retry: self.is_retry,
            has_step_retry: self.has_step_retry,
            step_timeout: self.step_timeout,
            staged_outputs: self.staged_outputs.clone(),
            prior_completed_uploads: self.prior_completed_uploads.clone(),
        }
    }

    /// Emit a log entry.
    pub fn log(&self, entry: JobLogEntry) {
        self.log_sink.try_send(entry);
    }

    /// Emit an info log.
    pub fn info(&self, message: impl Into<String>) {
        self.log(JobLogEntry::info(message));
    }

    /// Emit a warning log.
    pub fn warn(&self, message: impl Into<String>) {
        self.log(JobLogEntry::warn(message));
    }

    /// Emit an error log.
    pub fn error(&self, message: impl Into<String>) {
        self.log(JobLogEntry::error(message));
    }
}

/// Content and metadata snapshot of a local file at the time it was uploaded,
/// used to verify that the file on disk has not been replaced or modified
/// before reusing a prior upload confirmation or removing the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalFileFingerprint {
    pub size_bytes: u64,
    pub modified_secs: i64,
    pub modified_subsec_nanos: u32,
    /// Platform file identity (`(dev, ino)` on Unix, `(volume_serial, file_id)`
    /// on Windows) when available; detects atomic replacement even when size
    /// and modification time are preserved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<(u64, u64)>,
    /// Kernel-maintained inode/file change time (Unix `ctime`, Windows NTFS
    /// `ChangeTime`) as `(secs, subsec_nanos)` when available; advances on any
    /// write, replacement, or timestamp reset (`utimes` / `SetFileTime`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_time: Option<(i64, u32)>,
    /// SHA-256 content digest (full file up to 4 MiB; head, middle, and tail
    /// 64 KiB slices plus length for larger files), catching same-size content
    /// edits even on filesystems without inode or change-time tracking.
    pub content_sha256: [u8; 32],
}

/// A per-file upload confirmed as `COMPLETED` by an earlier attempt of the
/// same job, bound to the destination account (`account_uid`) and local file
/// version (`fingerprint`) so retries never skip a transfer or delete a local
/// source after an account switch or file replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmedUpload {
    pub remote_path: String,
    pub account_uid: u64,
    pub fingerprint: LocalFileFingerprint,
}

/// Terminal outcome of one uploaded file within an upload job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadItemStatus {
    Completed,
    Failed,
    Skipped,
}

impl UploadItemStatus {
    /// Persisted discriminator; must match the `upload_records.status` CHECK
    /// constraint values in `crate::database::models::upload_record::upload_status`.
    pub fn as_str(&self) -> &'static str {
        match self {
            UploadItemStatus::Completed => "COMPLETED",
            UploadItemStatus::Failed => "FAILED",
            UploadItemStatus::Skipped => "SKIPPED",
        }
    }
}

/// Per-file result of an upload job, produced by upload processors and
/// persisted to `upload_records` by `JobQueue::persist_upload_records`.
#[derive(Debug, Clone)]
pub struct UploadResultItem {
    pub local_path: String,
    /// Expanded remote destination. `None` when it could not be computed
    /// (e.g. failure synthesis in the worker pool).
    pub remote_path: Option<String>,
    /// Size captured before the transfer, so it survives `move` operations
    /// that delete the local source.
    pub size_bytes: Option<u64>,
    pub status: UploadItemStatus,
    pub error: Option<String>,
}

/// Output from a processor.
#[derive(Debug, Clone, Default)]
pub struct ProcessorOutput {
    /// Output file paths.
    pub outputs: Vec<String>,
    /// Processing duration in seconds.
    pub duration_secs: f64,
    /// Additional metadata as JSON.
    pub metadata: Option<String>,
    /// Intermediate artifacts created during processing.
    /// Used for observability and cleanup on failure.
    pub items_produced: Vec<String>,
    /// Input file size in bytes (for metrics).
    pub input_size_bytes: Option<u64>,
    /// Output file size in bytes (for metrics).
    pub output_size_bytes: Option<u64>,
    /// Input files that failed processing with their error messages.
    /// Each tuple contains (input_path, error_message).
    ///
    /// A non-empty list makes the worker pool fail the job, with an error that
    /// names every failed input, after it has recorded `items_produced` and
    /// `logs`; a DAG step then fails fast. Report the successful inputs'
    /// outputs anyway so a retry can resume past them. Upload processors do not
    /// use this list: they return `Err` on a partial failure, and the queue's
    /// failure path records every input as failed; `uploads` is persisted only
    /// when a job completes.
    pub failed_inputs: Vec<(String, String)>,
    /// Input files that were successfully processed.
    /// Used for partial failure reporting in multi-input jobs.
    pub succeeded_inputs: Vec<String>,
    /// Input files that were skipped (passed through) because the processor
    /// doesn't support them. These files are included in outputs for chaining.
    /// Each tuple contains (input_path, reason).
    pub skipped_inputs: Vec<(String, String)>,
    /// Per-file upload results. Filled only by upload processors; the queue
    /// persists these to `upload_records` when the job completes.
    pub uploads: Vec<UploadResultItem>,
    /// Per-file `local_path -> ConfirmedUpload` entries verified against an
    /// active destination account in this attempt, persisted on partial
    /// failure so retries of processors that opt in via
    /// [`Processor::carries_forward_completed_uploads`] can skip already
    /// uploaded files safely.
    pub confirmed_uploads: std::collections::HashMap<String, ConfirmedUpload>,
    /// Execution logs captured during processing.
    pub logs: Vec<JobLogEntry>,
    /// True when every remaining failure in `failed_inputs` is permanent and
    /// retrying the step with the same inputs and configuration cannot succeed.
    pub non_retryable: bool,
}

impl ProcessorOutput {
    /// `result` with `scrubber` applied to every message it reports.
    pub(crate) fn scrubbed(
        result: crate::Result<Self>,
        scrubber: &crate::proxies::OutputScrubber,
    ) -> crate::Result<Self> {
        let mut output = match result {
            Ok(output) => output,
            Err(crate::Error::Other(message)) => {
                return Err(crate::Error::Other(scrubber.scrub(&message)));
            }
            Err(crate::Error::Validation(message)) => {
                return Err(crate::Error::Validation(scrubber.scrub(&message)));
            }
            Err(crate::Error::NonRetryablePipeline(message)) => {
                return Err(crate::Error::NonRetryablePipeline(scrubber.scrub(&message)));
            }
            Err(error) => return Err(error),
        };
        for entry in &mut output.logs {
            entry.message = scrubber.scrub(&entry.message);
        }
        for (_, message) in output
            .failed_inputs
            .iter_mut()
            .chain(output.skipped_inputs.iter_mut())
        {
            *message = scrubber.scrub(message);
        }
        for error in output
            .uploads
            .iter_mut()
            .filter_map(|upload| upload.error.as_mut())
        {
            *error = scrubber.scrub(error);
        }
        Ok(output)
    }

    /// A single skipped file remains available for downstream processors.
    /// Whole-job passthrough and processor-specific batch metadata are separate.
    pub(super) fn skipped_file(
        input: &str,
        reason: &str,
        json_reason: &str,
        duration_secs: f64,
        logs: Vec<JobLogEntry>,
    ) -> Self {
        Self {
            outputs: vec![input.to_owned()],
            duration_secs,
            metadata: Some(
                serde_json::json!({ "status": "skipped", "reason": json_reason, "input": input })
                    .to_string(),
            ),
            skipped_inputs: vec![(input.to_owned(), reason.to_owned())],
            logs,
            ..Default::default()
        }
    }
}

/// Trait for pipeline processors.
#[async_trait]
pub trait Processor: Send + Sync {
    /// Get the processor type.
    fn processor_type(&self) -> ProcessorType;

    /// Whether this processor's jobs share one turn with every other such
    /// processor in its pool, so at most one of those jobs runs at a time. A
    /// job waiting for its turn stays pending: it holds no worker and its
    /// timeout has not started.
    fn runs_one_at_a_time(&self) -> bool {
        false
    }

    /// Get the job types this processor can handle.
    fn job_types(&self) -> Vec<&'static str>;

    /// Check if this processor can handle a job type.
    fn can_process(&self, job_type: &str) -> bool {
        self.job_types().contains(&job_type)
    }

    /// Process the input and produce output.
    ///
    /// # Cancel Safety
    ///
    /// This method MUST be cancel-safe. The worker pool may cancel the future if the job times out
    /// or if the application is shutting down. Implementations should ensure that cancellation
    /// does not leave the system in an inconsistent state (e.g., partial files should be cleaned up).
    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput>;

    /// Get the processor name.
    fn name(&self) -> &'static str;

    /// Indicates if this processor supports multiple inputs in a single job (batch processing).
    ///
    /// When `true`, the processor can handle multiple input files in a single `process()` call.
    /// When `false` (default), the worker pool splits a multi-input standalone job into one
    /// job per input; a multi-input DAG step job is failed instead, because a step must not
    /// complete more than once.
    fn supports_batch_input(&self) -> bool {
        false
    }

    /// Whether retries of this processor consume prior per-file
    /// [`ConfirmedUpload`] state carried in [`ProcessorContext`].
    fn carries_forward_completed_uploads(&self) -> bool {
        false
    }
}
