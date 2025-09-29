use super::config::MergedConfig;
use super::live_session::LiveSession;
use super::types::{Filter, StreamerState, StreamerUrl};
use crate::database::models::Streamer as DbStreamer;
use chrono::{DateTime, Utc};

pub struct Streamer {
    pub id: String,
    pub name: String,
    pub url: StreamerUrl,
    pub state: StreamerState,
    pub consecutive_error_count: u32,
    pub disabled_until: Option<DateTime<Utc>>,
    pub config: MergedConfig,
    pub filters: Vec<Filter>,
    pub live_sessions: Vec<LiveSession>,
    pub platform_config_id: String,
    pub template_config_id: Option<String>,
}

impl From<DbStreamer> for Streamer {
    fn from(db_streamer: DbStreamer) -> Self {
        Self {
            id: db_streamer.id,
            name: db_streamer.name,
            url: db_streamer.url.into(),
            state: db_streamer.state.parse().unwrap(),
            consecutive_error_count: db_streamer.consecutive_error_count.unwrap_or(0) as u32,
            disabled_until: db_streamer.disabled_until.and_then(|s| s.parse().ok()),
            config: MergedConfig::default(),
            filters: Vec::new(),
            live_sessions: Vec::new(),
            platform_config_id: db_streamer.platform_config_id,
            template_config_id: db_streamer.template_config_id,
        }
    }
}
