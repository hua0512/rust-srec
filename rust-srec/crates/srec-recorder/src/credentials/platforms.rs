//! Platform-specific account providers.

pub mod bigo;
pub mod bilibili;
pub mod douyu;
pub mod soop;
pub mod twitch;
pub mod weibo;

pub use bigo::BigoProvider;
pub use bilibili::BilibiliProvider;
pub use douyu::DouyuProvider;
pub use soop::SoopProvider;
pub use twitch::TwitchProvider;
pub use weibo::WeiboProvider;
