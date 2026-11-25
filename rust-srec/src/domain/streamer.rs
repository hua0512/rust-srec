//! Streamer domain module.

mod check_record;
mod entity;
mod state;

pub use check_record::{
    CheckOutcome, CheckRecord, CheckSource, FatalErrorType, FilterCause, MAX_ERROR_MESSAGE_LEN,
    SelectedStreamSummary,
};
pub use entity::Streamer;
pub use state::StreamerState;

use crate::domain::Priority;

/// Offline-check defaults when no configuration layer supplies a value. They
/// match the values the initial migration seeds into `global_config`.
pub(crate) const DEFAULT_OFFLINE_CHECK_COUNT: u32 = 3;
pub(crate) const DEFAULT_OFFLINE_CHECK_DELAY_MS: u64 = 10_000;

/// Parameters for partially updating a streamer.
#[derive(Clone)]
pub struct StreamerUpdateParams {
    pub id: String,
    pub name: Option<String>,
    pub url: Option<String>,
    /// Derived from `url` by the API layer, never supplied by clients.
    pub platform_config_id: Option<String>,
    pub template_config_id: Option<Option<String>>,
    pub priority: Option<Priority>,
    pub state: Option<StreamerState>,
    pub streamer_specific_config: Option<Option<String>>,
}
