//! Recording session domain types and persisted audit event contracts.
//!
//! ## Modules
//!
//! - `state`: [`EngineKind`], [`TerminalCause`], and [`OfflineSignal`] along
//!   with session-complete pipeline and authoritative-end policy helpers.
//! - `events`: typed wire format for the `session_events` audit log
//!   ([`SessionEvent`], [`SessionEventKind`], [`SessionEventPayload`],
//!   [`TerminalCauseDto`]).

pub mod events;
pub mod state;

pub use events::{SessionEvent, SessionEventKind, SessionEventPayload, TerminalCauseDto};
pub use state::{EngineKind, OfflineSignal, TerminalCause};
