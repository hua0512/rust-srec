//! Domain value objects.
//!
//! Value objects are immutable objects that represent concepts in the domain
//! and are defined by their attributes rather than identity.

mod danmu_statistics;
mod priority;
mod proxy_config;
mod retry_policy;
mod streamer_url;

pub use danmu_statistics::DanmuStatisticsConfig;
pub use priority::Priority;
pub use proxy_config::ProxyConfig;
pub use retry_policy::RetryPolicy;
pub use streamer_url::StreamerUrl;
