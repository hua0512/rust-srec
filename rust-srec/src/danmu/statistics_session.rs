use std::sync::Arc;

use chrono::Utc;
use platforms_parser::danmaku::{DanmuMessage, DanmuStatistics, StatisticsAggregator};
use tracing::warn;

use crate::database::models::{DanmuRateEntry, GiftTallyEntry, TopTalkerEntry, WordFrequencyEntry};
use crate::database::repositories::SessionRepository;
use crate::domain::DanmuStatisticsConfig;

use super::checkpoint;
use super::lifecycle::CollectionExitReason;

/// Owns aggregation, publication, and restart checkpoint policy for one collection.
pub(crate) struct StatisticsSession {
    session_id: String,
    aggregator: StatisticsAggregator,
    repository: Option<Arc<dyn SessionRepository>>,
    enabled: bool,
    last_persisted_total: u64,
    last_checkpoint_total: u64,
}

impl StatisticsSession {
    pub(crate) async fn load(
        session_id: String,
        repository: Option<Arc<dyn SessionRepository>>,
        config: DanmuStatisticsConfig,
    ) -> Self {
        let enabled = config.enabled;
        let repository = repository.filter(|_| enabled);
        let aggregator = checkpoint::load_or_new(
            repository.as_ref(),
            &session_id,
            config.to_aggregator_config(),
        )
        .await;
        let resumed_total = aggregator.total_count();

        Self {
            session_id,
            aggregator,
            repository,
            enabled,
            last_persisted_total: 0,
            last_checkpoint_total: resumed_total,
        }
    }

    pub(crate) fn record_message(&mut self, message: &DanmuMessage) {
        if self.enabled {
            self.aggregator.record_message(message);
        }
    }

    pub(crate) async fn persist_if_changed(&mut self) {
        if self.repository.is_none() {
            return;
        }
        let statistics = self.aggregator.current_stats();
        if statistics.total_count == self.last_persisted_total {
            return;
        }
        persist_statistics(self.repository.as_deref(), &self.session_id, &statistics).await;
        self.last_persisted_total = statistics.total_count;
    }

    pub(crate) async fn checkpoint_if_changed(&mut self) {
        if self.repository.is_none() || self.aggregator.total_count() == self.last_checkpoint_total
        {
            return;
        }
        let state = self.aggregator.export_state();
        checkpoint::save(self.repository.as_ref(), &self.session_id, &state).await;
        self.last_checkpoint_total = self.aggregator.total_count();
    }

    pub(crate) async fn finish(self, reason: CollectionExitReason) -> DanmuStatistics {
        if reason.retains_checkpoint() {
            checkpoint::save(
                self.repository.as_ref(),
                &self.session_id,
                &self.aggregator.export_state(),
            )
            .await;
        } else {
            checkpoint::discard(self.repository.as_ref(), &self.session_id).await;
        }

        let statistics = self.aggregator.finalize(Utc::now());
        persist_statistics(self.repository.as_deref(), &self.session_id, &statistics).await;
        statistics
    }
}

fn saturating_u64_to_i64(value: u64) -> i64 {
    if value > i64::MAX as u64 {
        i64::MAX
    } else {
        value as i64
    }
}

/// Serialize and upsert one statistics snapshot.
async fn persist_statistics(
    session_repo: Option<&dyn SessionRepository>,
    session_id: &str,
    statistics: &DanmuStatistics,
) {
    let Some(repo) = session_repo else {
        return;
    };

    fn to_json_field<T: serde::Serialize>(
        session_id: &str,
        field: &'static str,
        value: &T,
    ) -> Option<String> {
        match serde_json::to_string(value) {
            Ok(value) => Some(value),
            Err(error) => {
                warn!(session_id, field, %error, "Failed to serialize danmu statistics field");
                None
            }
        }
    }

    let rate_timeseries = statistics
        .rate_timeseries
        .iter()
        .map(|entry| DanmuRateEntry {
            ts: entry.timestamp.timestamp_millis(),
            count: saturating_u64_to_i64(entry.count),
        })
        .collect::<Vec<_>>();

    let to_talker_entry = |entry: &platforms_parser::danmaku::TopTalker| TopTalkerEntry {
        user_id: entry.user_id.clone(),
        username: entry.username.clone(),
        message_count: saturating_u64_to_i64(entry.message_count),
        error: saturating_u64_to_i64(entry.error),
    };
    let top_talkers = statistics
        .top_talkers
        .iter()
        .map(to_talker_entry)
        .collect::<Vec<_>>();
    let top_gifters = statistics
        .top_gifters
        .iter()
        .map(to_talker_entry)
        .collect::<Vec<_>>();
    let top_gifts = statistics
        .top_gifts
        .iter()
        .map(|entry| GiftTallyEntry {
            name: entry.name.clone(),
            count: saturating_u64_to_i64(entry.count),
        })
        .collect::<Vec<_>>();
    let word_frequency = statistics
        .word_frequency
        .iter()
        .map(|entry| WordFrequencyEntry {
            word: entry.word.clone(),
            count: saturating_u64_to_i64(entry.count),
            error: saturating_u64_to_i64(entry.error),
        })
        .collect::<Vec<_>>();

    let mut model = crate::database::models::DanmuStatisticsDbModel::new(session_id);
    model.total_danmus = saturating_u64_to_i64(statistics.total_count);
    model.unique_talkers = Some(saturating_u64_to_i64(statistics.unique_talkers));
    model.chat_count = Some(saturating_u64_to_i64(statistics.chat_count));
    model.gift_count = Some(saturating_u64_to_i64(statistics.gift_count));
    model.duration_secs = Some(saturating_u64_to_i64(statistics.duration_secs));
    model.start_time = statistics.start_time.map(|time| time.timestamp_millis());
    model.end_time = statistics.end_time.map(|time| time.timestamp_millis());
    model.rate_bucket_secs = Some(saturating_u64_to_i64(statistics.bucket_duration_secs));
    model.danmu_rate_timeseries = to_json_field(session_id, "rate_timeseries", &rate_timeseries);
    model.top_talkers = to_json_field(session_id, "top_talkers", &top_talkers);
    model.top_gifters = to_json_field(session_id, "top_gifters", &top_gifters);
    model.top_gifts = to_json_field(session_id, "top_gifts", &top_gifts);
    model.word_frequency = to_json_field(session_id, "word_frequency", &word_frequency);

    if let Err(error) = repo.upsert_danmu_statistics(&model).await {
        warn!(session_id, %error, "Failed to persist danmu statistics");
    }
}
