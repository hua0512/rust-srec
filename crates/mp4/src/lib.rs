mod box_utils;
mod fourcc;
pub mod fragment;
pub mod independent;
pub mod isobmff;
pub mod merge;
mod moof;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_support;

pub use fourcc::FourCc;
pub use media_types::Resolution;
