//! Streamer domain module.

mod check_record;
mod entity;
mod state;

pub use check_record::{
    CheckOutcome, CheckRecord, FilterCause, MAX_ERROR_MESSAGE_LEN, SelectedStreamSummary,
};
pub use entity::Streamer;
pub use state::StreamerState;
