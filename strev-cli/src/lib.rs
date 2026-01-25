//! Library target for the `strev` package.
//!
//! The primary deliverable of this package is the `strev` CLI binary
//! (`src/main.rs`). This library exists so CI can run `cargo test -p strev --doc`
//! for feature/doctype validation.

#[doc(hidden)]
pub use platforms_parser;
