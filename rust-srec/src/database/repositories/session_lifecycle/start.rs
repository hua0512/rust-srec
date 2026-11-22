//! Transactional session-start, resume, and title updates.

use std::collections::HashMap;

use jiff::Timestamp;
use sqlx::SqliteConnection;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::SessionLifecycleRepository;
use crate::Result;
use crate::database::repositories::{
    MonitorOutboxTxOps, SessionEventTxOps, SessionTxOps, StreamerTxOps,
};
use crate::domain::StreamerState;
use crate::monitor::{MonitorEvent, StreamInfo};
use crate::session::events::{SessionEventPayload, TerminalCauseDto};
use crate::streamer::state_store::StateChange;

/// Inputs required by [`SessionLifecycleRepository::start_or_resume`].
///
/// Intermittent streams are handled by the lifecycle's `Hysteresis` state.
/// This repository only reuses rows whose `end_time` is still `NULL`; ended
/// DB rows are final, so a later live observation creates a fresh row.
#[derive(Debug, Clone)]
pub struct StartSessionInputs {
    pub credential_binding: Option<crate::credentials::CredentialBinding>,
    pub streamer_id: String,
    pub streamer_name: String,
    pub streamer_url: String,
    pub current_avatar: Option<String>,
    pub new_avatar: Option<String>,
    pub title: String,
    pub category: Option<String>,
    pub streams: Vec<StreamInfo>,
    pub media_headers: Option<HashMap<String, String>>,
    pub media_extras: Option<HashMap<String, String>>,
    /// Reference time for the session `start_time` and event `timestamp`.
    pub now: Timestamp,
}

/// Outcome of [`SessionLifecycleRepository::start_or_resume`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartSessionOutcome {
    /// A brand-new session row was inserted.
    Created { session_id: String },
    /// The most-recent session row was still active (no end_time); only its
    /// titles and the streamer state were updated. This is also what the
    /// lifecycle returns when resuming a session out of `Hysteresis` —
    /// the session row was never ended in DB so reusing it is correct.
    ReusedActive { session_id: String },
    /// Nothing was written: the streamer row's `state` was inactive
    /// (user disabled/cancelled, or a fatal state) when read inside the
    /// transaction. The monitor's cache-based guards drop these live
    /// observations in the common case; this outcome is the authoritative
    /// answer when a live check races the state commit.
    SuppressedInactive { state: StreamerState },
}

impl StartSessionOutcome {
    /// Recording session id. `SuppressedInactive` carries none — same
    /// convention as `DownloadProgressEvent::session_id`: empty string
    /// means absent, and callers gate on the variant before using it.
    pub fn session_id(&self) -> &str {
        match self {
            Self::Created { session_id } | Self::ReusedActive { session_id } => session_id,
            Self::SuppressedInactive { .. } => "",
        }
    }
}

impl SessionLifecycleRepository {
    pub async fn start_or_resume(&self, inputs: StartSessionInputs) -> Result<StartSessionOutcome> {
        self.state_transaction("session start_or_resume", move |tx| {
            Box::pin(Self::start_or_resume_in_connection(tx, inputs))
        })
        .await
    }

    async fn start_or_resume_in_connection(
        tx: &mut SqliteConnection,
        inputs: StartSessionInputs,
    ) -> Result<StateChange<StartSessionOutcome>> {
        let mut committed_row = None;

        // Authoritative inactive guard (see `inactive_state`): without it,
        // `set_live` below would overwrite a just-committed user disable
        // with LIVE, and the enqueued `StreamerLive` event would start a
        // download for a disabled streamer.
        if let Some(state) = Self::inactive_state(&mut *tx, &inputs.streamer_id).await? {
            info!(
                streamer_id = %inputs.streamer_id,
                state = %state,
                "start_or_resume suppressed: streamer state is inactive"
            );
            return Ok(StateChange::row(
                StartSessionOutcome::SuppressedInactive { state },
                committed_row,
            ));
        }

        let last = SessionTxOps::get_last_session(&mut *tx, &inputs.streamer_id).await?;

        // Self-heal: end any stale active session rows for this streamer.
        let keep_id = last
            .as_ref()
            .filter(|s| s.end_time.is_none())
            .map(|s| s.id.as_str());
        let cleaned = SessionTxOps::end_all_active_for_streamer(
            &mut *tx,
            &inputs.streamer_id,
            keep_id,
            inputs.now,
        )
        .await?;
        if !cleaned.is_empty() {
            warn!(
                streamer_id = %inputs.streamer_id,
                kept = keep_id.unwrap_or("<none>"),
                count = cleaned.len(),
                "Cleaned up stale active session(s)"
            );
        }
        for stale_id in &cleaned {
            let payload = SessionEventPayload::SessionEnded {
                cause: TerminalCauseDto::Rejected {
                    reason: "stale_active_replaced".to_string(),
                },
                via_hysteresis: false,
            };
            let row = Self::event_row(stale_id, &inputs.streamer_id, &payload, inputs.now)?;
            SessionEventTxOps::insert(&mut *tx, &row).await?;
        }

        let outcome = match last {
            Some(session) if session.end_time.is_none() => {
                debug!("Reusing active session {}", session.id);
                SessionTxOps::update_titles(
                    &mut *tx,
                    &session.id,
                    session.titles.as_deref(),
                    &inputs.title,
                    inputs.now,
                )
                .await?;
                StartSessionOutcome::ReusedActive {
                    session_id: session.id,
                }
            }
            _ => {
                let new_id = Uuid::new_v4().to_string();
                SessionTxOps::create_session(
                    &mut *tx,
                    &new_id,
                    &inputs.streamer_id,
                    &inputs.streamer_name,
                    inputs.now,
                    &inputs.title,
                )
                .await?;
                let payload = SessionEventPayload::SessionStarted {
                    from_hysteresis: false,
                    title: Some(inputs.title.clone()),
                };
                let row = Self::event_row(&new_id, &inputs.streamer_id, &payload, inputs.now)?;
                SessionEventTxOps::insert(&mut *tx, &row).await?;
                info!("Created new session {}", new_id);
                StartSessionOutcome::Created { session_id: new_id }
            }
        };

        committed_row =
            StreamerTxOps::set_live_row(&mut *tx, &inputs.streamer_id, inputs.now).await?;

        if let Some(ref new_avatar) = inputs.new_avatar
            && !new_avatar.is_empty()
            && inputs.new_avatar != inputs.current_avatar
        {
            committed_row =
                StreamerTxOps::update_avatar_row(&mut *tx, &inputs.streamer_id, new_avatar).await?;
        }

        let credential_binding = if let Some(binding) = inputs.credential_binding {
            Some(
                super::super::credential_profiles::commit_session_binding(
                    tx,
                    outcome.session_id(),
                    &inputs.streamer_id,
                    &binding,
                    false,
                )
                .await?,
            )
        } else {
            None
        };
        let managed = credential_binding.is_some();
        let event = MonitorEvent::StreamerLive {
            runtime_instance: Some(crate::monitor::runtime_instance_id().to_owned()),
            credential_binding: credential_binding.map(Box::new),
            streamer_id: inputs.streamer_id.clone(),
            session_id: outcome.session_id().to_string(),
            streamer_name: inputs.streamer_name.clone(),
            streamer_url: inputs.streamer_url.clone(),
            title: inputs.title.clone(),
            category: inputs.category.clone(),
            streams: if managed {
                Vec::new()
            } else {
                inputs.streams.clone()
            },
            media_headers: if managed {
                None
            } else {
                inputs.media_headers.clone()
            },
            media_extras: if managed {
                None
            } else {
                inputs.media_extras.clone().map(Box::new)
            },
            timestamp: inputs.now,
        };
        MonitorOutboxTxOps::enqueue_event(&mut *tx, &inputs.streamer_id, &event).await?;

        Ok(StateChange::row(outcome, committed_row))
    }

    /// Set `streamers.state = 'LIVE'` and refresh `last_live_time` in a single
    /// `BEGIN IMMEDIATE` transaction.
    pub async fn mark_streamer_live(
        &self,
        streamer_id: &str,
        now: Timestamp,
    ) -> Result<Option<StreamerState>> {
        let streamer_id = streamer_id.to_owned();
        self.state_transaction("session mark_streamer_live", move |tx| {
            Box::pin(
                async move { Self::mark_streamer_live_in_connection(tx, &streamer_id, now).await },
            )
        })
        .await
    }

    async fn mark_streamer_live_in_connection(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        now: Timestamp,
    ) -> Result<StateChange<Option<StreamerState>>> {
        let mut committed_row = None;
        if let Some(state) = Self::inactive_state(&mut *tx, streamer_id).await? {
            return Ok(StateChange::row(Some(state), committed_row));
        }
        committed_row = StreamerTxOps::set_live_row(&mut *tx, streamer_id, now).await?;
        Ok(StateChange::row(None, committed_row))
    }

    /// Append `title` to an active session's title list unless it is already
    /// the latest entry. Returns whether a title was added.
    pub async fn add_session_title(
        &self,
        session_id: &str,
        title: &str,
        now: Timestamp,
    ) -> Result<bool> {
        let session_id = session_id.to_owned();
        let title = title.to_owned();
        self.write_transaction("session add_session_title", move |tx| {
            Box::pin(async move {
                Self::add_session_title_in_connection(tx, &session_id, &title, now).await
            })
        })
        .await
    }

    async fn add_session_title_in_connection(
        tx: &mut SqliteConnection,
        session_id: &str,
        title: &str,
        now: Timestamp,
    ) -> Result<bool> {
        let titles: Option<Option<String>> = sqlx::query_scalar(
            "SELECT titles FROM live_sessions WHERE id = ? AND end_time IS NULL",
        )
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(titles) = titles else {
            return Ok(false);
        };
        SessionTxOps::update_titles(tx, session_id, titles.as_deref(), title, now).await
    }
}
