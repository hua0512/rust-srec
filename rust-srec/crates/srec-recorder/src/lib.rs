//! Recording control plane, monitoring, download orchestration, danmu service,
//! credential management, proxy routing, and configuration crate for `rust-srec`.

pub mod config;
pub mod credentials;
pub mod danmu;
pub mod downloader;
pub mod error;
pub mod monitor;
pub mod proxies;
pub mod recorder;
pub mod session;
pub mod streamer;

pub use error::{RecorderError, RecorderResult};
pub(crate) use error::{RecorderError as Error, RecorderResult as Result};
