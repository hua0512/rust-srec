pub mod error;
pub mod event;
pub mod message;
pub mod provider;
pub mod registry;
pub mod statistics;
pub mod websocket;
pub mod writer;

pub use error::{DanmakuError, Result};
pub use event::{DanmuControlEvent, DanmuItem};
pub use message::{DanmuMessage, DanmuType};
pub use provider::{ConnectionConfig, DanmuConnection, DanmuProvider, DanmuStream};
pub use registry::ProviderRegistry;
pub use statistics::{
    AggregatorState, DanmuStatistics, RateDataPoint, StatisticsAggregator, StatisticsConfig,
    TopTalker, WordFrequency,
};
pub use websocket::{DanmuProtocol, WebSocketDanmuProvider};
pub use writer::{XmlDanmuWriter, escape_xml, message_type_to_int};

pub use crate::extractor::platforms::huya::danmu::HuyaDanmuProvider;
pub use crate::extractor::platforms::twitch::danmu::TwitchDanmuProvider;
