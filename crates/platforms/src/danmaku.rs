pub mod error;
pub mod event;
pub mod message;
pub mod provider;
pub mod registry;
pub mod sampler;
pub mod statistics;
pub mod websocket;
pub mod writer;

pub use error::{DanmakuError, Result};
pub use event::{DanmuControlEvent, DanmuItem};
pub use message::{DanmuMessage, DanmuType};
pub use provider::{ConnectionConfig, DanmuConnection, DanmuProvider};
pub use registry::ProviderRegistry;
pub use sampler::{
    DanmuSampler, DanmuSamplingConfig, FixedIntervalSampler, VelocitySampler, create_sampler,
};
pub use statistics::{
    DanmuStatistics, RateDataPoint, StatisticsAggregator, TopTalker, WordFrequency,
};
pub use websocket::{DanmuProtocol, WebSocketDanmuProvider};
pub use writer::{XmlDanmuWriter, escape_xml, message_type_to_int};

pub use crate::extractor::platforms::huya::danmu::HuyaDanmuProvider;
pub use crate::extractor::platforms::twitch::danmu::TwitchDanmuProvider;
