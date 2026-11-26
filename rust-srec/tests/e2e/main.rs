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
//! asserts the one it set, so that test can fail there. Two scenarios also
//! shorten process-wide timings for themselves: the pipeline shutdown grace
//! (in [`restart`]) and the auth-cache TTL (in [`sockets`]). In one process
//! these leak into later tests, so the tests that rely on the production
//! values, such as the change-password and import eviction checks, check
//! less there.

mod support;

use rust_srec::api::proto::{download_progress, log_event};

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
mod no_auth;
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
