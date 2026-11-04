mod builder;
pub mod danmu;
mod models;
pub(crate) mod utils;

pub use builder::URL_REGEX;
pub use builder::{TikTok, TikTokApiMode};
pub use danmu::{TikTokDanmuProtocol, TikTokDanmuProvider, create_tiktok_danmu_provider};

// Generated from `proto/tiktok.proto` by `generated_protos_are_current` in
// the parent module.
#[rustfmt::skip]
pub mod tiktok_proto;
