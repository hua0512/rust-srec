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

// Keep CPU work bounded across collectors. A cancelled caller leaves only
// pure codec work behind; its permit stays with that work until it finishes.
static CHECKPOINT_CPU: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn offload<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> std::io::Result<T> {
    let permit = CHECKPOINT_CPU
        .acquire()
        .await
        .map_err(std::io::Error::other)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(std::io::Error::other)?
}

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

    let state = match offload(move || decode(&stored)).await {
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
    state: AggregatorState,
) {
    let Some(repo) = session_repo else {
        return;
    };

    let version = i64::from(state.version);
    let bytes = match offload(move || encode(&state)).await {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!(session_id, %error, "danmu: failed to encode statistics checkpoint");
            return;
        }
    };

    if let Err(error) = repo
        .upsert_danmu_aggregator_state(session_id, version, &bytes)
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

    #[tokio::test]
    async fn encode_decode_round_trip_preserves_the_checkpoint() {
        let state = aggregator_with_messages(2_000).export_state();
        let expected = serde_json::to_value(&state).unwrap();
        let bytes = offload(move || encode(&state)).await.expect("encode");
        let restored = offload(move || decode(&bytes)).await.expect("decode");

        assert_eq!(serde_json::to_value(&restored).unwrap(), expected);

        let rebuilt = StatisticsAggregator::from_state(restored, StatisticsConfig::default())
            .expect("restored checkpoint loads");
        assert_eq!(rebuilt.total_count(), 2_000);
    }

    #[tokio::test]
    async fn offloaded_codec_runs_off_runtime_and_bounds_cpu_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let async_thread = std::thread::current().id();
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let active = active.clone();
            let peak = peak.clone();
            tasks.spawn(offload(move || {
                assert_ne!(std::thread::current().id(), async_thread);
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(count, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(5));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            }));
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(result) = tasks.join_next().await {
                result.unwrap().unwrap();
            }
        })
        .await
        .unwrap();
        assert!((1..=2).contains(&peak.load(Ordering::SeqCst)));
        assert_eq!(active.load(Ordering::SeqCst), 0);
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

    #[tokio::test]
    async fn decode_rejects_garbage_without_panicking() {
        assert!(offload(|| decode(b"not gzip at all")).await.is_err());
    }
}
