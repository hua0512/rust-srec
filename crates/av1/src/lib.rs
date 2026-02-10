//! A crate for decoding and encoding AV1 video headers and container formats.
//!
//! Supports:
//! - OBU (Open Bitstream Unit) header parsing and writing
//! - AV1 Codec Configuration Record (ISO BMFF / MPEG-2 TS)
//! - Sequence header OBU parsing
//! - IVF container format parsing and writing
//! - Low-overhead OBU bitstream parsing and writing
//! - Annex B length-delimited bitstream parsing and writing
//! - ISOBMFF sample payload parsing helpers
//!
//! ## License
//!
//! This project is licensed under the [MIT](./LICENSE.MIT) or
//! [Apache-2.0](./LICENSE.Apache-2.0) license. You can choose between one of
//! them if you use this work.
//!
//! `SPDX-License-Identifier: MIT OR Apache-2.0`
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod annex_b;
mod config;
pub mod error;
pub mod ivf;
mod obu;
pub mod obu_stream;
pub mod sample;

pub use config::{AV1CodecConfigurationRecord, AV1VideoDescriptor};
pub use error::{Av1Error, Result};
pub use obu::utils::{leb128_size, write_leb128};
pub use obu::{ObuExtensionHeader, ObuHeader, ObuType, seq};
