mod builder;
pub mod danmu;
mod models;
pub(crate) mod utils;

pub use builder::URL_REGEX;
pub use builder::{TikTok, TikTokApiMode};
pub use danmu::{TikTokDanmuProtocol, TikTokDanmuProvider, create_tiktok_danmu_provider};

pub mod tiktok_proto {
    include!(concat!(env!("OUT_DIR"), "/tiktok.webcast.rs"));
}
