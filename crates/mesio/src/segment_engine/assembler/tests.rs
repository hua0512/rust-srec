use super::*;
use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use crate::hls::engine::output::HlsEventSink;
use crate::hls::events::HlsStreamEvent;
use crate::segment_engine::config::GapSkipStrategy;
use crate::segment_engine::descriptor::{SegmentDescriptor, SegmentMeta, SegmentSource};
use crate::segment_engine::identity::SegmentKey;
use bytes::Bytes;
use std::time::Duration;
use url::Url;

fn descriptor(uri: &str, msn: u64, kind: SegmentKind) -> Arc<SegmentDescriptor> {
    Arc::new(SegmentDescriptor {
        key: SegmentKey {
            kind,
            uri: Arc::from(uri),
            byte_range: None,
            scope: None,
        },
        msn,
        source: SegmentSource::Playlist,
        parsed_url: Arc::new(Url::parse(uri).unwrap()),
        discontinuity: false,
        encryption: None,
        init_key: None,
        scramble_seed: None,
        meta: SegmentMeta::Hls(Arc::new(m3u8_rs::MediaSegment {
            uri: uri.to_string(),
            duration: 2.0,
            ..Default::default()
        })),
    })
}

fn ts_payload(msn: u64) -> AssemblerInput {
    AssemblerInput::Payload(SegmentPayload::Ts {
        data: Bytes::from(format!("seg{msn}")),
        descriptor: descriptor(
            &format!("https://e.com/seg{msn}.ts"),
            msn,
            SegmentKind::Media,
        ),
    })
}

fn mp4_media(msn: u64) -> AssemblerInput {
    AssemblerInput::Payload(SegmentPayload::Mp4Media {
        data: Bytes::from(format!("m{msn}")),
        descriptor: descriptor(
            &format!("https://e.com/seg{msn}.m4s"),
            msn,
            SegmentKind::Media,
        ),
    })
}

fn mp4_init(msn: u64) -> AssemblerInput {
    AssemblerInput::Payload(SegmentPayload::Mp4Init {
        data: Bytes::from(format!("init{msn}")),
        descriptor: descriptor(
            &format!("https://e.com/init{msn}.mp4"),
            msn,
            SegmentKind::Init,
        ),
    })
}

fn init_key(uri: &str) -> SegmentKey {
    SegmentKey {
        kind: SegmentKind::Init,
        uri: Arc::from(uri),
        byte_range: None,
        scope: None,
    }
}

/// An init payload plus the key media will reference via `init_key`.
fn mp4_init_keyed(uri: &str, msn: u64) -> (AssemblerInput, SegmentKey) {
    let key = init_key(uri);
    let payload = AssemblerInput::Payload(SegmentPayload::Mp4Init {
        data: Bytes::from(format!("init-{uri}")),
        descriptor: descriptor(uri, msn, SegmentKind::Init),
    });
    (payload, key)
}

/// An fMP4 media payload that depends on the given init key.
fn mp4_media_keyed(msn: u64, init: &SegmentKey) -> AssemblerInput {
    let uri = format!("https://e.com/seg{msn}.m4s");
    let mut d = (*descriptor(&uri, msn, SegmentKind::Media)).clone();
    d.init_key = Some(init.clone());
    AssemblerInput::Payload(SegmentPayload::Mp4Media {
        data: Bytes::from(format!("m{msn}")),
        descriptor: Arc::new(d),
    })
}

struct Harness {
    input_tx: mpsc::Sender<AssemblerInput>,
    event_rx: mpsc::Receiver<Result<HlsStreamEvent, HlsDownloaderError>>,
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<AssemblerExit>,
    holds: Arc<RateLimitHolds>,
}

fn spawn_assembler(mut config: HlsConfig, live: bool, initial_msn: u64) -> Harness {
    config.output_config.live_max_overall_stall_duration = None;
    spawn_with_config(config, live, initial_msn)
}

/// Like [`spawn_assembler`], with the stall deadline `config` sets.
fn spawn_with_config(config: HlsConfig, live: bool, initial_msn: u64) -> Harness {
    let (input_tx, input_rx) = mpsc::channel(64);
    let (event_tx, event_rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let assembler = SequenceAssembler::new(
        Arc::new(SegmentCoreConfig::from(&config)),
        input_rx,
        HlsEventSink::new(event_tx),
        live,
        initial_msn,
        cancel.clone(),
    );
    let holds = assembler.rate_limit_holds();
    let join = tokio::spawn(assembler.run());
    Harness {
        input_tx,
        event_rx,
        cancel,
        join,
        holds,
    }
}

async fn collect_data_uris(h: &mut Harness, n: usize) -> Vec<String> {
    let mut uris = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while uris.len() < n {
            match h.event_rx.recv().await {
                Some(Ok(HlsStreamEvent::Data(data))) => {
                    uris.push(
                        data.media_segment()
                            .map(|s| s.uri.clone())
                            .unwrap_or_default(),
                    );
                }
                Some(_) => {}
                None => break,
            }
        }
    })
    .await
    .expect("timed out collecting data events");
    uris
}

#[tokio::test]
async fn rate_limited_gaps_do_not_skip_the_cursor_or_an_interior_held_segment() {
    for initial_msn in [99, 100] {
        let (_input_tx, input_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let mut assembler = SequenceAssembler::new(
            Arc::new(SegmentCoreConfig::from(&HlsConfig::default())),
            input_rx,
            HlsEventSink::new(event_tx),
            true,
            initial_msn,
            CancellationToken::new(),
        );
        let held = descriptor("https://e.com/seg100.ts", 100, SegmentKind::Media);
        assembler.rate_limit_holds.hold(
            &held,
            Instant::now() + Duration::from_secs(30),
            Instant::now(),
        );
        let AssemblerInput::Payload(later) = ts_payload(101) else {
            unreachable!()
        };
        assembler.handle_payload(later).await;
        assembler.gap_state.as_mut().unwrap().detected_at = Instant::now() - Duration::from_secs(6);
        assembler.try_emit().await;
        assert_eq!(assembler.expected_next_media_sequence, 100);
        if initial_msn == 99 {
            assert!(matches!(
                event_rx.try_recv(),
                Ok(Ok(HlsStreamEvent::GapSkipped {
                    from_sequence: 99,
                    to_sequence: 100,
                    ..
                }))
            ));
        }
        assert!(
            event_rx.try_recv().is_err(),
            "held media must still be pending"
        );

        let AssemblerInput::Payload(recovered) = ts_payload(100) else {
            unreachable!()
        };
        assembler.handle_payload(recovered).await;
        assert_eq!(assembler.expected_next_media_sequence, 102);
        let mut uris = Vec::new();
        while let Ok(Ok(event)) = event_rx.try_recv() {
            match event {
                HlsStreamEvent::Data(data) => {
                    uris.push(data.media_segment().unwrap().uri.clone());
                }
                HlsStreamEvent::GapSkipped { .. } => panic!("recovered media was skipped"),
                _ => {}
            }
        }
        assert_eq!(uris, ["https://e.com/seg100.ts", "https://e.com/seg101.ts"]);
        assert!(
            assembler
                .rate_limit_holds
                .active_until(100, Instant::now())
                .is_none()
        );
    }
}

#[tokio::test]
async fn a_vod_segment_timeout_excludes_its_rate_limit_hold() {
    let mut config = HlsConfig::default();
    config.output_config.vod_segment_timeout = Some(Duration::from_secs(5));
    let (_input_tx, input_rx) = mpsc::channel(16);
    let (event_tx, mut event_rx) = mpsc::channel(16);
    let mut assembler = SequenceAssembler::new(
        Arc::new(SegmentCoreConfig::from(&config)),
        input_rx,
        HlsEventSink::new(event_tx),
        false,
        100,
        CancellationToken::new(),
    );
    // A fresh registry lets the hold and its paused time begin in the
    // past, before the assembler's construction-time scope update.
    assembler.rate_limit_holds = Arc::new(RateLimitHolds::new());
    let held_since = Instant::now() - Duration::from_secs(6);
    let held = descriptor("https://e.com/seg100.ts", 100, SegmentKind::Media);
    assembler
        .rate_limit_holds
        .set_scope(100, None, false, held_since);
    assembler
        .rate_limit_holds
        .hold(&held, held_since + Duration::from_secs(60), held_since);
    let AssemblerInput::Payload(later) = ts_payload(101) else {
        unreachable!()
    };
    assembler.handle_payload(later).await;

    // Held since the gap opened six seconds ago: none of it counts.
    let gap = assembler.gap_state.as_mut().unwrap();
    gap.detected_at = held_since;
    gap.paused_at_detection = Duration::ZERO;
    assembler.try_emit().await;
    assert_eq!(assembler.expected_next_media_sequence, 100);
    assert!(
        event_rx.try_recv().is_err(),
        "held media must still be pending"
    );

    // Once the hold ends, the gap's unpaused wait reaches the timeout.
    assembler
        .rate_limit_holds
        .release(&held.key, 100, Instant::now());
    assembler.gap_state.as_mut().unwrap().detected_at = held_since - Duration::from_secs(5);
    assembler.try_emit().await;
    assert!(matches!(
        event_rx.try_recv(),
        Ok(Ok(HlsStreamEvent::SegmentTimeout {
            sequence_number: 100,
            ..
        }))
    ));
    assert_eq!(assembler.expected_next_media_sequence, 102);
}

#[tokio::test]
async fn a_vod_segment_timeout_stops_before_held_media_inside_the_gap() {
    let mut config = HlsConfig::default();
    config.output_config.vod_segment_timeout = Some(Duration::from_secs(5));
    let (_input_tx, input_rx) = mpsc::channel(16);
    let (event_tx, mut event_rx) = mpsc::channel(16);
    let mut assembler = SequenceAssembler::new(
        Arc::new(SegmentCoreConfig::from(&config)),
        input_rx,
        HlsEventSink::new(event_tx),
        false,
        100,
        CancellationToken::new(),
    );
    let now = Instant::now();
    let held = descriptor("https://e.com/seg102.ts", 102, SegmentKind::Media);
    assembler
        .rate_limit_holds
        .hold(&held, now + Duration::from_secs(60), now);
    let AssemblerInput::Payload(later) = ts_payload(103) else {
        unreachable!()
    };
    assembler.handle_payload(later).await;

    // The cursor itself is not held, so its timeout runs normally.
    assembler.gap_state.as_mut().unwrap().detected_at = now - Duration::from_secs(6);
    assembler.try_emit().await;
    assert!(matches!(
        event_rx.try_recv(),
        Ok(Ok(HlsStreamEvent::SegmentTimeout {
            sequence_number: 100,
            ..
        }))
    ));
    assert_eq!(assembler.expected_next_media_sequence, 102);
    assert!(event_rx.try_recv().is_err());
}

#[tokio::test]
async fn init_rate_limit_holds_cover_startup_and_only_the_required_buffered_init() {
    let (_input_tx, input_rx) = mpsc::channel(16);
    let (event_tx, _event_rx) = mpsc::channel(16);
    let mut assembler = SequenceAssembler::new(
        Arc::new(SegmentCoreConfig::from(&HlsConfig::default())),
        input_rx,
        HlsEventSink::new(event_tx),
        true,
        100,
        CancellationToken::new(),
    );
    let unused = descriptor("https://e.com/unused.mp4", 99, SegmentKind::Init);
    let needed = descriptor("https://e.com/current.mp4", 100, SegmentKind::Init);
    let now = Instant::now();
    let until = now + Duration::from_secs(30);
    assembler.rate_limit_holds.hold(&unused, until, now);
    assembler.refresh_rate_limit_scope(now);
    assert_eq!(
        assembler.rate_limit_holds.clock().active_until(),
        Some(until)
    );
    assembler.has_emitted_media_segment = true;
    assembler.refresh_rate_limit_scope(now);
    assert_eq!(assembler.rate_limit_holds.clock().active_until(), None);

    let AssemblerInput::Payload(payload) = mp4_media_keyed(100, &needed.key) else {
        unreachable!()
    };
    assembler.handle_payload(payload).await;
    assembler.refresh_rate_limit_scope(now);
    assert_eq!(assembler.rate_limit_holds.clock().active_until(), None);
    assembler.rate_limit_holds.hold(&needed, until, now);
    assembler.refresh_rate_limit_scope(now);
    assert_eq!(
        assembler.rate_limit_holds.clock().active_until(),
        Some(until)
    );
    assembler.refresh_rate_limit_scope(until);
    assert_eq!(assembler.rate_limit_holds.clock().active_until(), None);
}

#[tokio::test]
async fn a_live_rate_limit_hold_defers_the_watchdog_but_not_cancellation_or_expiry() {
    for expired in [false, true] {
        let mut config = HlsConfig::default();
        config.output_config.live_max_overall_stall_duration = Some(Duration::from_millis(1));
        let (_input_tx, input_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let mut assembler = SequenceAssembler::new(
            Arc::new(SegmentCoreConfig::from(&config)),
            input_rx,
            HlsEventSink::new(event_tx),
            true,
            100,
            cancel.clone(),
        );
        // The stall clock and the hold start together: any time between
        // them would count against the 1 ms stall budget before the hold
        // could defer it.
        let now = Instant::now();
        assembler.last_input_received_time = Some(now);
        assembler.rate_limit_holds.hold(
            &descriptor("https://e.com/seg100.ts", 100, SegmentKind::Media),
            if expired {
                now
            } else {
                now + Duration::from_secs(30)
            },
            now,
        );
        let join = tokio::spawn(assembler.run());
        if expired {
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
                    .await
                    .unwrap(),
                Some(Err(HlsDownloaderError::Timeout { .. }))
            ));
        } else {
            assert!(
                tokio::time::timeout(Duration::from_millis(20), event_rx.recv())
                    .await
                    .is_err()
            );
            cancel.cancel();
            assert!(event_rx.recv().await.is_none());
        }
        tokio::time::timeout(Duration::from_secs(2), join)
            .await
            .unwrap()
            .unwrap();
    }
}

/// A live assembler that fails after `stall` without liveness.
fn spawn_stalling_assembler(stall: Duration) -> Harness {
    let mut config = HlsConfig::default();
    config.output_config.live_max_overall_stall_duration = Some(stall);
    spawn_with_config(config, true, 0)
}

/// Sends `make()` every 10 ms for as long as the assembler runs.
fn feed_continuously(h: &Harness, make: fn() -> AssemblerInput) {
    let input_tx = h.input_tx.clone();
    tokio::spawn(async move {
        while input_tx.send(make()).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
}

/// Control traffic is not media: a live session that only refreshes its
/// playlist, fails its downloads or loses ranges fails on the stall
/// deadline while those inputs keep arriving.
#[tokio::test]
async fn control_traffic_does_not_keep_a_stalled_live_stream_alive() {
    let control_traffic: [fn() -> AssemblerInput; 3] = [
        || {
            AssemblerInput::Notice(EngineNotice::ManifestRefreshed {
                sequence_base: 0,
                target_duration: 2.0,
            })
        },
        || media_failure(0),
        || AssemblerInput::Skipped {
            from_msn: 0,
            to_msn: 1,
        },
    ];
    for make in control_traffic {
        let mut h = spawn_stalling_assembler(Duration::from_millis(100));
        feed_continuously(&h, make);

        let error = first_error_before_data(&mut h).await;
        assert!(
            matches!(error, Some(DownloadError::Timeout { .. })),
            "{:?} kept the stream alive: {error:?}",
            make()
        );
    }
}

/// A track the section does not carry is absent by design, not stalled
/// (a DASH Period without it).
#[tokio::test]
async fn a_track_absent_by_design_keeps_a_live_stream_alive() {
    let stall = Duration::from_millis(300);
    let mut h = spawn_stalling_assembler(stall);
    feed_continuously(&h, || {
        AssemblerInput::Notice(EngineNotice::TrackAbsent {
            section_id: None,
            section_start: Duration::ZERO,
            after_sequence: 0,
        })
    });

    tokio::time::sleep(stall * 2).await;
    assert!(
        !matches!(h.event_rx.try_recv(), Ok(Err(_))),
        "the stall deadline fired"
    );
    assert!(!h.join.is_finished());
    h.cancel.cancel();
}

#[tokio::test]
async fn terminal_failure_unblocks_instead_of_stalling() {
    let mut config = HlsConfig::default();
    config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
    let mut h = spawn_assembler(config, true, 100);

    h.input_tx.send(ts_payload(101)).await.unwrap();
    // Without this, WaitIndefinitely would stall forever on 100.
    h.input_tx
        .send(terminal_failure(
            SegmentKey {
                kind: SegmentKind::Media,
                uri: Arc::from("https://e.com/seg100.ts"),
                byte_range: None,
                scope: None,
            },
            100,
        ))
        .await
        .unwrap();

    let mut saw_gap = false;
    let mut got = None;
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(evt) = h.event_rx.recv().await {
            match evt {
                Ok(HlsStreamEvent::GapSkipped {
                    reason: GapSkipReason::Upstream,
                    ..
                }) => {
                    saw_gap = true;
                }
                Ok(HlsStreamEvent::Data(d)) => {
                    got = d.media_segment().map(|s| s.uri.clone());
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("timed out");
    assert!(saw_gap);
    assert_eq!(got.as_deref(), Some("https://e.com/seg101.ts"));
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn skipped_range_advances_even_with_empty_buffer() {
    let mut config = HlsConfig::default();
    config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
    let mut h = spawn_assembler(config, true, 100);

    // Window slid: 100..=104 are gone; 105 arrives later.
    h.input_tx
        .send(AssemblerInput::Skipped {
            from_msn: 100,
            to_msn: 104,
        })
        .await
        .unwrap();
    h.input_tx.send(ts_payload(105)).await.unwrap();

    let uris = collect_data_uris(&mut h, 1).await;
    assert_eq!(uris, ["https://e.com/seg105.ts"]);
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn end_drains_in_order_then_stream_ended() {
    let mut h = spawn_assembler(HlsConfig::default(), false, 0);
    h.input_tx.send(ts_payload(1)).await.unwrap();
    h.input_tx.send(ts_payload(0)).await.unwrap();
    h.input_tx.send(ts_payload(2)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();

    let mut uris = Vec::new();
    let mut ended = false;
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(evt) = h.event_rx.recv().await {
            match evt {
                Ok(HlsStreamEvent::Data(d)) => {
                    uris.push(d.media_segment().map(|s| s.uri.clone()).unwrap_or_default());
                }
                Ok(HlsStreamEvent::StreamEnded) => {
                    ended = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("timed out");
    assert_eq!(
        uris,
        [
            "https://e.com/seg0.ts",
            "https://e.com/seg1.ts",
            "https://e.com/seg2.ts"
        ]
    );
    assert!(ended);
    let _ = h.join.await;
}

#[tokio::test]
async fn close_without_end_emits_nothing() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    h.input_tx.send(ts_payload(101)).await.unwrap();
    drop(h.input_tx);
    let _ = h.join.await;
    // Buffered 101 dropped; no StreamEnded, no Data.
    let mut events = Vec::new();
    while let Ok(evt) = h.event_rx.try_recv() {
        events.push(evt);
    }
    assert!(
        events.is_empty(),
        "cancel path must not emit, got {events:?}"
    );
}

#[tokio::test]
async fn fatal_drops_buffer_and_emits_error() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    h.input_tx.send(ts_payload(101)).await.unwrap();
    h.input_tx
        .send(AssemblerInput::Fatal(HlsDownloaderError::Playlist {
            reason: "watcher died".to_string(),
        }))
        .await
        .unwrap();

    let evt = tokio::time::timeout(Duration::from_secs(2), h.event_rx.recv())
        .await
        .expect("event")
        .expect("open");
    assert!(matches!(evt, Err(HlsDownloaderError::Playlist { .. })));
    let next = h.event_rx.recv().await;
    assert!(next.is_none(), "no events after the terminal Err");
    let _ = h.join.await;
}

#[tokio::test]
async fn fmp4_media_gated_until_init_and_init_emitted_first() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    h.input_tx.send(mp4_media(100)).await.unwrap();
    // Nothing should be emitted yet.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.event_rx.try_recv().is_err(), "media must wait for init");

    h.input_tx.send(mp4_init(100)).await.unwrap();
    let mut data = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while data.len() < 2 {
            if let Some(Ok(HlsStreamEvent::Data(d))) = h.event_rx.recv().await {
                data.push(d.is_init_segment());
            }
        }
    })
    .await
    .expect("timed out");
    assert_eq!(data, [true, false], "init must precede media");
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn no_more_inits_unblocks_vod_fmp4_media_with_missing_init_and_fails_empty_stream() {
    let mut h = spawn_assembler(HlsConfig::default(), false, 0);
    let missing_key = init_key("https://e.com/never.mp4");
    h.input_tx
        .send(mp4_media_keyed(0, &missing_key))
        .await
        .unwrap();
    h.input_tx.send(AssemblerInput::NoMoreInits).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();

    let error = first_error_before_data(&mut h).await;
    assert!(
        matches!(
            error,
            Some(DownloadError::InvalidContent {
                protocol: "hls",
                ..
            })
        ),
        "expected InvalidContent when VOD fMP4 has no usable init, got {error:?}"
    );
    assert!(matches!(h.join.await.unwrap(), AssemblerExit::Failed(_)));
}

#[tokio::test]
async fn buffered_media_protects_its_init_from_lru_eviction() {
    let mut config = HlsConfig::default();
    config.output_config.max_pending_init_segments = 1;
    let mut h = spawn_assembler(config, false, 100);
    let (init_a, key_a) = mp4_init_keyed("https://e.com/a.mp4", 99);
    let (init_b, key_b) = mp4_init_keyed("https://e.com/b.mp4", 102);

    // Media 101 (referencing init_a, whose msn 99 < cursor 100) buffers behind missing 100.
    h.input_tx.send(init_a).await.unwrap();
    h.input_tx.send(mp4_media_keyed(101, &key_a)).await.unwrap();
    // init_b arrives while cap == 1; init_a must NOT be evicted because media 101 needs it!
    h.input_tx.send(init_b).await.unwrap();
    h.input_tx
        .send(AssemblerInput::Skipped {
            from_msn: 100,
            to_msn: 100,
        })
        .await
        .unwrap();
    h.input_tx.send(mp4_media_keyed(102, &key_b)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();

    let (uris, _) = collect_until_stream_end(&mut h).await;
    assert_eq!(
        uris,
        [
            "https://e.com/a.mp4",
            "https://e.com/seg101.m4s",
            "https://e.com/b.mp4",
            "https://e.com/seg102.m4s",
        ]
    );
    h.join.await.unwrap();
}

#[tokio::test]
async fn buffer_pressure_forces_gap_skip() {
    let mut config = HlsConfig::default();
    config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
    config.output_config.buffer_limits.max_segments = 2;
    config.output_config.buffer_limits.max_bytes = 0;
    let mut h = spawn_assembler(config, true, 100);
    h.holds.hold(
        &descriptor("https://e.com/seg100.ts", 100, SegmentKind::Media),
        Instant::now() + Duration::from_secs(30),
        Instant::now(),
    );

    // 100 never arrives; the buffer fills to its cap with later segments.
    h.input_tx.send(ts_payload(101)).await.unwrap();
    h.input_tx.send(ts_payload(102)).await.unwrap();

    let mut saw_pressure_skip = false;
    let mut uris = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while uris.len() < 2 {
            match h.event_rx.recv().await {
                Some(Ok(HlsStreamEvent::GapSkipped {
                    reason: GapSkipReason::BufferPressure,
                    ..
                })) => saw_pressure_skip = true,
                Some(Ok(HlsStreamEvent::Data(d))) => {
                    uris.push(d.media_segment().map(|s| s.uri.clone()).unwrap_or_default());
                }
                Some(_) => {}
                None => break,
            }
        }
    })
    .await
    .expect("timed out: buffer-pressure skip did not fire");
    assert!(saw_pressure_skip, "WaitIndefinitely must yield at the cap");
    assert_eq!(uris, ["https://e.com/seg101.ts", "https://e.com/seg102.ts"]);
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn a_waiting_vod_stream_keeps_waiting_at_the_buffer_limit() {
    let mut config = HlsConfig::default();
    config.output_config.vod_gap_strategy = GapSkipStrategy::WaitIndefinitely;
    config.output_config.buffer_limits.max_segments = 2;
    config.output_config.buffer_limits.max_bytes = 0;
    let mut h = spawn_assembler(config, false, 100);

    h.input_tx.send(ts_payload(101)).await.unwrap();
    h.input_tx.send(ts_payload(102)).await.unwrap();
    h.input_tx.send(ts_payload(103)).await.unwrap();
    // Segment 100 is late: nothing may be emitted or skipped meanwhile.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), h.event_rx.recv())
            .await
            .is_err(),
        "the stream must wait for segment 100"
    );

    h.input_tx.send(ts_payload(100)).await.unwrap();
    let mut uris = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while uris.len() < 4 {
            match h.event_rx.recv().await {
                Some(Ok(HlsStreamEvent::Data(d))) => {
                    uris.push(d.media_segment().map(|s| s.uri.clone()).unwrap_or_default());
                }
                Some(Ok(HlsStreamEvent::GapSkipped { .. })) => {
                    panic!("segment 100 must not be given up on")
                }
                Some(_) => {}
                None => break,
            }
        }
    })
    .await
    .expect("timed out waiting for the stream to resume");
    assert_eq!(
        uris,
        [
            "https://e.com/seg100.ts",
            "https://e.com/seg101.ts",
            "https://e.com/seg102.ts",
            "https://e.com/seg103.ts",
        ]
    );
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn buffered_entries_the_cursor_passes_are_dropped_at_once() {
    let mut config = HlsConfig::default();
    config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
    let mut h = spawn_assembler(config, true, 100);

    // 101 and 102 wait behind 100; an upstream skip through 101 makes
    // the buffered 101 stale.
    h.input_tx.send(ts_payload(102)).await.unwrap();
    h.input_tx.send(ts_payload(101)).await.unwrap();
    h.input_tx
        .send(AssemblerInput::Skipped {
            from_msn: 100,
            to_msn: 101,
        })
        .await
        .unwrap();
    let uris = collect_data_uris(&mut h, 1).await;
    assert_eq!(uris, ["https://e.com/seg102.ts"]);

    // 104 is buffered under one URI while another copy of it fails;
    // once 103 is skipped, the cursor passes the dead 104 as well.
    h.input_tx.send(ts_payload(104)).await.unwrap();
    h.input_tx
        .send(terminal_failure(
            SegmentKey {
                kind: SegmentKind::Media,
                uri: Arc::from("https://other.example.com/seg104.ts"),
                byte_range: None,
                scope: None,
            },
            104,
        ))
        .await
        .unwrap();
    h.input_tx
        .send(AssemblerInput::Skipped {
            from_msn: 103,
            to_msn: 103,
        })
        .await
        .unwrap();
    h.input_tx.send(ts_payload(105)).await.unwrap();
    let uris = collect_data_uris(&mut h, 1).await;
    assert_eq!(uris, ["https://e.com/seg105.ts"]);
    h.cancel.cancel();
    // The assembler checks after every emission pass that nothing it
    // buffered lies below the cursor.
    assert!(h.join.await.is_ok());
}

#[tokio::test]
async fn late_payload_for_skipped_msn_is_stale_rejected() {
    let mut config = HlsConfig::default();
    config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
    let mut h = spawn_assembler(config, true, 100);

    h.input_tx
        .send(AssemblerInput::Skipped {
            from_msn: 100,
            to_msn: 100,
        })
        .await
        .unwrap();
    h.input_tx.send(ts_payload(101)).await.unwrap();
    let uris = collect_data_uris(&mut h, 1).await;
    assert_eq!(uris, ["https://e.com/seg101.ts"]);

    // Late completion for the skipped MSN must not be emitted.
    h.input_tx.send(ts_payload(100)).await.unwrap();
    h.input_tx.send(ts_payload(102)).await.unwrap();
    let uris = collect_data_uris(&mut h, 1).await;
    assert_eq!(uris, ["https://e.com/seg102.ts"]);
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn rotated_init_gates_media_until_its_own_init_arrives() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    let (init1, key1) = mp4_init_keyed("https://e.com/init1.mp4", 100);
    let (init2, key2) = mp4_init_keyed("https://e.com/init2.mp4", 101);

    // First init + first media emit normally.
    h.input_tx.send(init1).await.unwrap();
    h.input_tx.send(mp4_media_keyed(100, &key1)).await.unwrap();
    let first = collect_data_uris(&mut h, 2).await; // init1 then media100
    assert_eq!(first[1], "https://e.com/seg100.m4s");

    // Media 101 depends on the rotated init2, which has NOT arrived. Even
    // though an init was already seen, media 101 must be gated.
    h.input_tx.send(mp4_media_keyed(101, &key2)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        h.event_rx.try_recv().is_err(),
        "media must wait for its own rotated init, not emit under the old init"
    );

    // init2 arrives → media 101 unblocks, init2 emitted before it.
    h.input_tx.send(init2).await.unwrap();
    let next = collect_data_uris(&mut h, 2).await;
    assert_eq!(
        next,
        ["https://e.com/init2.mp4", "https://e.com/seg101.m4s"]
    );
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn ts_map_section_precedes_ts_media_as_ts_data() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    let key = init_key("https://e.com/pat-pmt.ts");
    h.input_tx
        .send(AssemblerInput::Payload(SegmentPayload::TsInit {
            data: Bytes::from_static(&[0x47, 0x40, 0x00, 0x10]),
            descriptor: descriptor("https://e.com/pat-pmt.ts", 100, SegmentKind::Init),
        }))
        .await
        .unwrap();
    let mut media = (*descriptor("https://e.com/seg100.ts", 100, SegmentKind::Media)).clone();
    media.init_key = Some(key);
    h.input_tx
        .send(AssemblerInput::Payload(SegmentPayload::Ts {
            data: Bytes::from_static(&[0x47, 0x01, 0x00, 0x10]),
            descriptor: Arc::new(media),
        }))
        .await
        .unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();

    let mut emitted = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = h.event_rx.recv().await {
            match event {
                Ok(HlsStreamEvent::Data(data)) => emitted.push((
                    data.media_segment()
                        .map(|s| s.uri.clone())
                        .unwrap_or_default(),
                    data.segment_type(),
                )),
                Ok(HlsStreamEvent::StreamEnded) => break,
                _ => {}
            }
        }
    })
    .await
    .expect("stream ends");
    assert_eq!(
        emitted,
        [
            ("https://e.com/pat-pmt.ts".to_string(), hls::SegmentType::Ts),
            ("https://e.com/seg100.ts".to_string(), hls::SegmentType::Ts),
        ]
    );
    h.join.await.unwrap();
}

#[tokio::test]
async fn switching_back_to_an_earlier_init_re_emits_it() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    let (init_a, key_a) = mp4_init_keyed("https://e.com/a.mp4", 100);
    let (init_b, key_b) = mp4_init_keyed("https://e.com/b.mp4", 101);
    h.input_tx.send(init_a).await.unwrap();
    h.input_tx.send(mp4_media_keyed(100, &key_a)).await.unwrap();
    h.input_tx.send(init_b).await.unwrap();
    h.input_tx.send(mp4_media_keyed(101, &key_b)).await.unwrap();
    // The store never re-downloads init A; its media must still be
    // preceded by it again, not decoded under B.
    h.input_tx.send(mp4_media_keyed(102, &key_a)).await.unwrap();
    h.input_tx.send(mp4_media_keyed(103, &key_a)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let (uris, _) = collect_until_stream_end(&mut h).await;
    assert_eq!(
        uris,
        [
            "https://e.com/a.mp4",
            "https://e.com/seg100.m4s",
            "https://e.com/b.mp4",
            "https://e.com/seg101.m4s",
            "https://e.com/a.mp4",
            "https://e.com/seg102.m4s",
            "https://e.com/seg103.m4s",
        ]
    );
    h.join.await.unwrap();
}

#[tokio::test]
async fn unreferenced_init_is_not_emitted_before_later_media() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    let (init_a, key_a) = mp4_init_keyed("https://e.com/a.mp4", 100);
    // An ad block's init at MSN 101; its media is skipped upstream.
    let (ad_init, _) = mp4_init_keyed("https://e.com/ad.mp4", 101);
    h.input_tx.send(init_a).await.unwrap();
    h.input_tx.send(mp4_media_keyed(100, &key_a)).await.unwrap();
    h.input_tx.send(ad_init).await.unwrap();
    h.input_tx
        .send(AssemblerInput::Skipped {
            from_msn: 101,
            to_msn: 102,
        })
        .await
        .unwrap();
    h.input_tx.send(mp4_media_keyed(103, &key_a)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let (uris, _) = collect_until_stream_end(&mut h).await;
    assert_eq!(
        uris,
        [
            "https://e.com/a.mp4",
            "https://e.com/seg100.m4s",
            "https://e.com/seg103.m4s",
        ]
    );
    h.join.await.unwrap();
}

#[tokio::test]
async fn active_init_use_keeps_it_ahead_of_unused_inits_in_lru() {
    let mut config = HlsConfig::default();
    config.output_config.max_pending_init_segments = 2;
    let mut h = spawn_assembler(config, true, 100);
    let (init_a, key_a) = mp4_init_keyed("https://e.com/a.mp4", 100);
    let (unused_init, _) = mp4_init_keyed("https://e.com/unused.mp4", 101);
    let (init_c, key_c) = mp4_init_keyed("https://e.com/c.mp4", 102);
    h.input_tx.send(init_a).await.unwrap();
    h.input_tx.send(mp4_media_keyed(100, &key_a)).await.unwrap();
    h.input_tx.send(unused_init).await.unwrap();
    h.input_tx.send(mp4_media_keyed(101, &key_a)).await.unwrap();
    h.input_tx.send(init_c).await.unwrap();
    h.input_tx.send(mp4_media_keyed(102, &key_c)).await.unwrap();
    h.input_tx.send(mp4_media_keyed(103, &key_a)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let (uris, _) = collect_until_stream_end(&mut h).await;
    assert_eq!(
        uris,
        [
            "https://e.com/a.mp4",
            "https://e.com/seg100.m4s",
            "https://e.com/seg101.m4s",
            "https://e.com/c.mp4",
            "https://e.com/seg102.m4s",
            "https://e.com/a.mp4",
            "https://e.com/seg103.m4s",
        ]
    );
    h.join.await.unwrap();
}

#[tokio::test]
async fn inits_fetched_ahead_of_their_media_are_not_evicted_before_use() {
    let mut config = HlsConfig::default();
    config.output_config.max_pending_init_segments = 2;
    let mut h = spawn_assembler(config, false, 100);
    let (init_a, key_a) = mp4_init_keyed("https://e.com/a.mp4", 100);
    let (init_b, key_b) = mp4_init_keyed("https://e.com/b.mp4", 101);
    let (init_c, key_c) = mp4_init_keyed("https://e.com/c.mp4", 102);
    // A finished stream's inits all arrive before the first media.
    h.input_tx.send(init_a).await.unwrap();
    h.input_tx.send(init_b).await.unwrap();
    h.input_tx.send(init_c).await.unwrap();
    h.input_tx.send(mp4_media_keyed(100, &key_a)).await.unwrap();
    h.input_tx.send(mp4_media_keyed(101, &key_b)).await.unwrap();
    h.input_tx.send(mp4_media_keyed(102, &key_c)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let (uris, _) = collect_until_stream_end(&mut h).await;
    assert_eq!(
        uris,
        [
            "https://e.com/a.mp4",
            "https://e.com/seg100.m4s",
            "https://e.com/b.mp4",
            "https://e.com/seg101.m4s",
            "https://e.com/c.mp4",
            "https://e.com/seg102.m4s",
        ]
    );
    h.join.await.unwrap();
}

#[tokio::test]
async fn init_stamped_with_a_later_msn_still_precedes_its_earliest_media() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 10);
    // A retried init fetch was re-planned from a later window and carries
    // MSN 12, but media 10 and 11 depend on it.
    let (init, key) = mp4_init_keyed("https://e.com/init.mp4", 12);
    h.input_tx.send(mp4_media_keyed(10, &key)).await.unwrap();
    h.input_tx.send(mp4_media_keyed(11, &key)).await.unwrap();
    h.input_tx.send(init).await.unwrap();
    h.input_tx.send(mp4_media_keyed(12, &key)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let (uris, _) = collect_until_stream_end(&mut h).await;
    assert_eq!(
        uris,
        [
            "https://e.com/init.mp4",
            "https://e.com/seg10.m4s",
            "https://e.com/seg11.m4s",
            "https://e.com/seg12.m4s",
        ]
    );
    h.join.await.unwrap();
}

/// Data URIs emitted before `StreamEnded`, and whether a discontinuity
/// event was observed.
async fn collect_until_stream_end(h: &mut Harness) -> (Vec<String>, bool) {
    let mut uris = Vec::new();
    let mut discontinuity = false;
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = h.event_rx.recv().await {
            match event {
                Ok(HlsStreamEvent::Data(data)) => uris.push(
                    data.media_segment()
                        .map(|s| s.uri.clone())
                        .unwrap_or_default(),
                ),
                Ok(HlsStreamEvent::DiscontinuityTagEncountered {}) => discontinuity = true,
                Ok(HlsStreamEvent::StreamEnded) => break,
                _ => {}
            }
        }
    })
    .await
    .expect("timed out waiting for StreamEnded");
    (uris, discontinuity)
}

fn http_error(status: u16, uri: &str) -> SharedError {
    SharedError::new(DownloadError::http_status(
        reqwest::StatusCode::from_u16(status).unwrap(),
        uri,
        "segment download",
    ))
}

/// A terminal failure reported as HTTP 404 of the segment itself.
fn terminal_failure(key: SegmentKey, msn: u64) -> AssemblerInput {
    AssemblerInput::TerminalFailed {
        error: http_error(404, &key.uri),
        key,
        msn,
        init_failed: false,
    }
}

/// Media the store did not fetch because its init failed with `error`.
fn undecodable_media(msn: u64, error: &SharedError) -> AssemblerInput {
    AssemblerInput::TerminalFailed {
        key: SegmentKey {
            kind: SegmentKind::Media,
            uri: Arc::from(format!("https://e.com/seg{msn}.m4s")),
            byte_range: None,
            scope: None,
        },
        msn,
        error: error.clone(),
        init_failed: true,
    }
}

fn media_failure(msn: u64) -> AssemblerInput {
    terminal_failure(
        SegmentKey {
            kind: SegmentKind::Media,
            uri: Arc::from(format!("https://e.com/seg{msn}.m4s")),
            byte_range: None,
            scope: None,
        },
        msn,
    )
}

/// The stream's terminal error, when it ends with one before any data.
async fn first_error_before_data(h: &mut Harness) -> Option<DownloadError> {
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = h.event_rx.recv().await {
            match event {
                Err(error) => return Some(error),
                Ok(HlsStreamEvent::Data(_) | HlsStreamEvent::StreamEnded) => return None,
                Ok(_) => {}
            }
        }
        None
    })
    .await
    .expect("timed out waiting for the stream to end")
}

fn failed_url(error: &Option<DownloadError>) -> Option<&str> {
    match error {
        Some(DownloadError::HttpStatus { url, .. }) => Some(url),
        _ => None,
    }
}

#[tokio::test]
async fn a_stream_whose_segments_all_failed_ends_with_the_failure() {
    let mut h = spawn_assembler(HlsConfig::default(), false, 0);
    h.input_tx.send(media_failure(0)).await.unwrap();
    h.input_tx.send(media_failure(1)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let error = first_error_before_data(&mut h).await;
    assert_eq!(failed_url(&error), Some("https://e.com/seg0.m4s"));
    assert!(matches!(h.join.await.unwrap(), AssemblerExit::Failed(_)));
}

#[tokio::test]
async fn an_empty_stream_without_failures_still_ends_cleanly() {
    let mut h = spawn_assembler(HlsConfig::default(), false, 0);
    h.input_tx
        .send(AssemblerInput::Skipped {
            from_msn: 0,
            to_msn: 1,
        })
        .await
        .unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let (uris, _) = collect_until_stream_end(&mut h).await;
    assert!(uris.is_empty());
    assert_eq!(h.join.await.unwrap(), AssemblerExit::Done);
}

#[tokio::test]
async fn a_finished_stream_that_delivered_nothing_reports_the_init_failure() {
    let mut h = spawn_assembler(HlsConfig::default(), false, 0);
    let key = init_key("https://e.com/init.mp4");
    let init_error = http_error(403, "https://e.com/init.mp4");
    h.input_tx.send(media_failure(0)).await.unwrap();
    h.input_tx
        .send(AssemblerInput::TerminalFailed {
            key: key.clone(),
            msn: 0,
            error: init_error.clone(),
            init_failed: false,
        })
        .await
        .unwrap();
    h.input_tx
        .send(undecodable_media(1, &init_error))
        .await
        .unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let error = first_error_before_data(&mut h).await;
    assert_eq!(failed_url(&error), Some("https://e.com/init.mp4"));
    h.join.await.unwrap();
}

#[tokio::test]
async fn a_finished_stream_records_what_follows_media_of_a_failed_init() {
    // A first Period's init fails, its media is not fetched; the next
    // Period records.
    let mut h = spawn_assembler(HlsConfig::default(), false, 0);
    let failed = init_key("https://e.com/p0.mp4");
    let error = http_error(404, "https://e.com/p0.mp4");
    h.input_tx
        .send(AssemblerInput::TerminalFailed {
            key: failed,
            msn: 0,
            error: error.clone(),
            init_failed: false,
        })
        .await
        .unwrap();
    h.input_tx.send(undecodable_media(0, &error)).await.unwrap();
    let (init, key) = mp4_init_keyed("https://e.com/p1.mp4", 1);
    h.input_tx.send(init).await.unwrap();
    h.input_tx.send(mp4_media_keyed(1, &key)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let (uris, _) = collect_until_stream_end(&mut h).await;
    assert_eq!(uris, ["https://e.com/p1.mp4", "https://e.com/seg1.m4s"]);
    h.join.await.unwrap();
}

#[tokio::test]
async fn a_live_stream_starting_with_media_of_a_failed_init_fails_at_once() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    let error = http_error(404, "https://e.com/init.mp4");
    h.input_tx
        .send(AssemblerInput::TerminalFailed {
            key: init_key("https://e.com/init.mp4"),
            msn: 100,
            error: error.clone(),
            init_failed: false,
        })
        .await
        .unwrap();
    // No End follows: the live stream would otherwise skip forever.
    h.input_tx
        .send(undecodable_media(100, &error))
        .await
        .unwrap();
    let error = first_error_before_data(&mut h).await;
    assert_eq!(failed_url(&error), Some("https://e.com/init.mp4"));
    h.join.await.unwrap();
}

#[tokio::test]
async fn a_live_stream_does_not_fail_on_an_init_of_media_it_already_passed() {
    // Media 100 failed on its own and the cursor moved on before its
    // init failed; media 101 has its own, working init.
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    h.input_tx.send(media_failure(100)).await.unwrap();
    h.input_tx
        .send(AssemblerInput::TerminalFailed {
            key: init_key("https://e.com/p0.mp4"),
            msn: 100,
            error: http_error(404, "https://e.com/p0.mp4"),
            init_failed: false,
        })
        .await
        .unwrap();
    let (init, key) = mp4_init_keyed("https://e.com/p1.mp4", 101);
    h.input_tx.send(init).await.unwrap();
    h.input_tx.send(mp4_media_keyed(101, &key)).await.unwrap();
    let uris = collect_data_uris(&mut h, 2).await;
    assert_eq!(uris, ["https://e.com/p1.mp4", "https://e.com/seg101.m4s"]);
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn a_live_stream_whose_first_media_needs_a_failed_map_fails_with_that_map() {
    let mut config = HlsConfig::default();
    config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
    let mut h = spawn_assembler(config, true, 100);
    let first = init_key("https://e.com/a.mp4");
    // A later map fails first, with another error.
    for (key, status) in [(init_key("https://e.com/b.mp4"), 403), (first.clone(), 503)] {
        h.input_tx
            .send(AssemblerInput::TerminalFailed {
                error: http_error(status, &key.uri),
                key,
                msn: 100,
                init_failed: false,
            })
            .await
            .unwrap();
    }
    // An HLS map's container is unknown until its media arrives.
    h.input_tx.send(mp4_media_keyed(100, &first)).await.unwrap();
    let error = first_error_before_data(&mut h).await;
    assert_eq!(failed_url(&error), Some("https://e.com/a.mp4"));
    h.join.await.unwrap();
}

#[tokio::test]
async fn init_whose_media_fails_is_superseded_without_emission() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    let (init1, _) = mp4_init_keyed("https://e.com/init1.mp4", 100);
    let (init2, key2) = mp4_init_keyed("https://e.com/init2.mp4", 101);
    h.input_tx.send(init1).await.unwrap();
    h.input_tx.send(media_failure(100)).await.unwrap();
    h.input_tx.send(init2).await.unwrap();
    h.input_tx.send(mp4_media_keyed(101, &key2)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let (uris, _) = collect_until_stream_end(&mut h).await;
    assert_eq!(
        uris,
        ["https://e.com/init2.mp4", "https://e.com/seg101.m4s"]
    );
    let _ = h.join.await;
}

#[tokio::test]
async fn init_without_media_is_not_emitted_at_stream_end() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    let (init, key) = mp4_init_keyed("https://e.com/init.mp4", 100);
    h.input_tx.send(init).await.unwrap();
    h.input_tx.send(mp4_media_keyed(100, &key)).await.unwrap();
    let (mut next_init, _) = mp4_init_keyed("https://e.com/init2.mp4", 101);
    if let AssemblerInput::Payload(SegmentPayload::Mp4Init { descriptor, .. }) = &mut next_init {
        let mut d = (**descriptor).clone();
        d.discontinuity = true;
        *descriptor = Arc::new(d);
    }
    h.input_tx.send(next_init).await.unwrap();
    h.input_tx.send(media_failure(101)).await.unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();
    let (uris, discontinuity) = collect_until_stream_end(&mut h).await;
    assert_eq!(uris, ["https://e.com/init.mp4", "https://e.com/seg100.m4s"]);
    assert!(
        !discontinuity,
        "a discontinuity is only emitted ahead of the init and media it applies to"
    );
    let _ = h.join.await;
}

#[tokio::test]
async fn an_init_sent_again_before_its_first_emission_keeps_its_discontinuity() {
    let mut h = spawn_assembler(HlsConfig::default(), true, 100);
    let (init, key) = mp4_init_keyed("https://e.com/init.mp4", 100);
    h.input_tx.send(init).await.unwrap();
    h.input_tx.send(mp4_media_keyed(100, &key)).await.unwrap();
    let (mut next_init, next_key) = mp4_init_keyed("https://e.com/init2.mp4", 101);
    let AssemblerInput::Payload(payload) = &mut next_init else {
        unreachable!()
    };
    if let SegmentPayload::Mp4Init { descriptor, .. } = payload {
        let mut d = (**descriptor).clone();
        d.discontinuity = true;
        *descriptor = Arc::new(d);
    }
    // The store sends a completed init again when it admits media 101.
    let sent_again = AssemblerInput::Payload(payload.replayed_at(101));
    h.input_tx.send(next_init).await.unwrap();
    h.input_tx.send(sent_again).await.unwrap();
    // Media 101, which carried the discontinuity as well, is lost.
    h.input_tx.send(media_failure(101)).await.unwrap();
    h.input_tx
        .send(mp4_media_keyed(102, &next_key))
        .await
        .unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();

    let (uris, discontinuity) = collect_until_stream_end(&mut h).await;
    assert_eq!(
        uris,
        [
            "https://e.com/init.mp4",
            "https://e.com/seg100.m4s",
            "https://e.com/init2.mp4",
            "https://e.com/seg102.m4s",
        ]
    );
    assert!(discontinuity);
    let _ = h.join.await;
}

#[tokio::test]
async fn init_terminal_failure_mid_stream_skips_dependent_media_not_stall() {
    let mut config = HlsConfig::default();
    config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
    let mut h = spawn_assembler(config, true, 100);
    let key = init_key("https://e.com/init.mp4");

    h.input_tx.send(ts_payload(100)).await.unwrap();
    // A map rotation terminally fails before its media payload arrives.
    h.input_tx
        .send(terminal_failure(key.clone(), 101))
        .await
        .unwrap();
    // Media depending on the failed init, plus a clear TS that follows.
    h.input_tx.send(mp4_media_keyed(101, &key)).await.unwrap();
    h.input_tx.send(ts_payload(102)).await.unwrap();

    // The dependent media is skipped (visible gap), 102 still emits — no
    // permanent gated stall, no unbounded buffer.
    let mut saw_gap = false;
    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(evt) = h.event_rx.recv().await {
            match evt {
                Ok(HlsStreamEvent::GapSkipped { .. }) => saw_gap = true,
                Ok(HlsStreamEvent::Data(d)) => {
                    got.extend(d.media_segment().map(|s| s.uri.clone()));
                    if got.len() == 2 {
                        break;
                    }
                }
                _ => {}
            }
        }
    })
    .await
    .expect("timed out: init failure stalled the stream");
    assert!(saw_gap, "failed-init media must surface as a gap skip");
    assert_eq!(got, ["https://e.com/seg100.ts", "https://e.com/seg102.ts"]);
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn gated_media_skips_under_live_buffer_pressure() {
    let mut config = HlsConfig::default();
    config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
    config.output_config.buffer_limits.max_segments = 2;
    config.output_config.buffer_limits.max_bytes = 0;
    let mut h = spawn_assembler(config, true, 100);
    let unknown_init = init_key("https://e.com/forgotten-init.mp4");

    h.input_tx
        .send(mp4_media_keyed(100, &unknown_init))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        h.event_rx.try_recv().is_err(),
        "gated media below the buffer limit should still wait for init"
    );

    h.input_tx.send(ts_payload(101)).await.unwrap();

    let mut saw_pressure_skip = false;
    let mut got = None;
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(evt) = h.event_rx.recv().await {
            match evt {
                Ok(HlsStreamEvent::GapSkipped {
                    from_sequence: 100,
                    to_sequence: 101,
                    reason: GapSkipReason::BufferPressure,
                }) => saw_pressure_skip = true,
                Ok(HlsStreamEvent::Data(d)) => {
                    got = d.media_segment().map(|s| s.uri.clone());
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("timed out: gated media stalled under buffer pressure");
    assert!(
        saw_pressure_skip,
        "gated media at the buffer limit must surface as a visible gap"
    );
    assert_eq!(got.as_deref(), Some("https://e.com/seg101.ts"));
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn flush_skips_fmp4_media_with_missing_init_as_visible_gap_after_prior_media() {
    let mut h = spawn_assembler(HlsConfig::default(), false, 99);
    let unknown_init = init_key("https://e.com/missing-init.mp4");

    h.input_tx.send(ts_payload(99)).await.unwrap();
    h.input_tx
        .send(mp4_media_keyed(100, &unknown_init))
        .await
        .unwrap();
    h.input_tx.send(AssemblerInput::End).await.unwrap();

    let mut saw_gap = false;
    let mut emitted_uris = Vec::new();
    let mut ended = false;
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(evt) = h.event_rx.recv().await {
            match evt {
                Ok(HlsStreamEvent::GapSkipped {
                    from_sequence: 100,
                    to_sequence: 101,
                    ..
                }) => saw_gap = true,
                Ok(HlsStreamEvent::Data(d)) => {
                    emitted_uris.push(d.media_segment().unwrap().uri.clone());
                }
                Ok(HlsStreamEvent::StreamEnded) => {
                    ended = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("timed out waiting for flush gap and end");
    assert!(saw_gap, "flush must surface unresolved fMP4 media as a gap");
    assert_eq!(emitted_uris, ["https://e.com/seg99.ts"]);
    assert!(ended, "flush must still end the stream");
    let _ = h.join.await;
}

#[tokio::test]
async fn duplicate_msn_does_not_double_count_gap_skip_threshold() {
    // SkipAfterCount(3): the gap at 100 must only skip once THREE distinct
    // later MSNs have arrived. A duplicate-MSN replacement must not count.
    let mut config = HlsConfig::default();
    config.output_config.live_gap_strategy = GapSkipStrategy::SkipAfterCount(3);
    let mut h = spawn_assembler(config, true, 100);

    // Emit one media first so count-based skipping is enabled (TS streams
    // have no startup suppression, but keep the cursor moving cleanly).
    h.input_tx.send(ts_payload(101)).await.unwrap();
    // Two distinct payloads at MSN 102 (duplicate MSN): only ONE distinct
    // later segment past the gap.
    let dup_a = AssemblerInput::Payload(SegmentPayload::Ts {
        data: Bytes::from_static(b"a"),
        descriptor: descriptor("https://e.com/a.ts", 102, SegmentKind::Media),
    });
    let dup_b = AssemblerInput::Payload(SegmentPayload::Ts {
        data: Bytes::from_static(b"b"),
        descriptor: descriptor("https://e.com/b.ts", 102, SegmentKind::Media),
    });
    h.input_tx.send(dup_a).await.unwrap();
    h.input_tx.send(dup_b).await.unwrap();

    // Two distinct later MSNs (101, 102) have arrived — below the threshold
    // of 3 — so NO gap skip must fire yet. (If the duplicate were counted,
    // segments_since_gap would reach 3 and skip prematurely.)
    let premature = tokio::time::timeout(Duration::from_millis(200), h.event_rx.recv()).await;
    assert!(
        premature.is_err(),
        "duplicate MSN must not trip the count threshold early"
    );

    // A genuine third distinct MSN trips it.
    h.input_tx.send(ts_payload(103)).await.unwrap();
    let saw_skip = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(evt) = h.event_rx.recv().await {
            if matches!(evt, Ok(HlsStreamEvent::GapSkipped { .. })) {
                return true;
            }
        }
        false
    })
    .await
    .expect("timed out");
    assert!(saw_skip, "third distinct segment should trip the threshold");
    h.cancel.cancel();
    let _ = h.join.await;
}

#[tokio::test]
async fn duplicate_msn_insert_does_not_leak_buffer_bytes() {
    // Two distinct payloads at the same MSN (URI changed → new key →
    // store treats as fresh work) must not double-count buffer bytes.
    let mut config = HlsConfig::default();
    config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
    config.output_config.buffer_limits.max_segments = 0;
    config.output_config.buffer_limits.max_bytes = 32;
    let mut h = spawn_assembler(config, true, 100);

    // Both buffer behind the missing 100..; second replaces first at 101.
    let dup_a = AssemblerInput::Payload(SegmentPayload::Ts {
        data: Bytes::from(vec![0u8; 20]),
        descriptor: descriptor("https://e.com/a.ts", 101, SegmentKind::Media),
    });
    let dup_b = AssemblerInput::Payload(SegmentPayload::Ts {
        data: Bytes::from(vec![0u8; 20]),
        descriptor: descriptor("https://e.com/b.ts", 101, SegmentKind::Media),
    });
    h.input_tx.send(dup_a).await.unwrap();
    h.input_tx.send(dup_b).await.unwrap();
    // Now deliver 100; if bytes leaked (40 > 32 cap) a BufferPressure skip
    // would have fired and dropped 100. With correct accounting (20 <= 32),
    // 100 then 101 emit in order.
    h.input_tx.send(ts_payload(100)).await.unwrap();

    let uris = collect_data_uris(&mut h, 2).await;
    assert_eq!(uris[0], "https://e.com/seg100.ts");
    assert_eq!(uris[1], "https://e.com/b.ts");
    h.cancel.cancel();
    let _ = h.join.await;
}

#[test]
fn dead_ranges_merge_and_query() {
    let mut dead = DeadRanges::default();
    dead.insert(10, 12);
    dead.insert(13, 14); // adjacent: merges
    dead.insert(20, 22);
    assert_eq!(dead.run_end(10), Some(14));
    assert_eq!(dead.run_end(14), Some(14));
    assert_eq!(dead.run_end(15), None);
    assert_eq!(dead.run_end(21), Some(22));
    dead.prune_below(15);
    assert_eq!(dead.run_end(10), None);
    assert_eq!(dead.run_end(21), Some(22));
}
