pub(crate) mod abogus;
pub(crate) mod apis;
mod builder;
pub mod danmu;

pub(crate) mod models;
pub(crate) mod sign;
pub(crate) mod utils;

pub use builder::URL_REGEX;
pub use builder::{Douyin, DouyinExtractorConfig};
pub use danmu::{DouyinDanmuProtocol, DouyinDanmuProvider, create_douyin_danmu_provider};

// TODO: REXPORT DOUYIN PROTO
pub mod douyin_proto {
    include!(concat!(env!("OUT_DIR"), "/douyin.rs"));
}
