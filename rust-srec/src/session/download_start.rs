//! Download-start payload bundled into [`crate::session::SessionTransition::Started`]
//! when the lifecycle wants the container's resume-download subscriber to
//! (re)start a download for the transitioning session.
//!
//! ## Why a separate type
//!
//! The lifecycle emits `Started` for two semantically-distinct cases:
//!
//! 1. **Fresh session** — `from_hysteresis: false`. The container's
//!    `MonitorEvent::StreamerLive` outbox handler already drives the
//!    download start; the `SessionTransition::Started` here is purely a
//!    notification / audit signal.
//! 2. **Hysteresis resume** — `from_hysteresis: true`. The lifecycle's
//!    fast-path resume short-circuits before `start_or_resume`, so no
//!    `MonitorEvent::StreamerLive` outbox event fires. The container's
//!    resume-download subscriber needs the same payload `StreamerLive`
//!    would have carried in order to drive `start_download_for_streamer`.
//!
//! Carrying the payload as `Option<Box<DownloadStartPayload>>` on the
//! `Started` variant keeps three properties:
//!
//! - **Test-fixture stability**: every existing `Started { .. }` literal
//!   defaults `download_start: None`; matchers using `..` need no edit.
//! - **Encapsulated dependency**: only this module imports `StreamInfo`
//!   and the header `HashMap` types; `transition.rs` imports the single
//!   `DownloadStartPayload`.
//! - **Bounded variant size**: a `Started` without a payload doesn't pay
//!   the size of `Vec<StreamInfo>` + two `Option<HashMap<…>>`. Boxed
//!   so the `None` arm stays one pointer wide.

use std::collections::HashMap;

use platforms_parser::media::StreamInfo;

/// Sidecar payload carried on [`crate::session::SessionTransition::Started`]
/// when the container is expected to (re)start a download for the
/// transitioning session.
///
/// `None` on a `Started` means "notification only" (used by tests and by
/// the fresh-session path where `MonitorEvent::StreamerLive` drives the
/// download). `Some(_)` means "please drive `start_download_for_streamer`."
///
/// `PartialEq` / `Eq` not derived because `StreamInfo` from
/// `platforms_parser` doesn't implement them and we don't want to
/// constrain the upstream crate's contract for a sidecar that doesn't
/// need value comparison.
#[derive(Debug, Clone)]
pub struct DownloadStartPayload {
    /// Streamer URL — feeds the engine config + audit log.
    pub streamer_url: String,
    /// Available stream candidates from the platform's status check.
    /// Selected via `StreamSelectionConfig` at download-start time.
    pub streams: Vec<StreamInfo>,
    /// Optional HTTP headers required by the upstream FLV/HLS request
    /// (referer, cookies, signed-token headers, etc.).
    pub media_headers: Option<HashMap<String, String>>,
    /// Optional platform-specific extras (e.g. Douyu `tt`/`vrid`,
    /// Bilibili `ksy_play_url_seq` overrides).
    pub media_extras: Option<HashMap<String, String>>,
}
