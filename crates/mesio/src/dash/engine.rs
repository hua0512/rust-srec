//! The DASH engine: one MPD watcher, one segment pipeline per selected
//! track, and a muxer.
//!
//! ```text
//! MpdWatcher ──watch──┬─> video: reactor (DashPlanner) -> assembler ─┐
//!                     └─> audio: reactor (DashPlanner) -> assembler ─┴─> muxer -> HlsData
//! ```
//!
//! The tracks share the HTTP clients, cache, metrics and event sink, but each
//! has its own byte budget: a budget shared by two independent reactors can
//! be held entirely by one of them and starve the other.
//!
//! The track set is fixed at start, from the Period recording starts in
//! (v1 limitation): a configured track that only a later Period carries is
//! reported with a warning, not recorded.

use std::sync::Arc;

use hls::HlsData;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use url::Url;

use crate::CacheManager;
use crate::DownloadError;
use crate::dash::config::DashConfig;
use crate::dash::manifest::{Manifest, PresentationType};
use crate::dash::mux::{MuxOutcome, MuxTrack, TrackSink, run_muxer};
use crate::dash::planner::{
    ConsumedFrontier, DashPlanner, PeriodKey, PlannerParams, StartPosition, decide_start,
    rejection_for,
};
use crate::dash::selection;
use crate::dash::watcher::{MpdFetcher, MpdWatcher};
use crate::downloader::ClientPool;
use crate::hls::engine::identity_policy;
use crate::segment_engine::assembler::SequenceAssembler;
use crate::segment_engine::budget::ByteBudget;
use crate::segment_engine::config::{FetchConfig, OutputConfig, SegmentCoreConfig};
use crate::segment_engine::descriptor::MediaTrack;
use crate::segment_engine::fetch::FetchContext;
use crate::segment_engine::metrics::PerformanceMetrics;
use crate::segment_engine::reactor::{ReactorConfig, Terminal, run_reactor};
use crate::segment_engine::store::StoreConfig;
use crate::session::EventSink;

/// Items buffered between a track's assembler and the muxer.
const TRACK_CHANNEL_CAPACITY: usize = 32;
/// Items buffered between the muxer and the consumer.
const OUTPUT_CHANNEL_CAPACITY: usize = 32;

pub(crate) struct TrackHandles {
    pub track: MediaTrack,
    pub reactor: JoinHandle<Terminal>,
    pub assembler: JoinHandle<()>,
}

pub(crate) struct DashEngineHandles {
    pub watcher: JoinHandle<()>,
    pub tracks: Vec<TrackHandles>,
    pub muxer: JoinHandle<MuxOutcome>,
    pub performance_metrics: Option<Arc<PerformanceMetrics>>,
}

/// The Representations recording starts with.
struct Selected {
    video: bool,
    audio: bool,
    /// Configured tracks the starting Period lacks.
    absent: Vec<MediaTrack>,
}

pub(crate) async fn start(
    url: Url,
    config: Arc<DashConfig>,
    clients: Arc<ClientPool>,
    cache_manager: Option<Arc<CacheManager>>,
    cancel: CancellationToken,
    events: Option<EventSink>,
) -> Result<
    (
        mpsc::Receiver<Result<HlsData, DownloadError>>,
        DashEngineHandles,
    ),
    DownloadError,
> {
    config.validate()?;
    let fetcher = MpdFetcher::new(
        Arc::clone(&clients),
        Arc::clone(&config),
        events.clone(),
        cancel.clone(),
    );
    let initial = fetcher.fetch(&url, None, true).await?;
    let manifest = Arc::clone(&initial.manifest);
    let now = initial.clock_offset.apply(config.clock.now());
    let start = decide_start(&manifest, now, config.live_start, &config.selection)?;
    let (start, selected) = select_tracks(&manifest, &config, start)?;
    let live = manifest.presentation_type == PresentationType::Dynamic;
    info!(
        live,
        video = selected.video,
        audio = selected.audio,
        periods = manifest.periods.len(),
        url = %crate::redact::Redacted(&url),
        "starting DASH engine"
    );

    let performance_metrics = config
        .output
        .metrics_enabled
        .then(|| Arc::new(PerformanceMetrics::new()));
    let core_config = Arc::new(core_config(&config));

    let watcher = MpdWatcher::new(Arc::new(fetcher), Arc::clone(&config), url, cancel.clone());
    let (snapshot_rx, watcher_handle) = watcher.spawn(initial);

    let both = selected.video && selected.audio;
    let mut tracks = Vec::new();
    let mut inputs = Vec::new();
    for (track, wanted) in [
        (MediaTrack::Video, selected.video),
        (MediaTrack::Audio, selected.audio),
    ] {
        if !wanted {
            continue;
        }
        let track_cancel = cancel.child_token();
        let engine = &config.engine;
        let split = config.budget_split;
        let video = track == MediaTrack::Video;
        let budget = Arc::new(ByteBudget::new(
            split.share(engine.max_inflight_download_bytes, video, both),
            split.share(engine.max_processing_bytes, video, both),
        ));
        let fetch_ctx = Arc::new(FetchContext {
            clients: Arc::clone(&clients),
            config: Arc::clone(&core_config),
            budget,
            decryptor: None,
            cache_manager: cache_manager.clone(),
            metrics: performance_metrics.clone(),
            cancel: track_cancel.clone(),
            events: events.clone(),
        });
        let frontier = Arc::new(ConsumedFrontier::default());
        let leader = tracks.is_empty();
        let planner = DashPlanner::new(PlannerParams {
            track,
            selection: Arc::new(config.selection.clone()),
            clock: Arc::clone(&config.clock),
            identity: identity_policy(&engine.identity_policy),
            frontier: Arc::clone(&frontier),
            start: start.clone(),
            absent_tracks: if leader {
                selected.absent.clone()
            } else {
                Vec::new()
            },
        });

        let concurrency = config.scheduler.download_concurrency.max(1);
        let assembler_capacity =
            (concurrency * config.scheduler.processed_segment_buffer_multiplier).max(1);
        let (assembler_tx, assembler_rx) = mpsc::channel(assembler_capacity);
        let (track_tx, track_rx) = mpsc::channel(TRACK_CHANNEL_CAPACITY);

        let mut assembler = SequenceAssembler::new(
            Arc::clone(&core_config),
            assembler_rx,
            TrackSink::new(track_tx, frontier),
            live,
            0,
            track_cancel.clone(),
        );
        if let Some(metrics) = &performance_metrics {
            assembler = assembler.with_performance_metrics(Arc::clone(metrics));
        }
        let assembler_handle = tokio::spawn(assembler.run());

        let reactor_config = ReactorConfig {
            store: StoreConfig {
                max_state_entries: engine.max_state_entries,
                retry_budget: config.fetcher.max_segment_retries,
                retry_delay_base: config.fetcher.segment_retry_delay_base,
                retry_delay_max: config.fetcher.max_segment_retry_delay,
                fallback_size_estimate: engine.initial_segment_size_estimate,
                max_segment_size: engine.max_segment_size_bytes,
                max_retained_inits: engine.max_retained_inits,
            },
            max_concurrency: concurrency,
            max_pending_payload_bytes: split.share(engine.max_pending_payload_bytes, video, both),
            max_pending_items: engine.max_pending_items,
        };
        let reactor_rx = snapshot_rx.clone();
        let reactor_cancel = track_cancel.clone();
        let reactor_handle = tokio::spawn(async move {
            let terminal = run_reactor(
                reactor_rx,
                assembler_tx,
                planner,
                fetch_ctx,
                reactor_config,
                reactor_cancel,
            )
            .await;
            debug!(%track, ?terminal, "DASH reactor terminated");
            terminal
        });

        tracks.push(TrackHandles {
            track,
            reactor: reactor_handle,
            assembler: assembler_handle,
        });
        inputs.push(MuxTrack {
            track,
            rx: track_rx,
            cancel: track_cancel,
        });
    }
    // The planners hold their own receivers; dropping this one lets the
    // watcher stop once every track is done.
    drop(snapshot_rx);

    let mut inputs = inputs.into_iter();
    let Some(leader) = inputs.next() else {
        return Err(DownloadError::Internal {
            reason: "DASH engine started without a track".to_owned(),
        });
    };
    let follower = inputs.next();
    let (out_tx, out_rx) = mpsc::channel(OUTPUT_CHANNEL_CAPACITY);
    let muxer = tokio::spawn(run_muxer(
        leader,
        follower,
        out_tx,
        events,
        config.mux.clone(),
        live.then_some(config.output.live_max_overall_stall_duration)
            .flatten(),
        cancel,
    ));

    Ok((
        out_rx,
        DashEngineHandles {
            watcher: watcher_handle,
            tracks,
            muxer,
            performance_metrics,
        },
    ))
}

/// Decides which tracks to record from the Period recording starts in, and
/// fails early (with a typed error) when nothing there can be recorded.
fn select_tracks(
    manifest: &Manifest,
    config: &DashConfig,
    start: Option<StartPosition>,
) -> Result<(StartPosition, Selected), DownloadError> {
    let period = match &start {
        Some(start) => manifest
            .periods
            .iter()
            .find(|period| PeriodKey::of(period).same(&start.period)),
        None => manifest.periods.first(),
    };
    let Some(period) = period else {
        return Err(DownloadError::Manifest {
            reason: "MPD has no Period".to_owned(),
        });
    };
    period.ensure_usable()?;
    for rejected in &period.rejected {
        warn!(
            adaptation_set = rejected.adaptation_set_id.as_deref(),
            representation = rejected.representation_id.as_deref(),
            feature = %rejected.feature,
            "DASH Representation skipped as unsupported"
        );
    }
    let selection = &config.selection;
    let video = selection
        .include_video
        .then(|| selection::select(period, MediaTrack::Video, selection))
        .flatten();
    let multiplexed = video.is_some_and(selection::is_multiplexed);
    let audio = (selection.include_audio && !multiplexed)
        .then(|| selection::select(period, MediaTrack::Audio, selection))
        .flatten();
    let mut absent = Vec::new();
    if selection.include_video && video.is_none() {
        absent.push(MediaTrack::Video);
    }
    if selection.include_audio && !multiplexed && audio.is_none() {
        absent.push(MediaTrack::Audio);
    }
    // A configured track the starting Period offers only in rejected form
    // cannot be recorded; starting without it would hide why.
    for &track in &absent {
        if let Some(feature) = rejection_for(period, track) {
            return Err(crate::dash::manifest::ManifestError::from(feature).into());
        }
    }
    let (Some(start), true) = (start, video.is_some() || audio.is_some()) else {
        return Err(DownloadError::Unsupported {
            protocol: "dash",
            feature: "no Representation matches the track selection".to_owned(),
        });
    };
    if [video, audio]
        .into_iter()
        .flatten()
        .any(|representation| representation.initialization.is_none())
    {
        return Err(DownloadError::Unsupported {
            protocol: "dash",
            feature: "Representation without an initialization segment".to_owned(),
        });
    }
    if multiplexed {
        debug!("video Representation carries audio; no separate audio track is recorded");
    }
    for track in &absent {
        info!(%track, "the starting Period has no such track; it is not recorded");
    }
    Ok((
        start,
        Selected {
            video: video.is_some(),
            audio: audio.is_some(),
            absent,
        },
    ))
}

fn core_config(config: &DashConfig) -> SegmentCoreConfig {
    let fetcher = &config.fetcher;
    let output = &config.output;
    SegmentCoreConfig {
        fetch: FetchConfig {
            params: config.base.params.clone(),
            segment_download_timeout: fetcher.segment_download_timeout,
            max_segment_retries: fetcher.max_segment_retries,
            segment_retry_delay_base: fetcher.segment_retry_delay_base,
            progress_emit_min_bytes: fetcher.progress_emit_min_bytes,
            progress_emit_min_interval: fetcher.progress_emit_min_interval,
            processed_segment_ttl: config.processed_segment_ttl,
            max_segment_size_bytes: config.engine.max_segment_size_bytes,
        },
        output: OutputConfig {
            live_reorder_buffer_duration: output.live_reorder_buffer_duration,
            live_reorder_buffer_max_segments: output.live_reorder_buffer_max_segments,
            gap_evaluation_interval: output.gap_evaluation_interval,
            max_pending_init_segments: output.max_pending_init_segments,
            live_max_overall_stall_duration: output.live_max_overall_stall_duration,
            live_gap_strategy: output.live_gap_strategy.clone(),
            vod_gap_strategy: output.vod_gap_strategy.clone(),
            vod_segment_timeout: output.vod_segment_timeout,
            buffer_limits: output.buffer_limits.clone(),
        },
    }
}

/// Waits for every engine task and reports how the session ended. The
/// muxer's outcome decides, since it is the only task that sees every
/// track, unless another task panicked: a panic is a pipeline error even
/// when the muxer saw only a closed channel.
pub(crate) async fn join(
    handles: DashEngineHandles,
    cancel: CancellationToken,
) -> crate::DownloadTerminal {
    use crate::DownloadTerminal;
    let DashEngineHandles {
        watcher,
        tracks,
        muxer,
        ..
    } = handles;
    let mut terminal = match muxer.await {
        Ok(MuxOutcome::Ended) => DownloadTerminal::AuthoritativeEnd,
        Ok(MuxOutcome::Cancelled) => DownloadTerminal::Cancelled,
        Ok(MuxOutcome::DownstreamClosed) => DownloadTerminal::DownstreamClosed,
        Ok(MuxOutcome::Failed(reason)) => DownloadTerminal::PipelineError(reason),
        Err(error) => task_failure("muxer", None, &error),
    };
    // Whatever the outcome, nothing downstream consumes the tracks anymore.
    cancel.cancel();
    let mut panicked = None;
    for handles in tracks {
        if let Err(error) = handles.reactor.await {
            panicked.get_or_insert(task_failure("reactor", Some(handles.track), &error));
        }
        if let Err(error) = handles.assembler.await {
            panicked.get_or_insert(task_failure("assembler", Some(handles.track), &error));
        }
    }
    if let Err(error) = watcher.await {
        panicked.get_or_insert(task_failure("MPD watcher", None, &error));
    }
    if let Some(failure) = panicked
        && !matches!(terminal, DownloadTerminal::PipelineError(_))
    {
        terminal = failure;
    }
    debug!(
        terminal = terminal_label(&terminal),
        "DASH pipeline tasks finished"
    );
    terminal
}

fn task_failure(
    task: &'static str,
    track: Option<MediaTrack>,
    error: &tokio::task::JoinError,
) -> crate::DownloadTerminal {
    let track = track.map(MediaTrack::as_str);
    if error.is_cancelled() {
        warn!(task, track, "DASH task was aborted");
    } else {
        warn!(task, track, error = %crate::redact::redact_diagnostic(error), "DASH task panicked");
    }
    crate::DownloadTerminal::PipelineError(Arc::from(format!("DASH {task} task failed")))
}

/// A log form of a terminal that leaves out failure details, which can
/// quote server responses.
fn terminal_label(terminal: &crate::DownloadTerminal) -> &'static str {
    match terminal {
        crate::DownloadTerminal::AuthoritativeEnd => "AuthoritativeEnd",
        crate::DownloadTerminal::Cancelled => "Cancelled",
        crate::DownloadTerminal::DownstreamClosed => "DownstreamClosed",
        crate::DownloadTerminal::PipelineError(_) => "PipelineError([redacted])",
    }
}
