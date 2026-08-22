mod builder;
pub mod danmu;
pub(crate) mod danmu_models;
mod models;
mod mp;
mod sign;
pub mod tars;
mod web;
mod wup;

pub use builder::Huya;
pub use builder::URL_REGEX;
pub use tars::*;

pub use sign::HuyaPlatform;
pub use sign::get_anticode;

// Re-export the danmu provider
pub use danmu::HuyaDanmuProvider;
pub use danmu::create_huya_danmu_provider;

// Re-export danmu models for internal use
pub(crate) use danmu_models::*;
