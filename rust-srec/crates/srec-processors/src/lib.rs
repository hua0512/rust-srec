//! Pipeline post-processing operators and BaiduPCS-Go CLI helpers for `rust-srec`.

pub mod baidupcs;
pub mod processors;

pub use processors::*;
pub use srec_core::{CoreError, CoreResult};
