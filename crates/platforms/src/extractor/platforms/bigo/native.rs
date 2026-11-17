//! BIGO's native app protocol.
//!
//! - [`ticket`] asks the website for the identity a native login uses, as a
//!   guest or as a signed-in account.
//! - [`linkd`] signs in to Linkd, BIGO's app service, and looks up a room's
//!   media servers.
//! - [`listener`] keeps a signed-in Linkd connection open for live
//!   notifications.

use thiserror::Error;

use crate::danmaku::DanmuProxy;

pub mod linkd;
pub mod listener;
pub mod ticket;

/// How native connections reach BIGO, both taken from the platform's proxy
/// route: the HTTP client for ticket requests, and the tunnel Linkd's TCP
/// connections go through (`None` connects directly).
#[derive(Clone)]
pub struct Route {
    pub client: reqwest::Client,
    pub proxy: Option<DanmuProxy>,
}

/// Why a native request failed.
#[derive(Debug, Error)]
pub enum NativeError {
    #[error("ticket request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("the BIGO account is signed out")]
    SignedOut,
    #[error("BIGO refuses native logins from this IP address")]
    IpRestricted,
    #[error("BIGO rejected the {request} (result {result})")]
    Rejected { request: &'static str, result: i32 },
    #[error("the room has ended")]
    RoomEnded,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("unexpected answer from BIGO: {0}")]
    Protocol(String),
}

impl From<bytes::TryGetError> for NativeError {
    fn from(error: bytes::TryGetError) -> Self {
        Self::Protocol(format!(
            "truncated message: needed {} more bytes, {} left",
            error.requested, error.available
        ))
    }
}
