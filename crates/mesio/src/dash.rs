//! MPEG-DASH (ISO/IEC 23009-1) support.
//!
//! [`manifest`] parses and resolves Media Presentation Descriptions and
//! computes which segments are available at a given wall-clock time. It is
//! pure: it performs no network I/O.

pub mod manifest;
