//! Library target for the `mesio` package.
//!
//! The primary deliverable of this package is the `mesio` CLI binary
//! (`src/main.rs`). This library exists so CI can run `cargo test -p mesio --doc`
//! for feature/doctype validation.

#[doc(hidden)]
pub use mesio_engine;
