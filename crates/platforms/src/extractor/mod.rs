mod default;
pub mod error;
pub mod factory;
pub mod platform_extractor;
pub mod platforms;
pub mod utils;

pub use default::{
    ProxyConfig, create_client, create_client_builder, default_factory, factory_with_proxy,
};

pub mod hls_extractor;
