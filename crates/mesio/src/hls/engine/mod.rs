//! The HLS engine: two owned loops plus an off-thread crypto pool.
//!
//! ```text
//! Task A  PlaylistWatcher    async playlist polling; emits PlaylistSnapshot
//!                                |  (coalescing watch channel)
//! Task B  Scheduler Reactor  owns SegmentStateStore; plans snapshots; drives
//!                            bounded fetch-and-process tasks (JoinSet);
//!                            forwards AssemblerInput downstream
//!                                |  (bounded mpsc, permit-reserve)
//! Task C  SequenceAssembler  reorder by MSN, init gating, gap policy,
//!                            terminal events -> HlsStreamEvent
//! ```
//!
//! Control-plane state lives in exactly one place — the reactor. Data-plane
//! payloads move by value as `Bytes` handles. See
//! `crates/mesio/docs/HLS_ENGINE_ARCHITECTURE.md` for the full design.

pub mod assembler;
pub mod budget;
pub mod crypto;
pub mod descriptor;
pub mod fetch;
pub mod identity;
pub mod input;
pub mod payload;
pub mod planner;
pub mod reactor;
pub mod store;
pub mod watcher;

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};
use url::Url;

use crate::CacheManager;
use crate::downloader::ClientPool;
use crate::hls::HlsDownloaderError;
use crate::hls::config::{HlsConfig, IdentityPolicyConfig};
use crate::hls::events::HlsStreamEvent;
use crate::hls::metrics::PerformanceMetrics;
use crate::hls::playlist::{InitialPlaylist, PlaylistEngine};
use crate::hls::twitch_processor::TwitchPlaylistProcessor;
use crate::session::EventSink;

use assembler::SequenceAssembler;
use budget::ByteBudget;
use crypto::{CryptoBackend, CryptoExecutor, KeyCache};
use fetch::FetchContext;
use identity::{SegmentIdentityPolicy, StripQueryIdentity};
use planner::PlannerContext;
pub use reactor::Terminal;
use reactor::{ReactorConfig, run_reactor};
use store::StoreConfig;
use watcher::PlaylistWatcher;

pub struct EngineHandles {
    pub watcher: JoinHandle<()>,
    pub reactor: JoinHandle<Terminal>,
    pub assembler: JoinHandle<()>,
    pub performance_metrics: Option<Arc<PerformanceMetrics>>,
}

/// `start` with a client pool built from `config.base`. Convenience for
/// callers (and the integration harness) that do not manage a shared pool.
pub async fn start_standalone(
    initial_url: String,
    config: HlsConfig,
    cache_manager: Option<Arc<CacheManager>>,
    cancel: CancellationToken,
    events: Option<EventSink>,
) -> Result<
    (
        mpsc::Receiver<Result<HlsStreamEvent, HlsDownloaderError>>,
        EngineHandles,
    ),
    HlsDownloaderError,
> {
    let clients = Arc::new(crate::downloader::create_client_pool(&config.base)?);
    start_with_events(
        initial_url,
        Arc::new(config),
        clients,
        cache_manager,
        cancel,
        events,
    )
    .await
}

pub async fn start_with_events(
    initial_url: String,
    config: Arc<HlsConfig>,
    clients: Arc<ClientPool>,
    cache_manager: Option<Arc<CacheManager>>,
    cancel: CancellationToken,
    events: Option<EventSink>,
) -> Result<
    (
        mpsc::Receiver<Result<HlsStreamEvent, HlsDownloaderError>>,
        EngineHandles,
    ),
    HlsDownloaderError,
> {
    let performance_metrics = config
        .output_config
        .metrics_enabled
        .then(|| Arc::new(PerformanceMetrics::new()));

    // --- Initial playlist + variant selection (one-shot, before any task) ---
    let playlist_engine = PlaylistEngine::new(
        Arc::clone(&clients),
        cache_manager.clone(),
        Arc::clone(&config),
    )
    .with_events(events.clone());
    let initial = playlist_engine.load_initial_playlist(&initial_url).await?;
    let (initial_media_playlist, base_url, media_playlist_url) = match &initial {
        InitialPlaylist::Master(_, _) => {
            let details = playlist_engine
                .select_media_playlist(&initial, &config.playlist_config.variant_selection_policy)
                .await?;
            let url = Url::parse(&details.url).map_err(|e| HlsDownloaderError::Playlist {
                reason: format!("invalid media playlist URL {}: {e}", details.url),
            })?;
            (details.playlist, details.base_url, url)
        }
        InitialPlaylist::Media(playlist, base) => {
            let url = Url::parse(&initial_url).map_err(|e| HlsDownloaderError::Playlist {
                reason: format!("invalid playlist URL {initial_url}: {e}"),
            })?;
            (playlist.clone(), base.clone(), url)
        }
    };
    let is_live = !initial_media_playlist.end_list;
    let initial_media_sequence = initial_media_playlist.media_sequence;
    info!(
        live = is_live,
        media_sequence = initial_media_sequence,
        url = %media_playlist_url,
        "starting HLS engine"
    );

    // --- Shared data-plane context ---
    let engine = &config.engine_config;
    let budget = Arc::new(ByteBudget::new(
        engine.max_inflight_download_bytes,
        engine.max_processing_bytes,
    ));
    let crypto_backend = if config.decryption_config.offload_decryption_to_cpu_pool {
        CryptoBackend::TokioBlocking
    } else {
        CryptoBackend::Inline
    };
    let fetch_ctx = Arc::new(FetchContext {
        clients: Arc::clone(&clients),
        config: Arc::clone(&config),
        budget,
        crypto: CryptoExecutor::new(crypto_backend),
        key_cache: KeyCache::new(
            config.decryption_config.key_cache_ttl,
            engine.key_cache_max_entries,
        ),
        cache_manager,
        metrics: performance_metrics.clone(),
        cancel: cancel.clone(),
        events: events.clone(),
    });

    // --- Identity policy + planner context (per-source) ---
    let policy = match &engine.identity_policy {
        IdentityPolicyConfig::FullUrl => SegmentIdentityPolicy::default(),
        IdentityPolicyConfig::StripQueryKeys(keys) => {
            SegmentIdentityPolicy::StripQuery(StripQueryIdentity::new(keys.iter().cloned()))
        }
    };
    let is_twitch = TwitchPlaylistProcessor::is_twitch_playlist(&base_url);
    let is_soop = super::soop_processor::is_soop_playlist(base_url.as_str());
    let planner_ctx = PlannerContext::new(policy, is_twitch, is_soop);
    debug!(twitch = is_twitch, soop = is_soop, "planner context built");

    // --- Channels ---
    let (client_event_tx, client_event_rx) = mpsc::channel(32);
    let concurrency = config.scheduler_config.download_concurrency.max(1);
    let assembler_capacity =
        (concurrency * config.scheduler_config.processed_segment_buffer_multiplier).max(1);
    let (assembler_tx, assembler_rx) = mpsc::channel(assembler_capacity);

    // --- Task A: watcher ---
    let watcher = PlaylistWatcher::new(
        Arc::clone(&clients),
        Arc::clone(&config),
        media_playlist_url,
        Arc::from(base_url.as_str()),
        cancel.clone(),
    )
    .with_events(events.clone());
    let (playlist_rx, watcher_handle) = watcher.spawn(initial_media_playlist);

    // --- Task C: assembler (spawned before the reactor so its receiver is
    // live the moment outcomes start flowing) ---
    let mut assembler = SequenceAssembler::new(
        Arc::clone(&config),
        assembler_rx,
        client_event_tx,
        is_live,
        initial_media_sequence,
        cancel.clone(),
    );
    if let Some(metrics) = &performance_metrics {
        assembler = assembler.with_performance_metrics(Arc::clone(metrics));
    }
    let assembler_handle = tokio::spawn(assembler.run());

    // --- Task B: reactor ---
    let reactor_config = ReactorConfig {
        store: StoreConfig {
            max_state_entries: engine.max_state_entries,
            retry_budget: engine.lifecycle_retry_budget,
            retry_delay_base: engine.lifecycle_retry_delay_base,
            retry_delay_max: engine.lifecycle_retry_delay_max,
            fallback_size_estimate: engine.initial_segment_size_estimate,
            max_segment_size: engine.max_segment_size_bytes,
            max_retained_inits: engine.max_retained_inits,
        },
        max_concurrency: concurrency,
        max_pending_payload_bytes: engine.max_pending_payload_bytes,
        max_pending_items: engine.max_pending_items,
    };
    let reactor_cancel = cancel.clone();
    let reactor_handle = tokio::spawn(async move {
        let terminal = run_reactor(
            playlist_rx,
            assembler_tx,
            planner_ctx,
            fetch_ctx,
            reactor_config,
            reactor_cancel,
        )
        .await;
        debug!(?terminal, "reactor terminated");
        terminal
    });

    Ok((
        client_event_rx,
        EngineHandles {
            watcher: watcher_handle,
            reactor: reactor_handle,
            assembler: assembler_handle,
            performance_metrics,
        },
    ))
}
