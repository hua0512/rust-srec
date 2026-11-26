//! Application-wide error types.
//!
//! The application error is the API crate's [`ServerError`](srec_api::ServerError),
//! which wraps the error of every lower crate; see its documentation for
//! where each kind of failure lives.

pub use srec_api::{ServerError as Error, ServerResult as Result};
