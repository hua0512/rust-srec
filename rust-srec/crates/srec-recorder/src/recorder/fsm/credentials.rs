//! Credential diagnostic backoff, 30-second [`RecoveredMedia`] validation,
//! and recovery probe reconciliation.

use std::time::{Duration, Instant};

use crate::downloader::DownloadFailureKind;
use crate::downloader::engine::EngineType;
use crate::session::TerminalCause;
use srec_core::domain::RetryPolicy;

use super::state::{
    AttemptPhase, FsmEffect, InFlightProbe, MonitorPhase, OpenSession, PendingDbKind,
    RecoveredMedia, SessionSlot, StagedRecoveredMedia, StepContext, StreamerFsm,
};
use crate::recorder::ports::{DbCommitOp, ProbeFailure, ProbePurpose, ProbeRequest, ProbeStatus};

/// Signed media URLs extracted during a credential diagnostic expire quickly.
pub const RECOVERED_MEDIA_TTL: Duration = Duration::from_secs(30);

/// Deterministic retry delay matching [`RetryPolicy::delay_for_attempt`] without
/// touching thread-local randomness.
///
/// Maps `jitter_bps` (`9000..=11000`) linearly into `0%..=25%` additive jitter
/// when `policy.use_jitter` is enabled, capped at `policy.max_delay_ms`.
pub fn deterministic_retry_delay(policy: &RetryPolicy, attempt: u32, jitter_bps: u16) -> Duration {
    if attempt >= policy.max_retries {
        return Duration::from_millis(policy.max_delay_ms);
    }

    let base_delay =
        policy.initial_delay_ms as f64 * policy.backoff_multiplier.powi(attempt as i32);
    let delay_ms = base_delay.min(policy.max_delay_ms as f64) as u64;

    if policy.use_jitter {
        let clamped = u64::from(jitter_bps.clamp(9000, 11000) - 9000);
        // 0..=2000 maps to 0.0..=0.25 of delay_ms.
        let jitter_ms = delay_ms.saturating_mul(clamped) / 8_000;
        Duration::from_millis(delay_ms.saturating_add(jitter_ms).min(policy.max_delay_ms))
    } else {
        Duration::from_millis(delay_ms)
    }
}

/// Take valid recovered media from `open` when its binding matches and it is
/// still within [`RECOVERED_MEDIA_TTL`] (30 seconds).
pub fn take_valid_recovered_media(open: &mut OpenSession, now: Instant) -> Option<RecoveredMedia> {
    let media = open.recovered_media.take()?;
    let binding_matches = open.credential_binding.as_ref() == Some(&media.binding);
    let fresh = now.saturating_duration_since(media.extracted_at) < RECOVERED_MEDIA_TTL;
    (binding_matches && fresh).then_some(media)
}

impl StreamerFsm {
    pub(super) fn try_begin_credential_diagnostic(
        &mut self,
        ctx: StepContext,
        session_id: &str,
        download_id: &str,
        engine_type: EngineType,
        kind: DownloadFailureKind,
        error: &str,
    ) -> bool {
        if !kind.requests_credential_diagnostic(engine_type) {
            return false;
        }
        let max_retries = self.config.download_retry_policy.max_retries;
        let managed = self.config.managed_credentials;
        let policy = self.config.download_retry_policy.clone();

        let SessionSlot::Open(open) = &mut self.session else {
            return false;
        };
        if open.session_id != session_id
            || open.quiet_period.is_some()
            || (!managed && open.credential_binding.is_none())
            || open.diagnosed_download_ids.contains(download_id)
            || open.diagnostic_attempts >= max_retries
        {
            return false;
        }

        let attempt_index = open.diagnostic_attempts;
        open.diagnostic_attempts = open.diagnostic_attempts.saturating_add(1);
        open.diagnosed_download_ids.insert(download_id.to_owned());

        let delay = deterministic_retry_delay(&policy, attempt_index, ctx.jitter_bps);
        let backoff_until = ctx.now.checked_add(delay).unwrap_or(ctx.now);
        open.attempt = AttemptPhase::DiagnosingCredentials {
            probe_id: None,
            backoff_until: Some(backoff_until),
            failed_download_id: download_id.to_owned(),
            original_cause: TerminalCause::Failed { kind },
            original_error: error.to_owned(),
        };
        true
    }

    /// Trigger the `Recovery` probe when a `DiagnosingCredentials` backoff timer fires.
    pub(super) fn on_credential_diagnostic_timer(
        &mut self,
        ctx: StepContext,
        effects: &mut Vec<FsmEffect>,
    ) {
        if self.in_flight_probe.is_some()
            || matches!(
                self.monitor,
                MonitorPhase::Stopping(_) | MonitorPhase::Terminated
            )
        {
            return;
        }

        let Some((session_id, binding, original_cause, original_error)) = (match &mut self.session {
            SessionSlot::Open(open) => match &mut open.attempt {
                AttemptPhase::DiagnosingCredentials {
                    probe_id: None,
                    backoff_until: Some(until),
                    original_cause,
                    original_error,
                    ..
                } if ctx.now >= *until => Some((
                    open.session_id.clone(),
                    open.credential_binding.clone(),
                    original_cause.clone(),
                    original_error.clone(),
                )),
                _ => None,
            },
            _ => None,
        }) else {
            return;
        };

        if self.errors.is_disabled(ctx.wall_now) {
            if let SessionSlot::Open(open) = &mut self.session {
                open.attempt = AttemptPhase::Idle;
            }
            self.finish_failed_download_with_cause(
                ctx,
                session_id,
                original_cause,
                original_error,
                effects,
            );
            return;
        }

        let probe_id = self.next_id();
        if let SessionSlot::Open(open) = &mut self.session
            && let AttemptPhase::DiagnosingCredentials {
                probe_id: slot,
                backoff_until,
                ..
            } = &mut open.attempt
        {
            *slot = Some(probe_id);
            *backoff_until = None;
        }

        self.in_flight_probe = Some(InFlightProbe {
            probe_id,
            purpose: ProbePurpose::Recovery,
            is_live_watchdog: false,
        });
        effects.push(FsmEffect::SpawnProbe(ProbeRequest {
            probe_id,
            streamer_id: self.streamer_id.clone(),
            purpose: ProbePurpose::Recovery,
            session_id: Some(session_id),
            credential_binding: binding,
            is_live_watchdog: false,
        }));
    }

    /// Reconcile a completed `ProbePurpose::Recovery` probe.
    pub(super) fn on_recovery_probe_completed(
        &mut self,
        ctx: StepContext,
        probe_id: u64,
        result: std::result::Result<ProbeStatus, ProbeFailure>,
        effects: &mut Vec<FsmEffect>,
    ) {
        let Some((session_id, original_cause, original_error)) = (match &mut self.session {
            SessionSlot::Open(open) => match &open.attempt {
                AttemptPhase::DiagnosingCredentials {
                    probe_id: Some(current),
                    original_cause,
                    original_error,
                    ..
                } if *current == probe_id => Some((
                    open.session_id.clone(),
                    original_cause.clone(),
                    original_error.clone(),
                )),
                _ => None,
            },
            _ => None,
        }) else {
            return;
        };

        if matches!(
            self.monitor,
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated
        ) {
            return;
        }

        match result {
            Ok(ProbeStatus::Live(payload)) if payload.credential_binding.is_some() => {
                let Some(binding) = payload.credential_binding.clone() else {
                    return;
                };
                let staged_media = match payload.credential_snapshot {
                    Some(snapshot) if !payload.streams.is_empty() => {
                        Some(Box::new(StagedRecoveredMedia {
                            snapshot,
                            streams: payload.streams,
                            media_headers: payload.media_headers,
                            media_extras: payload.media_extras,
                            extracted_at: ctx.now,
                        }))
                    }
                    _ => None,
                };
                self.enqueue_or_dispatch_db(
                    PendingDbKind::CommitCredentialBinding {
                        session_id: session_id.clone(),
                        staged_media,
                        original_cause,
                        original_error,
                    },
                    DbCommitOp::CommitCredentialBinding {
                        streamer_id: self.streamer_id.clone(),
                        session_id,
                        binding,
                    },
                    effects,
                );
            }
            Err(ProbeFailure::CredentialUnavailable { .. }) => {
                if let SessionSlot::Open(open) = &mut self.session {
                    open.attempt = AttemptPhase::WaitingCredentials;
                }
            }
            _ => {
                if let SessionSlot::Open(open) = &mut self.session {
                    open.attempt = AttemptPhase::Idle;
                }
                self.finish_failed_download_with_cause(
                    ctx,
                    session_id,
                    original_cause,
                    original_error,
                    effects,
                );
            }
        }
    }
}
