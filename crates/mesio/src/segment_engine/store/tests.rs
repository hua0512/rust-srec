use super::*;
use crate::segment_engine::descriptor::SegmentMeta;
use crate::segment_engine::identity::ByteRangeKey;
use bytes::Bytes;
use url::Url;

fn budget_unlimited() -> ByteBudget {
    ByteBudget::new(0, 0)
}

fn descriptor(uri: &str, msn: u64, kind: SegmentKind) -> SegmentDescriptor {
    descriptor_with_source(uri, msn, kind, SegmentSource::Playlist)
}

fn descriptor_with_source(
    uri: &str,
    msn: u64,
    kind: SegmentKind,
    source: SegmentSource,
) -> SegmentDescriptor {
    let url = Url::parse(uri).expect("test url");
    SegmentDescriptor {
        key: SegmentKey {
            kind,
            uri: Arc::from(uri),
            byte_range: None,
            scope: None,
        },
        msn,
        source,
        parsed_url: Arc::new(url),
        discontinuity: false,
        encryption: None,
        init_key: None,
        scramble_seed: None,
        meta: SegmentMeta::Hls(Arc::new(m3u8_rs::MediaSegment {
            uri: uri.to_string(),
            duration: 2.0,
            ..Default::default()
        })),
    }
}

fn payload_for(d: &Arc<SegmentDescriptor>) -> SegmentPayload {
    SegmentPayload::Ts {
        data: Bytes::from_static(b"data"),
        descriptor: Arc::clone(d),
    }
}

fn store() -> SegmentStateStore {
    SegmentStateStore::new(
        StoreConfig {
            retry_budget: 2,
            retry_delay_base: Duration::from_millis(10),
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    )
}

fn take_one(store: &mut SegmentStateStore, budget: &ByteBudget) -> Option<ReadyJob> {
    let (mut jobs, inputs) = store.next_ready_jobs(1, Instant::now(), budget);
    assert!(inputs.is_empty(), "unexpected admission terminals");
    jobs.pop()
}

#[test]
fn ingest_then_schedule_marks_in_flight_and_dedups() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );

    let job = take_one(&mut s, &b).expect("schedulable");
    assert_eq!(job.descriptor.msn, 1);

    // While in flight (and after), the same key never schedules again.
    assert!(take_one(&mut s, &b).is_none());
    let stats = s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    assert_eq!(stats.discovered, 0);
    assert_eq!(stats.refreshed, 1);
    assert!(take_one(&mut s, &b).is_none());
}

#[test]
fn priority_orders_init_media_prefetch_then_msn() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![
            descriptor_with_source(
                "https://e.com/pre.ts",
                1,
                SegmentKind::Media,
                SegmentSource::PlaylistPrefetch,
            ),
            descriptor("https://e.com/m5.ts", 5, SegmentKind::Media),
            descriptor("https://e.com/m3.ts", 3, SegmentKind::Media),
            descriptor("https://e.com/init.mp4", 3, SegmentKind::Init),
        ],
        Instant::now(),
    );

    let order: Vec<(SegmentKind, u64)> = std::iter::from_fn(|| take_one(&mut s, &b))
        .map(|j| (j.descriptor.key.kind, j.descriptor.msn))
        .collect();
    assert_eq!(
        order,
        vec![
            (SegmentKind::Init, 3),
            (SegmentKind::Media, 3),
            (SegmentKind::Media, 5),
            (SegmentKind::Media, 1), // prefetch ranks last despite lowest MSN
        ]
    );
}

#[test]
fn media_at_or_beyond_the_limit_waits_while_inits_are_admitted() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![
            descriptor("https://e.com/m4.ts", 4, SegmentKind::Media),
            descriptor("https://e.com/m3.ts", 3, SegmentKind::Media),
            descriptor("https://e.com/init.mp4", 9, SegmentKind::Init),
            descriptor("https://e.com/m5.ts", 5, SegmentKind::Media),
        ],
        Instant::now(),
    );
    let admitted = |s: &mut SegmentStateStore, limit| {
        let (jobs, inputs) = s.next_ready_jobs_within(
            10,
            Instant::now(),
            &b,
            Some(MediaWindow {
                cursor: 0,
                segments: limit,
                bytes: 0,
            }),
        );
        assert!(inputs.is_empty());
        jobs.into_iter()
            .map(|job| (job.descriptor.key.kind, job.descriptor.msn))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        admitted(&mut s, 4),
        [(SegmentKind::Init, 9), (SegmentKind::Media, 3)]
    );
    assert_eq!(admitted(&mut s, 4), []);
    assert_eq!(
        admitted(&mut s, 6),
        [(SegmentKind::Media, 4), (SegmentKind::Media, 5)]
    );
}

#[test]
fn far_future_vod_inits_wait_without_blocking_current_media_admission() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![
            descriptor("https://e.com/init0.mp4", 0, SegmentKind::Init),
            descriptor("https://e.com/init25.mp4", 25, SegmentKind::Init),
            descriptor("https://e.com/m0.m4s", 0, SegmentKind::Media),
            descriptor("https://e.com/m1.m4s", 1, SegmentKind::Media),
        ],
        Instant::now(),
    );

    let (jobs, inputs) = s.next_ready_jobs_within(
        10,
        Instant::now(),
        &b,
        Some(MediaWindow {
            cursor: 0,
            segments: 4,
            bytes: 0,
        }),
    );
    assert!(inputs.is_empty());
    let admitted: Vec<(SegmentKind, u64)> = jobs
        .into_iter()
        .map(|j| (j.descriptor.key.kind, j.descriptor.msn))
        .collect();
    // init0 (msn 0 < 16) and media 0..=1 are admitted; init25 (msn 25 >= 16)
    // stays gated without blocking class-1 media behind it.
    assert_eq!(
        admitted,
        vec![
            (SegmentKind::Init, 0),
            (SegmentKind::Media, 0),
            (SegmentKind::Media, 1),
        ]
    );
    assert!(s.has_unfinished_inits());

    // Once the cursor advances close enough, init25 is admitted.
    let (jobs, _) = s.next_ready_jobs_within(
        10,
        Instant::now(),
        &b,
        Some(MediaWindow {
            cursor: 10,
            segments: 4,
            bytes: 0,
        }),
    );
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].descriptor.msn, 25);
    assert_eq!(jobs[0].descriptor.key.kind, SegmentKind::Init);
}

#[test]
fn fmp4_media_completed_without_init_map_terminalizes_immediately() {
    let mut s = store();
    let b = budget_unlimited();
    let now = Instant::now();
    s.ingest(
        vec![
            descriptor("https://e.com/0.m4s", 0, SegmentKind::Media),
            descriptor("https://e.com/1.m4s", 1, SegmentKind::Media),
        ],
        now,
    );
    let window = MediaWindow {
        cursor: 0,
        segments: 4,
        bytes: 3 * 1024 * 1024,
    };
    let (mut jobs, _) = s.next_ready_jobs_within(4, now, &b, Some(window));
    assert_eq!(jobs.len(), 1, "3 MiB byte window fits one 2 MiB estimate");
    let first = jobs.pop().unwrap();
    let effects = s.apply_outcome(
        SegmentOutcome::Completed {
            key: first.descriptor.key.clone(),
            msn: 0,
            payload: SegmentPayload::Mp4Media {
                data: Bytes::from_static(b"moof"),
                descriptor: Arc::clone(&first.descriptor),
            },
        },
        now,
    );
    assert!(matches!(
        effects.assembler_inputs.as_slice(),
        [AssemblerInput::TerminalFailed {
            msn: 0,
            init_failed: true,
            error,
            ..
        }] if matches!(error.get(), DownloadError::InvalidContent { protocol: "hls", .. })
    ));

    // Its window bytes were released on terminalization so segment 1 can admit.
    let (jobs, _) = s.next_ready_jobs_within(4, now, &b, Some(window));
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].descriptor.msn, 1);
}

#[test]
fn retryable_failure_waits_for_deadline_then_reschedules() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let job = take_one(&mut s, &b).expect("schedulable");
    let key = job.descriptor.key.clone();

    let now = Instant::now();
    let out = s.apply_outcome(
        SegmentOutcome::Failed {
            key: key.clone(),
            msn: 1,
            class: FailureClass::Http(404),
            reason: Arc::from("404"),
            error: None,
        },
        now,
    );
    assert!(
        out.assembler_inputs.is_empty(),
        "retryable failure emits nothing yet"
    );
    let notice = out.retry_notice.expect("retry notice should be surfaced");
    assert_eq!(notice.attempt, 1);
    assert_eq!(notice.reason.as_ref(), "404");
    assert_eq!(notice.delay, Duration::from_millis(10));
    assert!(s.has_unfinished_work());

    // Before the deadline: not schedulable.
    let (jobs, _) = s.next_ready_jobs(1, now, &b);
    assert!(jobs.is_empty());

    // After the deadline: schedulable again.
    let later = now + Duration::from_millis(50);
    let (jobs, _) = s.next_ready_jobs(1, later, &b);
    assert_eq!(jobs.len(), 1);
}

#[test]
fn too_early_is_retried_like_not_found() {
    let mut s = store();
    let b = budget_unlimited();
    let now = Instant::now();
    s.ingest(
        vec![descriptor("https://e.com/1.m4s", 1, SegmentKind::Media)],
        now,
    );
    let job = take_one(&mut s, &b).expect("schedulable");
    let out = s.apply_outcome(
        SegmentOutcome::Failed {
            key: job.descriptor.key.clone(),
            msn: 1,
            class: FailureClass::Http(425),
            reason: Arc::from("425"),
            error: None,
        },
        now,
    );
    assert!(out.assembler_inputs.is_empty(), "425 is not terminal");
    assert_eq!(out.retry_notice.expect("retry scheduled").attempt, 1);
}

#[test]
fn retry_budget_exhaustion_terminalizes_and_informs_assembler() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let mut now = Instant::now();
    let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();

    for attempt in 0..3 {
        let out = s.apply_outcome(
            SegmentOutcome::Failed {
                key: key.clone(),
                msn: 1,
                class: FailureClass::Network,
                reason: Arc::from("reset"),
                error: None,
            },
            now,
        );
        if attempt < 2 {
            assert!(out.assembler_inputs.is_empty());
            assert!(out.retry_notice.is_some());
            now += Duration::from_secs(60);
            let (jobs, _) = s.next_ready_jobs(1, now, &b);
            assert_eq!(jobs.len(), 1, "attempt {attempt} reschedules");
        } else {
            assert!(matches!(
                out.assembler_inputs.as_slice(),
                [AssemblerInput::TerminalFailed { msn: 1, .. }]
            ));
            assert!(out.retry_notice.is_none());
        }
    }
    assert!(!s.has_unfinished_work());
    // Terminal failures are never rescheduled.
    now += Duration::from_secs(60);
    let (jobs, _) = s.next_ready_jobs(1, now, &b);
    assert!(jobs.is_empty());
}

#[test]
fn auth_failure_with_fresh_url_retries_without_fresh_url_terminalizes() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();

    // Re-discovery with a rotated fetch URL while the attempt is in
    // flight: same identity (FullUrl policy would fork — emulate a
    // token-aware policy by reusing the key with a new parsed_url).
    let mut refreshed = descriptor("https://e.com/1.ts", 1, SegmentKind::Media);
    refreshed.parsed_url = Arc::new(Url::parse("https://e.com/1.ts?token=fresh").unwrap());
    s.ingest(vec![refreshed], Instant::now());

    let out = s.apply_outcome(
        SegmentOutcome::Failed {
            key: key.clone(),
            msn: 1,
            class: FailureClass::Http(403),
            reason: Arc::from("403"),
            error: None,
        },
        Instant::now(),
    );
    assert!(
        out.assembler_inputs.is_empty(),
        "stale-URL 403 must reschedule"
    );
    assert!(out.retry_notice.is_some());

    // Retry runs with the freshest URL and 403s again -> terminal.
    let later = Instant::now() + Duration::from_secs(60);
    let (jobs, _) = s.next_ready_jobs(1, later, &b);
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        jobs[0].descriptor.parsed_url.as_str(),
        "https://e.com/1.ts?token=fresh"
    );
    let out = s.apply_outcome(
        SegmentOutcome::Failed {
            key,
            msn: 1,
            class: FailureClass::Http(403),
            reason: Arc::from("403"),
            error: None,
        },
        later,
    );
    assert!(matches!(
        out.assembler_inputs.as_slice(),
        [AssemblerInput::TerminalFailed { msn: 1, .. }]
    ));
    assert!(out.retry_notice.is_none());
}

#[test]
fn rediscovery_refreshes_url_for_pending_retry() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();
    let now = Instant::now();
    s.apply_outcome(
        SegmentOutcome::Failed {
            key: key.clone(),
            msn: 1,
            class: FailureClass::Http(404),
            reason: Arc::from("404"),
            error: None,
        },
        now,
    );

    let mut refreshed = descriptor("https://e.com/1.ts", 1, SegmentKind::Media);
    refreshed.parsed_url = Arc::new(Url::parse("https://e.com/1.ts?token=v2").unwrap());
    s.ingest(vec![refreshed], now);

    let (jobs, _) = s.next_ready_jobs(1, now + Duration::from_secs(60), &b);
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        jobs[0].descriptor.parsed_url.as_str(),
        "https://e.com/1.ts?token=v2",
        "retry must use the refreshed URL, never the stale one"
    );
}

fn http_failure(key: &SegmentKey, msn: u64, status: u16, url: &str) -> SegmentOutcome {
    SegmentOutcome::Failed {
        key: key.clone(),
        msn,
        class: FailureClass::Http(status),
        reason: Arc::from(format!("HTTP {status}")),
        error: Some(DownloadError::http_status(
            StatusCode::from_u16(status).unwrap(),
            url,
            "segment download",
        )),
    }
}

fn failed_url(input: &AssemblerInput) -> Option<String> {
    match input {
        AssemblerInput::TerminalFailed { error, .. } => match error.get() {
            DownloadError::HttpStatus { url, .. } => Some(url.clone()),
            _ => None,
        },
        _ => None,
    }
}

#[test]
fn a_terminal_http_failure_names_the_url_that_was_requested() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();
    // A refresh replaces the URL while the attempt is in flight.
    let mut refreshed = descriptor("https://e.com/1.ts", 1, SegmentKind::Media);
    refreshed.parsed_url = Arc::new(Url::parse("https://cdn2.e.com/1.ts").unwrap());
    s.ingest(vec![refreshed], Instant::now());
    let out = s.apply_outcome(
        http_failure(&key, 1, 410, "https://e.com/1.ts"),
        Instant::now(),
    );
    assert_eq!(
        out.assembler_inputs
            .iter()
            .map(failed_url)
            .collect::<Vec<_>>(),
        [Some("https://e.com/1.ts".to_owned())]
    );
}

/// An init, and three media segments that need it.
fn init_with_media(s: &mut SegmentStateStore) -> SegmentKey {
    let init = descriptor("https://e.com/init.mp4", 1, SegmentKind::Init);
    let init_key = init.key.clone();
    let media = (1..=3).map(|msn| {
        let mut media = descriptor(&format!("https://e.com/{msn}.m4s"), msn, SegmentKind::Media);
        media.init_key = Some(init_key.clone());
        media
    });
    s.ingest(std::iter::once(init).chain(media).collect(), Instant::now());
    init_key
}

#[test]
fn fmp4_media_of_a_failed_init_fails_with_it_unfetched() {
    let mut s = store();
    let b = budget_unlimited();
    let init_key = init_with_media(&mut s);
    let (jobs, _) = s.next_ready_jobs(2, Instant::now(), &b);
    let media = Arc::clone(&jobs[1].descriptor);
    // The first media shows the stream is fMP4 (HLS does not declare it).
    s.apply_outcome(
        SegmentOutcome::Completed {
            key: media.key.clone(),
            msn: 1,
            payload: SegmentPayload::Mp4Media {
                data: Bytes::from_static(b"m"),
                descriptor: media,
            },
        },
        Instant::now(),
    );
    let out = s.apply_outcome(
        http_failure(&init_key, 1, 400, "https://e.com/init.mp4"),
        Instant::now(),
    );
    assert_eq!(out.assembler_inputs.len(), 1);

    let (jobs, inputs) = s.next_ready_jobs(10, Instant::now(), &b);
    assert!(jobs.is_empty(), "undecodable media is not fetched");
    assert!(
        inputs.iter().all(|input| matches!(
            input,
            AssemblerInput::TerminalFailed {
                init_failed: true,
                ..
            }
        )),
        "{inputs:?}"
    );
    assert_eq!(
        inputs.iter().map(failed_url).collect::<Vec<_>>(),
        vec![Some("https://e.com/init.mp4".to_owned()); 2]
    );
    assert!(!s.has_unfinished_work());
}

#[test]
fn fmp4_media_of_another_map_says_nothing_about_a_failed_map() {
    let mut s = store();
    let b = budget_unlimited();
    // Map A's media arrives as fMP4.
    let map_a = descriptor("https://e.com/a.mp4", 1, SegmentKind::Init);
    let mut media_a = descriptor("https://e.com/1.m4s", 1, SegmentKind::Media);
    media_a.init_key = Some(map_a.key.clone());
    s.ingest(vec![map_a, media_a], Instant::now());
    let (jobs, _) = s.next_ready_jobs(2, Instant::now(), &b);
    let media = Arc::clone(&jobs[1].descriptor);
    s.apply_outcome(
        SegmentOutcome::Completed {
            key: media.key.clone(),
            msn: 1,
            payload: SegmentPayload::Mp4Media {
                data: Bytes::from_static(b"m"),
                descriptor: media,
            },
        },
        Instant::now(),
    );
    // Map B fails before any of its media arrived: they are fetched.
    let map_b = descriptor("https://e.com/b.mp4", 2, SegmentKind::Init);
    let map_b_key = map_b.key.clone();
    let mut media_b = descriptor("https://e.com/2.ts", 2, SegmentKind::Media);
    media_b.init_key = Some(map_b_key.clone());
    s.ingest(vec![map_b, media_b], Instant::now());
    take_one(&mut s, &b).unwrap();
    s.apply_outcome(
        http_failure(&map_b_key, 2, 400, "https://e.com/b.mp4"),
        Instant::now(),
    );
    let (jobs, inputs) = s.next_ready_jobs(10, Instant::now(), &b);
    assert!(inputs.is_empty(), "{inputs:?}");
    assert_eq!(jobs.len(), 1);
}

#[test]
fn a_failed_init_is_kept_while_media_needing_it_is_unfinished() {
    let mut s = SegmentStateStore::new(
        StoreConfig {
            retry_budget: 2,
            retry_delay_base: Duration::from_millis(10),
            max_retained_inits: 0,
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    );
    let b = budget_unlimited();
    let init_key = init_with_media(&mut s);
    let (jobs, _) = s.next_ready_jobs(3, Instant::now(), &b);
    let first = Arc::clone(&jobs[1].descriptor);
    let second = Arc::clone(&jobs[2].descriptor);
    s.apply_outcome(
        SegmentOutcome::Completed {
            key: first.key.clone(),
            msn: 1,
            payload: SegmentPayload::Mp4Media {
                data: Bytes::from_static(b"m"),
                descriptor: first,
            },
        },
        Instant::now(),
    );
    // The second media waits on a retry when its init fails.
    let now = Instant::now();
    s.apply_outcome(
        SegmentOutcome::Failed {
            key: second.key.clone(),
            msn: 2,
            class: FailureClass::Network,
            reason: Arc::from("reset"),
            error: None,
        },
        now,
    );
    s.apply_outcome(
        http_failure(&init_key, 1, 400, "https://e.com/init.mp4"),
        now,
    );
    // The window moves past them all.
    s.prune_below(10, 1);
    let (jobs, inputs) = s.next_ready_jobs(10, now + Duration::from_secs(60), &b);
    assert!(jobs.is_empty(), "undecodable media is not fetched");
    assert!(
        inputs.iter().all(|input| matches!(
            input,
            AssemblerInput::TerminalFailed {
                init_failed: true,
                ..
            }
        )),
        "{inputs:?}"
    );
    assert!(!inputs.is_empty());
}

#[test]
fn ts_media_of_a_failed_map_is_still_fetched() {
    let mut s = store();
    let b = budget_unlimited();
    let init_key = init_with_media(&mut s);
    let (jobs, _) = s.next_ready_jobs(2, Instant::now(), &b);
    let media = Arc::clone(&jobs[1].descriptor);
    s.apply_outcome(
        SegmentOutcome::Completed {
            key: media.key.clone(),
            msn: 1,
            payload: payload_for(&media),
        },
        Instant::now(),
    );
    s.apply_outcome(
        http_failure(&init_key, 1, 400, "https://e.com/init.mp4"),
        Instant::now(),
    );
    let (jobs, inputs) = s.next_ready_jobs(10, Instant::now(), &b);
    assert!(inputs.is_empty(), "{inputs:?}");
    assert_eq!(jobs.len(), 2);
}

#[test]
fn prefetch_upgrade_keeps_one_key_and_raises_priority() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![
            descriptor_with_source(
                "https://e.com/p.ts",
                10,
                SegmentKind::Media,
                SegmentSource::PlaylistPrefetch,
            ),
            descriptor("https://e.com/m.ts", 11, SegmentKind::Media),
        ],
        Instant::now(),
    );
    // Prefetch reappears as normal media: same key, upgraded source.
    let stats = s.ingest(
        vec![descriptor("https://e.com/p.ts", 10, SegmentKind::Media)],
        Instant::now(),
    );
    assert_eq!(stats.discovered, 0, "no second key for the same resource");
    assert_eq!(stats.refreshed, 1);

    // After upgrade it schedules by MSN order among playlist media.
    let first = take_one(&mut s, &b).unwrap();
    assert_eq!(first.descriptor.msn, 10);
    assert_eq!(first.descriptor.source, SegmentSource::Playlist);
}

#[test]
fn media_beyond_the_byte_window_waits_but_the_cursor_segment_is_always_admitted() {
    const MIB: u64 = 1024 * 1024;
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        (0..6)
            .map(|msn| descriptor(&format!("https://e.com/{msn}.ts"), msn, SegmentKind::Media))
            .collect(),
        Instant::now(),
    );
    let admit = |s: &mut SegmentStateStore, cursor: u64| -> Vec<u64> {
        let window = MediaWindow {
            cursor,
            segments: 0,
            bytes: 5 * MIB,
        };
        let (jobs, _) = s.next_ready_jobs_within(10, Instant::now(), &b, Some(window));
        jobs.into_iter()
            .map(|job| job.descriptor.msn)
            .collect::<Vec<_>>()
    };

    // Two 2 MiB estimates fit; a third would pass 5 MiB.
    assert_eq!(admit(&mut s, 0), [0, 1]);
    assert_eq!(admit(&mut s, 0), Vec::<u64>::new());
    // The cursor moving past 0 releases its bytes.
    assert_eq!(admit(&mut s, 1), [2]);
    assert_eq!(admit(&mut s, 1), Vec::<u64>::new());
    // A cursor at a segment held back by the bytes admits it regardless.
    assert_eq!(admit(&mut s, 3), [3, 4]);
    assert_eq!(admit(&mut s, 5), [5]);
}

#[test]
fn activating_a_byte_window_accounts_for_media_admitted_before_the_bound() {
    let mut s = SegmentStateStore::new(
        StoreConfig {
            fallback_size_estimate: 4,
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    );
    let b = budget_unlimited();
    let now = Instant::now();
    s.ingest(
        (0..4)
            .map(|msn| descriptor(&format!("https://e.com/{msn}.ts"), msn, SegmentKind::Media))
            .collect(),
        now,
    );
    let unbounded = MediaWindow {
        cursor: 0,
        segments: 0,
        bytes: 0,
    };
    let (mut jobs, inputs) = s.next_ready_jobs_within(2, now, &b, Some(unbounded));
    assert!(inputs.is_empty());
    let second = jobs.pop().expect("second unbounded admission");
    let first = jobs.pop().expect("first unbounded admission");
    let payload = payload_for(&second.descriptor);
    s.apply_outcome(
        SegmentOutcome::Completed {
            key: second.descriptor.key.clone(),
            msn: 1,
            payload,
        },
        now,
    );

    // The missing cursor's in-flight reservation and the later completed
    // payload already fill this byte window before its gate is enabled.
    let bounded = MediaWindow {
        bytes: 8,
        ..unbounded
    };
    assert!(
        s.next_ready_jobs_within(4, now, &b, Some(bounded))
            .0
            .is_empty()
    );

    let payload = payload_for(&first.descriptor);
    s.apply_outcome(
        SegmentOutcome::Completed {
            key: first.descriptor.key.clone(),
            msn: 0,
            payload,
        },
        now,
    );
    let advanced = MediaWindow {
        cursor: 2,
        ..bounded
    };
    let (jobs, inputs) = s.next_ready_jobs_within(4, now, &b, Some(advanced));
    assert!(inputs.is_empty());
    assert_eq!(
        jobs.iter()
            .map(|job| job.descriptor.msn)
            .collect::<Vec<_>>(),
        [2, 3]
    );
}

#[test]
fn a_failed_attempt_gives_back_its_bytes_in_the_window() {
    const MIB: u64 = 1024 * 1024;
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        (0..3)
            .map(|msn| descriptor(&format!("https://e.com/{msn}.ts"), msn, SegmentKind::Media))
            .collect(),
        Instant::now(),
    );
    let window = MediaWindow {
        cursor: 0,
        segments: 0,
        bytes: 3 * MIB,
    };
    let (jobs, _) = s.next_ready_jobs_within(10, Instant::now(), &b, Some(window));
    assert_eq!(jobs.len(), 1, "one 2 MiB estimate fits in 3 MiB");
    let job = jobs.into_iter().next().unwrap();
    s.apply_outcome(
        SegmentOutcome::Failed {
            key: job.descriptor.key.clone(),
            msn: 0,
            class: FailureClass::Network,
            reason: Arc::from("reset"),
            error: None,
        },
        Instant::now(),
    );
    // Segment 0 retries later; meanwhile its failed attempt holds nothing.
    let window = MediaWindow {
        cursor: 1,
        ..window
    };
    let (jobs, _) = s.next_ready_jobs_within(10, Instant::now(), &b, Some(window));
    assert_eq!(
        jobs.iter()
            .map(|job| job.descriptor.msn)
            .collect::<Vec<_>>(),
        [1]
    );
}

#[test]
fn completed_payload_flows_and_updates_estimate() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let job = take_one(&mut s, &b).unwrap();
    let before = s.current_size_estimate();
    let out = s.apply_outcome(
        SegmentOutcome::Completed {
            key: job.descriptor.key.clone(),
            msn: 1,
            payload: payload_for(&job.descriptor),
        },
        Instant::now(),
    );
    assert!(matches!(
        out.assembler_inputs.as_slice(),
        [AssemblerInput::Payload(_)]
    ));
    assert!(out.retry_notice.is_none());
    assert!(s.current_size_estimate() < before, "EMA moved toward 4B");
    assert!(!s.has_unfinished_work());
}

#[test]
fn admission_reserves_download_bytes_and_stops_at_budget() {
    let mut s = store();
    // Estimate fallback is 2 MiB; budget fits exactly one.
    let b = ByteBudget::new(3 * 1024 * 1024, 0);
    s.ingest(
        vec![
            descriptor("https://e.com/1.ts", 1, SegmentKind::Media),
            descriptor("https://e.com/2.ts", 2, SegmentKind::Media),
        ],
        Instant::now(),
    );
    let (jobs, inputs) = s.next_ready_jobs(8, Instant::now(), &b);
    assert!(inputs.is_empty());
    assert_eq!(jobs.len(), 1, "second admission must fail the byte gate");
    drop(jobs);
    // Reservation released: the second segment becomes admissible.
    let (jobs, _) = s.next_ready_jobs(8, Instant::now(), &b);
    assert_eq!(jobs.len(), 1);
}

#[test]
fn estimate_over_capacity_clamps_instead_of_terminalizing() {
    let mut s = store();
    let b = ByteBudget::new(1024, 0); // capacity below the 2 MiB estimate
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    // The estimate is a guess, not knowledge: the segment admits with a
    // capacity-clamped reservation and the body checks decide from real
    // sizes.
    let (jobs, inputs) = s.next_ready_jobs(1, Instant::now(), &b);
    assert!(inputs.is_empty());
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].reservation.held_bytes(), 1024);
}

#[test]
fn known_byterange_over_capacity_terminalizes_as_oversize() {
    let mut s = store();
    let b = ByteBudget::new(1024, 0);
    let mut d = descriptor("https://e.com/all.ts", 1, SegmentKind::Media);
    d.key.byte_range = Some(ByteRangeKey {
        length: 4096, // provably larger than the whole budget
        offset: 0,
    });
    s.ingest(vec![d], Instant::now());
    let (jobs, inputs) = s.next_ready_jobs(1, Instant::now(), &b);
    assert!(jobs.is_empty());
    assert!(matches!(
        inputs.as_slice(),
        [AssemblerInput::TerminalFailed { msn: 1, .. }]
    ));
    assert!(!s.has_unfinished_work());
}

#[test]
fn admission_terminalizations_are_capped_per_pass() {
    let mut s = store();
    let b = ByteBudget::new(1024, 0);
    // 8 segments, every one provably oversize.
    let descriptors: Vec<_> = (0..8u64)
        .map(|i| {
            let mut d = descriptor(&format!("https://e.com/{i}.ts"), i, SegmentKind::Media);
            d.key.byte_range = Some(ByteRangeKey {
                length: 4096,
                offset: 0,
            });
            d
        })
        .collect();
    s.ingest(descriptors, Instant::now());

    // One pass with 2 slots may terminalize at most 2; the rest stay
    // schedulable for later passes (producer suspension, not flooding).
    let (jobs, inputs) = s.next_ready_jobs(2, Instant::now(), &b);
    assert!(jobs.is_empty());
    assert_eq!(inputs.len(), 2);
    assert!(s.has_unfinished_work());

    let (_, inputs) = s.next_ready_jobs(2, Instant::now(), &b);
    assert_eq!(inputs.len(), 2);
}

#[test]
fn due_retry_promotion_clears_deadline_even_without_admission() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();
    let now = Instant::now();
    s.apply_outcome(
        SegmentOutcome::Failed {
            key,
            msn: 1,
            class: FailureClass::Timeout,
            reason: Arc::from("t"),
            error: None,
        },
        now,
    );
    assert!(s.next_retry_deadline().is_some());

    // After the deadline elapses, promotion alone (no admission — e.g.
    // the reactor's dispatch gate is closed) must drain the heap so the
    // reactor's retry-wake arm goes inert instead of firing forever.
    let later = now + Duration::from_secs(60);
    s.promote_due_retries(later);
    assert!(s.next_retry_deadline().is_none());
    assert!(s.has_unfinished_work(), "promoted entry is Queued work");
}

#[test]
fn byterange_estimate_uses_exact_length() {
    let mut s = store();
    let b = ByteBudget::new(1024, 0);
    let mut d = descriptor("https://e.com/all.ts", 1, SegmentKind::Media);
    d.key.byte_range = Some(ByteRangeKey {
        length: 100,
        offset: 0,
    });
    s.ingest(vec![d], Instant::now());
    let (jobs, inputs) = s.next_ready_jobs(1, Instant::now(), &b);
    assert!(inputs.is_empty());
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].reservation.held_bytes(), 100);
}

#[test]
fn prune_keeps_window_inflight_and_inits() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![
            descriptor("https://e.com/old.ts", 1, SegmentKind::Media),
            descriptor("https://e.com/init.mp4", 1, SegmentKind::Init),
            descriptor("https://e.com/cur.ts", 10, SegmentKind::Media),
            descriptor("https://e.com/run.ts", 2, SegmentKind::Media),
        ],
        Instant::now(),
    );
    // Put msn=1 media into Completed, msn=2 in flight (init is admitted
    // first by priority, complete it too).
    loop {
        let Some(job) = take_one(&mut s, &b) else {
            break;
        };
        let msn = job.descriptor.msn;
        let kind = job.descriptor.key.kind;
        if (msn == 1 || kind == SegmentKind::Init) && msn < 10 {
            s.apply_outcome(
                SegmentOutcome::Completed {
                    key: job.descriptor.key.clone(),
                    msn,
                    payload: payload_for(&job.descriptor),
                },
                Instant::now(),
            );
        }
        // msn 2 and 10 left in flight
    }

    s.prune_below(10, 10);

    // msn=1 Completed media evicted (below window); init kept; in-flight
    // msn=2 kept; msn=10 kept.
    assert!(
        !s.records
            .keys()
            .any(|k| k.uri.as_ref() == "https://e.com/old.ts")
    );
    assert!(
        s.records
            .keys()
            .any(|k| k.uri.as_ref() == "https://e.com/init.mp4")
    );
    assert!(
        s.records
            .keys()
            .any(|k| k.uri.as_ref() == "https://e.com/run.ts")
    );
    assert!(
        s.records
            .keys()
            .any(|k| k.uri.as_ref() == "https://e.com/cur.ts")
    );
}

#[test]
fn a_completed_init_is_sent_again_when_admitted_media_switches_to_it() {
    let mut s = store();
    let b = budget_unlimited();
    let init_a = descriptor("https://e.com/a.mp4", 0, SegmentKind::Init);
    let init_b = descriptor("https://e.com/b.mp4", 1, SegmentKind::Init);
    let media = |msn: u64, init: &SegmentDescriptor| {
        let mut media = descriptor(&format!("https://e.com/{msn}.m4s"), msn, SegmentKind::Media);
        media.init_key = Some(init.key.clone());
        media
    };
    s.ingest(
        vec![
            init_a.clone(),
            media(0, &init_a),
            init_b.clone(),
            media(1, &init_b),
            media(2, &init_a),
            media(3, &init_a),
        ],
        Instant::now(),
    );
    // Both inits complete before any media is admitted.
    let (inits, _) = s.next_ready_jobs(2, Instant::now(), &b);
    for job in &inits {
        s.apply_outcome(
            SegmentOutcome::Completed {
                key: job.descriptor.key.clone(),
                msn: job.descriptor.msn,
                payload: SegmentPayload::Mp4Init {
                    data: Bytes::from_static(b"init"),
                    descriptor: Arc::clone(&job.descriptor),
                },
            },
            Instant::now(),
        );
    }

    let (jobs, inputs) = s.next_ready_jobs(8, Instant::now(), &b);
    assert_eq!(jobs.len(), 4);
    let sent_again: Vec<(&str, u64)> = inputs
        .iter()
        .map(|input| match input {
            AssemblerInput::Payload(payload) => {
                (payload.descriptor().key.uri.as_ref(), payload.msn())
            }
            other => panic!("unexpected input {other:?}"),
        })
        .collect();
    // Media 3 stays on init A, so nothing is sent ahead of it.
    assert_eq!(
        sent_again,
        [
            ("https://e.com/a.mp4", 0),
            ("https://e.com/b.mp4", 1),
            ("https://e.com/a.mp4", 2),
        ]
    );
}

#[test]
fn completed_init_still_in_the_window_survives_the_retention_trim() {
    let mut s = SegmentStateStore::new(
        StoreConfig {
            max_retained_inits: 0,
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    );
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/init.mp4", 1, SegmentKind::Init)],
        Instant::now(),
    );
    let job = take_one(&mut s, &b).expect("admitted");
    s.apply_outcome(
        SegmentOutcome::Completed {
            key: job.descriptor.key.clone(),
            msn: 1,
            payload: payload_for(&job.descriptor),
        },
        Instant::now(),
    );
    // A later window still uses the same map.
    s.ingest(
        vec![descriptor("https://e.com/init.mp4", 20, SegmentKind::Init)],
        Instant::now(),
    );

    s.prune_below(15, 10);

    assert!(
        s.records.contains_key(&job.descriptor.key),
        "trimming it would make the next refresh re-download it"
    );
}

#[test]
fn budget_contention_does_not_spend_the_retry_budget() {
    let mut s = SegmentStateStore::new(
        StoreConfig {
            retry_budget: 1,
            retry_delay_base: Duration::ZERO,
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    );
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/big.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let fail = |s: &mut SegmentStateStore, class| {
        let job = take_one(s, &b).expect("admitted");
        s.apply_outcome(
            SegmentOutcome::Failed {
                key: job.descriptor.key.clone(),
                msn: 1,
                class,
                reason: Arc::from("failed"),
                error: None,
            },
            Instant::now(),
        )
    };

    for _ in 0..5 {
        let effects = fail(&mut s, FailureClass::OverBudget);
        assert!(effects.assembler_inputs.is_empty(), "still retrying");
    }
    // The one real retry is still available, then the segment ends.
    assert!(
        fail(&mut s, FailureClass::Timeout)
            .assembler_inputs
            .is_empty()
    );
    assert!(matches!(
        fail(&mut s, FailureClass::Timeout)
            .assembler_inputs
            .as_slice(),
        [AssemblerInput::TerminalFailed { msn: 1, .. }]
    ));
}

#[test]
fn rate_limiting_waits_out_the_cool_down_without_spending_the_retry_budget() {
    let mut s = SegmentStateStore::new(
        StoreConfig {
            retry_budget: 1,
            retry_delay_base: Duration::from_millis(10),
            retry_deadline: Some(Duration::from_secs(30)),
            max_throttled: Duration::from_secs(60),
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    );
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.m4s", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let mut now = Instant::now();
    let fail = |s: &mut SegmentStateStore, class, now: Instant| {
        let (mut jobs, inputs) = s.next_ready_jobs(1, now, &b);
        assert!(inputs.is_empty());
        let job = jobs.pop().expect("admitted once due");
        s.apply_outcome(
            SegmentOutcome::Failed {
                key: job.descriptor.key.clone(),
                msn: 1,
                class,
                reason: Arc::from("HTTP 429"),
                error: None,
            },
            now,
        )
    };
    let limited = FailureClass::RateLimited {
        status: 429,
        delay: Duration::from_secs(20),
        held_back: false,
    };

    // Three cool-downs of 20 s fit the 60 s bound; the budget of one
    // ordinary retry is untouched, and the 30 s retry deadline does not
    // count the time spent waiting them out.
    for _ in 0..3 {
        let effects = fail(&mut s, limited, now);
        assert!(effects.assembler_inputs.is_empty(), "still waiting");
        let notice = effects.retry_notice.expect("retry scheduled");
        assert_eq!(notice.delay, Duration::from_secs(20));
        assert_eq!(
            s.next_retry_deadline(),
            Some(now + Duration::from_secs(20)),
            "retried when the cool-down ends, not before"
        );
        assert!(s.next_ready_jobs(1, now, &b).0.is_empty());
        now += Duration::from_secs(20);
    }
    assert!(
        fail(&mut s, FailureClass::Timeout, now)
            .assembler_inputs
            .is_empty(),
        "the ordinary retry is still available"
    );
    now += Duration::from_secs(1);
    // A fourth cool-down would exceed the bound: the segment ends.
    let effects = fail(&mut s, limited, now);
    assert!(matches!(
        effects.assembler_inputs.as_slice(),
        [AssemblerInput::TerminalFailed { msn: 1, error, .. }]
            if matches!(error.get(), DownloadError::SegmentFetch { retryable: true, .. })
    ));
}

/// The retry deadline ends a segment whose attempts are slow to fail
/// while most of its retry count is unspent.
#[test]
fn a_failure_past_the_retry_deadline_is_terminal_with_retries_left() {
    let mut s = SegmentStateStore::new(
        StoreConfig {
            retry_budget: 10,
            retry_delay_base: Duration::from_millis(10),
            retry_deadline: Some(Duration::from_secs(45)),
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    );
    let b = budget_unlimited();
    let start = Instant::now();
    s.ingest(
        vec![descriptor("https://e.com/1.m4s", 1, SegmentKind::Media)],
        start,
    );

    for inside in [start, start + Duration::from_secs(44)] {
        assert!(
            fail_at(&mut s, &b, FailureClass::Timeout, inside)
                .assembler_inputs
                .is_empty(),
            "retried inside the deadline"
        );
    }
    let effects = fail_at(
        &mut s,
        &b,
        FailureClass::Timeout,
        start + Duration::from_secs(46),
    );
    assert!(matches!(
        effects.assembler_inputs.as_slice(),
        [AssemblerInput::TerminalFailed { msn: 1, .. }]
    ));
}

fn fail_at(
    s: &mut SegmentStateStore,
    b: &ByteBudget,
    class: FailureClass,
    now: Instant,
) -> OutcomeEffects {
    let (mut jobs, inputs) = s.next_ready_jobs(1, now, b);
    assert!(inputs.is_empty());
    let job = jobs.pop().expect("admitted once due");
    s.apply_outcome(
        SegmentOutcome::Failed {
            key: job.descriptor.key.clone(),
            msn: job.descriptor.msn,
            class,
            reason: Arc::from("refused"),
            error: None,
        },
        now,
    )
}

#[test]
fn rate_limit_holds_keep_the_first_deadline_until_output_consumes_the_payload() {
    let holds = Arc::new(RateLimitHolds::new());
    let mut s = SegmentStateStore::new(
        StoreConfig {
            max_throttled: Duration::from_secs(60),
            ..StoreConfig::default()
        },
        Arc::clone(&holds),
    );
    let b = budget_unlimited();
    let start = Instant::now();
    let descriptor = descriptor("https://e.com/1.m4s", 1, SegmentKind::Media);
    let key = descriptor.key.clone();
    s.ingest(vec![descriptor], start);
    let limited = FailureClass::RateLimited {
        status: 429,
        delay: Duration::from_secs(20),
        held_back: true,
    };

    fail_at(&mut s, &b, limited, start);
    let until = start + Duration::from_secs(60);
    assert_eq!(holds.first_media_hold(1, 2, start), Some((1, until)));
    let second = start + Duration::from_secs(20);
    fail_at(&mut s, &b, limited, second);
    assert_eq!(holds.first_media_hold(1, 2, second), Some((1, until)));

    let now = start + Duration::from_secs(40);
    let job = s
        .next_ready_jobs(1, now, &b)
        .0
        .pop()
        .expect("retry admitted");
    assert_eq!(holds.first_media_hold(1, 2, now), Some((1, until)));
    let payload = payload_for(&job.descriptor);
    s.apply_outcome(
        SegmentOutcome::Completed {
            key: key.clone(),
            msn: 1,
            payload,
        },
        now,
    );
    s.prune_below(2, 1);
    // Completion may still sit behind payload backpressure. Only output
    // consumption, not removal of the finished record, releases its hold.
    assert_eq!(holds.first_media_hold(1, 2, now), Some((1, until)));
    holds.release(&key, 1, now);
    assert_eq!(holds.first_media_hold(1, 2, now), None);
}

#[test]
fn ordinary_failure_and_backlog_pruning_release_rate_limit_holds() {
    let holds = Arc::new(RateLimitHolds::new());
    let mut s = SegmentStateStore::new(
        StoreConfig {
            retry_budget: 2,
            retry_delay_base: Duration::from_millis(10),
            ..StoreConfig::default()
        },
        Arc::clone(&holds),
    );
    let b = budget_unlimited();
    let start = Instant::now();
    holds.set_scope(1, None, false, start);
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        start,
    );
    let limited = FailureClass::RateLimited {
        status: 429,
        delay: Duration::from_secs(1),
        held_back: true,
    };
    fail_at(&mut s, &b, limited, start);
    assert!(holds.first_media_hold(1, 2, start).is_some());
    let now = start + Duration::from_secs(1);
    fail_at(&mut s, &b, FailureClass::Network, now);
    assert_eq!(holds.first_media_hold(1, 2, now), None);
    assert_eq!(holds.clock().paused_at(now), Duration::from_secs(1));
    assert_eq!(holds.clock().active_until(), None);
    assert_eq!(
        holds
            .clock()
            .deadline(start, Duration::ZERO, Duration::from_secs(10), now),
        start + Duration::from_secs(11),
    );

    let now = now + Duration::from_secs(1);
    fail_at(&mut s, &b, limited, now);
    assert!(holds.first_media_hold(1, 2, now).is_some());
    assert!(matches!(
        s.prune_below(2, 0).as_slice(),
        [AssemblerInput::Skipped {
            from_msn: 1,
            to_msn: 1
        }]
    ));
    assert_eq!(holds.first_media_hold(1, 2, now), None);
}

#[test]
fn immediate_rate_limit_retries_still_end_at_the_bound() {
    let mut s = SegmentStateStore::new(
        StoreConfig {
            retry_delay_base: Duration::ZERO,
            max_throttled: Duration::from_secs(60),
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    );
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.m4s", 1, SegmentKind::Media)],
        Instant::now(),
    );
    // A server that keeps asking for an immediate retry.
    let refused = FailureClass::RateLimited {
        status: 429,
        delay: Duration::ZERO,
        held_back: false,
    };
    let start = Instant::now();
    let mut now = start;
    let mut retries = 0;
    loop {
        let effects = fail_at(&mut s, &b, refused, now);
        if !effects.assembler_inputs.is_empty() {
            break;
        }
        let delay = effects.retry_notice.expect("retry scheduled").delay;
        assert!(delay >= MIN_RATE_LIMITED_RETRY, "never a tight loop");
        now += delay;
        retries += 1;
        assert!(retries < 10_000, "rate limiting must end");
    }
    assert!(now.duration_since(start) >= Duration::from_secs(60));
    assert!(now.duration_since(start) < Duration::from_secs(61));
}

#[test]
fn a_rate_limit_retry_never_lands_past_the_bound() {
    let mut s = SegmentStateStore::new(
        StoreConfig {
            max_throttled: Duration::from_secs(300),
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    );
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.m4s", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let refused = |seconds| FailureClass::RateLimited {
        status: 429,
        delay: Duration::from_secs(seconds),
        held_back: false,
    };
    let start = Instant::now();
    fail_at(&mut s, &b, refused(1), start);
    // A minute-long cool-down one second before the bound.
    let now = start + Duration::from_secs(299);
    let effects = fail_at(&mut s, &b, refused(60), now);
    assert_eq!(
        effects.retry_notice.expect("one more try").delay,
        Duration::from_secs(1)
    );
    assert_eq!(
        s.next_retry_deadline(),
        Some(start + Duration::from_secs(300))
    );
    let effects = fail_at(&mut s, &b, refused(60), start + Duration::from_secs(300));
    assert!(matches!(
        effects.assembler_inputs.as_slice(),
        [AssemblerInput::TerminalFailed { msn: 1, .. }]
    ));
}

#[test]
fn a_url_denied_after_rate_limiting_waits_once_for_a_refresh() {
    let mut s = store();
    let b = budget_unlimited();
    let signed = |token: &str| {
        let mut d = descriptor("https://e.com/1.ts", 1, SegmentKind::Media);
        d.parsed_url = Arc::new(Url::parse(&format!("https://e.com/1.ts?t={token}")).unwrap());
        d
    };
    s.ingest(vec![signed("a")], Instant::now());
    let mut now = Instant::now();
    let refused = FailureClass::RateLimited {
        status: 429,
        delay: Duration::from_secs(45),
        held_back: false,
    };
    assert!(
        fail_at(&mut s, &b, refused, now)
            .assembler_inputs
            .is_empty()
    );
    now += Duration::from_secs(45);

    // The token expired while the segment waited, and the playlist with
    // the fresh one has not been published yet.
    let effects = fail_at(&mut s, &b, FailureClass::Http(403), now);
    assert!(effects.assembler_inputs.is_empty(), "not terminal yet");
    assert_eq!(effects.retry_notice.expect("waits").delay, REFRESH_WAIT);
    assert!(s.next_ready_jobs(1, now, &b).0.is_empty());

    // Re-discovery publishes the fresh URL: the segment is retried at
    // once rather than after the wait.
    s.ingest(vec![signed("b")], now);
    let (jobs, _) = s.next_ready_jobs(1, now, &b);
    assert_eq!(jobs.len(), 1, "retried as soon as the URL is fresh");
    assert!(jobs[0].descriptor.parsed_url.as_str().ends_with("t=b"));
    // Denied again with the fresh URL and nothing newer: that denial is
    // real, and the one wait was used.
    let effects = s.apply_outcome(
        SegmentOutcome::Failed {
            key: jobs[0].descriptor.key.clone(),
            msn: 1,
            class: FailureClass::Http(403),
            reason: Arc::from("403"),
            error: None,
        },
        now,
    );
    assert!(matches!(
        effects.assembler_inputs.as_slice(),
        [AssemblerInput::TerminalFailed { msn: 1, .. }]
    ));
}

#[test]
fn a_refresh_wait_that_sees_no_fresh_url_ends_in_the_denial() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let now = Instant::now();
    let refused = FailureClass::RateLimited {
        status: 429,
        delay: Duration::from_secs(1),
        held_back: false,
    };
    fail_at(&mut s, &b, refused, now);
    let now = now + Duration::from_secs(1);
    assert!(
        fail_at(&mut s, &b, FailureClass::Http(401), now)
            .assembler_inputs
            .is_empty()
    );
    let effects = fail_at(&mut s, &b, FailureClass::Http(401), now + REFRESH_WAIT);
    assert!(matches!(
        effects.assembler_inputs.as_slice(),
        [AssemblerInput::TerminalFailed { msn: 1, .. }]
    ));
}

#[test]
fn a_refresh_wait_keeps_the_rate_limit_hold_within_its_bound() {
    let holds = Arc::new(RateLimitHolds::new());
    let mut s = SegmentStateStore::new(
        StoreConfig {
            max_throttled: Duration::from_secs(60),
            ..StoreConfig::default()
        },
        Arc::clone(&holds),
    );
    let b = budget_unlimited();
    let start = Instant::now();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        start,
    );
    let refused = FailureClass::RateLimited {
        status: 429,
        delay: Duration::from_secs(1),
        held_back: false,
    };
    fail_at(&mut s, &b, refused, start);
    let until = start + Duration::from_secs(60);
    let now = start + Duration::from_secs(1);
    let effects = fail_at(&mut s, &b, FailureClass::Http(403), now);
    assert_eq!(effects.retry_notice.expect("waits").delay, REFRESH_WAIT);
    assert_eq!(
        holds.first_media_hold(1, 2, now),
        Some((1, until)),
        "output keeps waiting for the fresh URL"
    );
    // A denial after the wait is real and ends the hold with the segment.
    fail_at(&mut s, &b, FailureClass::Http(403), now + REFRESH_WAIT);
    assert_eq!(holds.first_media_hold(1, 2, now + REFRESH_WAIT), None);
}

fn skipped_ranges(inputs: &[AssemblerInput]) -> Vec<(u64, u64)> {
    inputs
        .iter()
        .map(|input| match input {
            AssemblerInput::Skipped { from_msn, to_msn } => (*from_msn, *to_msn),
            other => panic!("unexpected input {other:?}"),
        })
        .collect()
}

fn media_msns(s: &SegmentStateStore) -> Vec<u64> {
    let mut msns: Vec<u64> = s.records.values().map(|r| r.descriptor.msn).collect();
    msns.sort_unstable();
    msns
}

#[test]
fn backlog_is_capped_by_max_state_entries_and_dropped_msns_are_skipped() {
    let mut s = SegmentStateStore::new(
        StoreConfig {
            max_state_entries: 32,
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    );
    s.ingest(
        (0..500)
            .map(|msn| descriptor(&format!("https://e.com/{msn}.ts"), msn, SegmentKind::Media))
            .collect(),
        Instant::now(),
    );

    // Downloads fell 500 segments behind a window that starts at 1000.
    let dropped = s.prune_below(1000, 100);

    assert_eq!(s.records.len(), 32);
    assert_eq!(
        media_msns(&s),
        (468..500).collect::<Vec<_>>(),
        "newest kept"
    );
    assert_eq!(skipped_ranges(&dropped), [(0, 467)]);
}

#[test]
fn backlog_is_capped_to_one_window() {
    let mut s = store();
    s.ingest(
        (0..20)
            .map(|msn| descriptor(&format!("https://e.com/{msn}.ts"), msn, SegmentKind::Media))
            .collect(),
        Instant::now(),
    );

    let dropped = s.prune_below(20, 5);

    assert_eq!(media_msns(&s), [15, 16, 17, 18, 19]);
    assert_eq!(skipped_ranges(&dropped), [(0, 14)]);
    // Within the bound nothing more is dropped.
    assert!(s.prune_below(20, 5).is_empty());
}

#[test]
fn backlog_drop_keeps_in_flight_work_and_msns_it_still_covers() {
    let mut s = store();
    let b = budget_unlimited();
    // msn 1 is in flight under one URI and re-keyed to another URI.
    s.ingest(
        vec![descriptor("https://e.com/1a.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let in_flight = take_one(&mut s, &b).expect("admitted");
    s.ingest(
        vec![
            descriptor("https://e.com/1b.ts", 1, SegmentKind::Media),
            descriptor("https://e.com/2.ts", 2, SegmentKind::Media),
        ],
        Instant::now(),
    );

    let dropped = s.prune_below(10, 0);

    assert!(s.records.contains_key(&in_flight.descriptor.key));
    assert_eq!(s.records.len(), 1);
    // msn 1 is still pending in flight, so only msn 2 is declared dead.
    assert_eq!(skipped_ranges(&dropped), [(2, 2)]);
}

#[test]
fn queued_segment_below_the_window_survives_prune_and_still_schedules() {
    let mut s = store();
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/old.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    // The window slides past it before it was admitted.
    s.prune_below(100, 10);

    let (jobs, inputs) = s.next_ready_jobs(8, Instant::now(), &b);
    assert_eq!(jobs.len(), 1, "the assembler is still waiting on msn 1");
    assert_eq!(jobs[0].descriptor.msn, 1);
    assert!(inputs.is_empty());
}

#[test]
fn retrying_segment_below_the_window_keeps_its_retry_and_reports_the_outcome() {
    let mut s = SegmentStateStore::new(
        StoreConfig {
            retry_budget: 1,
            retry_delay_base: Duration::ZERO,
            ..StoreConfig::default()
        },
        Arc::new(RateLimitHolds::new()),
    );
    let b = budget_unlimited();
    s.ingest(
        vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
        Instant::now(),
    );
    let fail = |s: &mut SegmentStateStore, key: &SegmentKey| {
        s.apply_outcome(
            SegmentOutcome::Failed {
                key: key.clone(),
                msn: 1,
                class: FailureClass::Http(404),
                reason: Arc::from("gone"),
                error: None,
            },
            Instant::now(),
        )
    };
    let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();
    fail(&mut s, &key);

    s.prune_below(100, 10);
    assert!(
        s.next_retry_deadline().is_some(),
        "retry survives the slide"
    );
    assert!(s.has_unfinished_work());

    // The retry fails too: the terminal failure reaches the assembler
    // instead of the MSN vanishing.
    let retried = take_one(&mut s, &b).expect("retry is admitted");
    let effects = fail(&mut s, &retried.descriptor.key);
    assert!(matches!(
        effects.assembler_inputs.as_slice(),
        [AssemblerInput::TerminalFailed { msn: 1, .. }]
    ));
    s.prune_below(100, 10);
    assert!(!s.has_unfinished_work());
    assert!(
        s.records.is_empty(),
        "finished below-window records are pruned"
    );
}
