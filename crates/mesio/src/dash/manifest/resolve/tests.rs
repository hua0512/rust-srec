use super::*;

fn url() -> Url {
    Url::parse("https://origin.example.com/live/stream.mpd?token=secret").unwrap()
}

fn manifest(xml: &str) -> Manifest {
    Manifest::parse(xml.as_bytes(), &url()).unwrap()
}

fn at(secs: i64) -> Timestamp {
    jiff::Timestamp::from_second(secs).unwrap()
}

#[test]
fn segment_template_attributes_inherit_per_attribute() {
    let manifest = manifest(
        r#"<MPD type="static" mediaPresentationDuration="PT8S">
  <Period>
<SegmentTemplate timescale="1000" duration="2000" startNumber="7"
                 media="$RepresentationID$/$Number%03d$.m4s"/>
<AdaptationSet mimeType="video/mp4" codecs="avc1.64001f" width="1280" height="720">
  <SegmentTemplate initialization="$RepresentationID$/init-$Bandwidth$.mp4"/>
  <Representation id="v1" bandwidth="800000"/>
  <Representation id="v2" bandwidth="1600000" height="1080">
    <SegmentTemplate startNumber="1"/>
  </Representation>
</AdaptationSet>
  </Period>
</MPD>"#,
    );
    let set = &manifest.periods[0].adaptation_sets[0];
    let v1 = &set.representations[0];
    assert_eq!(v1.codecs.as_deref(), Some("avc1.64001f"));
    assert_eq!((v1.width, v1.height), (Some(1280), Some(720)));
    assert_eq!(
        v1.initialization.as_ref().unwrap().url.as_str(),
        "https://origin.example.com/live/v1/init-800000.mp4"
    );
    let numbers: Vec<_> = v1
        .segments(at(0))
        .unwrap()
        .iter()
        .map(|s| s.unwrap().number)
        .collect();
    assert_eq!(numbers, [7, 8, 9, 10]);
    let first = v1.segments(at(0)).unwrap().first().unwrap().unwrap();
    assert_eq!(
        first.url.as_str(),
        "https://origin.example.com/live/v1/007.m4s"
    );

    let v2 = &set.representations[1];
    assert_eq!(v2.height, Some(1080));
    assert_eq!(v2.segments(at(0)).unwrap().numbers(), 1..5);
}

#[test]
fn base_url_chain_resolves_and_absolute_urls_reset_it() {
    let manifest = manifest(
        r#"<MPD type="static" mediaPresentationDuration="PT2S">
  <BaseURL availabilityTimeOffset="1">cdn/</BaseURL>
  <Period>
<BaseURL availabilityTimeOffset="2">period/</BaseURL>
<AdaptationSet mimeType="audio/mp4">
  <BaseURL>audio/</BaseURL>
  <SegmentTemplate duration="2" media="$Number$.m4s" initialization="init.mp4"/>
  <Representation id="a1" bandwidth="1">
    <BaseURL>a1/</BaseURL>
  </Representation>
  <Representation id="a2" bandwidth="1">
    <BaseURL availabilityTimeOffset="0.5">https://other.example.com/a2/</BaseURL>
  </Representation>
  <Representation id="a3" bandwidth="1">
    <BaseURL>/root/</BaseURL>
  </Representation>
</AdaptationSet>
  </Period>
</MPD>"#,
    );
    let set = &manifest.periods[0].adaptation_sets[0];
    let a1 = &set.representations[0];
    assert_eq!(a1.content_type, ContentType::Audio);
    assert_eq!(
        a1.initialization.as_ref().unwrap().url.as_str(),
        "https://origin.example.com/live/cdn/period/audio/a1/init.mp4"
    );
    assert_eq!(
        a1.availability_time_offset,
        AvailabilityTimeOffset::Finite(Duration::from_secs(3))
    );
    let a2 = &set.representations[1];
    assert_eq!(
        a2.segments(at(0))
            .unwrap()
            .first()
            .unwrap()
            .unwrap()
            .url
            .as_str(),
        "https://other.example.com/a2/1.m4s"
    );
    assert_eq!(
        a2.availability_time_offset,
        AvailabilityTimeOffset::Finite(Duration::from_millis(500))
    );
    let a3 = &set.representations[2];
    assert_eq!(
        a3.initialization.as_ref().unwrap().url.as_str(),
        "https://origin.example.com/root/init.mp4"
    );
}

#[test]
fn availability_time_offset_sums_base_url_and_segment_information() {
    let manifest = manifest(
        r#"<MPD type="dynamic" availabilityStartTime="1970-01-01T00:00:00Z">
  <BaseURL availabilityTimeOffset="0.5">https://cdn.example.com/</BaseURL>
  <Period start="PT0S">
<SegmentTemplate availabilityTimeOffset="5"/>
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate duration="2" media="$Number$.m4s" availabilityTimeOffset="1.5"/>
  <Representation id="v" bandwidth="1"/>
  <Representation id="low-latency" bandwidth="1">
    <SegmentTemplate availabilityTimeComplete="false"/>
  </Representation>
</AdaptationSet>
  </Period>
</MPD>"#,
    );
    let set = &manifest.periods[0].adaptation_sets[0];
    let full = &set.representations[0];
    // BaseURL 0.5 + innermost segment information 1.5 (overrides the
    // Period's 5).
    assert_eq!(
        full.availability_time_offset,
        AvailabilityTimeOffset::Finite(Duration::from_secs(2))
    );
    assert!(full.availability_time_complete);
    // At x = 10 with A = 2 the segment ending at 12 is available.
    assert_eq!(full.segments(at(10)).unwrap().numbers(), 1..7);

    let chunked = &set.representations[1];
    assert!(!chunked.availability_time_complete);
    assert_eq!(chunked.segments(at(10)).unwrap().numbers(), 1..6);
}

#[test]
fn period_timing_is_derived() {
    let manifest = manifest(
        r#"<MPD type="static" mediaPresentationDuration="PT60S">
  <Period id="a" duration="PT10S"/>
  <Period id="b" duration="PT20S"/>
  <Period id="c" start="PT40S"/>
</MPD>"#,
    );
    let timing: Vec<_> = manifest
        .periods
        .iter()
        .map(|p| {
            (
                p.id.as_deref().unwrap(),
                p.start.as_secs(),
                p.duration.map(|d| d.as_secs()),
            )
        })
        .collect();
    assert_eq!(
        timing,
        [("a", 0, Some(10)), ("b", 10, Some(30)), ("c", 40, Some(20))]
    );
}

#[test]
fn dynamic_periods_without_derivable_start_are_early_available() {
    let manifest = manifest(
        r#"<MPD type="dynamic" availabilityStartTime="2024-01-01T00:00:00Z">
  <Period id="live" start="PT100S"/>
  <Period id="next"/>
</MPD>"#,
    );
    assert_eq!(manifest.periods.len(), 1);
    assert_eq!(manifest.periods[0].duration, None);
    assert_eq!(manifest.early_available_periods, 1);
}

#[test]
fn inconsistent_periods_are_rejected() {
    for xml in [
        r#"<MPD type="static"><Period start="PT10S"/><Period start="PT5S"/></MPD>"#,
        r#"<MPD type="static"><Period/><Period/></MPD>"#,
        r#"<MPD type="static" mediaPresentationDuration="PT5S"><Period start="PT10S"/></MPD>"#,
        r#"<MPD type="dynamic"><Period start="PT0S"/></MPD>"#,
    ] {
        assert!(Manifest::parse(xml.as_bytes(), &url()).is_err(), "{xml}");
    }
}

#[test]
fn out_of_scope_content_is_rejected_per_element() {
    let manifest = manifest(
        r#"<MPD type="static" mediaPresentationDuration="PT2S"
 xmlns:xlink="http://www.w3.org/1999/xlink">
  <Period>
<SegmentTemplate duration="2" media="$Number$.m4s"/>
<AdaptationSet id="1" mimeType="video/mp4">
  <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
  <Representation id="drm" bandwidth="1"/>
</AdaptationSet>
<AdaptationSet id="2" mimeType="video/webm">
  <Representation id="webm" bandwidth="1"/>
</AdaptationSet>
<AdaptationSet id="3" mimeType="video/mp4">
  <Representation id="unindexed" bandwidth="1">
    <SegmentBase/>
  </Representation>
  <Representation id="clear" bandwidth="1"/>
  <Representation id="rep-drm" bandwidth="1">
    <ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"/>
  </Representation>
</AdaptationSet>
<AdaptationSet id="4" xlink:href="https://ads.example.com/set.xml"/>
<AdaptationSet id="5" mimeType="video/mp4">
  <EssentialProperty schemeIdUri="http://dashif.org/guidelines/trickmode" value="3"/>
  <Representation id="trick" bandwidth="1"/>
</AdaptationSet>
<AdaptationSet id="6" mimeType="video/mp4">
  <EssentialProperty schemeIdUri="urn:mpeg:mpegB:cicp:TransferCharacteristics" value="16"/>
  <Representation id="hdr" bandwidth="1"/>
</AdaptationSet>
  </Period>
</MPD>"#,
    );
    let period = &manifest.periods[0];
    let usable: Vec<_> = period
        .adaptation_sets
        .iter()
        .flat_map(|set| set.representations.iter().map(|r| r.id.as_str()))
        .collect();
    assert_eq!(usable, ["clear", "hdr"]);
    let rejected: Vec<_> = period
        .rejected
        .iter()
        .map(|r| {
            (
                r.adaptation_set_id.as_deref(),
                r.representation_id.as_deref(),
                r.feature.clone(),
            )
        })
        .collect();
    assert_eq!(
        rejected,
        [
            (Some("1"), None, UnsupportedFeature::ContentProtection),
            (
                Some("2"),
                Some("webm"),
                UnsupportedFeature::MimeType("video/webm".into())
            ),
            (
                Some("3"),
                Some("unindexed"),
                UnsupportedFeature::UnindexedResource
            ),
            (
                Some("3"),
                Some("rep-drm"),
                UnsupportedFeature::ContentProtection
            ),
            (
                Some("4"),
                None,
                UnsupportedFeature::XLink {
                    element: "AdaptationSet"
                }
            ),
            (
                Some("5"),
                None,
                UnsupportedFeature::EssentialProperty(
                    "http://dashif.org/guidelines/trickmode".into()
                )
            ),
        ]
    );
    assert!(period.ensure_usable().is_ok());
}

#[test]
fn xlink_period_is_unsupported() {
    let error = Manifest::parse(
        br#"<MPD xmlns:xlink="http://www.w3.org/1999/xlink"><Period xlink:href="https://x/p.xml"/></MPD>"#,
        &url(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ManifestError::Unsupported(UnsupportedFeature::XLink { element: "Period" })
    ));
}

#[test]
fn malformed_segment_information_fails_the_manifest() {
    let wrap = |inner: &str| {
        format!(
            r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period>
<AdaptationSet mimeType="video/mp4">{inner}<Representation id="v" bandwidth="1"/></AdaptationSet>
</Period></MPD>"#
        )
    };
    for inner in [
        r#"<SegmentTemplate duration="2"/>"#,
        r#"<SegmentTemplate duration="2" media="seg.m4s"/>"#,
        r#"<SegmentTemplate duration="2" media="$Unknown$.m4s"/>"#,
        r#"<SegmentTemplate duration="2" media="$Number$.m4s"><SegmentTimeline><S d="2"/></SegmentTimeline></SegmentTemplate>"#,
        r#"<SegmentTemplate duration="2" media="$Number$.m4s" initialization="$Number$.mp4"/>"#,
        r#"<SegmentTemplate duration="2" media="http://[bad/$Number$.m4s"/>"#,
        r#"<SegmentTemplate duration="2" media="$Number$"/><SegmentList duration="2"/>"#,
    ] {
        let xml = wrap(inner);
        let result = Manifest::parse(xml.as_bytes(), &url());
        assert!(
            matches!(&result, Err(error) if !matches!(error, ManifestError::Unsupported(_))),
            "{inner}: {result:?}"
        );
    }
    let xml = r#"<MPD><Period><AdaptationSet><Representation id="v" bandwidth="1"/></AdaptationSet></Period></MPD>"#;
    assert!(matches!(
        Manifest::parse(xml.as_bytes(), &url()),
        Err(ManifestError::MissingAttribute {
            attribute: "mimeType",
            ..
        })
    ));
    let duplicate = r#"<MPD><Period><AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1"/></AdaptationSet><AdaptationSet mimeType="audio/mp4"><Representation id="v" bandwidth="1"/></AdaptationSet></Period></MPD>"#;
    assert!(Manifest::parse(duplicate.as_bytes(), &url()).is_err());
}

#[test]
fn unsupported_segment_information_is_rejected() {
    let wrap = |inner: &str| {
        format!(
            r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period>
<AdaptationSet mimeType="video/mp4">{inner}<Representation id="v" bandwidth="1"/></AdaptationSet>
</Period></MPD>"#
        )
    };
    let cases = [
        (
            r#"<SegmentTemplate media="$Number$.m4s"/>"#,
            UnsupportedFeature::UntimedSegments,
        ),
        (
            r#"<SegmentTemplate duration="2" eptDelta="-5" media="$Number$.m4s"/>"#,
            UnsupportedFeature::EptDelta,
        ),
        (
            r#"<SegmentTemplate media="$Number$.m4s"><SegmentTimeline><S d="2" n="4"/></SegmentTimeline></SegmentTemplate>"#,
            UnsupportedFeature::TimelineAttribute("n"),
        ),
        (
            r#"<SegmentTemplate media="$Number$.m4s"><SegmentTimeline><S d="2" k="2"/></SegmentTimeline></SegmentTemplate>"#,
            UnsupportedFeature::TimelineAttribute("k"),
        ),
        (
            r#"<SegmentTemplate duration="2" media="$Number$-$SubNumber$.m4s"/>"#,
            UnsupportedFeature::TemplateIdentifier("SubNumber".into()),
        ),
        (
            r#"<BaseURL byteRange="$base$?r=$first$">a/</BaseURL><SegmentTemplate duration="2" media="$Number$"/>"#,
            UnsupportedFeature::BaseUrlByteRange,
        ),
        ("", UnsupportedFeature::UnindexedResource),
        (
            r#"<SegmentBase><RepresentationIndex sourceURL="index.sidx"/></SegmentBase>"#,
            UnsupportedFeature::RepresentationIndex,
        ),
        (
            r#"<SegmentBase indexRange="10-20"><RepresentationIndex sourceURL="index.sidx"/></SegmentBase>"#,
            UnsupportedFeature::RepresentationIndex,
        ),
        (
            r#"<SegmentBase indexRange="10-20" eptDelta="3"/>"#,
            UnsupportedFeature::EptDelta,
        ),
    ];
    for (inner, expected) in cases {
        let manifest = manifest(&wrap(inner));
        let period = &manifest.periods[0];
        assert!(period.adaptation_sets.is_empty(), "{inner}");
        assert_eq!(period.rejected[0].feature, expected, "{inner}");
    }
}

#[test]
fn utc_timing_and_location() {
    let parsed = manifest(
        r#"<MPD type="dynamic" availabilityStartTime="1970-01-01T00:00:00Z">
  <Location>next.mpd</Location>
  <UTCTiming schemeIdUri="urn:mpeg:dash:utc:ntp:2014" value="pool.ntp.org"/>
  <UTCTiming schemeIdUri="urn:mpeg:dash:utc:http-xsdate:2014" value="/time"/>
</MPD>"#,
    );
    assert_eq!(
        parsed.location.as_ref().map(Url::as_str),
        Some("https://origin.example.com/live/next.mpd")
    );
    assert_eq!(
        parsed.utc_timings,
        [UtcTiming::HttpXsDate {
            urls: vec![Url::parse("https://origin.example.com/time").unwrap()]
        }]
    );

    for bad_scheme in ["file:///etc/passwd", "ftp://origin.example.com/next.mpd"] {
        let non_http = manifest(&format!(
            r#"<MPD type="dynamic" availabilityStartTime="1970-01-01T00:00:00Z">
  <Location>{bad_scheme}</Location>
</MPD>"#
        ));
        assert_eq!(non_http.location, None, "{bad_scheme}");
    }
}

#[test]
fn has_scheme_follows_rfc_3986() {
    assert!(has_scheme("https://a/"));
    assert!(has_scheme("a+b.c-d:x"));
    assert!(!has_scheme("/path:with:colons"));
    assert!(!has_scheme("relative/seg.m4s"));
    assert!(!has_scheme("1http://a/"));
    assert!(!has_scheme(""));
}

fn wrap_set(inner: &str) -> String {
    format!(
        r#"<MPD type="static" mediaPresentationDuration="PT8S"><Period>
<AdaptationSet mimeType="video/mp4">{inner}</AdaptationSet>
</Period></MPD>"#
    )
}

#[test]
fn inherited_timelines_and_lists_are_shared_not_copied() {
    let parsed = manifest(&wrap_set(
        r#"<SegmentTemplate media="$Time$.m4s"><SegmentTimeline><S t="0" d="2" r="3"/></SegmentTimeline></SegmentTemplate>
<Representation id="a" bandwidth="1"/><Representation id="b" bandwidth="2"/>"#,
    ));
    let reps = &parsed.periods[0].adaptation_sets[0].representations;
    assert!(Arc::ptr_eq(
        &reps[0].addressing.runs,
        &reps[1].addressing.runs
    ));

    let parsed = manifest(&wrap_set(
        r#"<SegmentList duration="2"><SegmentURL media="1.m4s"/><SegmentURL media="2.m4s"/></SegmentList>
<Representation id="a" bandwidth="1"><BaseURL>a/</BaseURL></Representation>
<Representation id="b" bandwidth="2"><BaseURL>b/</BaseURL><SegmentList startNumber="5"/></Representation>"#,
    ));
    let reps = &parsed.periods[0].adaptation_sets[0].representations;
    let (SegmentUrls::List { urls: a, .. }, SegmentUrls::List { urls: b, .. }) =
        (&reps[0].addressing.urls, &reps[1].addressing.urls)
    else {
        panic!("expected list addressing");
    };
    assert!(Arc::ptr_eq(a, b));
    assert_eq!(reps[1].addressing_kind(), AddressingKind::List);
    let b_segments: Vec<_> = reps[1]
        .segments(at(0))
        .unwrap()
        .iter()
        .map(|segment| {
            let segment = segment.unwrap();
            (segment.number, segment.url.to_string())
        })
        .collect();
    assert_eq!(
        b_segments,
        [
            (5, "https://origin.example.com/live/b/1.m4s".to_owned()),
            (6, "https://origin.example.com/live/b/2.m4s".to_owned()),
        ]
    );
}

#[test]
fn hostile_amplification_is_rejected_cheaply() {
    // Inherited strings multiplied by many Representations exceed the
    // aggregate budget long before 10 000 × 4 KiB × several copies.
    let codecs = "a".repeat(4_000);
    let base = format!("https://cdn.example.com/{}/", "p".repeat(30_000));
    let reps: String = (0..10_000)
        .map(|i| format!(r#"<Representation id="{i}" bandwidth="1"/>"#))
        .collect();
    let xml = format!(
        r#"<MPD type="static" mediaPresentationDuration="PT2S"><BaseURL>{base}</BaseURL><Period>
<AdaptationSet mimeType="video/mp4" codecs="{codecs}"><SegmentTemplate duration="2" media="$RepresentationID$/$Number$"/>{reps}</AdaptationSet>
</Period></MPD>"#
    );
    assert!(matches!(
        Manifest::parse(xml.as_bytes(), &url()),
        Err(ManifestError::LimitExceeded {
            what: "resolved MPD data",
            ..
        })
    ));

    // Over-long identifiers and templates are refused while parsing.
    let long_id = "x".repeat(4 * 1024 * 1024);
    let xml = wrap_set(&format!(
        r#"<SegmentTemplate duration="2" media="{}$Number$"/><Representation id="{long_id}" bandwidth="1"/>"#,
        "$RepresentationID$".repeat(200_000)
    ));
    assert!(matches!(
        Manifest::parse(xml.as_bytes(), &url()),
        Err(ManifestError::LimitExceeded { .. })
    ));
    let id = "x".repeat(4_000);
    let xml = wrap_set(&format!(
        r#"<SegmentTemplate duration="2" media="{}$Number$"/><Representation id="{id}" bandwidth="1"/>"#,
        "$RepresentationID$".repeat(1_000)
    ));
    assert!(matches!(
        Manifest::parse(xml.as_bytes(), &url()),
        Err(ManifestError::LimitExceeded {
            what: "expanded URL template",
            ..
        })
    ));
}

#[test]
fn element_counts_are_bounded() {
    use super::super::limits::{MAX_PERIODS, MAX_REPRESENTATIONS, MAX_TIMELINE_ENTRIES};
    let periods = r#"<Period duration="PT1S"/>"#.repeat(MAX_PERIODS + 1);
    let xml = format!(r#"<MPD type="static">{periods}</MPD>"#);
    assert!(matches!(
        Manifest::parse(xml.as_bytes(), &url()),
        Err(ManifestError::LimitExceeded { .. })
    ));
    let reps: String = (0..=MAX_REPRESENTATIONS)
        .map(|i| format!(r#"<Representation id="{i}" bandwidth="1"/>"#))
        .collect();
    assert!(matches!(
        Manifest::parse(wrap_set(&reps).as_bytes(), &url()),
        Err(ManifestError::LimitExceeded { .. })
    ));
    let entries = r#"<S d="1"/>"#.repeat(MAX_TIMELINE_ENTRIES + 1);
    let xml = wrap_set(&format!(
        r#"<SegmentTemplate media="$Time$"><SegmentTimeline>{entries}</SegmentTimeline></SegmentTemplate><Representation id="v" bandwidth="1"/>"#
    ));
    assert!(matches!(
        Manifest::parse(xml.as_bytes(), &url()),
        Err(ManifestError::LimitExceeded { .. })
    ));
}

#[test]
fn negative_availability_time_offset_is_unsupported() {
    let manifest = manifest(&wrap_set(
        r#"<SegmentTemplate duration="2" media="$Number$"/>
<Representation id="negative" bandwidth="1"><SegmentTemplate availabilityTimeOffset="-1"/></Representation>
<Representation id="base" bandwidth="1"><BaseURL availabilityTimeOffset="-INF">x/</BaseURL></Representation>
<Representation id="fine" bandwidth="1"/>"#,
    ));
    let period = &manifest.periods[0];
    assert_eq!(period.adaptation_sets[0].representations.len(), 1);
    let features: Vec<_> = period.rejected.iter().map(|r| r.feature.clone()).collect();
    assert_eq!(
        features,
        [
            UnsupportedFeature::NegativeAvailabilityTimeOffset,
            UnsupportedFeature::NegativeAvailabilityTimeOffset,
        ]
    );
}

#[test]
fn contradictory_content_type_is_malformed() {
    let xml = r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period>
<AdaptationSet contentType="audio" mimeType="video/mp4"><SegmentTemplate duration="2" media="$Number$"/>
<Representation id="v" bandwidth="1"/></AdaptationSet></Period></MPD>"#;
    assert!(matches!(
        Manifest::parse(xml.as_bytes(), &url()),
        Err(ManifestError::Inconsistent { .. })
    ));
    let xml = xml.replace(r#"contentType="audio""#, r#"contentType="text""#);
    assert!(matches!(
        Manifest::parse(xml.as_bytes(), &url()),
        Err(ManifestError::Inconsistent { .. })
    ));
    let xml = xml.replace(r#"contentType="text""#, r#"contentType="video""#);
    assert!(Manifest::parse(xml.as_bytes(), &url()).is_ok());
}

#[test]
fn duplicate_ids_are_malformed() {
    for xml in [
        r#"<MPD type="static" mediaPresentationDuration="PT4S"><Period id="a" duration="PT2S"/><Period id="a"/></MPD>"#,
        r#"<MPD type="static" mediaPresentationDuration="PT2S"><Period><AdaptationSet id="1"/><AdaptationSet id="1"/></Period></MPD>"#,
    ] {
        assert!(
            matches!(
                Manifest::parse(xml.as_bytes(), &url()),
                Err(ManifestError::Inconsistent { .. })
            ),
            "{xml}"
        );
    }
    // The same AdaptationSet@id in different Periods is fine.
    let xml = r#"<MPD type="static" mediaPresentationDuration="PT4S"><Period duration="PT2S"><AdaptationSet id="1"/></Period><Period><AdaptationSet id="1"/></Period></MPD>"#;
    assert!(Manifest::parse(xml.as_bytes(), &url()).is_ok());
}

#[test]
fn addressing_conflict_at_one_level_is_malformed() {
    for inner in [
        r#"<SegmentBase/><SegmentTemplate duration="2" media="$Number$"/>"#,
        r#"<SegmentList duration="2"/><SegmentBase/>"#,
        r#"<SegmentTemplate duration="2" media="$Number$"/><SegmentList duration="2"/>"#,
    ] {
        let xml = wrap_set(&format!(
            r#"<Representation id="v" bandwidth="1">{inner}</Representation>"#
        ));
        assert!(
            matches!(
                Manifest::parse(xml.as_bytes(), &url()),
                Err(ManifestError::Inconsistent { .. })
            ),
            "{inner}"
        );
    }
    // Different levels are inheritance, not a conflict.
    let xml = wrap_set(
        r#"<SegmentTemplate duration="2" media="$Number$"/><Representation id="v" bandwidth="1"><SegmentBase/></Representation>"#,
    );
    let manifest = Manifest::parse(xml.as_bytes(), &url()).unwrap();
    assert_eq!(
        manifest.periods[0].rejected[0].feature,
        UnsupportedFeature::UnindexedResource
    );
}

fn subsegments(subsegments: &[(u64, u64, u64)]) -> Vec<super::super::Subsegment> {
    subsegments
        .iter()
        .map(|&(first, last, duration)| super::super::Subsegment {
            range: ByteRange { first, last },
            duration,
        })
        .collect()
}

fn indexed(entries: &[(u64, u64, u64)]) -> SubsegmentIndex {
    SubsegmentIndex::new(1000, 500, 900, &subsegments(entries)).unwrap()
}

#[test]
fn segment_base_waits_for_its_index() {
    let mut parsed = manifest(&wrap_set(
        r#"<Representation id="v" bandwidth="1"><BaseURL>v.mp4</BaseURL><SegmentBase indexRange="900-1099"/></Representation>"#,
    ));
    let representation = &mut parsed.periods[0].adaptation_sets[0].representations[0];
    assert_eq!(representation.addressing_kind(), AddressingKind::Indexed);
    let resource = Url::parse("https://origin.example.com/live/v.mp4").unwrap();
    assert_eq!(
        representation.pending_segment_index(),
        Some(&SegmentIndexLocation {
            url: resource.clone(),
            range: ByteRange {
                first: 900,
                last: 1099
            },
        })
    );
    // The initialization data precedes the index box, which may start
    // after `@indexRange` does; it is known once the index is read.
    assert_eq!(representation.initialization, None);
    assert!(matches!(
        representation.segments(at(0)),
        Err(ManifestError::SegmentIndexPending { .. })
    ));

    representation
        .apply_segment_index(&indexed(&[
            (1100, 1999, 2000),
            (2000, 2999, 2000),
            (3000, 3499, 1000),
        ]))
        .unwrap();
    assert_eq!(representation.pending_segment_index(), None);
    assert_eq!(
        representation.initialization,
        Some(InitSegment {
            url: resource.clone(),
            byte_range: Some(ByteRange {
                first: 0,
                last: 899
            }),
        })
    );
    assert!(representation.uses_segment_timeline());
    let segments: Vec<_> = representation
        .segments(at(0))
        .unwrap()
        .iter()
        .map(|segment| {
            let segment = segment.unwrap();
            assert_eq!(segment.url, resource);
            (
                segment.number,
                segment.time,
                segment.duration,
                segment.byte_range.map(|range| (range.first, range.last)),
            )
        })
        .collect();
    assert_eq!(
        segments,
        [
            (1, 500, 2000, Some((1100, 1999))),
            (2, 2500, 2000, Some((2000, 2999))),
            (3, 4500, 1000, Some((3000, 3499))),
        ]
    );
    assert!(
        representation
            .apply_segment_index(&indexed(&[(1, 2, 3)]))
            .is_err()
    );
}

#[test]
fn segment_base_inherits_and_takes_explicit_initialization() {
    let mut parsed = manifest(&wrap_set(
        r#"<SegmentBase timescale="90000" presentationTimeOffset="45000" indexRange="1-2"><Initialization sourceURL="init.mp4"/></SegmentBase>
<Representation id="v" bandwidth="1"><SegmentBase indexRange="600-700"><Initialization range="0-599"/></SegmentBase></Representation>
<Representation id="w" bandwidth="2"/>"#,
    ));
    let set = &mut parsed.periods[0].adaptation_sets[0];
    let document = url();
    let v = &set.representations[0];
    assert_eq!(v.pending_segment_index().unwrap().range.first, 600);
    // The Representation's `Initialization` replaces the inherited one.
    assert_eq!(
        v.initialization,
        Some(InitSegment {
            url: document.clone(),
            byte_range: Some(ByteRange {
                first: 0,
                last: 599
            }),
        })
    );
    assert_eq!(v.timescale(), 90_000);
    let w = &mut set.representations[1];
    assert_eq!(
        w.initialization.as_ref().map(|init| init.url.as_str()),
        Some("https://origin.example.com/live/init.mp4")
    );

    // 45000 / 90000 s is 500 ticks of the index's 1000 Hz.
    w.apply_segment_index(&indexed(&[(3, 9, 1000)])).unwrap();
    assert_eq!((w.timescale(), w.presentation_time_offset()), (1000, 500));
    let first = w.segments(at(0)).unwrap().first().unwrap().unwrap();
    assert_eq!((first.time, first.start_secs), (500, 0.0));
}

#[test]
fn an_unusable_segment_index_is_refused() {
    let xml = wrap_set(
        r#"<Representation id="v" bandwidth="1"><SegmentBase timescale="3" presentationTimeOffset="1" indexRange="0-9"/></Representation>"#,
    );
    for entries in [
        &[][..],
        &[(10, 20, 0)],
        &[(20, 10, 5)],
        &[(10, 20, 5), (20, 30, 5)],
        &[(10, 20, 5), (5, 8, 5)],
    ] {
        assert!(
            matches!(
                SubsegmentIndex::new(1000, 0, 0, &subsegments(entries)),
                Err(ManifestError::InvalidSegmentIndex { .. })
            ),
            "{entries:?}"
        );
    }
    // 1/3 s is not a whole number of milliseconds.
    let mut parsed = manifest(&xml);
    let representation = &mut parsed.periods[0].adaptation_sets[0].representations[0];
    assert!(matches!(
        representation.apply_segment_index(&indexed(&[(10, 20, 5)])),
        Err(ManifestError::Unsupported(
            UnsupportedFeature::FractionalPresentationTimeOffset
        ))
    ));
    assert!(representation.pending_segment_index().is_some());
}

#[test]
fn segment_base_is_refused_in_dynamic_mpds() {
    let manifest = manifest(
        r#"<MPD type="dynamic" availabilityStartTime="2020-01-01T00:00:00Z"><Period start="PT0S">
<AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1"><SegmentBase indexRange="10-20"/></Representation></AdaptationSet>
</Period></MPD>"#,
    );
    assert_eq!(
        manifest.periods[0].rejected[0].feature,
        UnsupportedFeature::LiveSegmentBase
    );
}

#[test]
fn segment_list_cardinality_matches_its_timeline() {
    let list = |timeline: &str, urls: usize| {
        let urls: String = (0..urls)
            .map(|i| format!(r#"<SegmentURL media="{i}.m4s"/>"#))
            .collect();
        wrap_set(&format!(
            r#"<SegmentList><SegmentTimeline>{timeline}</SegmentTimeline>{urls}</SegmentList><Representation id="v" bandwidth="1"/>"#
        ))
    };
    let parse = |xml: String| Manifest::parse(xml.as_bytes(), &url());
    // Two S entries with r=1 describe four segments.
    assert!(parse(list(r#"<S t="0" d="2" r="1"/><S d="2" r="1"/>"#, 4)).is_ok());
    for urls in [3, 5] {
        assert!(
            matches!(
                parse(list(r#"<S t="0" d="2" r="1"/><S d="2" r="1"/>"#, urls)),
                Err(ManifestError::Inconsistent { .. })
            ),
            "{urls}"
        );
    }
    // A trailing r=-1 takes the remaining URLs, at least one.
    let manifest = parse(list(r#"<S t="0" d="1"/><S d="2" r="-1"/>"#, 3)).unwrap();
    let representation = &manifest.periods[0].adaptation_sets[0].representations[0];
    assert!(representation.uses_segment_timeline());
    let segments: Vec<_> = representation
        .segments(at(0))
        .unwrap()
        .iter()
        .map(|segment| {
            let segment = segment.unwrap();
            (segment.time, segment.duration)
        })
        .collect();
    assert_eq!(segments, [(0, 1), (1, 2), (3, 2)]);
    assert!(matches!(
        parse(list(r#"<S t="0" d="1"/><S d="2" r="-1"/>"#, 1)),
        Err(ManifestError::Inconsistent { .. })
    ));
}

#[test]
fn mpd_chaining_and_unknown_essential_properties_are_unsupported() {
    for (xml, expected) in [
        (
            r#"<MPD type="static"><SupplementalProperty schemeIdUri="urn:mpeg:dash:mpd-chaining:2016" value="https://e.com/next.mpd"/><Period duration="PT1S"/></MPD>"#,
            UnsupportedFeature::MpdChaining("urn:mpeg:dash:mpd-chaining:2016".into()),
        ),
        (
            r#"<MPD type="static"><Period duration="PT1S"><EssentialProperty schemeIdUri="urn:mpeg:dash:fallback:2016" value="https://e.com/b.mpd"/></Period></MPD>"#,
            UnsupportedFeature::MpdChaining("urn:mpeg:dash:fallback:2016".into()),
        ),
        (
            r#"<MPD type="static"><EssentialProperty schemeIdUri="urn:example:must-understand"/><Period duration="PT1S"/></MPD>"#,
            UnsupportedFeature::EssentialProperty("urn:example:must-understand".into()),
        ),
    ] {
        let error = Manifest::parse(xml.as_bytes(), &url()).unwrap_err();
        assert!(
            matches!(&error, ManifestError::Unsupported(feature) if *feature == expected),
            "{xml}: {error}"
        );
    }
    // Unknown supplemental properties are optional and ignored.
    let xml = r#"<MPD type="static"><SupplementalProperty schemeIdUri="urn:example:hint"/><Period duration="PT1S"/></MPD>"#;
    assert!(Manifest::parse(xml.as_bytes(), &url()).is_ok());
}

#[test]
fn end_number_below_start_number_is_malformed() {
    let xml = wrap_set(
        r#"<SegmentTemplate duration="2" media="$Number$" startNumber="10" endNumber="9"/><Representation id="v" bandwidth="1"/>"#,
    );
    assert!(matches!(
        Manifest::parse(xml.as_bytes(), &url()),
        Err(ManifestError::Inconsistent { .. })
    ));
}
