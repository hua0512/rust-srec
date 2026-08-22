//! Persistence for danmu aggregator checkpoints.
//!
//! A checkpoint lets a collector restarted mid-session continue counting rather
//! than starting at zero and overwriting `danmu_statistics` with lower numbers.
//! It is stored gzip-compressed because the uncompressed JSON is a few hundred
//! kilobytes at the default capacities — far larger than the derived statistics —
//! and compresses heavily, being mostly repeated keys and short strings.

use std::io::{Read, Write};
use std::sync::Arc;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use platforms_parser::danmaku::{AggregatorState, StatisticsAggregator, StatisticsConfig};
use tracing::{debug, warn};

use crate::database::repositories::SessionRepository;

/// Compress a checkpoint for storage.
fn encode(state: &AggregatorState) -> std::io::Result<Vec<u8>> {
    let json = serde_json::to_vec(state)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&json)?;
    encoder.finish()
}

/// Decompress a stored checkpoint.
fn decode(bytes: &[u8]) -> std::io::Result<AggregatorState> {
    let mut json = Vec::new();
    GzDecoder::new(bytes).read_to_end(&mut json)?;
    Ok(serde_json::from_slice(&json)?)
}

/// Build an aggregator for `session_id`, resuming from a stored checkpoint when
/// one is usable.
///
/// Every failure path degrades to a fresh aggregator rather than propagating: a
/// recording must not be held up by an unreadable checkpoint. A checkpoint whose
/// layout version this build does not recognize is likewise ignored, which is how
/// `AggregatorState::VERSION` bumps stay safe.
pub(super) async fn load_or_new(
    session_repo: Option<&Arc<dyn SessionRepository>>,
    session_id: &str,
    config: StatisticsConfig,
) -> StatisticsAggregator {
    let Some(repo) = session_repo else {
        return StatisticsAggregator::with_settings(config);
    };

    let stored = match repo.get_danmu_aggregator_state(session_id).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return StatisticsAggregator::with_settings(config),
        Err(error) => {
            warn!(session_id, %error, "danmu: failed to read statistics checkpoint");
            return StatisticsAggregator::with_settings(config);
        }
    };

    let state = match decode(&stored) {
        Ok(state) => state,
        Err(error) => {
            warn!(session_id, %error, "danmu: discarding unreadable statistics checkpoint");
            return StatisticsAggregator::with_settings(config);
        }
    };

    let checkpoint_total = state.total_count();
    let version = state.version;
    match StatisticsAggregator::from_state(state, config.clone()) {
        Some(aggregator) => {
            debug!(
                session_id,
                checkpoint_total, "danmu: resumed statistics from checkpoint"
            );
            aggregator
        }
        None => {
            warn!(
                session_id,
                version,
                expected = AggregatorState::VERSION,
                "danmu: discarding statistics checkpoint written by another layout version"
            );
            StatisticsAggregator::with_settings(config)
        }
    }
}

/// Store a checkpoint, logging and continuing on failure.
pub(super) async fn save(
    session_repo: Option<&Arc<dyn SessionRepository>>,
    session_id: &str,
    state: &AggregatorState,
) {
    let Some(repo) = session_repo else {
        return;
    };

    let bytes = match encode(state) {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!(session_id, %error, "danmu: failed to encode statistics checkpoint");
            return;
        }
    };

    if let Err(error) = repo
        .upsert_danmu_aggregator_state(session_id, i64::from(state.version), &bytes)
        .await
    {
        warn!(session_id, %error, "danmu: failed to store statistics checkpoint");
    }
}

/// Drop the checkpoint for a session that will not be resumed.
pub(super) async fn discard(session_repo: Option<&Arc<dyn SessionRepository>>, session_id: &str) {
    let Some(repo) = session_repo else {
        return;
    };
    if let Err(error) = repo.delete_danmu_aggregator_state(session_id).await {
        warn!(session_id, %error, "danmu: failed to discard statistics checkpoint");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platforms_parser::danmaku::message::DanmuMessage;

    fn aggregator_with_messages(count: usize) -> StatisticsAggregator {
        let mut agg = StatisticsAggregator::with_settings(StatisticsConfig::default());
        for i in 0..count {
            agg.record_message(&DanmuMessage::chat(
                "id",
                format!("user-{}", i % 500),
                "观众昵称",
                "主播今天好厉害啊",
            ));
        }
        agg
    }

    #[test]
    fn encode_decode_round_trip_preserves_the_checkpoint() {
        let state = aggregator_with_messages(2_000).export_state();
        let bytes = encode(&state).expect("encode");
        let restored = decode(&bytes).expect("decode");

        assert_eq!(restored.version, state.version);
        assert_eq!(restored.total_count(), state.total_count());

        let rebuilt = StatisticsAggregator::from_state(restored, StatisticsConfig::default())
            .expect("restored checkpoint loads");
        assert_eq!(rebuilt.total_count(), 2_000);
    }

    /// The checkpoint is stored compressed because it is far larger than the
    /// derived statistics; this guards the assumption that gzip earns its keep on
    /// this shape of data (repeated keys, short CJK strings).
    #[test]
    fn encoding_compresses_substantially() {
        let state = aggregator_with_messages(5_000).export_state();
        let raw = serde_json::to_vec(&state).expect("serialize");
        let compressed = encode(&state).expect("encode");

        assert!(
            compressed.len() * 2 < raw.len(),
            "expected better than 2x compression, got {} -> {} bytes",
            raw.len(),
            compressed.len()
        );
    }

    #[test]
    fn decode_rejects_garbage_without_panicking() {
        assert!(decode(b"not gzip at all").is_err());
    }
}
