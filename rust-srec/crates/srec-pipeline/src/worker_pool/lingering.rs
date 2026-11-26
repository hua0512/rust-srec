//! Stalled blocking-operation tracking and timeout diagnostic formatting.

use std::sync::Arc;

use super::WorkerType;
use super::execution::JobFacts;
use crate::processors::ProcessorInput;
use crate::progress::{JobPhase, JobProgressSnapshot};

/// How long blocking work may outlive its ended job before the attempt is reported as a
/// stalled operation. Long enough that a cancelled job's normal cleanup never shows up.
pub(crate) const STALLED_OPERATION_THRESHOLD: std::time::Duration =
    std::time::Duration::from_secs(30);

/// A job attempt that already ended (timed out, cancelled, or stopped by the shutdown drain)
/// while blocking work its processor started, such as a copy waiting on an unresponsive
/// network share, still holds the log sink and with it the worker.
#[derive(Debug, Clone)]
pub struct StalledOperation {
    pub job_id: String,
    pub job_type: String,
    pub processor: &'static str,
    pub worker_type: WorkerType,
    pub streamer_id: String,
    /// File name of the first input, for telling operations of one job type apart.
    pub input_file_name: Option<String>,
    pub input_count: usize,
    /// When the attempt's outcome was decided.
    pub ended_at: jiff::Timestamp,
    ended: tokio::time::Instant,
}

impl StalledOperation {
    /// The attempt of `facts` whose job just ended.
    pub(super) fn ended(facts: &JobFacts, input: &ProcessorInput, worker_type: WorkerType) -> Self {
        Self {
            job_id: facts.id.clone(),
            job_type: facts.job_type.clone(),
            processor: facts.processor_name,
            worker_type,
            streamer_id: input.streamer_id.clone(),
            input_file_name: input.inputs.first().map(|first| {
                std::path::Path::new(first)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| first.clone())
            }),
            input_count: input.inputs.len(),
            ended_at: jiff::Timestamp::now(),
            ended: tokio::time::Instant::now(),
        }
    }

    /// How long the blocking work has outlived its job.
    pub fn stalled_for(&self) -> std::time::Duration {
        self.ended.elapsed()
    }
}

/// Attempts of one pool whose job ended before their blocking work returned, keyed by attempt
/// rather than job ID: a retry can run a successor under the same job ID meanwhile.
#[derive(Default)]
pub(super) struct LingeringAttempts {
    entries: parking_lot::Mutex<std::collections::HashMap<u64, StalledOperation>>,
}

impl LingeringAttempts {
    pub(super) fn register(
        self: &Arc<Self>,
        attempt: u64,
        operation: StalledOperation,
    ) -> LingeringGuard {
        self.entries.lock().insert(attempt, operation);
        LingeringGuard {
            registry: Arc::clone(self),
            attempt,
        }
    }

    pub(super) fn older_than(&self, min_age: std::time::Duration) -> Vec<StalledOperation> {
        self.entries
            .lock()
            .values()
            .filter(|operation| operation.stalled_for() >= min_age)
            .cloned()
            .collect()
    }
}

/// Removes its attempt's entry once the blocking work returns, or when the worker is aborted.
pub(super) struct LingeringGuard {
    registry: Arc<LingeringAttempts>,
    attempt: u64,
}

impl Drop for LingeringGuard {
    fn drop(&mut self) {
        self.registry.entries.lock().remove(&self.attempt);
    }
}

/// Opening words of the error recorded when a job outlives
/// `WorkerPoolConfig::job_timeout_secs`. `timeout_error` appends the job's context.
pub(super) const TIMED_OUT_ERROR: &str = "Job timed out";

/// Longest input file name carried in a timeout error, in characters. The message is stored on
/// the job row and delivered verbatim as a notification body, so one pathological name must not
/// crowd out the rest of the context.
const TIMEOUT_ERROR_FILE_NAME_LIMIT: usize = 80;

/// Describe a timed-out job well enough to act on the notification it produces: how long it was
/// given, which step and processor hung, what it was working on, and how far its last reported
/// progress got. The progress clause is what separates a stalled transfer from a slow one, so it
/// carries the age of that reading rather than only its position.
///
/// `progress` is the job's last snapshot, absent for a processor that reports none.
pub(super) fn timeout_error(
    job_timeout: std::time::Duration,
    facts: &JobFacts,
    input: &ProcessorInput,
    progress: Option<&JobProgressSnapshot>,
) -> String {
    let mut context = facts.job_type.clone();
    context.push_str(&format!(", processor '{}'", facts.processor_name));

    if let Some(first) = input.inputs.first() {
        let name = std::path::Path::new(first)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| first.clone());
        context.push_str(&format!(
            ", input \"{}\"",
            srec_core::utils::text::truncate_chars(&name, TIMEOUT_ERROR_FILE_NAME_LIMIT)
        ));
        let rest = input.inputs.len() - 1;
        if rest > 0 {
            context.push_str(&format!(" and {} more", rest));
        }
    }

    if let Some(snapshot) = progress {
        context.push_str(&progress_clause(snapshot));
    }

    format!(
        "{} after {} ({})",
        TIMED_OUT_ERROR,
        srec_core::utils::text::format_duration(job_timeout.as_secs_f64()),
        context
    )
}

/// Render how far a timed-out job's last progress reading got and how old it is. Every position
/// field is optional, so the clause keeps only what the processor actually reported; the age is
/// always known, since it comes from the snapshot's own timestamp.
pub(super) fn progress_clause(snapshot: &JobProgressSnapshot) -> String {
    let mut parts = Vec::new();

    if let Some(percent) = snapshot.percent {
        parts.push(format!("{:.0}%", percent));
    }
    match (snapshot.bytes_done, snapshot.bytes_total) {
        (Some(done), Some(total)) => parts.push(format!(
            "{} of {}",
            srec_core::utils::text::format_bytes(done),
            srec_core::utils::text::format_bytes(total)
        )),
        (Some(done), None) => parts.push(srec_core::utils::text::format_bytes(done)),
        _ => {}
    }
    if let Some(speed) = snapshot.speed_bytes_per_sec
        && speed.is_finite()
        && speed > 0.0
    {
        parts.push(format!(
            "at {}/s",
            srec_core::utils::text::format_bytes(speed as u64)
        ));
    }
    // The phase tells a job waiting to retry, finalizing its output or
    // preparing files to upload from one whose work stalled.
    if let Some(phase) = snapshot.phase {
        parts.push(
            match phase {
                JobPhase::Finalizing => "finalizing its output",
                JobPhase::Retrying => "waiting to retry",
                JobPhase::Preparing => "preparing files to upload",
            }
            .to_string(),
        );
    }

    // A reading from the future means the clock moved; report it as current rather than
    // as a negative age.
    let age =
        std::time::Duration::try_from(jiff::Timestamp::now().duration_since(snapshot.updated_at))
            .unwrap_or_default();
    parts.push(format!(
        "last updated {} ago",
        srec_core::utils::text::format_duration(age.as_secs_f64())
    ));

    format!(", progress {}", parts.join(", "))
}
