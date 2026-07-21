//! Platform-specific credential manager implementations.

pub mod bilibili;
pub mod soop;

pub use bilibili::BilibiliCredentialManager;
pub use soop::SoopCredentialManager;
