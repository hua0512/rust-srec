//! End-to-end tests: each test starts the real backend in-process (see
//! [`support::TestApp`]) and drives it over HTTP.
//!
//! Tests named `known_bug_*` are `#[ignore]`d and assert the correct behaviour
//! of a confirmed bug; they become normal tests when the bug is fixed.
//!
//! The suite is written for nextest's process-per-test model. `cargo test`
//! also runs it, but in one process every backend shares the `tracing`
//! subscriber and its log filter: a backend starting in another test
//! re-applies its stored filter while the log-filter test in [`sockets`]
//! asserts the one it set, so that test can fail there.

mod support;

// `api::proto` is not exported, so the socket tests compile its generated
// modules here. Not every generated message and accessor is used.
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "../../src/api/proto/download_progress.rs"]
mod download_progress;
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "../../src/api/proto/log_event.rs"]
mod log_event;

mod auth;
mod backup;
mod batch_detection;
mod config;
mod control;
mod credential_providers;
mod danmu;
mod filters;
mod health;
mod lifecycle;
mod mcp;
mod monitoring;
mod notifications;
mod output_names;
mod parse;
mod pipeline;
mod processors;
mod recording;
mod recording_failures;
mod recording_hls;
mod recording_selection;
mod restart;
mod sessions;
mod smoke;
mod sockets;
mod stream_proxy;
mod streamers;
mod telegram;
mod tools_engines;
mod tools_processors;
