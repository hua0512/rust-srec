//! Session lifecycle ownership.
//!
//! This module hosts the [`SessionLifecycle`] component — the single owner
//! of recording-session state. Two responsibilities live here:
//!
//! 1. **Session state coordination** around DB session row writes
//!    (create / resume / end). The atomic-tx bundle itself lives in
//!    [`crate::database::repositories::session_lifecycle`] because it is a
//!    concrete SQL repository.
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
//! - `state`: [`SessionState`] (Recording / Hysteresis / Ended FSM),
//!   `TerminalCause`, `OfflineSignal` and the
//!   `should_run_session_complete_pipeline` + `is_authoritative_end` policy
//!   methods.
//! - `transition`: [`SessionTransition`] — the broadcast event type
//!   (Started / Ending / Resumed / Ended).
//! - `classifier`: per-engine "is this engine failure a definitive offline
//!   signal?" classifier.
//! - `hysteresis`: hysteresis quiet-period primitives — config and
//!   per-session timer handle. The driver lives in `lifecycle`.
//! - `events`: typed wire-format for the `session_events` audit log
//!   (`SessionEventKind`, `SessionEventPayload`, `TerminalCauseDto`).
//! - `lifecycle`: the [`SessionLifecycle`] service itself.

pub(crate) mod classifier;
pub(crate) mod download_start;
pub(crate) mod events;
pub(crate) mod hysteresis;
pub(crate) mod lifecycle;
pub(crate) mod state;
pub(crate) mod transition;

pub use classifier::{EngineKind, OfflineClassifier};
pub use download_start::DownloadStartPayload;
pub use events::{SessionEvent, SessionEventKind, SessionEventPayload, TerminalCauseDto};
pub use hysteresis::{
    HysteresisConfig, HysteresisHandle, HysteresisOutcome, MAX_HYSTERESIS_WINDOW,
};
pub use lifecycle::{
    DEFAULT_TRANSITION_CHANNEL_CAPACITY, HysteresisWindowFn, LiveDetectedArgs, OfflineDetectedArgs,
    SessionLifecycle,
};
pub use state::{OfflineSignal, SessionState, TerminalCause};
pub use transition::SessionTransition;
