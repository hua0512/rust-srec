use super::danmu_statistics::DanmuStatistics;
use super::media_output::MediaOutput;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub struct LiveSession {
    pub id: String,
    pub streamer_id: String,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub titles: Vec<Value>,
    pub media_outputs: Vec<MediaOutput>,
    pub danmu_statistics: Option<DanmuStatistics>,
}

impl From<crate::database::models::LiveSession> for LiveSession {
    fn from(db_session: crate::database::models::LiveSession) -> Self {
        let titles = db_session
            .titles
            .map(|t| serde_json::from_str(&t).unwrap_or_default())
            .unwrap_or_default();

        let start_time = DateTime::parse_from_rfc3339(&db_session.start_time)
            .unwrap()
            .with_timezone(&Utc);

        let end_time = db_session.end_time.map(|t| {
            DateTime::parse_from_rfc3339(&t)
                .unwrap()
                .with_timezone(&Utc)
        });

        Self {
            id: db_session.id,
            streamer_id: db_session.streamer_id,
            start_time,
            end_time,
            titles,
            media_outputs: Vec::new(),
            danmu_statistics: None,
        }
    }
}
