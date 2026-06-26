//! Integration harness for the HLS engine: a scripted mock origin (axum)
//! replaying playlist generations and serving segment bodies with per-path
//! fault injection. Exercises the acceptance criteria end-to-end against the
//! public `HlsStreamEvent` stream — admission, dedup across refreshes, retry
//! pacing, window slides, ENDLIST drain ordering, encryption, and watcher
//! failure semantics.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::Response;
use bytes::Bytes;
use flv::FlvData;
use tokio_util::sync::CancellationToken;

use mesio_engine::flv::FlvProtocolConfig;
use mesio_engine::hls::engine::{self, EngineHandles};
use mesio_engine::hls::{
    GapSkipReason, HlsConfig, HlsDownloaderError, HlsStreamEvent, IdentityPolicyConfig,
};
use mesio_engine::{
    ContentSource, DownloadEvent, DownloadRequest, DownloadTerminal, MesioConfig, MesioDownloader,
    ProtocolSelection, ResourceId,
};

// --- Mock origin ---

#[derive(Default)]
struct FileEntry {
    body: Vec<u8>,
    fail_status: u16,
    fail_times: u32,
}

#[derive(Default)]
struct OriginState {
    playlists: Vec<String>,
    playlist_idx: usize,
    playlist_serves: u32,
    /// After this many successful playlist serves, refreshes fail with 500.
    playlist_fail_after: Option<u32>,
    files: HashMap<String, FileEntry>,
    hits: HashMap<String, u64>,
}

#[derive(Clone, Default)]
struct Origin(Arc<Mutex<OriginState>>);

impl Origin {
    fn new() -> Self {
        Self::default()
    }

    fn push_playlist(&self, body: impl Into<String>) {
        self.0.lock().unwrap().playlists.push(body.into());
    }

    fn add_file(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.0.lock().unwrap().files.insert(
            path.to_string(),
            FileEntry {
                body: body.into(),
                fail_status: 0,
                fail_times: 0,
            },
        );
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

    fn fail_playlist_after(&self, successful_serves: u32) {
        self.0.lock().unwrap().playlist_fail_after = Some(successful_serves);
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

async fn handler(State(origin): State<Origin>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/').to_string();
    let mut state = origin.0.lock().unwrap();
    *state.hits.entry(path.clone()).or_default() += 1;

    let respond = |status: StatusCode, body: Vec<u8>| {
        Response::builder()
            .status(status)
            .body(Body::from(body))
            .expect("response builds")
    };

    if path == "live.m3u8" {
        if let Some(after) = state.playlist_fail_after
            && state.playlist_serves >= after
        {
            return respond(StatusCode::INTERNAL_SERVER_ERROR, Vec::new());
        }
        state.playlist_serves += 1;
        let idx = state.playlist_idx;
        let body = state.playlists.get(idx).cloned().unwrap_or_default();
        if idx + 1 < state.playlists.len() {
            state.playlist_idx += 1;
        }
        return respond(StatusCode::OK, body.into_bytes());
    }

    match state.files.get_mut(&path) {
        Some(entry) => {
            if entry.fail_times > 0 {
                entry.fail_times -= 1;
                let status =
                    StatusCode::from_u16(entry.fail_status).unwrap_or(StatusCode::NOT_FOUND);
                return respond(status, Vec::new());
            }
            let body = entry.body.clone();
            respond(StatusCode::OK, body)
        }
        None => respond(StatusCode::NOT_FOUND, Vec::new()),
    }
}

// --- Helpers ---

/// TARGETDURATION:0 keeps the watcher's refresh cadence at the configured
/// minimum so live tests stay fast.
fn playlist(seq: u64, segments: &[&str], endlist: bool) -> String {
    let mut s = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:0\n#EXT-X-MEDIA-SEQUENCE:{seq}\n"
    );
    for seg in segments {
        s.push_str("#EXTINF:0.5,\n");
        s.push_str(seg);
        s.push('\n');
    }
    if endlist {
        s.push_str("#EXT-X-ENDLIST\n");
    }
    s
}

fn minimal_flv_bytes() -> Vec<u8> {
    let mut bytes = vec![
        b'F', b'L', b'V', 0x01, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00,
    ];
    bytes.extend_from_slice(&[
        0x09, // video tag
        0x00, 0x00, 0x01, // data size
        0x00, 0x00, 0x00, // timestamp
        0x00, // timestamp extended
        0x00, 0x00, 0x00, // stream id
        0x17, // keyframe AVC header byte
        0x00, 0x00, 0x00, 0x0C, // previous tag size
    ]);
    bytes
}

fn fast_config() -> HlsConfig {
    let mut config = HlsConfig::default();
    config.playlist_config.live_refresh_interval = Duration::from_millis(20);
    config.playlist_config.adaptive_refresh_enabled = false;
    config.playlist_config.live_max_refresh_retries = 2;
    config.playlist_config.live_refresh_retry_delay = Duration::from_millis(20);
    config.fetcher_config.segment_retry_delay_base = Duration::from_millis(10);
    config.engine_config.lifecycle_retry_delay_base = Duration::from_millis(20);
    config.engine_config.lifecycle_retry_delay_max = Duration::from_millis(50);
    config.output_config.live_max_overall_stall_duration = Some(Duration::from_secs(10));
    config
}

async fn run_engine(
    base: &str,
    config: HlsConfig,
) -> Vec<Result<HlsStreamEvent, HlsDownloaderError>> {
    let cancel = CancellationToken::new();
    let (mut rx, handles): (_, EngineHandles) = engine::start_standalone(
        format!("{base}/live.m3u8"),
        config,
        None,
        cancel.clone(),
        None,
    )
    .await
    .expect("engine starts");

    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let event = tokio::select! {
            event = rx.recv() => event,
            _ = tokio::time::sleep_until(deadline) => panic!(
                "timed out waiting for stream end; events so far: {events:?}"
            ),
        };
        match event {
            Some(event) => {
                let stop = matches!(event, Ok(HlsStreamEvent::StreamEnded) | Err(_));
                events.push(event);
                if stop {
                    break;
                }
            }
            None => break,
        }
    }
    cancel.cancel();
    let _ = handles.watcher.await;
    let _ = handles.reactor.await;
    let _ = handles.assembler.await;
    events
}

fn data_uris(events: &[Result<HlsStreamEvent, HlsDownloaderError>]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Ok(HlsStreamEvent::Data(data)) => Some(
                data.media_segment()
                    .map(|s| s.uri.clone())
                    .unwrap_or_default(),
            ),
            _ => None,
        })
        .collect()
}

fn ends_with_stream_ended(events: &[Result<HlsStreamEvent, HlsDownloaderError>]) -> bool {
    matches!(events.last(), Some(Ok(HlsStreamEvent::StreamEnded)))
}

fn saw_endlist(events: &[Result<HlsStreamEvent, HlsDownloaderError>]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, Ok(HlsStreamEvent::EndlistEncountered)))
}

// --- Scenarios ---

#[tokio::test(flavor = "multi_thread")]
async fn live_stream_emits_ordered_segments_and_drains_on_endlist() {
    let origin = Origin::new();
    // Overlapping windows: seg1/seg2 reappear across refreshes and must be
    // downloaded exactly once.
    origin.push_playlist(playlist(0, &["seg0.ts", "seg1.ts"], false));
    origin.push_playlist(playlist(1, &["seg1.ts", "seg2.ts"], false));
    origin.push_playlist(playlist(2, &["seg2.ts", "seg3.ts"], true));
    for i in 0..4 {
        origin.add_file(&format!("seg{i}.ts"), format!("payload{i}").into_bytes());
    }

    let base = origin.clone().serve().await;
    let events = run_engine(&base, fast_config()).await;

    let uris = data_uris(&events);
    let expected: Vec<String> = (0..4).map(|i| format!("{base}/seg{i}.ts")).collect();
    assert_eq!(uris, expected, "segments must emit in MSN order");
    assert!(saw_endlist(&events), "EndlistEncountered must be emitted");
    assert!(ends_with_stream_ended(&events));
    for i in 0..4 {
        assert_eq!(
            origin.hits(&format!("seg{i}.ts")),
            1,
            "seg{i} must download exactly once across refreshes"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn vod_playlist_drains_fully_and_ends() {
    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["a.ts", "b.ts", "c.ts"], true));
    origin.add_file("a.ts", b"A".to_vec());
    origin.add_file("b.ts", b"B".to_vec());
    origin.add_file("c.ts", b"C".to_vec());

    let base = origin.clone().serve().await;
    let events = run_engine(&base, fast_config()).await;

    assert_eq!(data_uris(&events).len(), 3);
    assert!(ends_with_stream_ended(&events));
}

#[tokio::test(flavor = "multi_thread")]
async fn rotated_auth_tokens_download_each_segment_once() {
    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts?token=a", "seg1.ts?token=a"], false));
    origin.push_playlist(playlist(
        0,
        &["seg0.ts?token=b", "seg1.ts?token=b", "seg2.ts?token=b"],
        false,
    ));
    origin.push_playlist(playlist(1, &["seg1.ts?token=c", "seg2.ts?token=c"], true));
    for i in 0..3 {
        origin.add_file(&format!("seg{i}.ts"), format!("payload{i}").into_bytes());
    }

    let mut config = fast_config();
    config.engine_config.identity_policy =
        IdentityPolicyConfig::StripQueryKeys(vec!["token".to_string()]);

    let base = origin.clone().serve().await;
    let events = run_engine(&base, config).await;

    assert_eq!(data_uris(&events).len(), 3);
    assert!(ends_with_stream_ended(&events));
    for i in 0..3 {
        assert_eq!(
            origin.hits(&format!("seg{i}.ts")),
            1,
            "rotated token must not re-download seg{i}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn transient_404_is_rescheduled_until_success() {
    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts", "seg1.ts"], false));
    origin.push_playlist(playlist(0, &["seg0.ts", "seg1.ts", "seg2.ts"], true));
    origin.add_file("seg0.ts", b"zero".to_vec());
    // CDN 404s twice for a segment that appears shortly after.
    origin.add_file_failing("seg1.ts", b"one".to_vec(), 404, 2);
    origin.add_file("seg2.ts", b"two".to_vec());

    let base = origin.clone().serve().await;
    let events = run_engine(&base, fast_config()).await;

    let uris = data_uris(&events);
    assert_eq!(
        uris,
        vec![
            format!("{base}/seg0.ts"),
            format!("{base}/seg1.ts"),
            format!("{base}/seg2.ts"),
        ],
        "late success must still emit in order"
    );
    assert_eq!(origin.hits("seg1.ts"), 3, "two 404s then one success");
    assert!(ends_with_stream_ended(&events));
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_failure_skips_segment_instead_of_stalling() {
    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts", "seg1.ts", "seg2.ts"], true));
    origin.add_file("seg0.ts", b"zero".to_vec());
    // seg1.ts is never served: 404 until the lifecycle budget exhausts.
    origin.add_file("seg2.ts", b"two".to_vec());

    let mut config = fast_config();
    config.engine_config.lifecycle_retry_budget = 1;

    let base = origin.clone().serve().await;
    let events = run_engine(&base, config).await;

    let uris = data_uris(&events);
    assert_eq!(
        uris,
        vec![format!("{base}/seg0.ts"), format!("{base}/seg2.ts")],
        "the dead MSN must be skipped, not waited on"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            Ok(HlsStreamEvent::GapSkipped {
                reason: GapSkipReason::Upstream,
                ..
            })
        )),
        "terminal failure must surface as an upstream gap skip"
    );
    assert_eq!(
        origin.hits("seg1.ts"),
        2,
        "initial attempt + one lifecycle retry, then terminal"
    );
    assert!(ends_with_stream_ended(&events));
}

#[tokio::test(flavor = "multi_thread")]
async fn window_slide_surfaces_explicit_skip() {
    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts", "seg1.ts"], false));
    // The window jumps from MSN 2 to MSN 5: 2..=4 were never observable.
    origin.push_playlist(playlist(5, &["seg5.ts", "seg6.ts"], true));
    for i in [0u64, 1, 5, 6] {
        origin.add_file(&format!("seg{i}.ts"), format!("payload{i}").into_bytes());
    }

    let base = origin.clone().serve().await;
    let events = run_engine(&base, fast_config()).await;

    let uris = data_uris(&events);
    assert_eq!(
        uris,
        vec![
            format!("{base}/seg0.ts"),
            format!("{base}/seg1.ts"),
            format!("{base}/seg5.ts"),
            format!("{base}/seg6.ts"),
        ]
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            Ok(HlsStreamEvent::GapSkipped {
                from_sequence: 2,
                to_sequence: 5,
                reason: GapSkipReason::Upstream,
            })
        )),
        "window slide must surface as an explicit skip, got {events:?}"
    );
    assert!(ends_with_stream_ended(&events));
}

#[tokio::test(flavor = "multi_thread")]
async fn encrypted_stream_decrypts_with_single_key_fetch() {
    use aes::Aes128;
    use cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
    type Aes128CbcEnc = cbc::Encryptor<Aes128>;

    let key = [0x42u8; 16];
    let iv = [0x13u8; 16];
    let encrypt = |plaintext: &[u8]| -> Vec<u8> {
        let cipher = Aes128CbcEnc::new_from_slices(&key, &iv).unwrap();
        let padded_len = ((plaintext.len() / 16) + 1) * 16;
        let mut buffer = vec![0u8; padded_len];
        buffer[..plaintext.len()].copy_from_slice(plaintext);
        cipher
            .encrypt_padded::<Pkcs7>(&mut buffer, plaintext.len())
            .unwrap()
            .to_vec()
    };

    let origin = Origin::new();
    let key_line =
        "#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x13131313131313131313131313131313\n";
    let mut body = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:0\n#EXT-X-MEDIA-SEQUENCE:0\n{key_line}"
    );
    for i in 0..3 {
        body.push_str(&format!("#EXTINF:0.5,\nseg{i}.ts\n"));
    }
    body.push_str("#EXT-X-ENDLIST\n");
    origin.push_playlist(body);
    origin.add_file("key.bin", key.to_vec());
    for i in 0..3 {
        origin.add_file(
            &format!("seg{i}.ts"),
            encrypt(format!("clear-payload-{i}").as_bytes()),
        );
    }

    let base = origin.clone().serve().await;
    let events = run_engine(&base, fast_config()).await;

    let payloads: Vec<Bytes> = events
        .iter()
        .filter_map(|e| match e {
            Ok(HlsStreamEvent::Data(data)) => data.data().cloned(),
            _ => None,
        })
        .collect();
    assert_eq!(payloads.len(), 3);
    for (i, payload) in payloads.iter().enumerate() {
        assert_eq!(
            payload.as_ref(),
            format!("clear-payload-{i}").as_bytes(),
            "segment {i} must decrypt to the original plaintext"
        );
    }
    assert_eq!(
        origin.hits("key.bin"),
        1,
        "concurrent segments must share one key fetch (single-flight + cache)"
    );
    assert!(ends_with_stream_ended(&events));
}

#[tokio::test(flavor = "multi_thread")]
async fn byterange_segments_emit_requested_slices_when_origin_ignores_range() {
    let origin = Origin::new();
    let body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:0\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:0.5,\n#EXT-X-BYTERANGE:4@2\nfile.ts\n#EXTINF:0.5,\n#EXT-X-BYTERANGE:3\nfile.ts\n#EXT-X-ENDLIST\n";
    origin.push_playlist(body);
    origin.add_file("file.ts", b"ABCDEFGHIJ".to_vec());

    let base = origin.clone().serve().await;
    let events = run_engine(&base, fast_config()).await;

    let payloads: Vec<Bytes> = events
        .iter()
        .filter_map(|e| match e {
            Ok(HlsStreamEvent::Data(data)) => data.data().cloned(),
            _ => None,
        })
        .collect();
    assert_eq!(
        payloads,
        vec![Bytes::from_static(b"CDEF"), Bytes::from_static(b"GHI")]
    );
    assert!(ends_with_stream_ended(&events));
}

#[tokio::test(flavor = "multi_thread")]
async fn watcher_failure_terminates_with_error_not_clean_end() {
    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts"], false));
    origin.add_file("seg0.ts", b"zero".to_vec());
    // Initial load succeeds; every refresh after that fails until the
    // watcher exhausts its retries.
    origin.fail_playlist_after(1);

    let base = origin.clone().serve().await;
    let events = run_engine(&base, fast_config()).await;

    assert!(
        matches!(events.last(), Some(Err(_))),
        "watcher failure must surface as a terminal Err, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Ok(HlsStreamEvent::StreamEnded))),
        "a watcher failure must never masquerade as a clean StreamEnded"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn fmp4_init_segment_emits_before_media() {
    let origin = Origin::new();
    let body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:0\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:0.5,\nseg0.m4s\n#EXTINF:0.5,\nseg1.m4s\n#EXT-X-ENDLIST\n";
    origin.push_playlist(body);
    origin.add_file("init.mp4", b"init-bytes".to_vec());
    origin.add_file("seg0.m4s", b"media0".to_vec());
    origin.add_file("seg1.m4s", b"media1".to_vec());

    let base = origin.clone().serve().await;
    let events = run_engine(&base, fast_config()).await;

    let data: Vec<bool> = events
        .iter()
        .filter_map(|e| match e {
            Ok(HlsStreamEvent::Data(d)) => Some(d.is_init_segment()),
            _ => None,
        })
        .collect();
    assert_eq!(
        data,
        vec![true, false, false],
        "init must be emitted before any fMP4 media"
    );
    assert_eq!(origin.hits("init.mp4"), 1);
    assert!(ends_with_stream_ended(&events));
}

#[tokio::test(flavor = "multi_thread")]
async fn extensionless_fmp4_uses_map_semantics_not_url_suffix() {
    let origin = Origin::new();
    let body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:0\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"init\"\n#EXTINF:0.5,\nseg0\n#EXT-X-ENDLIST\n";
    origin.push_playlist(body);
    origin.add_file("init", b"init-bytes".to_vec());
    origin.add_file("seg0", b"media0".to_vec());

    let base = origin.clone().serve().await;
    let events = run_engine(&base, fast_config()).await;

    let segment_types: Vec<hls::SegmentType> = events
        .iter()
        .filter_map(|e| match e {
            Ok(HlsStreamEvent::Data(d)) => Some(d.segment_type()),
            _ => None,
        })
        .collect();
    assert_eq!(
        segment_types,
        vec![hls::SegmentType::M4sInit, hls::SegmentType::M4sMedia],
        "EXT-X-MAP, not filename suffix, determines fMP4 payload shape"
    );
    assert!(ends_with_stream_ended(&events));
}

#[tokio::test(flavor = "multi_thread")]
async fn fmp4_init_terminal_failure_skips_dependent_media() {
    let origin = Origin::new();
    let body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:0\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:0.5,\nseg0.m4s\n#EXTINF:0.5,\nseg1.m4s\n#EXT-X-ENDLIST\n";
    origin.push_playlist(body);
    // init.mp4 is never served (persistent 404) → terminal after the budget.
    origin.add_file_failing("init.mp4", b"x".to_vec(), 404, u32::MAX);
    origin.add_file("seg0.m4s", b"media0".to_vec());
    origin.add_file("seg1.m4s", b"media1".to_vec());

    let mut config = fast_config();
    config.engine_config.lifecycle_retry_budget = 1;

    let base = origin.clone().serve().await;
    let events = run_engine(&base, config).await;

    // No media can be decoded without the init; the stream must end cleanly
    // (skips, no data) rather than hang.
    assert!(data_uris(&events).is_empty(), "no media is decodable");
    assert!(
        ends_with_stream_ended(&events),
        "stream must terminate, not stall: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn media_sequence_reset_terminates_with_error() {
    let origin = Origin::new();
    origin.push_playlist(playlist(1000, &["s1000.ts"], false));
    // A drastic backwards jump in MEDIA-SEQUENCE: a stream restart.
    origin.push_playlist(playlist(0, &["r0.ts", "r1.ts"], false));
    origin.add_file("s1000.ts", b"a".to_vec());
    origin.add_file("r0.ts", b"b".to_vec());
    origin.add_file("r1.ts", b"c".to_vec());

    let base = origin.clone().serve().await;
    let events = run_engine(&base, fast_config()).await;

    assert!(
        matches!(events.last(), Some(Err(_))),
        "an unrecoverable MSN reset must surface as a terminal error, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Ok(HlsStreamEvent::StreamEnded))),
        "a reset is not a clean end"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn public_downloader_api_streams_data_and_end_marker() {
    use futures::StreamExt;
    use mesio_engine::hls::HlsDownloader;

    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts", "seg1.ts"], true));
    origin.add_file("seg0.ts", b"zero".to_vec());
    origin.add_file("seg1.ts", b"one".to_vec());
    let base = origin.clone().serve().await;

    let downloader = HlsDownloader::new(fast_config()).expect("downloader builds");
    let request = DownloadRequest::from_url(&format!("{base}/live.m3u8"))
        .expect("valid URL")
        .with_protocol(ProtocolSelection::Hls(Default::default()))
        .with_cancel(CancellationToken::new());
    let mut stream = downloader
        .start_session(request)
        .await
        .expect("download starts")
        .items;

    let mut segment_types = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(15), stream.next())
        .await
        .expect("stream item")
    {
        let data = item.expect("no stream error");
        segment_types.push(data.segment_type());
    }
    assert_eq!(
        segment_types,
        vec![
            hls::SegmentType::Ts,
            hls::SegmentType::Ts,
            hls::SegmentType::EndMarker,
        ],
        "public API EOF marker must be emitted after all media data"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_public_downloader_stream_cancels_live_engine() {
    use futures::StreamExt;
    use mesio_engine::hls::HlsDownloader;

    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts"], false));
    origin.add_file("seg0.ts", b"zero".to_vec());
    let base = origin.clone().serve().await;

    let downloader = HlsDownloader::new(fast_config()).expect("downloader builds");
    let request = DownloadRequest::from_url(&format!("{base}/live.m3u8"))
        .expect("valid URL")
        .with_protocol(ProtocolSelection::Hls(Default::default()))
        .with_cancel(CancellationToken::new());
    let mut stream = downloader
        .start_session(request)
        .await
        .expect("download starts")
        .items;

    let first = tokio::time::timeout(Duration::from_secs(15), stream.next())
        .await
        .expect("first stream item")
        .expect("stream item")
        .expect("no stream error");
    assert_eq!(first.segment_type(), hls::SegmentType::Ts);

    drop(stream);
    tokio::time::sleep(Duration::from_millis(80)).await;
    let settled_hits = origin.hits("live.m3u8");
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(
        origin.hits("live.m3u8"),
        settled_hits,
        "dropping the public stream must stop live playlist polling"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn session_events_observe_hls_segment_downloads() {
    use futures::StreamExt;
    use mesio_engine::hls::HlsDownloader;

    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts"], true));
    origin.add_file("seg0.ts", b"segment-session-progress".to_vec());
    let base = origin.clone().serve().await;

    let downloader = HlsDownloader::new(fast_config()).expect("downloader builds");
    let request = DownloadRequest::from_url(&format!("{base}/live.m3u8"))
        .expect("valid URL")
        .with_protocol(ProtocolSelection::Hls(Default::default()));
    let session = downloader
        .start_session(request)
        .await
        .expect("session starts");
    let mut items = session.items;
    let mut events = session.events;

    let mut seen_items = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(15), items.next())
        .await
        .expect("stream item")
    {
        let data = item.expect("no stream error");
        seen_items.push(data.segment_type());
        if data.segment_type() == hls::SegmentType::EndMarker {
            break;
        }
    }

    let mut captured = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        captured.push(event);
    }

    assert!(
        seen_items.contains(&hls::SegmentType::Ts),
        "session should yield media item before event assertions"
    );
    assert!(
        captured.iter().any(|event| matches!(
            event,
            DownloadEvent::ResourceStarted {
                resource: ResourceId::HlsSegment { .. },
                ..
            }
        )),
        "expected segment resource-start event, got {captured:?}"
    );
    assert!(
        captured.iter().any(|event| matches!(
            event,
            DownloadEvent::Progress {
                resource: ResourceId::HlsSegment { .. },
                bytes_delta,
                ..
            } if *bytes_delta > 0
        )),
        "expected segment progress event, got {captured:?}"
    );
    assert!(
        captured.iter().any(|event| matches!(
            event,
            DownloadEvent::ResourceFinished {
                resource: ResourceId::HlsSegment { .. },
                bytes,
                from_cache: false,
            } if *bytes == b"segment-session-progress".len() as u64
        )),
        "expected segment resource-finished event, got {captured:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mesio_downloader_hls_sources_fail_over_with_discontinuity() {
    use futures::StreamExt;

    let first_origin = Origin::new();
    first_origin.push_playlist(playlist(0, &["first0.ts"], false));
    first_origin.add_file("first0.ts", b"first".to_vec());
    first_origin.fail_playlist_after(1);
    let first_base = first_origin.clone().serve().await;

    let second_origin = Origin::new();
    second_origin.push_playlist(playlist(0, &["second0.ts"], true));
    second_origin.add_file("second0.ts", b"second".to_vec());
    let second_base = second_origin.clone().serve().await;

    let downloader = MesioDownloader::new(MesioConfig {
        hls: fast_config(),
        ..Default::default()
    });
    let request = DownloadRequest::from_url(&format!("{first_base}/live.m3u8"))
        .expect("valid URL")
        .with_protocol(ProtocolSelection::Hls(Default::default()))
        .add_source(ContentSource::new(format!("{first_base}/live.m3u8"), 0))
        .add_source(ContentSource::new(format!("{second_base}/live.m3u8"), 1));
    let session = downloader
        .start_hls(request)
        .await
        .expect("source session starts");
    let mut items = session.items;
    let mut events = session.events;

    let mut item_types = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(15), items.next())
        .await
        .expect("stream item")
    {
        let data = item.expect("no stream error");
        item_types.push(data.segment_type());
        if matches!(
            data,
            hls::HlsData::EndMarker(Some(hls::SplitReason::EndOfStream))
        ) {
            break;
        }
    }

    let mut selected = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let DownloadEvent::SourceSelected { url, attempt, .. } = event {
            selected.push((url.to_string(), attempt));
        }
    }

    assert_eq!(
        item_types,
        vec![
            hls::SegmentType::Ts,
            hls::SegmentType::EndMarker,
            hls::SegmentType::Ts,
            hls::SegmentType::EndMarker,
        ],
        "HLS failover should synthesize a discontinuity before the next source"
    );
    assert_eq!(
        selected,
        vec![
            (format!("{first_base}/live.m3u8"), 1),
            (format!("{second_base}/live.m3u8"), 2),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mesio_downloader_hls_source_exhaustion_does_not_emit_extra_discontinuity() {
    use futures::StreamExt;

    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts"], false));
    origin.add_file("seg0.ts", b"zero".to_vec());
    origin.fail_playlist_after(1);
    let base = origin.clone().serve().await;

    let downloader = MesioDownloader::new(MesioConfig {
        hls: fast_config(),
        ..Default::default()
    });
    let request = DownloadRequest::from_url(&format!("{base}/live.m3u8"))
        .expect("valid URL")
        .with_protocol(ProtocolSelection::Hls(Default::default()))
        .add_source(ContentSource::new(format!("{base}/live.m3u8"), 0));
    let session = downloader
        .start_hls(request)
        .await
        .expect("source session starts");
    let mut items = session.items;

    let mut item_types = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(15), items.next())
        .await
        .expect("stream item")
    {
        match item {
            Ok(data) => item_types.push(data.segment_type()),
            Err(_) => break,
        }
    }

    assert_eq!(
        item_types,
        vec![hls::SegmentType::Ts],
        "a discontinuity marker is only valid between two source sessions"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mesio_downloader_flv_source_selection_emits_event() {
    use futures::StreamExt;

    let origin = Origin::new();
    origin.add_file(
        "stream.flv",
        vec![
            b'F', b'L', b'V', 0x01, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00,
        ],
    );
    let base = origin.clone().serve().await;

    let downloader = MesioDownloader::new(MesioConfig::default());
    let request = DownloadRequest::from_url(&format!("{base}/ignored.flv"))
        .expect("valid URL")
        .with_protocol(ProtocolSelection::Flv(Default::default()))
        .add_source(ContentSource::new(format!("{base}/stream.flv"), 7));
    let session = downloader
        .start_flv(request)
        .await
        .expect("FLV session starts from selected source");
    let mut events = session.events;

    let mut selected = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let DownloadEvent::SourceSelected {
            url,
            priority,
            attempt,
        } = event
        {
            selected = Some((url.to_string(), priority, attempt));
            break;
        }
    }

    assert_eq!(selected, Some((format!("{base}/stream.flv"), 7, 1)));
}

#[tokio::test(flavor = "multi_thread")]
async fn mesio_downloader_rejects_unimplemented_flv_reconnect_modes() {
    let downloader = MesioDownloader::new(MesioConfig::default());
    let request = DownloadRequest::from_url("http://127.0.0.1:9/stream.flv")
        .expect("valid URL")
        .with_protocol(ProtocolSelection::Flv(mesio_engine::FlvRequestOptions {
            reconnect: mesio_engine::FlvReconnect::ReconnectSameSourceWithDiscontinuity,
        }));

    let err = match downloader.start_flv(request).await {
        Ok(_) => panic!("unsupported reconnect mode must be rejected before network I/O"),
        Err(err) => err,
    };

    assert!(matches!(
        err,
        mesio_engine::DownloadError::Configuration { reason }
            if reason.contains("FLV reconnect mode")
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn mesio_downloader_rejects_flv_reconnect_from_request_options() {
    let downloader = MesioDownloader::new(MesioConfig::default());
    let mut request =
        DownloadRequest::from_url("http://127.0.0.1:9/stream.flv").expect("valid URL");
    request.options.flv.reconnect = mesio_engine::FlvReconnect::SwitchSourceWithDiscontinuity;

    let err = match downloader.start_flv(request).await {
        Ok(_) => panic!("unsupported reconnect mode must be rejected before network I/O"),
        Err(err) => err,
    };

    assert!(matches!(
        err,
        mesio_engine::DownloadError::Configuration { reason }
            if reason.contains("FLV reconnect mode")
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn hls_progress_zero_threshold_emits_for_segment_chunk() {
    use futures::StreamExt;
    use mesio_engine::hls::HlsDownloader;

    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts"], true));
    origin.add_file("seg0.ts", b"chunk-progress".to_vec());
    let base = origin.clone().serve().await;

    let mut config = fast_config();
    config.fetcher_config.progress_emit_min_bytes = 0;
    config.fetcher_config.progress_emit_min_interval = Duration::ZERO;
    let downloader = HlsDownloader::new(config).expect("downloader builds");
    let request = DownloadRequest::from_url(&format!("{base}/live.m3u8"))
        .expect("valid URL")
        .with_protocol(ProtocolSelection::Hls(Default::default()));
    let session = downloader
        .start_session(request)
        .await
        .expect("download starts");
    let mut items = session.items;
    let mut events = session.events;

    while let Some(item) = tokio::time::timeout(Duration::from_secs(15), items.next())
        .await
        .expect("stream item")
    {
        if matches!(
            item.expect("no stream error"),
            hls::HlsData::EndMarker(Some(hls::SplitReason::EndOfStream))
        ) {
            break;
        }
    }

    let mut saw_progress = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let DownloadEvent::Progress {
            resource: ResourceId::HlsSegment { .. },
            bytes_delta,
            bytes_total,
        } = event
        {
            saw_progress = bytes_delta == b"chunk-progress".len() as u64
                && bytes_total == b"chunk-progress".len() as u64;
            if saw_progress {
                break;
            }
        }
    }

    assert!(saw_progress, "expected per-chunk HLS progress event");
}

#[tokio::test(flavor = "multi_thread")]
async fn flv_progress_zero_threshold_emits_for_stream_chunk() {
    use futures::StreamExt;
    use mesio_engine::flv::FlvDownloader;

    let origin = Origin::new();
    let body = minimal_flv_bytes();
    let body_len = body.len() as u64;
    origin.add_file("stream.flv", body);
    let base = origin.clone().serve().await;

    let config = FlvProtocolConfig {
        progress_emit_min_bytes: 0,
        progress_emit_min_interval: Duration::ZERO,
        ..Default::default()
    };
    let downloader = FlvDownloader::with_config(config).expect("downloader builds");
    let request = DownloadRequest::from_url(&format!("{base}/stream.flv"))
        .expect("valid URL")
        .with_protocol(ProtocolSelection::Flv(Default::default()));
    let session = downloader
        .start_session(request)
        .await
        .expect("download starts");
    let mut items = session.items;
    let mut events = session.events;

    while let Some(item) = tokio::time::timeout(Duration::from_secs(15), items.next())
        .await
        .expect("stream item")
    {
        if matches!(item.expect("no stream error"), FlvData::Tag(_)) {
            break;
        }
    }

    let mut saw_progress = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let DownloadEvent::Progress {
            resource: ResourceId::FlvStream { .. },
            bytes_delta,
            bytes_total,
        } = event
        {
            saw_progress = bytes_delta == body_len && bytes_total == body_len;
            if saw_progress {
                break;
            }
        }
    }

    assert!(saw_progress, "expected per-chunk FLV progress event");
}

#[tokio::test(flavor = "multi_thread")]
async fn hls_download_handle_join_reports_authoritative_end() {
    use futures::StreamExt;
    use mesio_engine::hls::HlsDownloader;

    let origin = Origin::new();
    origin.push_playlist(playlist(0, &["seg0.ts"], true));
    origin.add_file("seg0.ts", b"zero".to_vec());
    let base = origin.clone().serve().await;

    let downloader = HlsDownloader::new(fast_config()).expect("downloader builds");
    let request = DownloadRequest::from_url(&format!("{base}/live.m3u8"))
        .expect("valid URL")
        .with_protocol(ProtocolSelection::Hls(Default::default()));
    let session = downloader
        .start_session(request)
        .await
        .expect("download starts");
    let handle = session.handle.clone();
    let mut stream = session.items;

    while let Some(item) = tokio::time::timeout(Duration::from_secs(15), stream.next())
        .await
        .expect("stream item")
    {
        if matches!(
            item.expect("no stream error"),
            hls::HlsData::EndMarker(Some(hls::SplitReason::EndOfStream))
        ) {
            break;
        }
    }

    assert_eq!(
        handle.join().await.expect("join handle").expect("join ok"),
        DownloadTerminal::AuthoritativeEnd
    );
}
