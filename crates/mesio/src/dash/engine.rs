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
//! (v1 limitation): a configured track that Period lacks is not recorded
//! in this session, even once it appears (in a later Period or in an update
//! of the starting one), which is reported with a warning. A recorded
//! track missing from a later Period leaves a gap there, recorded with the
//! other track alone.
//!
//! Only the first `BaseURL` at each MPD level is used; alternatives (other
//! CDNs) are noted once, at start.

use std::sync::Arc;

use hls::HlsData;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, info_span, warn};
use url::Url;

use crate::CacheManager;
use crate::DownloadError;
use crate::dash::config::DashConfig;
use crate::dash::manifest::{Manifest, PresentationType, RejectedContent, Representation};
use crate::dash::mux::{MuxOutcome, MuxTrack, TrackSink, run_muxer};
use crate::dash::planner::{
    ConsumedFrontier, DashPlanner, PeriodKey, PlannerParams, StartPosition, decide_start,
    rejection_for,
};
use crate::dash::segment_index::OnIndexFailure;
use crate::dash::selection;
use crate::dash::watcher::{MpdFetcher, MpdWatcher};
use crate::downloader::ClientPool;
use crate::hls::engine::identity_policy;
use crate::segment_engine::assembler::{AssemblerExit, SequenceAssembler};
use crate::segment_engine::budget::ByteBudget;
use crate::segment_engine::config::SegmentCoreConfig;
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
    pub assembler: JoinHandle<AssemblerExit>,
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
    let throttle_deadline =
        tokio::time::Instant::now() + clients.rate_limiter().policy().max_throttled;
    let mut initial = fetcher.fetch(&url, None, true, throttle_deadline).await?;
    let now = initial.clock_offset.apply(config.clock.now());
    let start = decide_start(&initial.manifest, now, config.live_start, &config.selection)?;
    let (start, selected) = select_tracks(&initial.manifest, &config, start)?;
    let live = initial.manifest.presentation_type == PresentationType::Dynamic;
    if initial.manifest.ignored_base_urls > 0 {
        debug!(
            ignored = initial.manifest.ignored_base_urls,
            "the MPD offers alternative BaseURLs; only the first at each level is used"
        );
    }
    info!(
        live,
        video = selected.video,
        audio = selected.audio,
        periods = initial.manifest.periods.len(),
        url = %crate::redact::Redacted(&url),
        "starting DASH engine"
    );
    // Only now are the recorded tracks known, and with them the Segment
    // Indexes the planners need; an unused Representation's index is never
    // read, so it cannot fail the session.
    let recorded: Vec<MediaTrack> = [
        (MediaTrack::Video, selected.video),
        (MediaTrack::Audio, selected.audio),
    ]
    .into_iter()
    .filter_map(|(track, recorded)| recorded.then_some(track))
    .collect();
    fetcher.record_tracks(recorded.clone());
    fetcher
        .read_segment_indexes(
            Arc::make_mut(&mut initial.manifest),
            &url,
            OnIndexFailure::Fail,
            throttle_deadline,
        )
        .await?;
    if let Some(period) = initial
        .manifest
        .periods
        .iter()
        .find(|period| PeriodKey::of(period).same(&start.period))
    {
        ensure_initialization(
            recorded
                .iter()
                .filter_map(|&track| selection::select(period, track, &config.selection)),
        )?;
    }

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
            live_start: config.live_start,
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

        // The track's logs carry its name. The download metrics are
        // shared by both tracks and logged once, by `join`.
        let span = info_span!("dash_track", %track);
        let mut assembler = SequenceAssembler::new(
            Arc::clone(&core_config),
            assembler_rx,
            TrackSink::new(track_tx, frontier),
            live,
            0,
            track_cancel.clone(),
        );
        let emit_cursor = (!live).then(|| assembler.subscribe_cursor());
        let assembler_handle = tokio::spawn(assembler.run().instrument(span.clone()));

        let reactor_config = ReactorConfig {
            store: StoreConfig {
                max_state_entries: engine.max_state_entries,
                retry_budget: config.fetcher.max_segment_retries,
                retry_delay_base: config.fetcher.segment_retry_delay_base,
                retry_delay_max: config.fetcher.max_segment_retry_delay,
                max_throttled: clients.rate_limiter().policy().max_throttled,
                fallback_size_estimate: engine.initial_segment_size_estimate,
                max_segment_size: engine.max_segment_size_bytes,
                max_retained_inits: engine.max_retained_inits,
            },
            max_concurrency: concurrency,
            max_pending_payload_bytes: split.share(engine.max_pending_payload_bytes, video, both),
            max_pending_items: engine.max_pending_items,
            emit_cursor,
        };
        let reactor_rx = snapshot_rx.clone();
        let reactor_cancel = track_cancel.clone();
        let reactor_handle = tokio::spawn(
            async move {
                let terminal = run_reactor(
                    reactor_rx,
                    assembler_tx,
                    planner,
                    fetch_ctx,
                    reactor_config,
                    reactor_cancel,
                )
                .await;
                debug!(?terminal, "DASH reactor terminated");
                terminal
            }
            .instrument(span),
        );

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
    // Text, image and metadata tracks are never recorded, so offering them
    // is expected and their rejections are summarized; any other rejection
    // may cost the recording a track or a rendition and is warned about.
    let mut not_audio_video = 0_usize;
    for rejected in &period.rejected {
        if rejected.content == RejectedContent::Other {
            not_audio_video += 1;
            debug!(
                adaptation_set = rejected.adaptation_set_id.as_deref(),
                representation = rejected.representation_id.as_deref(),
                feature = %rejected.feature,
                "DASH text, image or metadata Representation not recorded"
            );
            continue;
        }
        warn!(
            adaptation_set = rejected.adaptation_set_id.as_deref(),
            representation = rejected.representation_id.as_deref(),
            feature = %rejected.feature,
            "DASH Representation skipped as unsupported"
        );
    }
    if not_audio_video > 0 {
        info!(
            representations = not_audio_video,
            "DASH text, image and metadata tracks (subtitles, thumbnails) are not recorded"
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
    ensure_initialization([video, audio].into_iter().flatten())?;
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

/// A recorded Representation needs an initialization segment. A
/// `SegmentBase` one whose index is unread may still find it there (see
/// `Representation::apply_segment_index`), so it is checked again once the
/// index is read.
fn ensure_initialization<'a>(
    representations: impl IntoIterator<Item = &'a Representation>,
) -> Result<(), DownloadError> {
    if representations.into_iter().any(|representation| {
        representation.initialization.is_none() && representation.pending_segment_index().is_none()
    }) {
        return Err(DownloadError::Unsupported {
            protocol: "dash",
            feature: "Representation without an initialization segment".to_owned(),
        });
    }
    Ok(())
}

fn core_config(config: &DashConfig) -> SegmentCoreConfig {
    crate::hls::engine::core_config(
        &config.base,
        &config.fetcher,
        &config.output,
        &config.engine,
        config.processed_segment_ttl,
    )
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
        performance_metrics,
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
    if let Some(metrics) = performance_metrics {
        metrics.log_summary("dash");
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
