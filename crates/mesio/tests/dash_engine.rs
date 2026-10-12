//! Integration harness for the DASH engine: a scripted mock origin (axum)
//! serving MPD generations and fMP4 media, driven through the public
//! `DashDownloader` / `MesioDownloader` API.
//!
//! Media comes from the ffmpeg-generated fixtures of `crates/mp4`; live
//! streams reuse their first segment with the `tfdt` moved to each
//! segment's time, which yields structurally valid, decodable fragments at
//! any position. Live availability is driven by a manual clock.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::Response;
use bytes::Bytes;
use chrono::{DateTime, TimeZone, Utc};
use futures::StreamExt;
use hls::{HlsData, M4sData, SplitReason};
use mp4::independent::IndependentFragmentCheck;

use mesio_engine::dash::manifest::ManualClock;
use mesio_engine::dash::{DashConfig, DashDownloader, LiveStartPolicy};
use mesio_engine::{
    ContentSource, DashRequestOptions, DownloadError, DownloadEvent, DownloadRequest,
    DownloadSession, DownloadTerminal, DownloaderSession, MediaEngine, MediaTrack, MesioConfig,
    MesioDownloader, ProtocolSelection,
};

const VIDEO_INIT: &[u8] = include_bytes!("../../mp4/tests/fixtures/dash/video-init.mp4");
const VIDEO_1: &[u8] = include_bytes!("../../mp4/tests/fixtures/dash/video-1.m4s");
const VIDEO_2: &[u8] = include_bytes!("../../mp4/tests/fixtures/dash/video-2.m4s");
const AUDIO_INIT: &[u8] = include_bytes!("../../mp4/tests/fixtures/dash/audio-init.mp4");
const AUDIO_1: &[u8] = include_bytes!("../../mp4/tests/fixtures/dash/audio-1.m4s");
const AUDIO_2: &[u8] = include_bytes!("../../mp4/tests/fixtures/dash/audio-2.m4s");
const AUDIO_3: &[u8] = include_bytes!("../../mp4/tests/fixtures/dash/audio-3.m4s");

/// Media timescales of the fixtures (`mdhd`).
const VIDEO_TIMESCALE: u64 = 12_288;
const AUDIO_TIMESCALE: u64 = 48_000;

const TEST_TIMEOUT: Duration = Duration::from_secs(20);

// --- Mock origin ---

/// Serves `{prefix}/{n}.m4s` for any `n` from one fixture segment, with the
/// decode time of segment number `n` (1-based, one second each).
#[derive(Clone)]
struct Numbered {
    segment: &'static [u8],
    timescale: u64,
}

#[derive(Default)]
struct FileEntry {
    body: Vec<u8>,
    fail_status: u16,
    fail_times: u32,
}

#[derive(Default)]
struct OriginState {
    mpd: String,
    mpd_serves: u32,
    /// After this many successful MPD serves, the MPD answers 500.
    mpd_fail_after: Option<u32>,
    /// The MPD answers this status (e.g. 404) forever.
    mpd_status: Option<u16>,
    files: HashMap<String, FileEntry>,
    numbered: HashMap<String, Numbered>,
    /// Path prefixes answered only once their gate opens.
    gates: HashMap<String, tokio::sync::watch::Receiver<bool>>,
    hits: HashMap<String, u64>,
}

#[derive(Clone, Default)]
struct Origin(Arc<Mutex<OriginState>>);

impl Origin {
    fn set_mpd(&self, body: impl Into<String>) {
        self.0.lock().unwrap().mpd = body.into();
    }

    fn add_file(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.add_file_failing(path, body, 0, 0);
    }

    fn add_file_failing(&self, path: &str, body: impl Into<Vec<u8>>, status: u16, times: u32) {
        self.0.lock().unwrap().files.insert(
            path.to_string(),
            FileEntry {
                body: body.into(),
                fail_status: status,
                fail_times: times,
            },
        );
    }

    fn add_numbered(&self, prefix: &str, segment: &'static [u8], timescale: u64) {
        self.0
            .lock()
            .unwrap()
            .numbered
            .insert(prefix.to_string(), Numbered { segment, timescale });
    }

    fn add_av(&self) {
        self.add_file("v/init.mp4", VIDEO_INIT);
        self.add_file("a/init.mp4", AUDIO_INIT);
        self.add_numbered("v", VIDEO_1, VIDEO_TIMESCALE);
        self.add_numbered("a", AUDIO_1, AUDIO_TIMESCALE);
    }

    /// Holds requests under `prefix` until the returned gate is opened.
    fn gate(&self, prefix: &str) -> tokio::sync::watch::Sender<bool> {
        let (open, gate) = tokio::sync::watch::channel(false);
        self.0
            .lock()
            .unwrap()
            .gates
            .insert(prefix.to_string(), gate);
        open
    }

    fn fail_mpd_after(&self, serves: u32) {
        self.0.lock().unwrap().mpd_fail_after = Some(serves);
    }

    fn mpd_status(&self, status: u16) {
        self.0.lock().unwrap().mpd_status = Some(status);
    }

    fn hits(&self, path: &str) -> u64 {
        self.0.lock().unwrap().hits.get(path).copied().unwrap_or(0)
    }

    async fn serve(self) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock origin");
        let addr = listener.local_addr().expect("local addr");
        let app = Router::new().fallback(handler).with_state(self);
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }
}

fn respond(status: StatusCode, body: Vec<u8>) -> Response {
    Response::builder()
        .status(status)
        .body(Body::from(body))
        .expect("response builds")
}

async fn handler(State(origin): State<Origin>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/').to_string();
    let gate = {
        let state = origin.0.lock().unwrap();
        state
            .gates
            .iter()
            .find(|(prefix, _)| path.starts_with(prefix.as_str()))
            .map(|(_, gate)| gate.clone())
    };
    if let Some(mut gate) = gate {
        let _ = gate.wait_for(|open| *open).await;
    }
    let mut state = origin.0.lock().unwrap();
    *state.hits.entry(path.clone()).or_default() += 1;

    if path.ends_with(".mpd") {
        if let Some(status) = state.mpd_status {
            return respond(StatusCode::from_u16(status).unwrap(), Vec::new());
        }
        if state
            .mpd_fail_after
            .is_some_and(|after| state.mpd_serves >= after)
        {
            return respond(StatusCode::INTERNAL_SERVER_ERROR, Vec::new());
        }
        state.mpd_serves += 1;
        return respond(StatusCode::OK, state.mpd.clone().into_bytes());
    }
    if let Some(entry) = state.files.get_mut(&path) {
        if entry.fail_times > 0 {
            entry.fail_times -= 1;
            return respond(
                StatusCode::from_u16(entry.fail_status).unwrap_or(StatusCode::NOT_FOUND),
                Vec::new(),
            );
        }
        return respond(StatusCode::OK, entry.body.clone());
    }
    if let Some((prefix, name)) = path.rsplit_once('/')
        && let Some(numbered) = state.numbered.get(prefix)
        && let Some(number) = name
            .strip_suffix(".m4s")
            .and_then(|number| number.parse::<u64>().ok())
        && number > 0
    {
        let body = with_tfdt(numbered.segment, (number - 1) * numbered.timescale);
        return respond(StatusCode::OK, body);
    }
    respond(StatusCode::NOT_FOUND, Vec::new())
}

/// `segment` with its (version 1) `tfdt` set to `decode_time`.
fn with_tfdt(segment: &[u8], decode_time: u64) -> Vec<u8> {
    let mut data = segment.to_vec();
    let at = data
        .windows(4)
        .position(|window| window == b"tfdt")
        .expect("segment has a tfdt");
    assert_eq!(data[at + 4], 1, "fixture tfdt is version 1");
    data[at + 8..at + 16].copy_from_slice(&decode_time.to_be_bytes());
    data
}

// --- MPDs ---

fn ast() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()
}

/// A/V `$Number$` templates of one-second segments. `kind` is `dynamic` or
/// `static`; a static MPD lasts `duration_secs`.
fn number_mpd(dynamic: bool, duration_secs: u64, video_id: &str) -> String {
    let presentation = if dynamic {
        r#"type="dynamic" availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT0.1S""#.to_string()
    } else {
        format!(
            r#"type="static" availabilityStartTime="2024-01-01T00:00:00Z" mediaPresentationDuration="PT{duration_secs}S""#
        )
    };
    format!(
        r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" {presentation} profiles="urn:mpeg:dash:profile:isoff-live:2011">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4" segmentAlignment="true" startWithSAP="1">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="1" timescale="1" startNumber="1"/>
      <Representation id="{video_id}" bandwidth="40000" width="64" height="64" codecs="avc1.64000a"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="en">
      <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="1" timescale="1" startNumber="1"/>
      <Representation id="a" bandwidth="24000" codecs="mp4a.40.2"/>
    </AdaptationSet>
  </Period>
</MPD>"#
    )
}

/// The fixtures exactly, addressed by `$Time$` timelines: two video
/// segments (2.0 s) and three audio segments (2.09 s), so the audio track
/// ends after the video.
fn static_timeline_mpd() -> String {
    r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2.1S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/t$Time$.m4s" initialization="v/init.mp4" timescale="12288">
        <SegmentTimeline><S t="0" d="12288" r="1"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v" bandwidth="40000" codecs="avc1.64000a"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="a/t$Time$.m4s" initialization="a/init.mp4" timescale="48000">
        <SegmentTimeline><S t="0" d="45056"/><S d="51200"/><S d="4096"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="a" bandwidth="24000" codecs="mp4a.40.2"/>
    </AdaptationSet>
  </Period>
</MPD>"#
        .to_string()
}

fn add_timeline_files(origin: &Origin) {
    origin.add_file("v/init.mp4", VIDEO_INIT);
    origin.add_file("a/init.mp4", AUDIO_INIT);
    origin.add_file("v/t0.m4s", VIDEO_1);
    origin.add_file("v/t12288.m4s", VIDEO_2);
    origin.add_file("a/t0.m4s", AUDIO_1);
    origin.add_file("a/t45056.m4s", AUDIO_2);
    origin.add_file("a/t96256.m4s", AUDIO_3);
}

// --- Harness ---

fn config(clock: Option<Arc<ManualClock>>) -> DashConfig {
    let mut config = DashConfig::default();
    // Generous: under a loaded test runner a timed-out request would be
    // retried and change what the tests count.
    config.manifest.fetch_timeout = Duration::from_secs(15);
    config.fetcher.segment_download_timeout = Duration::from_secs(15);
    config.manifest.min_refresh_interval = Duration::from_millis(50);
    config.manifest.max_refresh_retries = 2;
    config.manifest.retry_delay = Duration::from_millis(20);
    config.manifest.sync_clock = false;
    config.fetcher.segment_retry_delay_base = Duration::from_millis(10);
    config.fetcher.max_segment_retry_delay = Duration::from_millis(50);
    config.output.live_max_overall_stall_duration = Some(Duration::from_secs(10));
    config.output.gap_evaluation_interval = Duration::from_millis(20);
    if let Some(clock) = clock {
        config.clock = clock;
    }
    config
}

fn live_clock(elapsed_secs: i64) -> Arc<ManualClock> {
    Arc::new(ManualClock::new(
        ast() + chrono::TimeDelta::seconds(elapsed_secs),
    ))
}

async fn start(url: &str, config: DashConfig) -> Result<DownloadSession<HlsData>, DownloadError> {
    let downloader = DashDownloader::new(config)?;
    downloader
        .start(DownloadRequest::from_url(url).expect("valid URL"))
        .await
}

/// Reads items until the stream ends (or `stop` says so), bounded by the
/// test timeout.
async fn read_until(
    items: &mut mesio_engine::BoxMediaStream<HlsData, DownloadError>,
    out: &mut Vec<HlsData>,
    mut stop: impl FnMut(&[HlsData]) -> bool,
) -> Option<DownloadError> {
    loop {
        if stop(out) {
            return None;
        }
        let item = tokio::time::timeout(TEST_TIMEOUT, items.next())
            .await
            .unwrap_or_else(|_| panic!("timed out; items so far: {}", describe(out)));
        match item {
            Some(Ok(item)) => out.push(item),
            Some(Err(error)) => return Some(error),
            None => return None,
        }
    }
}

/// Collects a session's events as they come, so the bounded event channel
/// never drops any.
struct EventLog {
    events: Arc<Mutex<Vec<DownloadEvent>>>,
    task: tokio::task::JoinHandle<()>,
}

impl EventLog {
    fn collect(mut stream: mesio_engine::DownloadEventStream) -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let task = tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                sink.lock().unwrap().push(event);
            }
        });
        Self { events, task }
    }

    fn snapshot(&self) -> Vec<DownloadEvent> {
        self.events.lock().unwrap().clone()
    }

    /// Every event, once the session's event sinks are gone.
    async fn finish(self) -> Vec<DownloadEvent> {
        let _ = tokio::time::timeout(TEST_TIMEOUT, self.task).await;
        self.events.lock().unwrap().clone()
    }
}

/// Successful downloads of `path` (an MPD or a segment) reported by the
/// session. Unlike origin hits, a request retried after a transport error
/// counts once, so this shows how often a resource was planned and fetched.
fn downloads(events: &[DownloadEvent], path: &str) -> usize {
    let path_of = |url: &str| {
        url::Url::parse(url)
            .map(|url| url.path().trim_start_matches('/').to_string())
            .unwrap_or_default()
    };
    events
        .iter()
        .filter(|event| match event {
            DownloadEvent::ResourceFinished { resource, .. } => match resource {
                mesio_engine::ResourceId::DashSegment { key, .. } => path_of(&key.uri) == path,
                mesio_engine::ResourceId::DashManifest { url } => path_of(url) == path,
                _ => false,
            },
            _ => false,
        })
        .count()
}

async fn drain(
    session: DownloadSession<HlsData>,
) -> (
    Vec<HlsData>,
    Option<DownloadError>,
    Vec<DownloadEvent>,
    DownloadTerminal,
) {
    let DownloadSession {
        mut items,
        events,
        handle,
    } = session;
    let log = EventLog::collect(events);
    let mut out = Vec::new();
    let error = read_until(&mut items, &mut out, |_| false).await;
    drop(items);
    let terminal = tokio::time::timeout(TEST_TIMEOUT, handle.join())
        .await
        .expect("lifecycle finishes")
        .expect("lifecycle handle")
        .expect("lifecycle joins");
    assert_eq!(handle.dropped_events(), 0, "event counts are complete");
    (out, error, log.finish().await, terminal)
}

fn describe(items: &[HlsData]) -> String {
    items
        .iter()
        .map(|item| match item {
            HlsData::M4sData(M4sData::InitSegment(_)) => "init".to_string(),
            HlsData::M4sData(M4sData::Segment(segment)) => {
                format!("media({})", fragments(&segment.data).len())
            }
            HlsData::EndMarker(reason) => format!("end({reason:?})"),
            HlsData::TsData(_) => "ts".to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn inits(items: &[HlsData]) -> Vec<&Bytes> {
    items
        .iter()
        .filter_map(|item| match item {
            HlsData::M4sData(M4sData::InitSegment(init)) => Some(&init.data),
            _ => None,
        })
        .collect()
}

fn media(items: &[HlsData]) -> Vec<&hls::M4sSegmentData> {
    items
        .iter()
        .filter_map(|item| match item {
            HlsData::M4sData(M4sData::Segment(segment)) => Some(segment),
            _ => None,
        })
        .collect()
}

fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= data.len() {
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        assert!(size >= 8 && at + size <= data.len(), "box size in bounds");
        let fourcc: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
        out.push((fourcc, &data[at + 8..at + size]));
        at += size;
    }
    out
}

/// `(mfhd sequence, track ID, tfdt)` of every fragment in `data`.
fn fragments(data: &[u8]) -> Vec<(u32, u32, u64)> {
    boxes(data)
        .into_iter()
        .filter(|(fourcc, _)| fourcc == b"moof")
        .map(|(_, moof)| {
            let children = boxes(moof);
            let mfhd = children.iter().find(|(f, _)| f == b"mfhd").unwrap().1;
            let traf = children.iter().find(|(f, _)| f == b"traf").unwrap().1;
            let traf_children = boxes(traf);
            let tfhd = traf_children.iter().find(|(f, _)| f == b"tfhd").unwrap().1;
            let tfdt = traf_children.iter().find(|(f, _)| f == b"tfdt").unwrap().1;
            let decode_time = match tfdt[0] {
                1 => u64::from_be_bytes(tfdt[4..12].try_into().unwrap()),
                _ => u64::from(u32::from_be_bytes(tfdt[4..8].try_into().unwrap())),
            };
            (
                u32::from_be_bytes(mfhd[4..8].try_into().unwrap()),
                u32::from_be_bytes(tfhd[4..8].try_into().unwrap()),
                decode_time,
            )
        })
        .collect()
}

/// Waits until the origin served `path`.
async fn wait_for_hit(origin: &Origin, path: &str) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        while origin.hits(path) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{path} was never requested"));
}

fn count_track(items: &[HlsData], track: u32) -> usize {
    media(items)
        .iter()
        .flat_map(|segment| fragments(&segment.data))
        .filter(|(_, id, _)| *id == track)
        .count()
}

fn ends_with_end_of_stream(items: &[HlsData]) -> bool {
    matches!(
        items.last(),
        Some(HlsData::EndMarker(Some(SplitReason::EndOfStream)))
    )
}

/// The output is one valid two-track fragmented MP4 stream: the init
/// declares H.264 + AAC, every item's fragments name a declared track, and
/// `mfhd` sequence numbers increase strictly across the whole stream.
fn assert_two_track_stream(items: &[HlsData]) {
    let init = inits(items)[0];
    let info = mp4::isobmff::parse_init_segment(init);
    assert!(info.has_h264 && info.has_aac, "{info:?}");
    let check = IndependentFragmentCheck::from_init(init).expect("parsable two-track init");
    let first = media(items)[0];
    assert!(
        check.is_independent(&first.data),
        "the first item starts both tracks"
    );
    let sequences: Vec<u32> = media(items)
        .iter()
        .flat_map(|segment| fragments(&segment.data))
        .map(|(sequence, track, _)| {
            assert!(track == 1 || track == 2, "track {track}");
            sequence
        })
        .collect();
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "{sequences:?}"
    );
}

// --- Scenarios ---

#[tokio::test(flavor = "multi_thread")]
async fn static_presentation_merges_separate_tracks_and_ends() {
    let origin = Origin::default();
    origin.set_mpd(static_timeline_mpd());
    add_timeline_files(&origin);
    let base = origin.clone().serve().await;

    let session = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .expect("session starts");
    let (items, error, events, terminal) = drain(session).await;

    assert!(error.is_none(), "{error:?}");
    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    assert!(ends_with_end_of_stream(&items), "{}", describe(&items));
    assert_eq!(inits(&items).len(), 1);
    assert_two_track_stream(&items);
    assert_eq!(count_track(&items, 1), 2, "every video fragment");
    assert_eq!(count_track(&items, 2), 3, "every audio fragment");
    // The audio track outlasts the video: its tail is the last item.
    let last = *media(&items).last().unwrap();
    assert!(
        fragments(&last.data)
            .iter()
            .all(|(_, track, _)| *track == 2)
    );
    // Static from the start: one MPD fetch, each segment once.
    assert_eq!(origin.hits("manifest.mpd"), 1);
    assert_eq!(downloads(&events, "a/t96256.m4s"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn live_number_template_follows_the_clock_and_drains_when_static() {
    let origin = Origin::default();
    origin.add_av();
    origin.set_mpd(number_mpd(true, 0, "v"));
    let base = origin.clone().serve().await;
    let clock = live_clock(10);
    let mut config = config(Some(Arc::clone(&clock)));
    config.live_start = LiveStartPolicy::PresentationDelay(Some(Duration::from_secs(3)));

    let session = start(&format!("{base}/manifest.mpd?auth=tok"), config)
        .await
        .expect("session starts");
    let DownloadSession {
        mut items,
        events,
        handle,
    } = session;
    let log = EventLog::collect(events);
    let mut out = Vec::new();
    // Segments 8, 9, 10 cover 7–10 s: three behind the 10 s edge. The fixture
    // audio stops short of each second, so the newest video segment waits
    // for the next audio segment to know its audio is complete.
    let error = read_until(&mut items, &mut out, |out| media(out).len() >= 2).await;
    assert!(error.is_none(), "{error:?}");

    clock.advance(Duration::from_secs(2));
    let error = read_until(&mut items, &mut out, |out| media(out).len() >= 4).await;
    assert!(error.is_none(), "{error:?}");
    origin.set_mpd(number_mpd(false, 12, "v"));
    let error = read_until(&mut items, &mut out, |_| false).await;
    assert!(error.is_none(), "{error:?}");
    let terminal = tokio::time::timeout(TEST_TIMEOUT, handle.join())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    assert!(ends_with_end_of_stream(&out), "{}", describe(&out));
    assert_two_track_stream(&out);
    let segments = media(&out);
    assert_eq!(segments.len(), 5, "{}", describe(&out));
    for (index, segment) in segments.iter().enumerate() {
        let number = 8 + index as u64;
        let video: Vec<u64> = fragments(&segment.data)
            .iter()
            .filter(|(_, track, _)| *track == 1)
            .map(|(_, _, time)| *time)
            .collect();
        assert_eq!(video, [(number - 1) * VIDEO_TIMESCALE], "segment {number}");
        assert_eq!(
            fragments(&segment.data).len(),
            2,
            "one audio fragment per second"
        );
        assert!((segment.segment.duration - 1.0).abs() < 1e-3);
        let expected = ast() + chrono::TimeDelta::seconds(number as i64 - 1);
        assert_eq!(
            segment
                .segment
                .program_date_time
                .map(|time| time.with_timezone(&Utc)),
            Some(expected)
        );
        assert!(!segment.segment.uri.contains("tok"), "URI is redacted");
    }
    let events = log.finish().await;
    for number in 8..=12 {
        assert_eq!(downloads(&events, &format!("v/{number}.m4s")), 1);
    }
    assert_eq!(origin.hits("v/7.m4s"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn duration_template_advances_with_the_clock_without_mpd_updates() {
    let origin = Origin::default();
    origin.add_av();
    // No minimumUpdatePeriod: the MPD never changes and is fetched once.
    origin.set_mpd(number_mpd(true, 0, "v").replace(r#" minimumUpdatePeriod="PT0.1S""#, ""));
    let base = origin.clone().serve().await;
    let clock = live_clock(10);
    let mut config = config(Some(Arc::clone(&clock)));
    config.live_start = LiveStartPolicy::LiveEdge;

    let session = start(&format!("{base}/manifest.mpd"), config)
        .await
        .expect("session starts");
    let DownloadSession {
        items,
        events,
        handle,
    } = session;
    let log = EventLog::collect(events);
    wait_for_hit(&origin, "v/10.m4s").await;
    clock.advance(Duration::from_secs(2));
    // Segments 11 and 12 are planned together and may be requested in
    // either order; wait until both have been downloaded.
    tokio::time::timeout(TEST_TIMEOUT, async {
        while ["v/11.m4s", "v/12.m4s", "a/11.m4s", "a/12.m4s"]
            .iter()
            .any(|path| downloads(&log.snapshot(), path) == 0)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the clock alone makes segments 11 and 12 available");
    drop(items);
    tokio::time::timeout(TEST_TIMEOUT, handle.join())
        .await
        .unwrap();
    let events = log.finish().await;
    assert_eq!(origin.hits("manifest.mpd"), 1, "never refreshed");
    assert_eq!(downloads(&events, "manifest.mpd"), 1);
    assert_eq!(downloads(&events, "v/11.m4s"), 1, "planned once");
    assert_eq!(downloads(&events, "v/12.m4s"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn timeline_generations_are_planned_once_across_mpd_updates() {
    let origin = Origin::default();
    origin.add_file("v/init.mp4", VIDEO_INIT);
    let mpd = |kind: &str, first: u64, count: u64| {
        let repeats = count - 1;
        let t = first * VIDEO_TIMESCALE;
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" {kind} availabilityStartTime="2024-01-01T00:00:00Z">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/t$Time$.m4s" initialization="v/init.mp4" timescale="12288">
        <SegmentTimeline><S t="{t}" d="12288" r="{repeats}"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v" bandwidth="40000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
        )
    };
    for index in 0..16u64 {
        origin.add_file(
            &format!("v/t{}.m4s", index * VIDEO_TIMESCALE),
            with_tfdt(VIDEO_1, index * VIDEO_TIMESCALE),
        );
    }
    let dynamic = r#"type="dynamic" minimumUpdatePeriod="PT0.1S""#;
    origin.set_mpd(mpd(dynamic, 0, 10));
    let base = origin.clone().serve().await;
    let clock = live_clock(10);
    let mut config = config(Some(Arc::clone(&clock)));
    config.live_start = LiveStartPolicy::LiveEdge;
    config.selection.include_audio = false;

    let session = start(&format!("{base}/manifest.mpd"), config)
        .await
        .expect("session starts");
    let DownloadSession {
        mut items,
        events,
        handle,
    } = session;
    let log = EventLog::collect(events);
    let mut out = Vec::new();
    assert!(
        read_until(&mut items, &mut out, |out| !media(out).is_empty())
            .await
            .is_none()
    );

    // The window slides: the oldest entries expire, three new ones appear.
    clock.advance(Duration::from_secs(3));
    origin.set_mpd(mpd(dynamic, 3, 10));
    assert!(
        read_until(&mut items, &mut out, |out| media(out).len() >= 4)
            .await
            .is_none()
    );
    origin.set_mpd(mpd(
        r#"type="static" mediaPresentationDuration="PT13S""#,
        0,
        13,
    ));
    assert!(read_until(&mut items, &mut out, |_| false).await.is_none());
    let terminal = tokio::time::timeout(TEST_TIMEOUT, handle.join())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    let times: Vec<u64> = media(&out)
        .iter()
        .flat_map(|segment| fragments(&segment.data))
        .map(|(_, _, time)| time / VIDEO_TIMESCALE)
        .collect();
    assert_eq!(times, [9, 10, 11, 12], "{}", describe(&out));
    let events = log.finish().await;
    for index in 9..13 {
        assert_eq!(
            downloads(&events, &format!("v/t{}.m4s", index * VIDEO_TIMESCALE)),
            1
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_only_selection_passes_one_normalized_track_through() {
    let origin = Origin::default();
    origin.set_mpd(static_timeline_mpd());
    add_timeline_files(&origin);
    let base = origin.clone().serve().await;
    let mut config = config(None);
    config.selection.include_video = false;

    let session = start(&format!("{base}/manifest.mpd"), config)
        .await
        .expect("session starts");
    let (items, error, _, terminal) = drain(session).await;

    assert!(error.is_none(), "{error:?}");
    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    let info = mp4::isobmff::parse_init_segment(inits(&items)[0]);
    assert!(info.has_aac && !info.has_h264);
    let segments = media(&items);
    assert_eq!(segments.len(), 3);
    for segment in segments {
        assert_eq!(&segment.data[4..8], b"moof", "styp/sidx are dropped");
        assert!(
            fragments(&segment.data)
                .iter()
                .all(|(_, track, _)| *track == 1)
        );
    }
    assert_eq!(origin.hits("v/t0.m4s"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_segments_are_retried() {
    let origin = Origin::default();
    origin.set_mpd(static_timeline_mpd());
    add_timeline_files(&origin);
    origin.add_file_failing("v/t12288.m4s", VIDEO_2, 404, 2);
    let base = origin.clone().serve().await;

    let session = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .expect("session starts");
    let (items, error, events, _) = drain(session).await;

    assert!(error.is_none(), "{error:?}");
    assert_eq!(count_track(&items, 1), 2, "the retried segment is recorded");
    // Two 404s, then the download; a transport-level retry may add hits.
    assert!(origin.hits("v/t12288.m4s") >= 3);
    assert_eq!(downloads(&events, "v/t12288.m4s"), 1);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DownloadEvent::RetryScheduled { .. }))
    );
}

fn failed_url(error: &Option<DownloadError>) -> Option<&str> {
    match error {
        Some(DownloadError::HttpStatus { url, .. }) => Some(url),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_video_init_that_cannot_be_fetched_fails_with_its_status() {
    let origin = Origin::default();
    origin.set_mpd(static_timeline_mpd());
    add_timeline_files(&origin);
    origin.add_file_failing("v/init.mp4", VIDEO_INIT, 404, u32::MAX);
    let base = origin.clone().serve().await;

    let session = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .expect("session starts");
    let (items, error, _, _) = drain(session).await;

    assert!(media(&items).is_empty(), "{}", describe(&items));
    assert!(
        failed_url(&error).is_some_and(|url| url.ends_with("/v/init.mp4")),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_audio_init_that_cannot_be_fetched_drops_only_the_audio() {
    let origin = Origin::default();
    origin.set_mpd(static_timeline_mpd());
    add_timeline_files(&origin);
    origin.add_file_failing("a/init.mp4", AUDIO_INIT, 404, u32::MAX);
    let base = origin.clone().serve().await;

    let session = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .expect("session starts");
    let (items, error, events, _) = drain(session).await;

    assert!(error.is_none(), "{error:?}");
    assert_eq!(count_track(&items, 1), 2, "every video fragment");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DownloadEvent::TrackDropped { .. })),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_stream_whose_init_cannot_be_fetched_fails_without_waiting() {
    let origin = Origin::default();
    origin.add_av();
    origin.add_file_failing("v/init.mp4", VIDEO_INIT, 404, u32::MAX);
    origin.set_mpd(number_mpd(true, 0, "v"));
    let base = origin.clone().serve().await;

    let session = start(
        &format!("{base}/manifest.mpd"),
        config(Some(live_clock(10))),
    )
    .await
    .expect("session starts");
    // The stall watchdog is 10 s; the failure must come from the init.
    let (items, error, _, _) = drain(session).await;

    assert!(media(&items).is_empty(), "{}", describe(&items));
    assert!(
        failed_url(&error).is_some_and(|url| url.ends_with("/v/init.mp4")),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mpd_refresh_failures_end_the_stream_after_the_retry_budget() {
    let origin = Origin::default();
    origin.add_av();
    origin.set_mpd(number_mpd(true, 0, "v"));
    origin.fail_mpd_after(1);
    let base = origin.clone().serve().await;

    let session = start(
        &format!("{base}/manifest.mpd"),
        config(Some(live_clock(10))),
    )
    .await
    .expect("session starts");
    let (_, error, _, terminal) = drain(session).await;

    assert!(
        matches!(error, Some(DownloadError::HttpStatus { status, .. }) if status == 500),
        "{error:?}"
    );
    assert!(matches!(terminal, DownloadTerminal::PipelineError(_)));
    // The initial fetch plus at most one refresh per allowed retry and the
    // last one (a refresh that fails before reaching the origin counts as
    // a failure without a hit).
    let fetches = origin.hits("manifest.mpd");
    assert!((2..=1 + 3).contains(&fetches), "{fetches} MPD fetches");
}

#[tokio::test(flavor = "multi_thread")]
async fn period_change_starts_a_new_file_epoch_without_cross_epoch_merging() {
    let origin = Origin::default();
    origin.add_file("v/init.mp4", VIDEO_INIT);
    origin.add_file("a/init.mp4", AUDIO_INIT);
    for period in ["p0", "p1"] {
        origin.add_numbered(&format!("{period}/v"), VIDEO_1, VIDEO_TIMESCALE);
        origin.add_numbered(&format!("{period}/a"), AUDIO_1, AUDIO_TIMESCALE);
    }
    let period = |id: &str, start: u64| {
        format!(
            r#"<Period id="{id}" start="PT{start}S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="{id}/v/$Number$.m4s" initialization="v/init.mp4" duration="1" timescale="1"/>
      <Representation id="v" bandwidth="40000"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="{id}/a/$Number$.m4s" initialization="a/init.mp4" duration="1" timescale="1"/>
      <Representation id="a" bandwidth="24000"/>
    </AdaptationSet>
  </Period>"#
        )
    };
    origin.set_mpd(format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT4S">
  {}
  {}
</MPD>"#,
        period("p0", 0),
        period("p1", 2)
    ));
    let base = origin.clone().serve().await;

    let session = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .expect("session starts");
    let (items, error, _, terminal) = drain(session).await;

    assert!(error.is_none(), "{error:?}");
    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    let barrier = items
        .iter()
        .position(|item| {
            matches!(
                item,
                HlsData::EndMarker(Some(SplitReason::StreamStructureChange { .. }))
            )
        })
        .unwrap_or_else(|| panic!("no epoch barrier: {}", describe(&items)));
    assert!(matches!(
        items.get(barrier + 1),
        Some(HlsData::M4sData(M4sData::InitSegment(_)))
    ));
    assert_eq!(inits(&items).len(), 2);
    // Every item before the barrier belongs to the first Period, after it
    // to the second (shifted by its 2 s start).
    for (index, item) in items.iter().enumerate() {
        if let HlsData::M4sData(M4sData::Segment(segment)) = item {
            for (_, track, time) in fragments(&segment.data) {
                let timescale = if track == 1 {
                    VIDEO_TIMESCALE
                } else {
                    AUDIO_TIMESCALE
                };
                let in_second = time >= 2 * timescale;
                assert_eq!(
                    in_second,
                    index > barrier,
                    "fragment at {time} in item {index}"
                );
            }
        }
    }
    assert_two_track_stream(&items[barrier + 1..]);
}

/// A/V `$Number$` presentation whose video path names the Representation;
/// `early` makes video segments available a second ahead of the clock.
fn switch_mpd(dynamic: bool, video_id: &str, early: bool) -> String {
    let presentation = if dynamic {
        r#"type="dynamic" minimumUpdatePeriod="PT0.1S""#
    } else {
        r#"type="static" mediaPresentationDuration="PT11S""#
    };
    let offset = if early {
        r#" availabilityTimeOffset="1""#
    } else {
        ""
    };
    format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" {presentation} availabilityStartTime="2024-01-01T00:00:00Z">
  <Period id="p0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="$RepresentationID$/$Number$.m4s" initialization="v/init.mp4" duration="1" timescale="1"{offset}/>
      <Representation id="{video_id}" bandwidth="40000"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="1" timescale="1"/>
      <Representation id="a" bandwidth="24000"/>
    </AdaptationSet>
  </Period>
</MPD>"#
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn representation_switch_is_an_epoch_change() {
    let origin = Origin::default();
    origin.add_av();
    origin.add_numbered("v-hd", VIDEO_1, VIDEO_TIMESCALE);
    origin.add_numbered("v-sd", VIDEO_1, VIDEO_TIMESCALE);
    origin.set_mpd(switch_mpd(true, "v-hd", false));
    let base = origin.clone().serve().await;
    let clock = live_clock(10);
    let mut config = config(Some(Arc::clone(&clock)));
    config.live_start = LiveStartPolicy::LiveEdge;

    let session = start(&format!("{base}/manifest.mpd"), config)
        .await
        .expect("session starts");
    let DownloadSession {
        mut items, handle, ..
    } = session;
    let mut out = Vec::new();
    wait_for_hit(&origin, "v-hd/10.m4s").await;
    wait_for_hit(&origin, "a/10.m4s").await;
    // The replacement offers segment 11 a second early. The clock stays at
    // 10 s, so only a planner that consumed the replacement can request it:
    // the request proves the switch was planned, with no clock race.
    origin.set_mpd(switch_mpd(true, "v-sd", true));
    wait_for_hit(&origin, "v-sd/11.m4s").await;
    // Audio segment 11 completes the audio of video segment 10.
    clock.advance(Duration::from_secs(1));
    assert!(
        read_until(&mut items, &mut out, |out| !media(out).is_empty())
            .await
            .is_none()
    );
    // At 11 s the early offer covers segment 12 too; the final MPD ends at
    // 11 s, so it is only recorded if planned before that MPD is published.
    wait_for_hit(&origin, "v-sd/12.m4s").await;
    origin.set_mpd(switch_mpd(false, "v-sd", false));
    assert!(read_until(&mut items, &mut out, |_| false).await.is_none());
    tokio::time::timeout(TEST_TIMEOUT, handle.join())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(
        out.iter().any(|item| matches!(
            item,
            HlsData::EndMarker(Some(SplitReason::StreamStructureChange { description }))
                if description.contains("v-hd") && description.contains("v-sd")
        )),
        "{}",
        describe(&out)
    );
    assert_eq!(inits(&out).len(), 2);
    let video: Vec<u64> = media(&out)
        .iter()
        .flat_map(|segment| fragments(&segment.data))
        .filter(|(_, track, _)| *track == 1)
        .map(|(_, _, time)| time / VIDEO_TIMESCALE)
        .collect();
    // Segment 10 under v-hd; 11 and 12 (offered a second early) under v-sd.
    assert_eq!(video, [9, 10, 11]);
    assert_eq!(origin.hits("v-hd/11.m4s"), 0);
    assert_eq!(origin.hits("v-sd/10.m4s"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn stalled_audio_does_not_hold_back_video() {
    let origin = Origin::default();
    origin.set_mpd(static_timeline_mpd());
    add_timeline_files(&origin);
    // Audio media is held until the video has been written.
    let audio = origin.gate("a/t");
    let base = origin.clone().serve().await;
    let mut config = config(None);
    config.mux.max_audio_lag = Duration::from_millis(200);

    let session = start(&format!("{base}/manifest.mpd"), config)
        .await
        .expect("session starts");
    let DownloadSession {
        mut items,
        mut events,
        handle,
    } = session;
    let mut out = Vec::new();
    assert!(
        read_until(&mut items, &mut out, |out| count_track(out, 1) >= 2)
            .await
            .is_none(),
        "video is written while no audio can arrive"
    );
    assert_eq!(count_track(&out, 2), 0);
    audio.send_replace(true);
    assert!(read_until(&mut items, &mut out, |_| false).await.is_none());
    let terminal = tokio::time::timeout(TEST_TIMEOUT, handle.join())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    assert!(ends_with_end_of_stream(&out));
    let mut stalled = false;
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(100), events.next()).await
    {
        stalled |= matches!(
            event,
            DownloadEvent::TrackStalled {
                track: MediaTrack::Audio,
                ..
            }
        );
    }
    assert!(stalled, "a stall event was emitted");
}

#[tokio::test(flavor = "multi_thread")]
async fn protected_and_webm_presentations_are_rejected_with_typed_errors() {
    let origin = Origin::default();
    let base = origin.clone().serve().await;
    let url = format!("{base}/manifest.mpd");
    for body in [
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2S">
  <Period><AdaptationSet mimeType="video/mp4">
    <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
    <SegmentTemplate media="$Number$.m4s" initialization="init.mp4" duration="1" timescale="1"/>
    <Representation id="v" bandwidth="1"/>
  </AdaptationSet></Period>
</MPD>"#,
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2S">
  <Period><AdaptationSet mimeType="video/webm">
    <SegmentTemplate media="$Number$.webm" initialization="init.webm" duration="1" timescale="1"/>
    <Representation id="v" bandwidth="1"/>
  </AdaptationSet></Period>
</MPD>"#,
    ] {
        origin.set_mpd(body);
        let error = start(&url, config(None))
            .await
            .err()
            .expect("unsupported presentation fails to start");
        assert!(
            matches!(
                error,
                DownloadError::Unsupported {
                    protocol: "dash",
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.is_non_recoverable_source_error());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_stream_cancels_the_session() {
    let origin = Origin::default();
    origin.add_av();
    origin.set_mpd(number_mpd(true, 0, "v"));
    let base = origin.clone().serve().await;

    let session = start(
        &format!("{base}/manifest.mpd"),
        config(Some(live_clock(10))),
    )
    .await
    .expect("session starts");
    let DownloadSession {
        mut items, handle, ..
    } = session;
    let mut out = Vec::new();
    assert!(
        read_until(&mut items, &mut out, |out| !media(out).is_empty())
            .await
            .is_none()
    );
    drop(items);

    let terminal = tokio::time::timeout(TEST_TIMEOUT, handle.join())
        .await
        .expect("cancellation finishes the session")
        .unwrap()
        .unwrap();
    assert_eq!(terminal, DownloadTerminal::Cancelled);
    let fetches = origin.hits("manifest.mpd");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        origin.hits("manifest.mpd"),
        fetches,
        "nothing polls after cancel"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sources_fail_over_to_the_next_mpd() {
    let broken = Origin::default();
    broken.mpd_status(404);
    let broken_base = broken.clone().serve().await;

    let origin = Origin::default();
    origin.set_mpd(static_timeline_mpd());
    add_timeline_files(&origin);
    let base = origin.clone().serve().await;

    let downloader = MesioDownloader::new(MesioConfig {
        dash: config(None),
        ..Default::default()
    });
    let request = DownloadRequest::from_url(&format!("{broken_base}/manifest.mpd"))
        .expect("valid URL")
        .add_source(ContentSource::new(format!("{broken_base}/manifest.mpd"), 0))
        .add_source(ContentSource::new(format!("{base}/manifest.mpd"), 1));
    let session = downloader.start(request).await.expect("session starts");
    assert!(matches!(session, DownloaderSession::Dash(_)));
    let session = session.into_dash().unwrap();
    let (items, error, events, terminal) = drain(session).await;

    assert!(error.is_none(), "{error:?}");
    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    assert_eq!(count_track(&items, 1), 2);
    let selected: Vec<(String, u32)> = events
        .iter()
        .filter_map(|event| match event {
            DownloadEvent::SourceSelected { url, attempt, .. } => Some((url.to_string(), *attempt)),
            _ => None,
        })
        .collect();
    assert_eq!(
        selected,
        [
            (format!("{broken_base}/manifest.mpd"), 1),
            (format!("{base}/manifest.mpd"), 2),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn request_options_override_the_track_selection() {
    let origin = Origin::default();
    origin.set_mpd(static_timeline_mpd());
    add_timeline_files(&origin);
    let base = origin.clone().serve().await;

    let downloader = DashDownloader::new(config(None)).unwrap();
    let mut selection = config(None).selection;
    selection.include_audio = false;
    let request = DownloadRequest::from_url(&format!("{base}/manifest.mpd"))
        .unwrap()
        .with_protocol(ProtocolSelection::Dash(DashRequestOptions {
            selection: Some(selection),
        }));
    let session = downloader.start(request).await.expect("session starts");
    let (items, error, _, _) = drain(session).await;

    assert!(error.is_none(), "{error:?}");
    assert_eq!(count_track(&items, 2), 0);
    let requested: HashSet<&str> = ["a/init.mp4", "a/t0.m4s"].into();
    for path in requested {
        assert_eq!(origin.hits(path), 0, "{path}");
    }
}

/// A static A/V presentation of `count` one-second `$Number$` segments.
fn long_static(origin: &Origin, count: u64) -> String {
    origin.add_av();
    number_mpd(false, count, "v")
}

/// Media segments the origin served under `prefix/`.
fn segment_hits(origin: &Origin, prefix: &str) -> u64 {
    origin
        .0
        .lock()
        .unwrap()
        .hits
        .iter()
        .filter(|(path, _)| path.starts_with(prefix) && path.ends_with(".m4s"))
        .map(|(_, hits)| *hits)
        .sum()
}

/// Waits until downloads stop although segments remain: with nobody
/// reading the items, every buffer between the origin and the consumer is
/// full and the engine's last delivery is blocked.
async fn wait_until_backpressured(origin: &Origin, total: u64) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let mut last = u64::MAX;
        loop {
            let served = segment_hits(origin, "v/");
            if served == last && served > 0 {
                break;
            }
            last = served;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .expect("downloads stall behind the unread consumer");
    let served = segment_hits(origin, "v/");
    assert!(
        served < total,
        "the presentation must outlast the buffers ({served} of {total} served)"
    );
}

/// One track, minimal buffers: what is downloaded before backpressure stops
/// the pipeline stays far below the presentation length.
fn backpressure_config() -> DashConfig {
    let mut config = config(None);
    config.selection.include_audio = false;
    config.scheduler.download_concurrency = 1;
    config.scheduler.processed_segment_buffer_multiplier = 1;
    config.engine.max_pending_items = 1;
    config
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_session_whose_consumer_stopped_reading_finishes() {
    let origin = Origin::default();
    let mpd = long_static(&origin, 200);
    origin.set_mpd(mpd);
    let base = origin.clone().serve().await;

    let session = start(&format!("{base}/manifest.mpd"), backpressure_config())
        .await
        .expect("session starts");
    // Hold the item stream without reading it until output is saturated.
    let DownloadSession {
        items: _held,
        handle,
        ..
    } = session;
    wait_until_backpressured(&origin, 200).await;
    handle.cancel();
    let terminal = tokio::time::timeout(TEST_TIMEOUT, handle.join())
        .await
        .expect("a saturated session still finishes on cancel")
        .unwrap()
        .unwrap();
    assert_eq!(terminal, DownloadTerminal::Cancelled);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_failover_session_whose_consumer_stopped_reading_finishes() {
    let origin = Origin::default();
    let mpd = long_static(&origin, 200);
    origin.set_mpd(mpd);
    let base = origin.clone().serve().await;

    let downloader = MesioDownloader::new(MesioConfig {
        dash: backpressure_config(),
        ..Default::default()
    });
    let url = format!("{base}/manifest.mpd");
    let request = DownloadRequest::from_url(&url)
        .unwrap()
        .add_source(ContentSource::new(url.clone(), 0));
    let session = downloader
        .start_dash(request)
        .await
        .expect("session starts");
    let DownloadSession {
        items: _held,
        handle,
        ..
    } = session;
    wait_until_backpressured(&origin, 200).await;
    handle.cancel();
    let terminal = tokio::time::timeout(TEST_TIMEOUT, handle.join())
        .await
        .expect("the failover task finishes on cancel")
        .unwrap()
        .unwrap();
    assert_eq!(terminal, DownloadTerminal::Cancelled);
}

#[tokio::test(flavor = "multi_thread")]
async fn many_short_periods_keep_every_init_until_its_media_is_written() {
    let origin = Origin::default();
    origin.add_file("v/init.mp4", VIDEO_INIT);
    origin.add_file("a/init.mp4", AUDIO_INIT);
    let periods: String = (0..10)
        .map(|index| {
            origin.add_numbered(&format!("p{index}/v"), VIDEO_1, VIDEO_TIMESCALE);
            origin.add_numbered(&format!("p{index}/a"), AUDIO_1, AUDIO_TIMESCALE);
            format!(
                r#"<Period id="p{index}" duration="PT1S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="p{index}/v/$Number$.m4s" initialization="v/init.mp4" duration="1" timescale="1"/>
      <Representation id="v" bandwidth="40000"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="p{index}/a/$Number$.m4s" initialization="a/init.mp4" duration="1" timescale="1"/>
      <Representation id="a" bandwidth="24000"/>
    </AdaptationSet>
  </Period>"#
            )
        })
        .collect();
    origin.set_mpd(format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">{periods}</MPD>"#
    ));
    let base = origin.clone().serve().await;

    let session = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .expect("session starts");
    let (items, error, _, terminal) = drain(session).await;

    assert!(error.is_none(), "{error:?}");
    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    assert_eq!(inits(&items).len(), 10, "{}", describe(&items));
    assert_eq!(count_track(&items, 1), 10, "no media lost its init");
    assert_eq!(count_track(&items, 2), 10);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_later_protected_period_ends_the_recording_with_a_typed_error() {
    let origin = Origin::default();
    origin.add_av();
    origin.set_mpd(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="clear" duration="PT2S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="1" timescale="1"/>
      <Representation id="v" bandwidth="40000"/>
    </AdaptationSet>
  </Period>
  <Period id="drm" duration="PT2S">
    <AdaptationSet mimeType="video/mp4">
      <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
      <SegmentTemplate media="d/$Number$.m4s" initialization="d/init.mp4" duration="1" timescale="1"/>
      <Representation id="v" bandwidth="40000"/>
    </AdaptationSet>
  </Period>
</MPD>"#,
    );
    let base = origin.clone().serve().await;

    let session = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .expect("session starts");
    let (items, error, _, terminal) = drain(session).await;

    assert_eq!(count_track(&items, 1), 2, "the clear Period is recorded");
    assert!(
        matches!(
            error,
            Some(DownloadError::Unsupported {
                protocol: "dash",
                ..
            })
        ),
        "{error:?}"
    );
    assert!(matches!(terminal, DownloadTerminal::PipelineError(_)));
    assert_eq!(origin.hits("d/init.mp4"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_output_splits_through_the_hls_pipeline_at_independent_items() {
    use hls_fix::{HlsPipeline, HlsPipelineConfig};
    use pipeline_common::config::PipelineConfig;
    use pipeline_common::{ManualSplitControl, PipelineError, PipelineProvider, StreamerContext};

    let origin = Origin::default();
    let mpd = long_static(&origin, 6);
    origin.set_mpd(mpd);
    let base = origin.clone().serve().await;
    let session = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .expect("session starts");
    let (items, error, _, _) = drain(session).await;
    assert!(error.is_none(), "{error:?}");

    let control = Arc::new(ManualSplitControl::default());
    control.enable();
    control
        .request(Duration::from_secs(30))
        .expect("manual cut requested");
    let context = Arc::new(
        StreamerContext::new(tokio_util::sync::CancellationToken::new())
            .with_manual_split(Arc::clone(&control)),
    );
    let common = PipelineConfig {
        max_duration: Some(Duration::from_millis(2500)),
        ..PipelineConfig::default()
    };
    let pipeline =
        HlsPipeline::with_config(context, &common, HlsPipelineConfig::default()).build_pipeline();
    let mut output = Vec::new();
    pipeline
        .run(
            items.into_iter().map(Ok::<_, PipelineError>),
            &mut |item: Result<HlsData, PipelineError>| output.push(item.expect("processed")),
        )
        .expect("pipeline runs");

    let mut files: Vec<Vec<&HlsData>> = vec![Vec::new()];
    let mut reasons = Vec::new();
    for item in &output {
        match item {
            HlsData::EndMarker(reason) => {
                reasons.push(reason.clone());
                files.push(Vec::new());
            }
            item => files.last_mut().unwrap().push(item),
        }
    }
    assert!(
        reasons
            .iter()
            .any(|reason| matches!(reason, Some(SplitReason::Manual { .. }))),
        "{reasons:?}"
    );
    assert!(
        reasons
            .iter()
            .any(|reason| matches!(reason, Some(SplitReason::DurationLimit))),
        "{reasons:?}"
    );
    let files: Vec<_> = files.into_iter().filter(|file| !file.is_empty()).collect();
    assert!(files.len() >= 3, "{} files", files.len());
    for file in files {
        // Every file starts with the two-track init, then an item that
        // starts both tracks independently.
        let Some(HlsData::M4sData(M4sData::InitSegment(init))) = file.first() else {
            panic!("a file without its init");
        };
        let check = IndependentFragmentCheck::from_init(&init.data).expect("two-track init");
        let Some(HlsData::M4sData(M4sData::Segment(first))) = file.get(1) else {
            panic!("a file without media");
        };
        assert!(check.is_independent(&first.data));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn presentations_longer_than_the_planning_window_are_recorded_whole() {
    let origin = Origin::default();
    let mpd = long_static(&origin, 300);
    origin.set_mpd(mpd);
    let base = origin.clone().serve().await;
    let mut config = config(None);
    config.selection.include_audio = false;

    let session = start(&format!("{base}/manifest.mpd"), config)
        .await
        .expect("session starts");
    let (items, error, events, terminal) = drain(session).await;

    assert!(error.is_none(), "{error:?}");
    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    assert_eq!(count_track(&items, 1), 300);
    for number in [1, 256, 257, 300] {
        assert_eq!(
            downloads(&events, &format!("v/{number}.m4s")),
            1,
            "segment {number}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_static_segment_outlasting_many_later_ones_is_still_recorded() {
    let origin = Origin::default();
    let mpd = long_static(&origin, 300);
    origin.set_mpd(mpd);
    let held = origin.gate("v/10.m4s");
    let base = origin.clone().serve().await;
    let mut config = config(None);
    config.selection.include_audio = false;

    let session = start(&format!("{base}/manifest.mpd"), config)
        .await
        .expect("session starts");
    // Segment 10 is answered only once nothing else is downloading: by
    // then, far more later segments than the reorder buffer holds could
    // have completed.
    let release = {
        let origin = origin.clone();
        tokio::spawn(async move {
            wait_until_backpressured(&origin, 300).await;
            held.send_replace(true);
        })
    };
    let (items, error, _, terminal) = drain(session).await;
    release.await.expect("segment 10 is released");

    assert!(error.is_none(), "{error:?}");
    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    let seconds: Vec<u64> = media(&items)
        .iter()
        .flat_map(|segment| fragments(&segment.data))
        .map(|(_, _, time)| time / VIDEO_TIMESCALE)
        .collect();
    assert_eq!(seconds, (0..300).collect::<Vec<_>>());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_static_presentation_with_a_protected_first_period_fails_typed() {
    let origin = Origin::default();
    origin.add_av();
    origin.set_mpd(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="drm" duration="PT2S">
    <AdaptationSet mimeType="video/mp4">
      <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
      <SegmentTemplate media="d/$Number$.m4s" initialization="d/init.mp4" duration="1" timescale="1"/>
      <Representation id="v" bandwidth="40000"/>
    </AdaptationSet>
  </Period>
  <Period id="clear" duration="PT2S">
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="1" timescale="1"/>
      <Representation id="v" bandwidth="40000"/>
    </AdaptationSet>
  </Period>
</MPD>"#,
    );
    let base = origin.clone().serve().await;

    let error = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .err()
        .expect("the first Period cannot be skipped");
    assert!(
        matches!(
            error,
            DownloadError::Unsupported {
                protocol: "dash",
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(segment_hits(&origin, "v/"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_audio_only_period_between_video_periods_is_recorded_in_order() {
    let origin = Origin::default();
    origin.add_file("v/init.mp4", VIDEO_INIT);
    origin.add_file("a/init.mp4", AUDIO_INIT);
    let video = |id: &str| {
        format!(
            r#"<AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="{id}/v/$Number$.m4s" initialization="v/init.mp4" duration="1" timescale="1"/>
      <Representation id="v" bandwidth="40000"/>
    </AdaptationSet>"#
        )
    };
    let audio = |id: &str| {
        format!(
            r#"<AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="{id}/a/$Number$.m4s" initialization="a/init.mp4" duration="1" timescale="1"/>
      <Representation id="a" bandwidth="24000"/>
    </AdaptationSet>"#
        )
    };
    for id in ["p0", "p1", "p2"] {
        origin.add_numbered(&format!("{id}/v"), VIDEO_1, VIDEO_TIMESCALE);
        origin.add_numbered(&format!("{id}/a"), AUDIO_1, AUDIO_TIMESCALE);
    }
    origin.set_mpd(format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="p0" duration="PT2S">{}{}</Period>
  <Period id="p1" duration="PT2S">{}</Period>
  <Period id="p2" duration="PT2S">{}{}</Period>
</MPD>"#,
        video("p0"),
        audio("p0"),
        audio("p1"),
        video("p2"),
        audio("p2"),
    ));
    let base = origin.clone().serve().await;

    let session = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .expect("session starts");
    let (items, error, _, terminal) = drain(session).await;

    assert!(error.is_none(), "{error:?}");
    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    assert_eq!(origin.hits("p1/v/1.m4s"), 0);
    // Three epochs: A/V, audio only, A/V.
    let mut epochs: Vec<Vec<&HlsData>> = vec![Vec::new()];
    for item in &items {
        match item {
            HlsData::EndMarker(Some(SplitReason::StreamStructureChange { .. })) => {
                epochs.push(Vec::new())
            }
            HlsData::EndMarker(Some(SplitReason::EndOfStream)) => {}
            item => epochs.last_mut().unwrap().push(item),
        }
    }
    assert_eq!(epochs.len(), 3, "{}", describe(&items));
    // (track ID, decode time in seconds) of every fragment of an epoch.
    let content = |epoch: &[&HlsData]| -> (bool, Vec<(u32, f64)>) {
        let Some(HlsData::M4sData(M4sData::InitSegment(init))) = epoch.first() else {
            panic!("an epoch without its init");
        };
        let info = mp4::isobmff::parse_init_segment(&init.data);
        assert!(info.has_aac);
        let mut fragments_seen = Vec::new();
        for item in &epoch[1..] {
            let HlsData::M4sData(M4sData::Segment(segment)) = item else {
                panic!("an init inside an epoch");
            };
            for (_, track, time) in fragments(&segment.data) {
                let timescale = if info.has_h264 && track == 1 {
                    VIDEO_TIMESCALE
                } else {
                    AUDIO_TIMESCALE
                };
                fragments_seen.push((track, time as f64 / timescale as f64));
            }
        }
        (info.has_h264, fragments_seen)
    };
    let (first_video, first) = content(&epochs[0]);
    let (gap_video, gap) = content(&epochs[1]);
    let (last_video, last) = content(&epochs[2]);
    assert!(first_video && !gap_video && last_video);
    assert_eq!(
        first.len(),
        4,
        "two video and two audio fragments: {first:?}"
    );
    assert_eq!(gap, [(1, 2.0), (1, 3.0)], "p1's audio, on its own track");
    assert_eq!(last.len(), 4, "{last:?}");
    assert!(last.iter().all(|&(_, time)| time >= 4.0));
    for segment in media(&items[..]) {
        assert!(segment.segment.duration > 0.0 || fragments(&segment.data).is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_subtitles_do_not_count_against_the_media_tracks() {
    let origin = Origin::default();
    origin.add_av();
    origin.set_mpd(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="1" timescale="1"/>
      <Representation id="v" bandwidth="40000"/>
    </AdaptationSet>
    <AdaptationSet contentType="text" mimeType="text/vtt" lang="en">
      <Representation id="subtitles" bandwidth="100">
        <BaseURL>subtitles.vtt</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#,
    );
    let base = origin.clone().serve().await;

    // Default selection asks for audio too; the only rejection is text.
    let session = start(&format!("{base}/manifest.mpd"), config(None))
        .await
        .expect("video and rejected subtitles still record");
    let (items, error, _, terminal) = drain(session).await;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(terminal, DownloadTerminal::AuthoritativeEnd);
    assert_eq!(count_track(&items, 1), 2);
}
