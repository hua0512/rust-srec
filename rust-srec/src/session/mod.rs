//! Session lifecycle ownership.
//!
//! This module hosts the [`SessionLifecycle`] component — the single owner
//! of recording-session state. Two responsibilities live here:
//!
//! 1. **DB session row writes** (create / resume / end) wrapped in atomic
//!    transactions alongside streamer-state updates and the monitor outbox.
//!    The atomic-tx bundle is what `monitor::service::handle_live` and
//!    `handle_offline_with_session` did historically; their bodies have moved
//!    into [`repository`].
//! 2. **Emission of [`SessionTransition`] events** — a narrow broadcast
//!    stream consumed by `pipeline::manager`, `services::container`,
//!    `notification::service`, and `api::routes::sessions`. Replaces the
//!    per-component reconstruction of "is this session done?" from raw
//!    download/monitor events that produced PR #524 and the
//!    home-page-vs-session-detail divergence on 2026-04-22.
//!
//! ## What `SessionLifecycle` does **not** own
//!
//! Pipeline scheduling for the four DAG kinds (per-segment video,
//! per-segment danmu, paired-segment, session-complete) stays in
//! `pipeline::manager`. `SessionLifecycle::Ended` means "no more bytes will
//! arrive", not "all post-processing is done". The session-complete DAG
//! still drains all in-flight per-segment / paired DAGs before firing —
//! that ordering invariant is enforced by `pipeline::coordination`, unchanged.
//!
//! ## Modules
//!
//! - [`state`]: `SessionState` (Recording / Hysteresis / Ended FSM),
//!   `TerminalCause`, `OfflineSignal` and the
//!   `should_run_session_complete_pipeline` + `is_authoritative_end` policy
//!   methods.
//! - [`transition`]: `SessionTransition` — the broadcast event type
//!   (Started / Ending / Resumed / Ended).
//! - [`classifier`]: per-engine "is this engine failure a definitive offline
//!   signal?" classifier.
//! - [`hysteresis`]: hysteresis quiet-period primitives — config and
//!   per-session timer handle. The driver lives in [`lifecycle`].
//! - [`events`]: typed wire-format for the `session_events` audit log
//!   (`SessionEventKind`, `SessionEventPayload`, `TerminalCauseDto`).
//! - [`repository`]: atomic-tx wrappers around `SessionTxOps` /
//!   `StreamerTxOps` / `MonitorOutboxTxOps`.
//! - [`lifecycle`]: the `SessionLifecycle` service itself.

pub mod classifier;
pub mod download_start;
pub mod events;
pub mod hysteresis;
pub mod lifecycle;
pub mod repository;
pub mod state;
pub mod transition;

pub use classifier::{EngineKind, OfflineClassifier};
pub use download_start::DownloadStartPayload;
pub use events::{SessionEventKind, SessionEventPayload, TerminalCauseDto};
pub use hysteresis::{
    HysteresisConfig, HysteresisHandle, HysteresisOutcome, MAX_HYSTERESIS_WINDOW,
};
pub use lifecycle::{
    DEFAULT_TRANSITION_CHANNEL_CAPACITY, HysteresisWindowFn, LiveDetectedArgs, OfflineDetectedArgs,
    SessionLifecycle,
};
pub use repository::{
    EndSessionInputs, EndSessionOutcome, SessionLifecycleRepository, StartSessionInputs,
    StartSessionOutcome,
};
pub use state::{OfflineSignal, SessionState, TerminalCause};
pub use transition::SessionTransition;
