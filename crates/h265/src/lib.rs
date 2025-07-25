//! A pure Rust implementation of the HEVC/H.265 decoder.
//! This crate is a fork of [scuffle_h265](https://crates.io/crates/scuffle_h265)
//! Credits goes to [scuffle](https://github.com/ScuffleCloud/scuffle)
//!
//! This crate is designed to provide a simple and safe interface to decode HEVC/H.265 SPS NALUs.
//! ## Examples
//!
//! ```
//! use h265::SpsNALUnit;
//!
//! # fn test() -> std::io::Result<()> {
//! # let data = b"\x42\x01\x01\x01\x40\x00\x00\x03\x00\x90\x00\x00\x03\x00\x00\x03\x00\x78\xa0\x03\xc0\x80\x11\x07\xcb\x96\xb4\xa4\x25\x92\xe3\x01\x6a\x02\x02\x02\x08\x00\x00\x03\x00\x08\x00\x00\x03\x00\xf3\x00\x2e\xf2\x88\x00\x02\x62\x5a\x00\x00\x13\x12\xd0\x20";
//! # let reader = std::io::Cursor::new(data);
//! let nalu = SpsNALUnit::parse(reader)?;
//! println!("Parsed SPS NALU: {:?}", nalu);
//! # Ok(())
//! # }
//! ```
//!
//! ## License
//!
//! This project is licensed under the MIT or Apache-2.0 license.
//! You can choose between one of them if you use this work.
//!
//! `SPDX-License-Identifier: MIT OR Apache-2.0`
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![deny(missing_docs)]
#![deny(unsafe_code)]
#![deny(unreachable_pub)]

mod config;
mod enums;
mod nal_unit_header;
mod rbsp_trailing_bits;
mod sps;

pub use config::{HEVCDecoderConfigurationRecord, NaluArray};
pub use enums::*;
pub use sps::*;
