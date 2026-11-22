use std::time::Duration;

use futures::StreamExt;
use mp4::independent::IndependentFragmentCheck;
use url::Url;

use super::*;
use crate::segment_engine::descriptor::SegmentSource;
use crate::segment_engine::identity::{SegmentKey, SegmentKind};
use crate::session::DownloadEventStream;

const VIDEO_INIT: &[u8] = include_bytes!("../../../../mp4/tests/fixtures/dash/video-init.mp4");
const VIDEO_1: &[u8] = include_bytes!("../../../../mp4/tests/fixtures/dash/video-1.m4s");
const VIDEO_2: &[u8] = include_bytes!("../../../../mp4/tests/fixtures/dash/video-2.m4s");
const AUDIO_INIT: &[u8] = include_bytes!("../../../../mp4/tests/fixtures/dash/audio-init.mp4");
const AUDIO_1: &[u8] = include_bytes!("../../../../mp4/tests/fixtures/dash/audio-1.m4s");
const AUDIO_2: &[u8] = include_bytes!("../../../../mp4/tests/fixtures/dash/audio-2.m4s");
const AUDIO_3: &[u8] = include_bytes!("../../../../mp4/tests/fixtures/dash/audio-3.m4s");
const WAIT: Duration = Duration::from_secs(5);

fn descriptor(
    track: MediaTrack,
    kind: SegmentKind,
    period: Option<&str>,
    period_start_secs: u64,
    number: u64,
) -> Arc<SegmentDescriptor> {
    descriptor_with_pto(track, kind, period, period_start_secs, number, 0)
}

/// A descriptor whose Representation has a presentation time offset
/// (in milliseconds: the test MPD timescale is 1000).
fn descriptor_with_pto(
    track: MediaTrack,
    kind: SegmentKind,
    period: Option<&str>,
    period_start_secs: u64,
    number: u64,
    pto_ms: u64,
) -> Arc<SegmentDescriptor> {
    let url = Url::parse(&format!(
        "https://cdn.example.com/{track}/{number}.m4s?token=secret"
    ))
    .unwrap();
    Arc::new(SegmentDescriptor {
        key: SegmentKey {
            kind,
            uri: Arc::from(url.as_str()),
            byte_range: None,
            scope: Some(Arc::from(format!("{track}/{period:?}"))),
        },
        msn: number,
        source: SegmentSource::Playlist,
        parsed_url: Arc::new(url),
        discontinuity: false,
        encryption: None,
        init_key: None,
        scramble_seed: None,
        meta: SegmentMeta::Dash(Arc::new(DashSegmentMeta {
            track,
            period_id: period.map(Arc::from),
            period_start: Duration::from_secs(period_start_secs),
            representation_id: Arc::from(track.as_str()),
            number,
            time: 0,
            duration: 0,
            timescale: 1000,
            presentation_time_offset: pto_ms,
            availability_start_time: None,
        })),
    })
}

fn init(track: MediaTrack, period: Option<&str>, start: u64, data: &[u8]) -> TrackItem {
    Ok(AssembledItem::Payload(SegmentPayload::Mp4Init {
        data: Bytes::copy_from_slice(data),
        descriptor: descriptor(track, SegmentKind::Init, period, start, 0),
    }))
}

fn media(
    track: MediaTrack,
    period: Option<&str>,
    start: u64,
    number: u64,
    data: Bytes,
) -> TrackItem {
    Ok(AssembledItem::Payload(SegmentPayload::Mp4Media {
        data,
        descriptor: descriptor(track, SegmentKind::Media, period, start, number),
    }))
}

/// `segment` with its (version 1) `tfdt` set to `decode_time`.
fn with_tfdt(segment: &[u8], decode_time: u64) -> Bytes {
    let mut data = segment.to_vec();
    let at = data
        .windows(4)
        .position(|window| window == b"tfdt")
        .expect("segment has a tfdt");
    assert_eq!(data[at + 4], 1, "fixture tfdt is version 1");
    data[at + 8..at + 16].copy_from_slice(&decode_time.to_be_bytes());
    Bytes::from(data)
}

/// `init` as another encoder configuration would produce it: equal in
/// all but its `mdhd` modification time, so the merged init differs.
fn changed(init: &[u8]) -> Vec<u8> {
    let mut data = init.to_vec();
    let at = data
        .windows(4)
        .position(|window| window == b"mdhd")
        .expect("init has an mdhd");
    // Version 0: creation, then modification time, 32 bits each.
    assert_eq!(data[at + 4], 0, "fixture mdhd is version 0");
    data[at + 12..at + 16].copy_from_slice(&0x5eed_u32.to_be_bytes());
    data
}

/// `(fourcc, body)` of each box in `data[start..end]`.
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= data.len() {
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let fourcc: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
        out.push((fourcc, &data[at + 8..at + size]));
        at += size;
    }
    out
}

/// `(mfhd sequence, track ID)` of every fragment in `data`.
fn fragments(data: &[u8]) -> Vec<(u32, u32)> {
    boxes(data)
        .into_iter()
        .filter(|(fourcc, _)| fourcc == b"moof")
        .map(|(_, moof)| {
            let children = boxes(moof);
            let mfhd = children.iter().find(|(f, _)| f == b"mfhd").unwrap().1;
            let traf = children.iter().find(|(f, _)| f == b"traf").unwrap().1;
            let tfhd = boxes(traf)
                .into_iter()
                .find(|(f, _)| f == b"tfhd")
                .unwrap()
                .1;
            (
                u32::from_be_bytes(mfhd[4..8].try_into().unwrap()),
                u32::from_be_bytes(tfhd[4..8].try_into().unwrap()),
            )
        })
        .collect()
}

struct Harness {
    video: mpsc::Sender<TrackItem>,
    audio: Option<mpsc::Sender<TrackItem>>,
    out: mpsc::Receiver<Result<HlsData, DownloadError>>,
    events: DownloadEventStream,
    muxer: tokio::task::JoinHandle<MuxOutcome>,
}

fn start(leader: MediaTrack, with_follower: bool, config: DashMuxConfig) -> Harness {
    start_with_clock(leader, with_follower, config, None, None)
}

fn start_with_clock(
    leader: MediaTrack,
    with_follower: bool,
    config: DashMuxConfig,
    follower_clock: Option<watch::Receiver<ThrottleClock>>,
    media_stall: Option<MediaStall>,
) -> Harness {
    let (video, video_rx) = mpsc::channel(64);
    let (out_tx, out) = mpsc::channel(64);
    let (sink, events) = EventSink::channel(64);
    let (audio, follower) = if with_follower {
        let (audio, audio_rx) = mpsc::channel(64);
        (
            Some(audio),
            Some(MuxTrack {
                track: MediaTrack::Audio,
                rx: audio_rx,
                rate_limit_clock: follower_clock,
                cancel: CancellationToken::new(),
            }),
        )
    } else {
        (None, None)
    };
    let muxer = tokio::spawn(run_muxer(
        MuxTrack {
            track: leader,
            rx: video_rx,
            rate_limit_clock: None,
            cancel: CancellationToken::new(),
        },
        follower,
        out_tx,
        Some(sink),
        config,
        media_stall,
        CancellationToken::new(),
    ));
    Harness {
        video,
        audio,
        out,
        events,
        muxer,
    }
}

impl Harness {
    async fn video(&self, item: TrackItem) {
        self.video.send(item).await.unwrap();
    }

    async fn audio(&self, item: TrackItem) {
        self.audio.as_ref().unwrap().send(item).await.unwrap();
    }

    async fn finish(mut self) -> (Vec<HlsData>, MuxOutcome, Vec<DownloadEvent>) {
        let mut items = Vec::new();
        while let Some(item) = tokio::time::timeout(WAIT, self.out.recv())
            .await
            .expect("muxer output within the timeout")
        {
            match item {
                Ok(item) => items.push(item),
                Err(error) => panic!("unexpected stream error: {error}"),
            }
        }
        let outcome = tokio::time::timeout(WAIT, self.muxer)
            .await
            .expect("muxer finishes")
            .unwrap();
        drop(self.video);
        drop(self.audio);
        let mut events = Vec::new();
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(50), self.events.next()).await
        {
            events.push(event);
        }
        (items, outcome, events)
    }
}

fn init_data(items: &[HlsData]) -> Vec<&Bytes> {
    items
        .iter()
        .filter_map(|item| match item {
            HlsData::M4sData(M4sData::InitSegment(init)) => Some(&init.data),
            _ => None,
        })
        .collect()
}

fn media_data(items: &[HlsData]) -> Vec<&Bytes> {
    items
        .iter()
        .filter_map(|item| match item {
            HlsData::M4sData(M4sData::Segment(segment)) => Some(&segment.data),
            _ => None,
        })
        .collect()
}

fn lag(lag: Duration) -> DashMuxConfig {
    DashMuxConfig {
        max_audio_lag: lag,
        ..DashMuxConfig::default()
    }
}

#[tokio::test(start_paused = true)]
async fn bounded_audio_throttling_extends_audio_and_media_stall_waits() {
    let until = Instant::now() + Duration::from_secs(120);
    let mut clock = ThrottleClock::default();
    clock.set_hold(Instant::now().into_std(), Some(until.into_std()));
    let (clock_tx, clock_rx) = watch::channel(clock);
    let mut h = start_with_clock(
        MediaTrack::Video,
        true,
        lag(Duration::from_secs(10)),
        Some(clock_rx.clone()),
        Some(MediaStall {
            timeout: Duration::from_secs(60),
            clock: clock_rx,
        }),
    );
    h.audio(init(MediaTrack::Audio, Some("p0"), 0, AUDIO_INIT))
        .await;
    h.video(init(MediaTrack::Video, Some("p0"), 0, VIDEO_INIT))
        .await;
    h.video(media(
        MediaTrack::Video,
        Some("p0"),
        0,
        1,
        Bytes::from_static(VIDEO_1),
    ))
    .await;
    h.video(media(
        MediaTrack::Video,
        Some("p0"),
        0,
        2,
        Bytes::from_static(VIDEO_2),
    ))
    .await;
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert!(h.out.try_recv().is_err(), "still waiting for held audio");
    assert!(!h.muxer.is_finished());

    // Recovery can clear the hold before its payload crosses the track
    // channel. Both queued slices retain the actual pause in their clocks.
    clock_tx.send_modify(|clock| {
        clock.set_hold(Instant::now().into_std(), None);
    });
    h.audio(media(
        MediaTrack::Audio,
        Some("p0"),
        0,
        1,
        Bytes::from_static(AUDIO_1),
    ))
    .await;
    h.audio(media(
        MediaTrack::Audio,
        Some("p0"),
        0,
        2,
        Bytes::from_static(AUDIO_2),
    ))
    .await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    let (items, outcome, events) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let all: Vec<_> = media_data(&items)
        .into_iter()
        .flat_map(|data| fragments(data))
        .collect();
    assert_eq!(all.iter().filter(|(_, track)| *track == 1).count(), 2);
    assert_eq!(all.iter().filter(|(_, track)| *track == 2).count(), 2);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, DownloadEvent::TrackStalled { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn a_closed_throttle_publisher_cannot_suspend_audio_forever() {
    let until = Instant::now() + Duration::from_secs(30);
    let mut clock = ThrottleClock::default();
    clock.set_hold(Instant::now().into_std(), Some(until.into_std()));
    let (clock_tx, clock_rx) = watch::channel(clock);
    let h = start_with_clock(
        MediaTrack::Video,
        true,
        lag(Duration::from_secs(10)),
        Some(clock_rx),
        None,
    );
    h.audio(init(MediaTrack::Audio, Some("p0"), 0, AUDIO_INIT))
        .await;
    h.video(init(MediaTrack::Video, Some("p0"), 0, VIDEO_INIT))
        .await;
    h.video(media(
        MediaTrack::Video,
        Some("p0"),
        0,
        1,
        Bytes::from_static(VIDEO_1),
    ))
    .await;
    drop(clock_tx);
    tokio::time::sleep(Duration::from_secs(41)).await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    let (items, outcome, events) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(media_data(&items).len(), 1);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DownloadEvent::TrackStalled { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn a_short_throttle_preserves_the_remaining_audio_and_media_stall_budgets() {
    let (clock_tx, clock_rx) = watch::channel(ThrottleClock::default());
    let mut h = start_with_clock(
        MediaTrack::Video,
        true,
        lag(Duration::from_secs(10)),
        Some(clock_rx.clone()),
        Some(MediaStall {
            timeout: Duration::from_secs(60),
            clock: clock_rx,
        }),
    );
    h.audio(init(MediaTrack::Audio, Some("p0"), 0, AUDIO_INIT))
        .await;
    h.video(init(MediaTrack::Video, Some("p0"), 0, VIDEO_INIT))
        .await;
    h.video(media(
        MediaTrack::Video,
        Some("p0"),
        0,
        1,
        Bytes::from_static(VIDEO_1),
    ))
    .await;
    tokio::time::sleep(Duration::from_secs(9)).await;
    clock_tx.send_modify(|clock| {
        let now = Instant::now().into_std();
        clock.set_hold(now, Some(now + Duration::from_secs(300)));
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    clock_tx.send_modify(|clock| {
        clock.set_hold(Instant::now().into_std(), None);
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        h.out.try_recv().is_err(),
        "the last half-second is still available"
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(next_kind(&mut h.out).await, "init");
    assert_eq!(next_kind(&mut h.out).await, "media");

    // The refusal ended after one second. It cannot defer an unrelated
    // subsequent media stall until the unused five-minute allowance.
    tokio::time::sleep(Duration::from_secs(59)).await;
    assert!(h.out.try_recv().is_err());
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(matches!(
        h.out.recv().await,
        Some(Err(DownloadError::Timeout { .. }))
    ));
    assert!(matches!(h.muxer.await.unwrap(), MuxOutcome::Failed(_)));
}

#[tokio::test(start_paused = true)]
async fn a_bounded_leader_hold_does_not_disable_the_media_watchdog() {
    let until = Instant::now() + Duration::from_secs(120);
    let mut clock = ThrottleClock::default();
    clock.set_hold(Instant::now().into_std(), Some(until.into_std()));
    let (_clock_tx, clock_rx) = watch::channel(clock);
    let mut h = start_with_clock(
        MediaTrack::Video,
        false,
        lag(Duration::from_secs(10)),
        None,
        Some(MediaStall {
            timeout: Duration::from_secs(60),
            clock: clock_rx,
        }),
    );
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert!(
        !h.muxer.is_finished(),
        "the bounded hold extends the watchdog"
    );
    tokio::time::sleep(Duration::from_secs(91)).await;
    assert!(matches!(
        h.out.recv().await,
        Some(Err(DownloadError::Timeout { .. }))
    ));
    assert!(matches!(h.muxer.await.unwrap(), MuxOutcome::Failed(_)));
}

#[tokio::test(start_paused = true)]
async fn a_throttled_audio_tail_is_waited_for_after_video_ends() {
    let until = Instant::now() + Duration::from_secs(120);
    let mut clock = ThrottleClock::default();
    clock.set_hold(Instant::now().into_std(), Some(until.into_std()));
    let (_clock_tx, clock_rx) = watch::channel(clock);
    let mut h = start_with_clock(
        MediaTrack::Video,
        true,
        lag(Duration::from_secs(10)),
        Some(clock_rx),
        None,
    );
    h.audio(init(MediaTrack::Audio, Some("p0"), 0, AUDIO_INIT))
        .await;
    h.audio(media(
        MediaTrack::Audio,
        Some("p0"),
        0,
        1,
        Bytes::from_static(AUDIO_1),
    ))
    .await;
    // The first audio fragment ends before the first video fragment.
    // Both initial audio fragments are needed to write that video slice.
    h.audio(media(
        MediaTrack::Audio,
        Some("p0"),
        0,
        2,
        Bytes::from_static(AUDIO_2),
    ))
    .await;
    h.video(init(MediaTrack::Video, Some("p0"), 0, VIDEO_INIT))
        .await;
    h.video(media(
        MediaTrack::Video,
        Some("p0"),
        0,
        1,
        Bytes::from_static(VIDEO_1),
    ))
    .await;
    assert_eq!(next_kind(&mut h.out).await, "init");
    assert_eq!(next_kind(&mut h.out).await, "media");
    h.video(Ok(AssembledItem::StreamEnded)).await;
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert!(
        !h.muxer.is_finished(),
        "the held audio tail is still expected"
    );
    h.audio(media(
        MediaTrack::Audio,
        Some("p0"),
        0,
        3,
        Bytes::from_static(AUDIO_3),
    ))
    .await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;
    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(
        media_data(&items)
            .into_iter()
            .flat_map(|data| fragments(data))
            .filter(|(_, track)| *track == 2)
            .count(),
        1,
        "the final audio fragment is retained"
    );
}

#[tokio::test(start_paused = true)]
async fn bounded_throttling_does_not_override_mux_buffer_pressure() {
    let until = Instant::now() + Duration::from_secs(120);
    let mut clock = ThrottleClock::default();
    clock.set_hold(Instant::now().into_std(), Some(until.into_std()));
    let (_clock_tx, clock_rx) = watch::channel(clock);
    let mut config = lag(Duration::from_secs(10));
    config.max_buffered_bytes = 1;
    let mut h = start_with_clock(MediaTrack::Video, true, config, Some(clock_rx), None);
    h.audio(init(MediaTrack::Audio, Some("p0"), 0, AUDIO_INIT))
        .await;
    h.video(init(MediaTrack::Video, Some("p0"), 0, VIDEO_INIT))
        .await;
    h.video(media(
        MediaTrack::Video,
        Some("p0"),
        0,
        1,
        Bytes::from_static(VIDEO_1),
    ))
    .await;
    assert_eq!(next_kind(&mut h.out).await, "init");
    assert_eq!(next_kind(&mut h.out).await, "media");
    assert!(
        Instant::now() < until,
        "buffer pressure still makes progress"
    );
    h.audio(Ok(AssembledItem::StreamEnded)).await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    assert_eq!(h.finish().await.1, MuxOutcome::Ended);
}

#[tokio::test]
async fn failed_audio_is_dropped_and_video_continues() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let v = MediaTrack::Video;
    h.audio(Err(DownloadError::Timeout {
        reason: "stalled".to_owned(),
    }))
    .await;
    h.video(init(v, None, 0, VIDEO_INIT)).await;
    h.video(media(v, None, 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(media(v, None, 0, 2, Bytes::from_static(VIDEO_2)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, events) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(media_data(&items).len(), 2);
    let inits = init_data(&items);
    assert!(!mp4::isobmff::parse_init_segment(inits[0]).has_aac);
    assert!(events.iter().any(|event| matches!(
        event,
        DownloadEvent::TrackDropped {
            track: MediaTrack::Audio,
            ..
        }
    )));
}

#[tokio::test]
async fn identical_inits_continue_the_output_across_a_period_change() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
    h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
    h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
        .await;
    // p1 repeats both inits and restarts the media timeline at 1 s.
    h.video(init(v, Some("p1"), 1, VIDEO_INIT)).await;
    h.video(media(v, Some("p1"), 1, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    h.audio(init(a, Some("p1"), 1, AUDIO_INIT)).await;
    h.audio(media(a, Some("p1"), 1, 1, Bytes::from_static(AUDIO_1)))
        .await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let kinds: Vec<&str> = items.iter().map(kind).collect();
    assert_eq!(kinds, ["init", "media", "media", "end"]);
    let all: Vec<(u32, u32)> = media_data(&items)
        .iter()
        .flat_map(|data| fragments(data))
        .collect();
    assert_eq!(
        all.iter().map(|(_, track)| *track).collect::<Vec<_>>(),
        [1, 2, 1, 2],
        "one fragment of each track per Period"
    );
    assert!(
        all.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "fragment sequence numbers continue: {all:?}"
    );
}

/// `(track, decode time in ms)` of every fragment written, per file.
fn written_ms(items: &[HlsData]) -> Vec<Vec<(u32, u64)>> {
    let mut files = vec![Vec::new()];
    for item in items {
        match item {
            HlsData::EndMarker(Some(SplitReason::StreamStructureChange { .. })) => {
                files.push(Vec::new());
            }
            HlsData::M4sData(M4sData::Segment(segment)) => {
                let file = files.last_mut().expect("a file");
                for (fourcc, moof) in boxes(&segment.data) {
                    if &fourcc != b"moof" {
                        continue;
                    }
                    let traf = boxes(moof)
                        .into_iter()
                        .find(|(f, _)| f == b"traf")
                        .unwrap()
                        .1;
                    let children = boxes(traf);
                    let tfhd = children.iter().find(|(f, _)| f == b"tfhd").unwrap().1;
                    let tfdt = children.iter().find(|(f, _)| f == b"tfdt").unwrap().1;
                    let track = u32::from_be_bytes(tfhd[4..8].try_into().unwrap());
                    let time = u64::from_be_bytes(tfdt[4..12].try_into().unwrap());
                    let timescale = if track == 1 { 12_288 } else { 48_000 };
                    file.push((track, time * 1000 / timescale));
                }
            }
            _ => {}
        }
    }
    files
}

/// `p`'s video and audio from `second` (one second each), Period
/// starting at `start` seconds with media time 0 there.
async fn send_second(h: &Harness, period: &str, start: u64, second: u64, number: u64) {
    let offset = second - start;
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    h.video(media(
        v,
        Some(period),
        start,
        number,
        with_tfdt(VIDEO_1, offset * 12_288),
    ))
    .await;
    h.audio(media(
        a,
        Some(period),
        start,
        number,
        with_tfdt(AUDIO_1, offset * 48_000),
    ))
    .await;
}

async fn send_inits(h: &Harness, period: &str, start: u64) {
    h.video(init(MediaTrack::Video, Some(period), start, VIDEO_INIT))
        .await;
    h.audio(init(MediaTrack::Audio, Some(period), start, AUDIO_INIT))
        .await;
}

async fn end_both(h: &Harness) {
    h.video(Ok(AssembledItem::StreamEnded)).await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;
}

#[tokio::test]
async fn a_period_repeating_written_media_is_written_once() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    send_inits(&h, "p0", 0).await;
    for second in 0..2 {
        send_second(&h, "p0", 0, second, second + 1).await;
    }
    // p1 starts at 1 s: its first second repeats p0's overhang.
    send_inits(&h, "p1", 1).await;
    for second in 1..3 {
        send_second(&h, "p1", 1, second, second).await;
    }
    end_both(&h).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(
        written_ms(&items),
        [vec![
            (1, 0),
            (2, 0),
            (1, 1000),
            (2, 1000),
            (1, 2000),
            (2, 2000)
        ]]
    );
}

#[tokio::test]
async fn media_the_previous_period_did_not_write_is_kept() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let v = MediaTrack::Video;
    send_inits(&h, "p0", 0).await;
    send_second(&h, "p0", 0, 0, 1).await;
    // p0's overhang video (1–2 s) failed; its audio arrived. Whatever
    // its MPD said, p1's segment holds 1–2 s.
    h.video(Ok(AssembledItem::GapSkipped {
        from_sequence: 2,
        to_sequence: 3,
        reason: crate::segment_engine::assembler::GapSkipReason::Upstream,
    }))
    .await;
    h.audio(media(
        MediaTrack::Audio,
        Some("p0"),
        0,
        2,
        with_tfdt(AUDIO_1, 48_000),
    ))
    .await;
    send_inits(&h, "p1", 1).await;
    send_second(&h, "p1", 1, 1, 1).await;
    h.video(media(v, Some("p1"), 1, 2, with_tfdt(VIDEO_1, 12_288)))
        .await;
    end_both(&h).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let files = written_ms(&items);
    assert_eq!(files.len(), 1, "one file");
    let track = |id: u32| -> Vec<u64> {
        files[0]
            .iter()
            .filter(|(track, _)| *track == id)
            .map(|&(_, ms)| ms)
            .collect()
    };
    assert_eq!(track(1), [0, 1000, 2000], "p1 fills the video");
    assert_eq!(track(2), [0, 1000], "p0's audio, once");
}

#[tokio::test]
async fn repeats_are_judged_across_an_intervening_period() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    send_inits(&h, "p0", 0).await;
    for second in 0..3 {
        send_second(&h, "p0", 0, second, second + 1).await;
    }
    // An update reveals p1 at 1 s and p2 at 2 s, both inside p0's
    // written 0–3 s.
    send_inits(&h, "p1", 1).await;
    send_second(&h, "p1", 1, 1, 1).await;
    send_inits(&h, "p2", 2).await;
    for second in 2..4 {
        send_second(&h, "p2", 2, second, second - 1).await;
    }
    end_both(&h).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let expected: Vec<(u32, u64)> = (0..4)
        .flat_map(|second| [(1, second * 1000), (2, second * 1000)])
        .collect();
    assert_eq!(written_ms(&items), [expected]);
}

#[tokio::test]
async fn only_the_repeated_fragments_of_a_segment_are_dropped() {
    let h = start(MediaTrack::Video, false, lag(WAIT));
    let v = MediaTrack::Video;
    h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
    h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(media(v, Some("p0"), 0, 2, with_tfdt(VIDEO_1, 12_288)))
        .await;
    // p0's 2–3 s failed; p1 (from 1 s) has one segment of two
    // fragments, 1–2 s and 2–3 s.
    h.video(Ok(AssembledItem::GapSkipped {
        from_sequence: 3,
        to_sequence: 4,
        reason: crate::segment_engine::assembler::GapSkipReason::Upstream,
    }))
    .await;
    h.video(init(v, Some("p1"), 1, VIDEO_INIT)).await;
    let mut two = VIDEO_1.to_vec();
    two.extend_from_slice(&with_tfdt(VIDEO_1, 12_288));
    h.video(media(v, Some("p1"), 1, 1, Bytes::from(two))).await;
    h.video(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(written_ms(&items), [vec![(1, 0), (1, 1000), (1, 2000)]]);
}

#[tokio::test]
async fn a_new_epoch_straddling_the_written_end_starts_a_new_file() {
    let h = start(MediaTrack::Video, false, lag(WAIT));
    let v = MediaTrack::Video;
    h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
    h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(media(v, Some("p0"), 0, 2, with_tfdt(VIDEO_1, 12_288)))
        .await;
    // p1's first fragment, 1.5–2.5 s, reaches past the written 2 s.
    h.video(init(v, Some("p1"), 1, VIDEO_INIT)).await;
    h.video(media(v, Some("p1"), 1, 1, with_tfdt(VIDEO_1, 6_144)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(
        written_ms(&items),
        [vec![(1, 0), (1, 1000)], vec![(1, 1500)]],
        "kept whole, in a new file"
    );
}

#[tokio::test]
async fn a_track_first_written_after_a_split_does_not_split_again() {
    let h = start(MediaTrack::Video, true, lag(Duration::from_millis(50)));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    send_inits(&h, "p0", 100).await;
    send_second(&h, "p0", 100, 100, 1).await;
    // p1 restarts at 10 s; its video is written, in a new file, before
    // its audio arrives.
    send_inits(&h, "p1", 10).await;
    h.video(media(v, Some("p1"), 10, 1, Bytes::from_static(VIDEO_1)))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Its first audio, 10.5–11.5 s, lies far behind p0's audio, which
    // is in the previous file only.
    h.audio(media(a, Some("p1"), 10, 1, with_tfdt(AUDIO_1, 24_000)))
        .await;
    h.video(media(v, Some("p1"), 10, 2, with_tfdt(VIDEO_1, 12_288)))
        .await;
    end_both(&h).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(
        written_ms(&items),
        [
            vec![(1, 100_000), (2, 100_000)],
            vec![(1, 10_000), (1, 11_000), (2, 10_500)]
        ]
    );
}

#[tokio::test]
async fn a_hole_in_written_media_is_filled_by_the_next_period() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    send_inits(&h, "p0", 0).await;
    send_second(&h, "p0", 0, 0, 1).await;
    // p0's 1–2 s failed on both tracks; 2–3 s was written.
    let skipped = || {
        Ok(AssembledItem::GapSkipped {
            from_sequence: 2,
            to_sequence: 3,
            reason: crate::segment_engine::assembler::GapSkipReason::Upstream,
        })
    };
    h.video(skipped()).await;
    h.audio(skipped()).await;
    send_second(&h, "p0", 0, 2, 3).await;
    // p1, revealed at 1 s, holds the missing second: it needs a new
    // file, as the old one already reaches 3 s.
    send_inits(&h, "p1", 1).await;
    for second in 1..4 {
        send_second(&h, "p1", 1, second, second).await;
    }
    end_both(&h).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(
        written_ms(&items),
        [
            vec![(1, 0), (2, 0), (1, 2000), (2, 2000)],
            vec![
                (1, 1000),
                (2, 1000),
                (1, 2000),
                (2, 2000),
                (1, 3000),
                (2, 3000)
            ]
        ]
    );
}

#[tokio::test]
async fn a_hole_in_an_audio_tail_is_filled_by_the_next_period() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let a = MediaTrack::Audio;
    send_inits(&h, "p0", 0).await;
    send_second(&h, "p0", 0, 0, 1).await;
    // p0's audio runs past its last video: 1–2 s and 3–4 s, written
    // together as its tail; 2–3 s failed.
    h.audio(media(a, Some("p0"), 0, 2, with_tfdt(AUDIO_1, 48_000)))
        .await;
    h.audio(Ok(AssembledItem::GapSkipped {
        from_sequence: 3,
        to_sequence: 4,
        reason: crate::segment_engine::assembler::GapSkipReason::Upstream,
    }))
    .await;
    h.audio(media(a, Some("p0"), 0, 4, with_tfdt(AUDIO_1, 3 * 48_000)))
        .await;
    // p1 starts at 2 s, at the hole.
    send_inits(&h, "p1", 2).await;
    for second in 2..4 {
        send_second(&h, "p1", 2, second, second - 1).await;
    }
    end_both(&h).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    // p1's 2–3 s audio fills the hole, in a new file as the old one
    // already reaches 4 s.
    assert_eq!(
        written_ms(&items),
        [
            vec![(1, 0), (2, 0), (2, 1000), (2, 3000)],
            vec![(1, 2000), (2, 2000), (1, 3000), (2, 3000)]
        ]
    );
}

#[tokio::test]
async fn a_reset_audio_tail_is_written_whole() {
    let h = start(MediaTrack::Video, true, lag(Duration::from_millis(50)));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    send_inits(&h, "p0", 17).await;
    for second in 17..20 {
        send_second(&h, "p0", 17, second, second - 16).await;
    }
    // p1 resets to 14 s. Its video is written, in a new file, before
    // its audio arrives, and then ends: its audio, 14–21 s, is the
    // tail, partly at times p0's audio was written at.
    send_inits(&h, "p1", 14).await;
    h.video(media(v, Some("p1"), 14, 1, Bytes::from_static(VIDEO_1)))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    for second in 14..21 {
        h.audio(media(
            a,
            Some("p1"),
            14,
            second - 13,
            with_tfdt(AUDIO_1, (second - 14) * 48_000),
        ))
        .await;
    }
    h.audio(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let files = written_ms(&items);
    assert_eq!(files.len(), 2);
    // 14–15 s arrived after its video was written: late, as ever.
    let audio: Vec<u64> = files[1]
        .iter()
        .filter(|(track, _)| *track == 2)
        .map(|&(_, ms)| ms)
        .collect();
    assert_eq!(audio, [15_000, 16_000, 17_000, 18_000, 19_000, 20_000]);
}

#[tokio::test]
async fn media_written_before_a_reset_is_not_a_repeat_after_it() {
    let h = start(MediaTrack::Video, false, lag(WAIT));
    let v = MediaTrack::Video;
    // p0 writes 3–4 s; p1 resets to 1 s and runs to 3 s; p2 then
    // reaches 3–4 s again, which the new timeline never wrote.
    for (period, start, seconds) in [("p0", 3, 3..4), ("p1", 1, 1..3), ("p2", 3, 3..5)] {
        h.video(init(v, Some(period), start, VIDEO_INIT)).await;
        for second in seconds {
            h.video(media(
                v,
                Some(period),
                start,
                second,
                with_tfdt(VIDEO_1, (second - start) * 12_288),
            ))
            .await;
        }
    }
    h.video(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(
        written_ms(&items),
        [
            vec![(1, 3000)],
            vec![(1, 1000), (1, 2000), (1, 3000), (1, 4000)]
        ]
    );
}

#[tokio::test]
async fn a_timestamp_reset_at_a_period_change_splits_and_drops_nothing() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    // p0 at 100 s; p1 restarts at 10 s: a new file, nothing dropped;
    // p2 follows p1 exactly and continues that file.
    for (period, start) in [("p0", 100), ("p1", 10), ("p2", 11)] {
        send_inits(&h, period, start).await;
        send_second(&h, period, start, start, 1).await;
    }
    end_both(&h).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(
        written_ms(&items),
        [
            vec![(1, 100_000), (2, 100_000)],
            vec![(1, 10_000), (2, 10_000), (1, 11_000), (2, 11_000)]
        ]
    );
}

#[tokio::test]
async fn late_first_audio_of_a_continued_file_never_goes_back_in_time() {
    let h = start(MediaTrack::Video, true, lag(Duration::from_millis(50)));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
    h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
    h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
        .await;
    // Audio reaches past p0's only video segment, to about 2 s.
    h.audio(media(a, Some("p0"), 0, 2, with_tfdt(AUDIO_1, 48_000)))
        .await;
    // p1 starts at 1 s with identical inits; its audio init comes with
    // its video, its audio media only after the video was written.
    h.audio(init(a, Some("p1"), 1, AUDIO_INIT)).await;
    h.video(init(v, Some("p1"), 1, VIDEO_INIT)).await;
    h.video(media(v, Some("p1"), 1, 1, Bytes::from_static(VIDEO_1)))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    // From 1.5 s: inside the audio p0 already wrote.
    h.audio(media(a, Some("p1"), 1, 1, with_tfdt(AUDIO_1, 24_000)))
        .await;
    h.video(media(v, Some("p1"), 1, 2, with_tfdt(VIDEO_1, 12_288)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let kinds: Vec<&str> = items.iter().map(kind).collect();
    assert_eq!(
        kinds,
        [
            "init", "media", "media", "media", "epoch", "init", "media", "end"
        ]
    );
    let split = items
        .iter()
        .position(|item| kind(item) == "epoch")
        .expect("a split");
    let before: Vec<u32> = media_data(&items[..split])
        .iter()
        .flat_map(|data| fragments(data))
        .map(|(_, track)| track)
        .collect();
    assert_eq!(before, [1, 2, 2, 1], "p1's video continued the file");
    let after: Vec<u32> = media_data(&items[split..])
        .iter()
        .flat_map(|data| fragments(data))
        .map(|(_, track)| track)
        .collect();
    assert_eq!(after, [1, 2], "p1's audio opened a new file");
}

#[tokio::test]
async fn single_track_is_normalized_through_the_merger() {
    let h = start(MediaTrack::Audio, false, lag(WAIT));
    let a = MediaTrack::Audio;
    h.video(init(a, None, 0, AUDIO_INIT)).await;
    h.video(media(a, None, 0, 1, Bytes::from_static(AUDIO_1)))
        .await;
    h.video(media(a, None, 0, 2, Bytes::from_static(AUDIO_2)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let segments = media_data(&items);
    assert_eq!(segments.len(), 2);
    for data in segments {
        // `styp` and `sidx` of the source are gone.
        assert_eq!(&data[4..8], b"moof");
        assert_eq!(fragments(data).len(), 1);
    }
    let sequences: Vec<u32> = media_data(&items)
        .iter()
        .flat_map(|data| fragments(data))
        .map(|(sequence, _)| sequence)
        .collect();
    assert_eq!(sequences, [1, 2]);
}

#[tokio::test]
async fn video_ending_first_waits_for_the_audio_tail() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    h.video(init(v, None, 0, VIDEO_INIT)).await;
    h.video(media(v, None, 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(media(v, None, 0, 2, Bytes::from_static(VIDEO_2)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    h.audio(init(a, None, 0, AUDIO_INIT)).await;
    h.audio(media(a, None, 0, 1, Bytes::from_static(AUDIO_1)))
        .await;
    h.audio(media(a, None, 0, 2, Bytes::from_static(AUDIO_2)))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.audio(media(a, None, 0, 3, Bytes::from_static(AUDIO_3)))
        .await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let segments = media_data(&items);
    let tail = segments.last().unwrap();
    assert!(
        fragments(tail).iter().all(|(_, track)| *track == 2),
        "audio tail item"
    );
    let audio = segments
        .iter()
        .flat_map(|data| fragments(data))
        .filter(|(_, track)| *track == 2)
        .count();
    assert_eq!(audio, 3);
}

/// One init and one media segment carrying both fixture tracks, as a
/// multiplexed Representation serves them.
fn multiplexed() -> (Bytes, Bytes) {
    let init = merge_init(&[
        Bytes::from_static(VIDEO_INIT),
        Bytes::from_static(AUDIO_INIT),
    ])
    .unwrap();
    let mut segment = Vec::new();
    let mut writer = FragmentWriter::new();
    for (track, data) in [(0, VIDEO_1), (1, AUDIO_1)] {
        for fragment in init
            .parse_media_segment(track, &Bytes::from_static(data), 0)
            .unwrap()
        {
            writer.write(fragment, &mut segment).unwrap();
        }
    }
    (init.data().clone(), Bytes::from(segment))
}

#[tokio::test]
async fn a_multiplexed_representation_is_passed_through_unchanged() {
    let (muxed_init, muxed_media) = multiplexed();
    let h = start(MediaTrack::Video, false, lag(WAIT));
    let v = MediaTrack::Video;
    h.video(init(v, None, 0, &muxed_init)).await;
    h.video(media(v, None, 0, 1, muxed_media.clone())).await;
    h.video(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert_eq!(init_data(&items), [&muxed_init]);
    assert_eq!(media_data(&items), [&muxed_media]);
    let tracks: Vec<u32> = fragments(&muxed_media).iter().map(|(_, id)| *id).collect();
    assert_eq!(tracks, [1, 2], "genuine two-track media");
    let check = IndependentFragmentCheck::from_init(&muxed_init).unwrap();
    assert!(check.is_independent(&muxed_media));
}

#[tokio::test]
async fn protected_or_malformed_multiplexed_inits_are_not_passed_through() {
    let (muxed_init, _) = multiplexed();
    let mut protected = muxed_init.to_vec();
    let at = protected
        .windows(4)
        .position(|window| window == b"mp4a")
        .unwrap();
    protected[at..at + 4].copy_from_slice(b"enca");
    let malformed = {
        // Drop the audio track's `trex`: its fragments could not be read.
        let mut data = muxed_init.to_vec();
        let at = data
            .windows(4)
            .rposition(|window| window == b"trex")
            .unwrap()
            - 4;
        data.drain(at..at + 32);
        let mvex = data
            .windows(4)
            .position(|window| window == b"mvex")
            .unwrap()
            - 4;
        let size = u32::from_be_bytes(data[mvex..mvex + 4].try_into().unwrap()) - 32;
        data[mvex..mvex + 4].copy_from_slice(&size.to_be_bytes());
        let moov = data
            .windows(4)
            .position(|window| window == b"moov")
            .unwrap()
            - 4;
        let size = u32::from_be_bytes(data[moov..moov + 4].try_into().unwrap()) - 32;
        data[moov..moov + 4].copy_from_slice(&size.to_be_bytes());
        data
    };
    for (data, protected) in [(protected, true), (malformed, false)] {
        let mut h = start(MediaTrack::Video, false, lag(WAIT));
        h.video(init(MediaTrack::Video, None, 0, &data)).await;
        let item = tokio::time::timeout(WAIT, h.out.recv())
            .await
            .unwrap()
            .unwrap();
        match item {
            Err(DownloadError::Unsupported { .. }) => assert!(protected),
            Err(DownloadError::InvalidContent { .. }) => assert!(!protected),
            other => panic!("expected a typed rejection, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn audio_left_at_a_period_change_is_written_under_its_own_init() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
    h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
    h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
        .await;
    // Starts at 1 s, after the only video segment of p0 ends.
    h.audio(media(a, Some("p0"), 0, 2, with_tfdt(AUDIO_1, 48_000)))
        .await;
    h.video(init(v, Some("p1"), 2, &changed(VIDEO_INIT))).await;
    h.video(media(v, Some("p1"), 2, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    h.audio(init(a, Some("p1"), 2, AUDIO_INIT)).await;
    h.audio(media(a, Some("p1"), 2, 1, Bytes::from_static(AUDIO_1)))
        .await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let kinds: Vec<String> = items
        .iter()
        .map(|item| match item {
            HlsData::M4sData(M4sData::InitSegment(_)) => "init".to_owned(),
            HlsData::M4sData(M4sData::Segment(segment)) => {
                let tracks: Vec<String> = fragments(&segment.data)
                    .iter()
                    .map(|(_, track)| track.to_string())
                    .collect();
                format!("media[{}]", tracks.join(","))
            }
            HlsData::EndMarker(Some(SplitReason::StreamStructureChange { .. })) => {
                "epoch".to_owned()
            }
            HlsData::EndMarker(Some(SplitReason::EndOfStream)) => "end".to_owned(),
            _ => "other".to_owned(),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "init",
            "media[1,2]",
            "media[2]",
            "epoch",
            "init",
            "media[1,2]",
            "end"
        ]
    );
}

#[tokio::test]
async fn fragments_straddling_the_period_start_get_a_common_origin() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    // PTO 0.5 s: both tracks' first fragments start half a second before
    // the Period.
    let item = |track, kind, number, data: Bytes| -> TrackItem {
        let descriptor = descriptor_with_pto(track, kind, None, 0, number, 500);
        Ok(AssembledItem::Payload(match kind {
            SegmentKind::Init => SegmentPayload::Mp4Init { data, descriptor },
            SegmentKind::Media => SegmentPayload::Mp4Media { data, descriptor },
        }))
    };
    h.video(item(
        v,
        SegmentKind::Init,
        0,
        Bytes::from_static(VIDEO_INIT),
    ))
    .await;
    h.video(item(v, SegmentKind::Media, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    h.audio(item(
        a,
        SegmentKind::Init,
        0,
        Bytes::from_static(AUDIO_INIT),
    ))
    .await;
    h.audio(item(a, SegmentKind::Media, 1, Bytes::from_static(AUDIO_1)))
        .await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let segments = media_data(&items);
    assert_eq!(segments.len(), 1, "nothing was rejected");
    let decode_times: Vec<(u32, u64)> = boxes(segments[0])
        .into_iter()
        .filter(|(fourcc, _)| fourcc == b"moof")
        .map(|(_, moof)| {
            let traf = boxes(moof)
                .into_iter()
                .find(|(f, _)| f == b"traf")
                .unwrap()
                .1;
            let children = boxes(traf);
            let tfhd = children.iter().find(|(f, _)| f == b"tfhd").unwrap().1;
            let tfdt = children.iter().find(|(f, _)| f == b"tfdt").unwrap().1;
            (
                u32::from_be_bytes(tfhd[4..8].try_into().unwrap()),
                u64::from_be_bytes(tfdt[4..12].try_into().unwrap()),
            )
        })
        .collect();
    // Lifted by the same 0.5 s: both start at zero, keeping their offset.
    assert_eq!(decode_times, [(1, 0), (2, 0)]);
}

#[tokio::test]
async fn fragments_dropped_before_the_origin_do_not_reject_their_segment() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    // Audio PTO 1 s: the segment's first fragment ends before the video
    // starts and is dropped; only the second is written.
    let audio = |kind, number, data: Bytes| -> TrackItem {
        let descriptor = descriptor_with_pto(a, kind, None, 0, number, 1000);
        Ok(AssembledItem::Payload(match kind {
            SegmentKind::Init => SegmentPayload::Mp4Init { data, descriptor },
            SegmentKind::Media => SegmentPayload::Mp4Media { data, descriptor },
        }))
    };
    let mut two_fragments = AUDIO_1.to_vec();
    two_fragments.extend_from_slice(&with_tfdt(AUDIO_1, 48_000));
    h.video(init(v, None, 0, VIDEO_INIT)).await;
    h.video(media(v, None, 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    h.audio(audio(SegmentKind::Init, 0, Bytes::from_static(AUDIO_INIT)))
        .await;
    h.audio(audio(SegmentKind::Media, 1, Bytes::from(two_fragments)))
        .await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, events) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, DownloadEvent::TrackDropped { .. })),
        "{events:?}"
    );
    let segments = media_data(&items);
    assert_eq!(segments.len(), 1);
    let tracks: Vec<u32> = fragments(segments[0]).iter().map(|(_, id)| *id).collect();
    assert_eq!(tracks, [1, 2], "the kept audio fragment is written");
}

/// `item` re-keyed into another epoch scope: the same Period and
/// Representation id under a new init, as the planner issues after a PTO
/// or sample entry change.
fn rescoped(item: TrackItem, scope: &str) -> TrackItem {
    let Ok(AssembledItem::Payload(payload)) = item else {
        panic!("test items are payloads");
    };
    let rekey = |descriptor: &Arc<SegmentDescriptor>| {
        let mut descriptor = descriptor.as_ref().clone();
        descriptor.key.scope = Some(Arc::from(scope));
        Arc::new(descriptor)
    };
    Ok(AssembledItem::Payload(match payload {
        SegmentPayload::Mp4Init { data, descriptor } => SegmentPayload::Mp4Init {
            data,
            descriptor: rekey(&descriptor),
        },
        SegmentPayload::Mp4Media { data, descriptor } => SegmentPayload::Mp4Media {
            data,
            descriptor: rekey(&descriptor),
        },
        other => other,
    }))
}

fn absent(period: &str, start_secs: u64, after_sequence: u64) -> TrackItem {
    Ok(AssembledItem::Notice(EngineNotice::TrackAbsent {
        section_id: Some(Arc::from(period)),
        section_start: Duration::from_secs(start_secs),
        after_sequence,
    }))
}

#[tokio::test]
async fn an_audio_only_period_announced_by_the_video_planner_is_recorded_live() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
    h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
    h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
        .await;
    // An ad break without video: no video item will come for p1, whose
    // gap follows video sequence number 1.
    h.video(absent("p1", 1, 2)).await;
    h.audio(init(a, Some("p1"), 1, AUDIO_INIT)).await;
    h.audio(media(a, Some("p1"), 1, 1, Bytes::from_static(AUDIO_1)))
        .await;
    let mut h = h;
    let mut seen = Vec::new();
    while seen.len() < 5 {
        let item = tokio::time::timeout(WAIT, h.out.recv())
            .await
            .expect("the audio-only Period is written without waiting for video")
            .unwrap()
            .unwrap();
        seen.push(kind(&item));
    }
    assert_eq!(seen, ["init", "media", "epoch", "init", "media"]);
    // Video returns with the next Period.
    h.video(init(v, Some("p2"), 2, VIDEO_INIT)).await;
    h.video(media(v, Some("p2"), 2, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    h.audio(init(a, Some("p2"), 2, AUDIO_INIT)).await;
    h.audio(media(a, Some("p2"), 2, 1, Bytes::from_static(AUDIO_1)))
        .await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;
    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let kinds: Vec<&str> = items.iter().map(kind).collect();
    assert_eq!(kinds, ["epoch", "init", "media", "end"]);
    let tracks: Vec<u32> = fragments(media_data(&items)[0])
        .iter()
        .map(|(_, id)| *id)
        .collect();
    assert_eq!(tracks, [1, 2], "back to video and audio");
}

/// A p0 A/V start, then `count` audio segments of the audio-only p1, under
/// a buffer far smaller than that audio.
async fn long_audio_only_period(video_ended: bool, count: u64) -> Vec<&'static str> {
    let config = DashMuxConfig {
        max_audio_lag: WAIT,
        max_buffered_bytes: 16 * 1024,
    };
    let mut h = start(MediaTrack::Video, true, config);
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
    h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    if video_ended {
        h.video(Ok(AssembledItem::StreamEnded)).await;
    } else {
        h.video(absent("p1", 1, 2)).await;
    }
    h.audio(init(a, Some("p0"), 0, AUDIO_INIT)).await;
    h.audio(media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)))
        .await;
    h.audio(init(a, Some("p1"), 1, AUDIO_INIT)).await;
    let audio = h.audio.take().unwrap();
    let feeder = tokio::spawn(async move {
        for number in 1..=count {
            audio
                .send(media(
                    a,
                    Some("p1"),
                    1,
                    number,
                    with_tfdt(AUDIO_1, (number - 1) * 48_000),
                ))
                .await
                .unwrap();
        }
        audio
    });
    let mut kinds = Vec::new();
    // Everything of p1 is written while the video track stays silent
    // (or has ended) and audio keeps coming.
    while kinds.iter().filter(|kind| **kind == "media").count() < 1 + count as usize {
        let item = tokio::time::timeout(WAIT, h.out.recv())
            .await
            .expect("audio of the audio-only Period keeps being written")
            .unwrap()
            .unwrap();
        kinds.push(kind(&item));
    }
    let audio = feeder.await.unwrap();
    audio.send(Ok(AssembledItem::StreamEnded)).await.unwrap();
    if !video_ended {
        h.video(Ok(AssembledItem::StreamEnded)).await;
    }
    let ending = tokio::time::timeout(WAIT, h.out.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    kinds.push(kind(&ending));
    let outcome = tokio::time::timeout(WAIT, h.muxer).await.unwrap().unwrap();
    assert_eq!(outcome, MuxOutcome::Ended);
    kinds
}

#[tokio::test]
async fn an_audio_only_period_is_written_completely_while_video_is_silent() {
    let kinds = long_audio_only_period(false, 40).await;
    let audio_only = kinds
        .iter()
        .skip(4)
        .filter(|kind| **kind == "media")
        .count();
    assert_eq!(&kinds[..4], ["init", "media", "epoch", "init"]);
    assert_eq!(audio_only, 40);
    assert_eq!(kinds.last(), Some(&"end"));
}

#[tokio::test]
async fn an_audio_only_period_is_written_completely_after_video_ended() {
    let kinds = long_audio_only_period(true, 40).await;
    let audio_only = kinds
        .iter()
        .skip(4)
        .filter(|kind| **kind == "media")
        .count();
    assert_eq!(&kinds[..4], ["init", "media", "epoch", "init"]);
    assert_eq!(audio_only, 40);
    assert_eq!(kinds.last(), Some(&"end"));
}

#[tokio::test]
async fn absence_notices_cannot_keep_a_session_without_media_alive() {
    let (video, video_rx) = mpsc::channel(64);
    let (out_tx, mut out) = mpsc::channel(64);
    let muxer = tokio::spawn(run_muxer(
        MuxTrack {
            track: MediaTrack::Video,
            rx: video_rx,
            rate_limit_clock: None,
            cancel: CancellationToken::new(),
        },
        None,
        out_tx,
        None,
        lag(WAIT),
        Some(MediaStall {
            timeout: Duration::from_millis(200),
            clock: watch::channel(ThrottleClock::default()).1,
        }),
        CancellationToken::new(),
    ));
    let v = MediaTrack::Video;
    video
        .send(init(v, Some("p0"), 0, VIDEO_INIT))
        .await
        .unwrap();
    video
        .send(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
        .await
        .unwrap();
    // The only other track failed; video keeps reporting it is absent.
    let notices = tokio::spawn(async move {
        for _ in 0..100 {
            if video.send(absent("p1", 1, 2)).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let mut last = None;
    while let Some(item) = tokio::time::timeout(WAIT, out.recv())
        .await
        .expect("the session ends")
    {
        last = Some(item);
    }
    assert!(
        matches!(last, Some(Err(DownloadError::Timeout { .. }))),
        "{last:?}"
    );
    let outcome = tokio::time::timeout(WAIT, muxer).await.unwrap().unwrap();
    assert!(matches!(outcome, MuxOutcome::Failed(_)));
    notices.abort();
}

#[tokio::test(start_paused = true)]
async fn time_blocked_on_a_slow_consumer_is_not_a_media_stall() {
    let (video, video_rx) = mpsc::channel(64);
    // Room for the first group only; the next delivery blocks.
    let (out_tx, mut out) = mpsc::channel(2);
    let stall = Duration::from_secs(60);
    let muxer = tokio::spawn(run_muxer(
        MuxTrack {
            track: MediaTrack::Video,
            rx: video_rx,
            rate_limit_clock: None,
            cancel: CancellationToken::new(),
        },
        None,
        out_tx,
        None,
        lag(WAIT),
        Some(MediaStall {
            timeout: stall,
            clock: watch::channel(ThrottleClock::default()).1,
        }),
        CancellationToken::new(),
    ));
    let v = MediaTrack::Video;
    video
        .send(init(v, Some("p0"), 0, VIDEO_INIT))
        .await
        .unwrap();
    for number in 1..=2 {
        video
            .send(media(
                v,
                Some("p0"),
                0,
                number,
                with_tfdt(VIDEO_1, (number - 1) * 12_288),
            ))
            .await
            .unwrap();
    }
    // The consumer reads nothing for longer than the stall setting.
    tokio::time::sleep(stall + Duration::from_secs(30)).await;
    for expected in ["init", "media", "media"] {
        let item = out.recv().await.unwrap().unwrap();
        assert_eq!(kind(&item), expected);
    }
    // A quiet stretch shorter than the stall setting follows.
    for _ in 0..5 {
        video.send(absent("p1", 1, 3)).await.unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    assert!(out.try_recv().is_err(), "no timeout error");
    assert!(!muxer.is_finished());
    video.send(Ok(AssembledItem::StreamEnded)).await.unwrap();
    assert_eq!(kind(&out.recv().await.unwrap().unwrap()), "end");
    assert_eq!(muxer.await.unwrap(), MuxOutcome::Ended);
}

#[tokio::test]
async fn changed_init_audio_tails_survive_a_period_change() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    h.video(init(v, Some("p0"), 0, VIDEO_INIT)).await;
    h.video(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.audio(rescoped(init(a, Some("p0"), 0, AUDIO_INIT), "a#1"))
        .await;
    h.audio(rescoped(
        media(a, Some("p0"), 0, 1, Bytes::from_static(AUDIO_1)),
        "a#1",
    ))
    .await;
    // p0's audio switches init after the only video segment.
    h.audio(rescoped(
        init(a, Some("p0"), 0, &changed(AUDIO_INIT)),
        "a#2",
    ))
    .await;
    h.audio(rescoped(
        media(a, Some("p0"), 0, 2, with_tfdt(AUDIO_1, 48_000)),
        "a#2",
    ))
    .await;
    h.video(init(v, Some("p1"), 2, &changed(VIDEO_INIT))).await;
    h.video(media(v, Some("p1"), 2, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    h.audio(init(a, Some("p1"), 2, AUDIO_INIT)).await;
    h.audio(media(a, Some("p1"), 2, 1, Bytes::from_static(AUDIO_1)))
        .await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let kinds: Vec<&str> = items.iter().map(kind).collect();
    assert_eq!(
        kinds,
        [
            "init", "media", "epoch", "init", "media", "epoch", "init", "media", "end"
        ]
    );
    let tail = media_data(&items)[1];
    assert!(
        fragments(tail).iter().all(|(_, id)| *id == 2),
        "the a#2 tail"
    );
}

#[tokio::test]
async fn an_audio_tail_under_a_changed_init_is_announced_with_it() {
    let h = start(MediaTrack::Video, true, lag(WAIT));
    let (v, a) = (MediaTrack::Video, MediaTrack::Audio);
    // Same Period and Representation id, but a new init (as the planner
    // issues after a PTO or sample entry change): a new epoch scope.
    h.video(init(v, None, 0, VIDEO_INIT)).await;
    h.video(media(v, None, 0, 1, Bytes::from_static(VIDEO_1)))
        .await;
    h.video(Ok(AssembledItem::StreamEnded)).await;
    h.audio(rescoped(init(a, None, 0, AUDIO_INIT), "a#1")).await;
    h.audio(rescoped(
        media(a, None, 0, 1, Bytes::from_static(AUDIO_1)),
        "a#1",
    ))
    .await;
    h.audio(rescoped(init(a, None, 0, &changed(AUDIO_INIT)), "a#2"))
        .await;
    h.audio(rescoped(
        media(a, None, 0, 2, with_tfdt(AUDIO_1, 48_000)),
        "a#2",
    ))
    .await;
    h.audio(Ok(AssembledItem::StreamEnded)).await;

    let (items, outcome, _) = h.finish().await;
    assert_eq!(outcome, MuxOutcome::Ended);
    let kinds: Vec<&str> = items.iter().map(kind).collect();
    assert_eq!(kinds, ["init", "media", "epoch", "init", "media", "end"]);
    let tail = media_data(&items)[1];
    assert!(fragments(tail).iter().all(|(_, id)| *id == 2));
}

#[tokio::test]
async fn an_unexplained_leader_closure_is_a_pipeline_error() {
    let mut h = start(MediaTrack::Video, false, lag(WAIT));
    h.video(init(MediaTrack::Video, None, 0, VIDEO_INIT)).await;
    drop(h.video);
    let item = tokio::time::timeout(WAIT, h.out.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(item, Err(DownloadError::Internal { .. })));
    let outcome = tokio::time::timeout(WAIT, h.muxer).await.unwrap().unwrap();
    assert!(matches!(outcome, MuxOutcome::Failed(_)));
}

/// A single-track muxer with its cancel token, for tests that end the
/// session themselves.
type OutputRx = mpsc::Receiver<Result<HlsData, DownloadError>>;

fn start_cancellable() -> (
    mpsc::Sender<TrackItem>,
    OutputRx,
    CancellationToken,
    tokio::task::JoinHandle<MuxOutcome>,
) {
    let (video, video_rx) = mpsc::channel(64);
    let (out_tx, out) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let muxer = tokio::spawn(run_muxer(
        MuxTrack {
            track: MediaTrack::Video,
            rx: video_rx,
            rate_limit_clock: None,
            cancel: CancellationToken::new(),
        },
        None,
        out_tx,
        None,
        lag(WAIT),
        None,
        cancel.clone(),
    ));
    (video, out, cancel, muxer)
}

fn kind(item: &HlsData) -> &'static str {
    match item {
        HlsData::M4sData(M4sData::InitSegment(_)) => "init",
        HlsData::M4sData(M4sData::Segment(_)) => "media",
        HlsData::EndMarker(Some(SplitReason::StreamStructureChange { .. })) => "epoch",
        HlsData::EndMarker(Some(SplitReason::EndOfStream)) => "end",
        _ => "other",
    }
}

async fn next_kind(out: &mut OutputRx) -> &'static str {
    let item = tokio::time::timeout(WAIT, out.recv())
        .await
        .expect("muxer output")
        .expect("open output")
        .expect("no stream error");
    kind(&item)
}

#[tokio::test]
async fn an_epoch_without_media_is_never_announced() {
    let v = MediaTrack::Video;
    for cancel_session in [false, true] {
        let (video, mut out, cancel, muxer) = start_cancellable();
        video
            .send(init(v, Some("p0"), 0, VIDEO_INIT))
            .await
            .unwrap();
        video
            .send(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
            .await
            .unwrap();
        assert_eq!(next_kind(&mut out).await, "init");
        assert_eq!(next_kind(&mut out).await, "media");
        // The next Period's init arrives; its media never does.
        video
            .send(init(v, Some("p1"), 1, VIDEO_INIT))
            .await
            .unwrap();
        if cancel_session {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        } else {
            video.send(Ok(AssembledItem::StreamEnded)).await.unwrap();
        }
        let outcome = tokio::time::timeout(WAIT, muxer).await.unwrap().unwrap();
        let mut rest = Vec::new();
        while let Ok(item) = out.try_recv() {
            rest.push(kind(&item.unwrap()));
        }
        if cancel_session {
            assert_eq!(outcome, MuxOutcome::Cancelled);
            assert!(rest.is_empty(), "{rest:?}");
        } else {
            assert_eq!(outcome, MuxOutcome::Ended);
            assert_eq!(rest, ["end"]);
        }
    }
}

#[tokio::test]
async fn an_epoch_is_announced_together_with_its_first_media() {
    let v = MediaTrack::Video;
    let (video, mut out, cancel, muxer) = start_cancellable();
    video
        .send(init(v, Some("p0"), 0, VIDEO_INIT))
        .await
        .unwrap();
    video
        .send(media(v, Some("p0"), 0, 1, Bytes::from_static(VIDEO_1)))
        .await
        .unwrap();
    video
        .send(init(v, Some("p1"), 1, &changed(VIDEO_INIT)))
        .await
        .unwrap();
    video
        .send(media(v, Some("p1"), 1, 1, Bytes::from_static(VIDEO_1)))
        .await
        .unwrap();
    assert_eq!(next_kind(&mut out).await, "init");
    assert_eq!(next_kind(&mut out).await, "media");
    assert_eq!(next_kind(&mut out).await, "epoch");
    // Cancelling right after the barrier cannot strand its init: the
    // whole group was delivered at once.
    cancel.cancel();
    assert_eq!(next_kind(&mut out).await, "init");
    assert_eq!(next_kind(&mut out).await, "media");
    tokio::time::timeout(WAIT, muxer).await.unwrap().unwrap();
}

#[tokio::test]
async fn cancellation_is_not_blocked_by_a_consumer_that_stopped_reading() {
    let (video, video_rx) = mpsc::channel(64);
    // Never read; room for exactly the first group (init + media).
    let (out_tx, _held) = mpsc::channel(2);
    let probe = out_tx.clone();
    let cancel = CancellationToken::new();
    let muxer = tokio::spawn(run_muxer(
        MuxTrack {
            track: MediaTrack::Video,
            rx: video_rx,
            rate_limit_clock: None,
            cancel: CancellationToken::new(),
        },
        None,
        out_tx,
        None,
        lag(WAIT),
        None,
        cancel.clone(),
    ));
    video
        .send(init(MediaTrack::Video, None, 0, VIDEO_INIT))
        .await
        .unwrap();
    for number in 1..=4 {
        video
            .send(media(
                MediaTrack::Video,
                None,
                0,
                number,
                with_tfdt(VIDEO_1, (number - 1) * 12_288),
            ))
            .await
            .unwrap();
    }
    // The output is full and the muxer has taken the second media item,
    // whose delivery it is now blocked on: it reads nothing further.
    tokio::time::timeout(WAIT, async {
        while probe.capacity() != 0 || video.capacity() != 62 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the muxer blocks on a full output");
    cancel.cancel();
    let outcome = tokio::time::timeout(WAIT, muxer)
        .await
        .expect("cancellation finishes the muxer")
        .unwrap();
    assert_eq!(outcome, MuxOutcome::Cancelled);
}

#[tokio::test]
async fn protected_video_fails_with_a_typed_error() {
    let mut protected = VIDEO_INIT.to_vec();
    let at = protected
        .windows(4)
        .position(|window| window == b"avc1")
        .unwrap();
    protected[at..at + 4].copy_from_slice(b"encv");
    let mut h = start(MediaTrack::Video, false, lag(WAIT));
    h.video(init(MediaTrack::Video, None, 0, &protected)).await;

    let item = tokio::time::timeout(WAIT, h.out.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        item,
        Err(DownloadError::Unsupported {
            protocol: "dash",
            ..
        })
    ));
    let outcome = tokio::time::timeout(WAIT, h.muxer).await.unwrap().unwrap();
    assert!(matches!(outcome, MuxOutcome::Failed(_)));
}

#[test]
fn time_shift_moves_media_time_onto_the_presentation_timeline() {
    // Period at 10 s, PTO 2 s in a 1 kHz MPD timescale, 48 kHz media.
    assert_eq!(
        time_shift(Duration::from_secs(10), 2_000, 1_000, 48_000),
        Some(10 * 48_000 - 2 * 48_000)
    );
    assert_eq!(time_shift(Duration::ZERO, 0, 1, 90_000), Some(0));
    assert_eq!(time_shift(Duration::MAX, 0, 1, u32::MAX), None);
}
