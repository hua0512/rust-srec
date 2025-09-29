use serde_json::Value;

pub struct DanmuStatistics {
    pub id: String,
    pub session_id: String,
    pub total_danmus: u64,
    pub danmu_rate_timeseries: Vec<Value>,
    pub top_talkers: Value,
    pub word_frequency: Value,
}

impl From<crate::database::models::DanmuStatistics> for DanmuStatistics {
    fn from(model: crate::database::models::DanmuStatistics) -> Self {
        Self {
            id: model.id,
            session_id: model.session_id,
            total_danmus: model.total_danmus as u64,
            danmu_rate_timeseries: model
                .danmu_rate_timeseries
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            top_talkers: model
                .top_talkers
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(Value::Null),
            word_frequency: model
                .word_frequency
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(Value::Null),
        }
    }
}
