//! MPD fixtures through the public `mesio::dash::manifest` API, with fixed
//! clocks. No network access.

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use mesio_engine::DownloadError;
use mesio_engine::dash::manifest::{
    AddressingKind, AvailabilityTimeOffset, ByteRange, ContentType, Manifest, ManifestError,
    PresentationType, Representation, UnsupportedFeature, UtcTiming,
};
use url::Url;

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/dash/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn parse(xml: &str, document: &str) -> Manifest {
    Manifest::parse(xml.as_bytes(), &Url::parse(document).unwrap()).unwrap()
}

fn at(secs: i64, nanos: u32) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, nanos).unwrap()
}

fn representation<'a>(manifest: &'a Manifest, period: usize, id: &str) -> &'a Representation {
    manifest.periods[period]
        .adaptation_sets
        .iter()
        .flat_map(|set| &set.representations)
        .find(|representation| representation.id == id)
        .unwrap_or_else(|| panic!("no Representation {id}"))
}

const NEW_YEAR_2024: i64 = 1_704_067_200;

#[test]
fn livesim_number_window_follows_wall_clock() {
    let manifest = parse(
        &fixture("livesim-number.mpd"),
        "https://livesim2.example.com/livesim2/testpic_2s/Manifest.mpd",
    );
    assert_eq!(manifest.presentation_type, PresentationType::Dynamic);
    assert_eq!(
        manifest.suggested_presentation_delay,
        Some(Duration::from_secs(4))
    );
    assert_eq!(manifest.minimum_update_period, Some(Duration::from_secs(2)));
    assert_eq!(
        manifest.utc_timings,
        [UtcTiming::HttpXsDate {
            urls: vec![Url::parse("https://time.akamai.com/?iso&ms").unwrap()]
        }]
    );

    let video = representation(&manifest, 0, "V300");
    assert_eq!(video.content_type, ContentType::Video);
    assert_eq!((video.width, video.height), (Some(640), Some(360)));
    assert_eq!(video.start_with_sap, Some(1));
    assert_eq!(
        video.initialization.as_ref().unwrap().url.as_str(),
        "https://livesim2.example.com/livesim2/testpic_2s/V300/init.mp4"
    );

    // x = 1 704 067 200 s, D = 2 s, N = 0: n_max = x/2 − 1; the 5-minute
    // buffer keeps segments ending at or after x − 300.
    let window = video.segments(at(NEW_YEAR_2024, 0)).unwrap();
    assert_eq!(window.numbers(), 852_033_449..852_033_600);
    assert_eq!(window.len(), 151);
    let newest = window.last().unwrap().unwrap();
    assert_eq!(newest.number, 852_033_599);
    assert_eq!(newest.time, 852_033_599 * 180_000);
    assert_eq!(newest.duration, 180_000);
    assert_eq!(newest.start_secs, (NEW_YEAR_2024 - 2) as f64);
    assert_eq!(
        newest.url.as_str(),
        "https://livesim2.example.com/livesim2/testpic_2s/V300/852033599.m4s"
    );

    // Exact boundary: one nanosecond earlier the newest segment is not out.
    let before = video.segments(at(NEW_YEAR_2024 - 1, 999_999_999)).unwrap();
    assert_eq!(before.numbers().end, 852_033_599);

    let audio = representation(&manifest, 0, "A48");
    assert_eq!(audio.content_type, ContentType::Audio);
    assert_eq!(
        audio.segments(at(NEW_YEAR_2024, 0)).unwrap().numbers(),
        852_033_449..852_033_600
    );
}

#[test]
fn livesim_without_time_shift_buffer_spans_the_whole_period_lazily() {
    let xml = fixture("livesim-number.mpd").replace(r#" timeShiftBufferDepth="PT5M""#, "");
    let manifest = parse(&xml, "https://livesim2.example.com/Manifest.mpd");
    assert_eq!(manifest.time_shift_buffer_depth, None);
    let window = representation(&manifest, 0, "V300")
        .segments(at(NEW_YEAR_2024, 0))
        .unwrap();
    // 54 years of 2 s segments, never materialized.
    assert_eq!(window.numbers(), 0..852_033_600);
    let tail: Vec<u64> = window
        .iter()
        .rev()
        .take(2)
        .map(|segment| segment.unwrap().number)
        .collect();
    assert_eq!(tail, [852_033_599, 852_033_598]);
    assert_eq!(window.first().unwrap().unwrap().number, 0);
}

#[test]
fn time_timeline_with_omitted_t_and_open_repeat() {
    let manifest = parse(
        &fixture("timeline-time.mpd"),
        "https://live.example.com/channel/manifest.mpd",
    );
    let now = at(NEW_YEAR_2024 + 20, 0);

    // Video ends: 2, 4, 6 (r=2), 7 (omitted t), then every 2 s from 9 (r=-1).
    // At x = 20 the newest ends at 19; the 10 s buffer keeps ends ≥ 10.
    let video = representation(&manifest, 0, "1080p");
    let window = video.segments(now).unwrap();
    let segments: Vec<_> = window.iter().map(Result::unwrap).collect();
    let times: Vec<_> = segments.iter().map(|segment| segment.time).collect();
    assert_eq!(times, [810_000, 990_000, 1_170_000, 1_350_000, 1_530_000]);
    assert_eq!(segments[0].number, 6);
    assert_eq!(
        segments[4].url.as_str(),
        "https://live.example.com/channel/video/1080p/1530000.m4s"
    );

    let edge = video.segments(at(NEW_YEAR_2024 + 19, 0)).unwrap();
    assert_eq!(
        edge.last().unwrap().unwrap().time,
        1_530_000,
        "inclusive at 19 s"
    );
    let early = video.segments(at(NEW_YEAR_2024 + 18, 999_999_999)).unwrap();
    assert_eq!(early.last().unwrap().unwrap().time, 1_350_000);

    // Audio: d = 96256 / 48000 ≈ 2.0053 s; 9 segments ended by 20 s, the
    // first four ended before 10 s.
    let audio = representation(&manifest, 0, "aac");
    let window = audio.segments(now).unwrap();
    assert_eq!(window.numbers(), 5..10);
    assert_eq!(
        window.last().unwrap().unwrap().url.as_str(),
        "https://live.example.com/channel/audio/000000770048.m4s"
    );
}

#[test]
fn segment_list_with_files_and_byte_ranges() {
    let manifest = parse(
        &fixture("segment-list.mpd"),
        "https://vod.example.com/a/b.mpd",
    );
    let files = representation(&manifest, 0, "files");
    assert_eq!(
        files.initialization.as_ref().unwrap().url.as_str(),
        "https://vod.example.com/a/media/files/init.mp4"
    );
    let segments: Vec<_> = files
        .segments(at(0, 0))
        .unwrap()
        .iter()
        .map(Result::unwrap)
        .collect();
    let urls: Vec<_> = segments
        .iter()
        .map(|segment| segment.url.as_str())
        .collect();
    assert_eq!(
        urls,
        [
            "https://vod.example.com/a/media/files/seg1.m4s",
            "https://vod.example.com/a/media/files/seg2.m4s",
            "https://vod.example.com/a/media/files/seg3.m4s",
        ]
    );

    let ranges = representation(&manifest, 0, "ranges");
    let init = ranges.initialization.as_ref().unwrap();
    assert_eq!(
        init.url.as_str(),
        "https://vod.example.com/a/media/single.mp4"
    );
    assert_eq!(
        init.byte_range,
        Some(ByteRange {
            first: 0,
            last: 799
        })
    );
    let segments: Vec<_> = ranges
        .segments(at(0, 0))
        .unwrap()
        .iter()
        .map(Result::unwrap)
        .collect();
    let summary: Vec<_> = segments
        .iter()
        .map(|segment| (segment.number, segment.start_secs, segment.byte_range))
        .collect();
    assert_eq!(
        summary,
        [
            (
                10,
                0.0,
                Some(ByteRange {
                    first: 800,
                    last: 50_799
                })
            ),
            (
                11,
                4.0,
                Some(ByteRange {
                    first: 50_800,
                    last: 100_799
                })
            ),
            (
                12,
                8.0,
                Some(ByteRange {
                    first: 100_800,
                    last: 140_000
                })
            ),
        ]
    );
}

#[test]
fn multi_period_with_derived_timing_and_presentation_time_offset() {
    let manifest = parse(
        &fixture("multi-period.mpd"),
        "https://vod.example.com/show/manifest.mpd",
    );
    let timing: Vec<_> = manifest
        .periods
        .iter()
        .map(|period| {
            (
                period.id.as_deref().unwrap(),
                period.start.as_secs(),
                period.duration.unwrap().as_secs(),
            )
        })
        .collect();
    assert_eq!(
        timing,
        [("intro", 0, 30), ("main", 30, 30), ("credits", 60, 30)]
    );

    let intro = representation(&manifest, 0, "v")
        .segments(at(0, 0))
        .unwrap();
    assert_eq!(
        intro.len(),
        8,
        "ceil(30 s / 4 s), the last overlapping the end"
    );
    assert_eq!(
        intro.last().unwrap().unwrap().url.as_str(),
        "https://vod.example.com/show/intro/00008.m4s"
    );

    let main = representation(&manifest, 1, "v");
    assert_eq!(main.presentation_time_offset(), 900_000);
    let segments: Vec<_> = main
        .segments(at(0, 0))
        .unwrap()
        .iter()
        .map(Result::unwrap)
        .collect();
    let starts: Vec<_> = segments.iter().map(|segment| segment.start_secs).collect();
    assert_eq!(starts, [30.0, 36.0, 42.0, 48.0, 54.0]);
    assert_eq!(
        segments[0].url.as_str(),
        "https://vod.example.com/show/main/0000900000.m4s",
        "$Time$ is the raw media time"
    );

    let credits = representation(&manifest, 2, "v");
    assert!(credits.initialization.is_none());
    let starts: Vec<_> = credits
        .segments(at(0, 0))
        .unwrap()
        .iter()
        .map(|segment| segment.unwrap().start_secs)
        .collect();
    assert_eq!(starts, [60.0, 70.0, 80.0]);
}

#[test]
fn base_urls_and_availability_time_offsets() {
    let manifest = parse(
        &fixture("base-url-ato.mpd"),
        "https://origin.example.com/app/live.mpd",
    );
    let relative = representation(&manifest, 0, "relative");
    assert_eq!(
        relative.initialization.as_ref().unwrap().url.as_str(),
        "https://origin.example.com/app/cdn/live/hd/init.mp4"
    );
    // MPD 0.5 + Period 0.25 + SegmentTemplate 1.
    assert_eq!(
        relative.availability_time_offset,
        AvailabilityTimeOffset::Finite(Duration::from_millis(1750))
    );

    let absolute = representation(&manifest, 0, "absolute");
    assert_eq!(
        absolute.initialization.as_ref().unwrap().url.as_str(),
        "https://edge.example.net/abs/init.mp4"
    );
    // The absolute BaseURL drops the outer offsets; the Representation's
    // SegmentTemplate overrides the Period's.
    assert_eq!(
        absolute.availability_time_offset,
        AvailabilityTimeOffset::Finite(Duration::from_secs(1))
    );

    // Period starts at 10 s; at x = 30.5 s, 20.5 s into the Period.
    let now = at(NEW_YEAR_2024 + 30, 500_000_000);
    assert_eq!(relative.segments(now).unwrap().numbers(), 1..12);
    assert_eq!(absolute.segments(now).unwrap().numbers(), 1..11);

    let infinite = representation(&manifest, 0, "infinite");
    assert_eq!(
        infinite.availability_time_offset,
        AvailabilityTimeOffset::Infinite
    );
    assert!(matches!(
        infinite.segments(now),
        Err(ManifestError::Unsupported(
            UnsupportedFeature::UnboundedAvailability
        ))
    ));
}

#[test]
fn protected_and_webm_content_is_rejected() {
    let manifest = parse(&fixture("drm.mpd"), "https://vod.example.com/drm.mpd");
    let period = &manifest.periods[0];
    assert!(period.adaptation_sets.is_empty());
    let features: Vec<_> = period.rejected.iter().map(|r| r.feature.clone()).collect();
    assert_eq!(
        features,
        [
            UnsupportedFeature::ContentProtection,
            UnsupportedFeature::MimeType("audio/webm".to_owned()),
        ]
    );
    let error: DownloadError = period.ensure_usable().unwrap_err().into();
    assert!(error.is_non_recoverable_source_error());
    assert_eq!(
        error.to_string(),
        "unsupported dash feature: content protection (DRM)"
    );
}

#[test]
fn malformed_durations_and_dates_are_manifest_errors() {
    for attributes in [
        r#"mediaPresentationDuration="P1M""#,
        r#"mediaPresentationDuration="PT-5S""#,
        r#"mediaPresentationDuration="10""#,
        r#"timeShiftBufferDepth="PT""#,
        r#"type="dynamic" availabilityStartTime="2024-02-30T00:00:00Z""#,
        r#"type="dynamic" availabilityStartTime="1704067200""#,
        r#"publishTime="2024-01-01""#,
    ] {
        let xml = format!(r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" {attributes}/>"#);
        let error = Manifest::parse(xml.as_bytes(), &Url::parse("https://e.com/a.mpd").unwrap())
            .unwrap_err();
        assert!(
            matches!(error, ManifestError::InvalidAttribute { .. }),
            "{attributes}: {error}"
        );
        let error: DownloadError = error.into();
        assert!(
            matches!(error, DownloadError::Manifest { .. }),
            "{attributes}"
        );
        assert!(!error.is_non_recoverable_source_error());
    }
}

fn dynamic(attributes: &str, set: &str) -> Manifest {
    let xml = format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
     availabilityStartTime="2024-01-01T00:00:00Z" {attributes}>
  <Period id="0" start="PT0S">
    <AdaptationSet mimeType="video/mp4">{set}<Representation id="v" bandwidth="1"/></AdaptationSet>
  </Period>
</MPD>"#
    );
    parse(&xml, "https://live.example.com/manifest.mpd")
}

#[test]
fn addressing_kind_and_identity_are_exposed() {
    let manifest = parse(
        &fixture("timeline-time.mpd"),
        "https://live.example.com/channel/manifest.mpd",
    );
    let video = representation(&manifest, 0, "1080p");
    assert_eq!(video.addressing_kind(), AddressingKind::Time);
    assert!(video.uses_segment_timeline());

    let manifest = parse(&fixture("livesim-number.mpd"), "https://e.com/m.mpd");
    let video = representation(&manifest, 0, "V300");
    assert_eq!(video.addressing_kind(), AddressingKind::Number);
    assert!(!video.uses_segment_timeline());

    let manifest = parse(&fixture("segment-list.mpd"), "https://e.com/m.mpd");
    assert_eq!(
        representation(&manifest, 0, "files").addressing_kind(),
        AddressingKind::List
    );
}

#[test]
fn overlapping_open_repeat_never_returns_unavailable_segments() {
    let manifest = dynamic(
        "",
        r#"<SegmentTemplate media="$Time$.m4s"><SegmentTimeline><S t="0" d="10" r="-1"/><S t="35" d="1" r="-1"/></SegmentTimeline></SegmentTemplate>"#,
    );
    let video = representation(&manifest, 0, "v");
    for x in 0..=60 {
        let window = video.segments(at(NEW_YEAR_2024 + x, 0)).unwrap();
        let mut previous_end = 0;
        for segment in &window {
            let segment = segment.unwrap();
            let end = segment.time + segment.duration;
            assert!(end <= x as u64, "x={x}: segment ending at {end} returned");
            assert!(end > previous_end, "x={x}: ends must increase");
            previous_end = end;
        }
    }
    let window = video.segments(at(NEW_YEAR_2024 + 37, 0)).unwrap();
    let ends: Vec<_> = window
        .iter()
        .map(|segment| {
            let segment = segment.unwrap();
            segment.time + segment.duration
        })
        .collect();
    assert_eq!(ends, [10, 20, 30, 35, 36, 37]);
}

#[test]
fn infinite_offset_is_bounded_by_finite_addressing() {
    let now = at(NEW_YEAR_2024, 0);
    let timeline = dynamic(
        "",
        r#"<SegmentTemplate media="$Time$" availabilityTimeOffset="INF"><SegmentTimeline><S t="0" d="2" r="2"/></SegmentTimeline></SegmentTemplate>"#,
    );
    assert_eq!(
        representation(&timeline, 0, "v")
            .segments(now)
            .unwrap()
            .len(),
        3
    );
    let end_number = dynamic(
        "",
        r#"<SegmentTemplate media="$Number$" duration="2" endNumber="4" availabilityTimeOffset="INF"/>"#,
    );
    assert_eq!(
        representation(&end_number, 0, "v")
            .segments(now)
            .unwrap()
            .numbers(),
        1..5
    );
    let list = dynamic(
        "",
        r#"<SegmentList duration="2" availabilityTimeOffset="INF"><SegmentURL media="a"/><SegmentURL media="b"/></SegmentList>"#,
    );
    assert_eq!(
        representation(&list, 0, "v").segments(now).unwrap().len(),
        2
    );
    let open = dynamic(
        "",
        r#"<SegmentTemplate media="$Number$" duration="2" availabilityTimeOffset="INF"/>"#,
    );
    assert!(matches!(
        representation(&open, 0, "v").segments(now),
        Err(ManifestError::Unsupported(
            UnsupportedFeature::UnboundedAvailability
        ))
    ));
}

#[test]
fn segment_numbers_beyond_u64_are_typed_errors() {
    let manifest = dynamic(
        "",
        r#"<SegmentTemplate media="$Number$" duration="2" startNumber="18446744073709551610"/>"#,
    );
    let video = representation(&manifest, 0, "v");
    let window = video.segments(at(NEW_YEAR_2024 + 8, 0)).unwrap();
    let numbers = window.numbers();
    assert_eq!(numbers.end - numbers.start, window.len());
    assert!(matches!(
        video.segments(at(NEW_YEAR_2024 + 20, 0)),
        Err(ManifestError::Overflow { .. })
    ));
}

#[test]
fn availability_end_time_ends_the_presentation() {
    let manifest = dynamic(
        r#"availabilityEndTime="2024-01-01T00:01:00Z""#,
        r#"<SegmentTemplate media="$Number$" duration="2"/>"#,
    );
    assert_eq!(
        manifest.availability_end_time,
        Some(Utc.timestamp_opt(NEW_YEAR_2024 + 60, 0).unwrap())
    );
    let video = representation(&manifest, 0, "v");
    assert_eq!(video.segments(at(NEW_YEAR_2024 + 60, 0)).unwrap().len(), 30);
    assert!(
        video
            .segments(at(NEW_YEAR_2024 + 60, 1))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn sub_nanosecond_times_round_half_up() {
    // availabilityStartTime rounds up to 1 ns; the first 2 s segment is then
    // available from 2 s + 1 ns.
    let xml = r#"<MPD type="dynamic" availabilityStartTime="2024-01-01T00:00:00.0000000005Z">
  <Period start="PT0S"><AdaptationSet mimeType="video/mp4">
    <SegmentTemplate media="$Number$" duration="2"/><Representation id="v" bandwidth="1"/>
  </AdaptationSet></Period></MPD>"#;
    let manifest = parse(xml, "https://e.com/m.mpd");
    let video = representation(&manifest, 0, "v");
    assert!(video.segments(at(NEW_YEAR_2024 + 2, 0)).unwrap().is_empty());
    assert_eq!(video.segments(at(NEW_YEAR_2024 + 2, 1)).unwrap().len(), 1);
    // A Period start of 0.0000000004 s rounds down to 0.
    let xml = xml
        .replace(".0000000005Z", "Z")
        .replace(r#"start="PT0S""#, r#"start="PT0.0000000004S""#);
    let manifest = parse(&xml, "https://e.com/m.mpd");
    assert_eq!(manifest.periods[0].start, Duration::ZERO);
}
