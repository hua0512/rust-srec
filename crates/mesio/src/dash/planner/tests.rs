use super::*;
use crate::dash::manifest::ManualClock;

const MPD_URL: &str = "https://cdn.example.com/live/manifest.mpd?token=abc";

fn ast() -> Timestamp {
    "2024-01-01T00:00:00Z".parse::<jiff::Timestamp>().unwrap()
}

/// A planner driven like the engine drives it: the start is decided
/// from the first MPD, the manifest generation advances only when the
/// MPD changes, and consumption is simulated through the frontier.
struct Fixture {
    track: MediaTrack,
    live_start: LiveStartPolicy,
    clock: Arc<ManualClock>,
    frontier: Arc<ConsumedFrontier>,
    planner: Option<DashPlanner>,
    generation: u64,
    manifest_generation: u64,
    last_mpd: Option<String>,
    /// Configured tracks the session does not record (see
    /// [`PlannerParams::absent_tracks`]).
    absent_tracks: Vec<MediaTrack>,
}

impl Fixture {
    fn new(track: MediaTrack, live_start: LiveStartPolicy, elapsed_secs: i64) -> Self {
        Self {
            track,
            live_start,
            clock: Arc::new(ManualClock::new(
                ast() + jiff::SignedDuration::from_secs(elapsed_secs),
            )),
            frontier: Arc::new(ConsumedFrontier::default()),
            planner: None,
            generation: 0,
            manifest_generation: 0,
            last_mpd: None,
            absent_tracks: Vec::new(),
        }
    }

    fn snapshot(&mut self, mpd: &str, edit: impl FnOnce(&mut Manifest)) -> MpdSnapshot {
        let url = Url::parse(MPD_URL).unwrap();
        let mut manifest = Manifest::parse(mpd.as_bytes(), &url).expect("valid MPD");
        edit(&mut manifest);
        self.generation += 1;
        if self.last_mpd.as_deref() != Some(mpd) {
            self.manifest_generation += 1;
            self.last_mpd = Some(mpd.to_owned());
        }
        MpdSnapshot {
            generation: self.generation,
            manifest_generation: self.manifest_generation,
            manifest: Arc::new(manifest),
            document_url: Arc::new(url.clone()),
            parent_params: crate::segment_engine::descriptor::query_params(url.query()).into(),
            fetched_at: self.clock.now(),
            clock_offset: crate::dash::manifest::ClockOffset::ZERO,
            terminal: None,
        }
    }

    fn plan(&mut self, mpd: &str) -> PlanOutcome {
        self.plan_edited(mpd, |_| {})
    }

    /// Plans `mpd` after `edit` changed its resolved form, as the
    /// watcher does when it applies Segment Indexes.
    fn plan_edited(&mut self, mpd: &str, edit: impl FnOnce(&mut Manifest)) -> PlanOutcome {
        let snapshot = self.snapshot(mpd, edit);
        let planner = self.planner.get_or_insert_with(|| {
            let start = decide_start(
                &snapshot.manifest,
                self.clock.now(),
                self.live_start,
                &DashSelectionConfig::default(),
            )
            .expect("start decision")
            .expect("a starting Period");
            DashPlanner::new(PlannerParams {
                track: self.track,
                selection: Arc::new(DashSelectionConfig::default()),
                clock: Arc::clone(&self.clock) as Arc<dyn Clock>,
                identity: SegmentIdentityPolicy::default(),
                frontier: Arc::clone(&self.frontier),
                start,
                live_start: self.live_start,
                absent_tracks: self.absent_tracks.clone(),
            })
        });
        planner.plan(&snapshot)
    }

    /// The assembler emitted everything planned so far.
    fn consume_all(&self) {
        if let Some(planner) = &self.planner {
            self.frontier.advance_to(planner.next_ordinal);
        }
    }

    fn advance(&self, secs: u64) {
        self.clock.advance(std::time::Duration::from_secs(secs));
    }
}

fn media(outcome: &PlanOutcome) -> Vec<(u64, &DashSegmentMeta, &str)> {
    outcome
        .descriptors
        .iter()
        .filter(|descriptor| descriptor.key.kind == SegmentKind::Media)
        .map(|descriptor| match &descriptor.meta {
            SegmentMeta::Dash(meta) => (
                descriptor.msn,
                meta.as_ref(),
                descriptor.parsed_url.as_str(),
            ),
            SegmentMeta::Hls(_) => panic!("DASH planner produced HLS metadata"),
        })
        .collect()
}

/// The wall-clock anchor a segment's output would carry now.
fn wall_clock(meta: &DashSegmentMeta) -> Option<Timestamp> {
    meta.availability_start_time
        .as_ref()
        .and_then(WallClockAnchor::get)
}

/// Whether `outcome` plans media and all of it is anchored at [`ast`].
fn anchored(outcome: &PlanOutcome) -> bool {
    let media = media(outcome);
    !media.is_empty()
        && media
            .iter()
            .all(|(_, meta, _)| wall_clock(meta) == Some(ast()))
}

fn init(outcome: &PlanOutcome) -> &SegmentDescriptor {
    outcome
        .descriptors
        .iter()
        .find(|descriptor| descriptor.key.kind == SegmentKind::Init)
        .expect("init descriptor")
}

fn number_mpd(extra: &str) -> String {
    format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S" {extra}>
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="2000" timescale="1000" startNumber="1"/>
  <Representation id="v1" bandwidth="1000000"/>
</AdaptationSet>
<AdaptationSet mimeType="audio/mp4" lang="en">
  <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="2000" timescale="1000" startNumber="1"/>
  <Representation id="a1" bandwidth="128000"/>
</AdaptationSet>
  </Period>
</MPD>"#
    )
}

#[test]
fn a_track_appearing_in_the_running_period_is_reported_once_and_not_planned() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 20);
    fixture.absent_tracks = vec![MediaTrack::Audio];
    let video_only = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="2000" timescale="1000" startNumber="1"/>
  <Representation id="v1" bandwidth="1000000"/>
</AdaptationSet>
  </Period>
</MPD>"#;
    let absent = |fixture: &Fixture| fixture.planner.as_ref().unwrap().absent_tracks.clone();
    fixture.plan(video_only);
    fixture.consume_all();
    fixture.advance(2);
    fixture.plan(video_only);
    assert_eq!(
        absent(&fixture),
        [MediaTrack::Audio],
        "nothing to report yet"
    );

    // An update of the same Period adds audio: reported on this
    // snapshot, whether or not it plans anything, and never again.
    fixture.consume_all();
    let with_audio = number_mpd("");
    let update = fixture.plan(&with_audio);
    assert!(absent(&fixture).is_empty());
    assert!(
        media(&update)
            .iter()
            .all(|(_, meta, _)| meta.track == MediaTrack::Video)
    );
    fixture.advance(2);
    let next = fixture.plan(&with_audio);
    assert!(absent(&fixture).is_empty());
    assert!(
        media(&next)
            .iter()
            .all(|(_, meta, _)| meta.track == MediaTrack::Video)
    );
}

#[test]
fn number_template_starts_behind_the_edge_and_plans_new_segments_once() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 20);
    let mpd = number_mpd("");
    let first = fixture.plan(&mpd);
    // 3 × 2 s behind t=20 s → the segment covering 14 s, number 8.
    let numbers: Vec<(u64, u64)> = media(&first)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.number))
        .collect();
    assert_eq!(numbers, [(0, 8), (1, 9), (2, 10)]);
    assert_eq!(
        init(&first).key.scope,
        media(&first)
            .first()
            .map(|_| first.descriptors[0].key.scope.clone())
            .unwrap()
    );
    assert!(first.terminal.is_none());
    assert_eq!(first.prune.map(|prune| prune.start), Some(0));
    assert!(matches!(
        first.notices.as_slice(),
        [EngineNotice::ManifestRefreshed {
            sequence_base: 0,
            ..
        }]
    ));

    let again = fixture.plan(&mpd);
    assert!(media(&again).is_empty(), "nothing is planned twice");
    assert!(again.notices.is_empty());

    fixture.consume_all();
    fixture.advance(4);
    let next = fixture.plan(&mpd);
    let numbers: Vec<(u64, u64)> = media(&next)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.number))
        .collect();
    assert_eq!(numbers, [(3, 11), (4, 12)]);
}

#[test]
fn segment_urls_inherit_the_mpd_query_and_carry_epoch_identity() {
    let mut fixture = Fixture::new(MediaTrack::Audio, LiveStartPolicy::LiveEdge, 20);
    let outcome = fixture.plan(&number_mpd(""));
    let planned = media(&outcome);
    assert_eq!(planned.len(), 1, "the live edge is the newest segment");
    assert_eq!(
        planned[0].2,
        "https://cdn.example.com/live/a/10.m4s?token=abc"
    );
    assert_eq!(
        init(&outcome).parsed_url.as_str(),
        "https://cdn.example.com/live/a/init.mp4?token=abc"
    );
    let media_descriptor = outcome
        .descriptors
        .iter()
        .find(|descriptor| descriptor.key.kind == SegmentKind::Media)
        .unwrap();
    assert_eq!(
        media_descriptor.init_key.as_ref(),
        Some(&init(&outcome).key)
    );
    assert!(media_descriptor.key.scope.is_some());
    assert_eq!(
        media_descriptor.meta.container(),
        Some(crate::segment_engine::container::ContainerKind::Fmp4)
    );
}

#[test]
fn timeline_generations_keep_consecutive_ordinals_when_entries_expire() {
    let timeline = |entries: &str| {
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Time$.m4s" initialization="v/init.mp4" timescale="1000">
    <SegmentTimeline>{entries}</SegmentTimeline>
  </SegmentTemplate>
  <Representation id="v1" bandwidth="1000000"/>
</AdaptationSet>
  </Period>
</MPD>"#
        )
    };
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
    let first = fixture.plan(&timeline(r#"<S t="0" d="2000" r="4"/>"#));
    let times: Vec<(u64, u64)> = media(&first)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.time))
        .collect();
    assert_eq!(times, [(0, 8000)]);

    fixture.consume_all();
    fixture.advance(4);
    // The first three entries expired; numbers shift, times do not.
    let next = fixture.plan(&timeline(r#"<S t="6000" d="2000" r="3"/>"#));
    let times: Vec<(u64, u64)> = media(&next)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.time))
        .collect();
    assert_eq!(times, [(1, 10000), (2, 12000)]);
    assert!(media(&next).iter().all(|(_, _, url)| url.contains("/v/1")));
}

#[test]
fn period_change_opens_a_new_epoch_with_its_own_init() {
    let two_periods = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="p0/$Number$.m4s" initialization="init.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
  <Period id="p1" start="PT10S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="p1/$Number$.m4s" initialization="init.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#;
    let mut fixture = Fixture::new(
        MediaTrack::Video,
        LiveStartPolicy::PresentationDelay(Some(Duration::from_secs(4))),
        8,
    );
    let first = fixture.plan(two_periods);
    let first_init = init(&first).key.clone();
    assert_eq!(
        media(&first)
            .iter()
            .map(|(msn, _, _)| *msn)
            .collect::<Vec<_>>(),
        [0, 1]
    );

    fixture.consume_all();
    fixture.advance(6);
    let next = fixture.plan(two_periods);
    let planned: Vec<(u64, Option<&str>)> = media(&next)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.period_id.as_deref()))
        .collect();
    assert_eq!(planned, [(2, Some("p0")), (3, Some("p1")), (4, Some("p1"))]);
    let p1_media = next
        .descriptors
        .iter()
        .find(|descriptor| descriptor.msn == 3 && descriptor.key.kind == SegmentKind::Media)
        .unwrap();
    let second_init = p1_media.init_key.clone().unwrap();
    assert!(
        next.descriptors
            .iter()
            .any(|descriptor| descriptor.key == second_init),
        "the new epoch's init is planned with its first media"
    );
    assert_eq!(second_init.uri, first_init.uri, "same URL in both Periods");
    assert_ne!(second_init, first_init, "but a distinct resource");
}

/// `@duration` Periods of 2 s segments: `(id, start)`, the last one
/// open, each numbered from 1 under its own URLs.
fn duration_periods(ast: &str, periods: &[(&str, u64)]) -> String {
    let periods: String = periods
        .iter()
        .map(|(id, start)| {
            format!(
                r#"<Period id="{id}" start="PT{start}S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="{id}/$Number$.m4s" initialization="init.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>"#
            )
        })
        .collect();
    format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="{ast}" minimumUpdatePeriod="PT2S">
  {periods}
</MPD>"#
    )
}

fn planned_periods(outcome: &PlanOutcome) -> Vec<(u64, Option<&str>, u64)> {
    media(outcome)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.period_id.as_deref(), meta.number))
        .collect()
}

fn absent_periods(outcome: &PlanOutcome) -> Vec<(Option<&str>, u64)> {
    outcome
        .notices
        .iter()
        .filter_map(|notice| match notice {
            EngineNotice::TrackAbsent {
                section_id,
                after_sequence,
                ..
            } => Some((section_id.as_deref(), *after_sequence)),
            _ => None,
        })
        .collect()
}

/// Plans the open p0 of `duration_periods` from 8 s through 14 s:
/// ordinals 0–3 are its segments 4–7, the last two (10–14 s) past
/// where p1 will start.
fn plan_open_p0_through_14s(fixture: &mut Fixture) {
    let open = duration_periods("2024-01-01T00:00:00Z", &[("p0", 0)]);
    assert_eq!(planned_periods(&fixture.plan(&open)), [(0, Some("p0"), 4)]);
    fixture.consume_all();
    fixture.advance(6);
    assert_eq!(
        planned_periods(&fixture.plan(&open)),
        [(1, Some("p0"), 5), (2, Some("p0"), 6), (3, Some("p0"), 7)]
    );
}

#[test]
fn a_period_revealed_inside_planned_media_is_planned_from_its_start() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 8);
    plan_open_p0_through_14s(&mut fixture);
    fixture.consume_all();
    // p1's first two segments repeat p0's 10–14 s; whether p0's copy
    // reached the output is only known downstream.
    let revealed = duration_periods("2024-01-01T00:00:00Z", &[("p0", 0), ("p1", 10)]);
    let outcome = fixture.plan(&revealed);
    assert_eq!(
        planned_periods(&outcome),
        [(4, Some("p1"), 1), (5, Some("p1"), 2)]
    );
    fixture.consume_all();
    fixture.advance(2);
    let next = fixture.plan(&revealed);
    assert_eq!(planned_periods(&next), [(6, Some("p1"), 3)]);
}

#[test]
fn a_period_inside_planned_media_lets_later_gaps_be_announced() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 8);
    plan_open_p0_through_14s(&mut fixture);
    fixture.consume_all();
    // p1 (10–12 s) lies within p0's overhang; p2 carries no video.
    let revealed = duration_periods("2024-01-01T00:00:00Z", &[("p0", 0), ("p1", 10)])
        .replace("</MPD>", &format!("{}\n</MPD>", r#"<Period id="p2" start="PT12S">
<AdaptationSet mimeType="audio/mp4">
  <SegmentTemplate media="p2/a/$Number$.m4s" initialization="a/init.mp4" duration="2" timescale="1"/>
  <Representation id="a" bandwidth="1000"/>
</AdaptationSet>
  </Period>"#));
    let outcome = fixture.plan(&revealed);
    assert_eq!(planned_periods(&outcome), [(4, Some("p1"), 1)]);
    assert_eq!(absent_periods(&outcome), [(Some("p2"), 5)]);
}

#[test]
fn a_gap_is_announced_after_an_inner_periods_media_expires() {
    let mpd = |periods: &str| {
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S" timeShiftBufferDepth="PT2S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="p0/$Number$.m4s" initialization="init.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
  {periods}
</MPD>"#
        )
    };
    let p1 = r#"<Period id="p1" start="PT10S" duration="PT2S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="p1/$Number$.m4s" initialization="init.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>"#;
    let p2 = r#"<Period id="p2" start="PT12S">
<AdaptationSet mimeType="audio/mp4">
  <SegmentTemplate media="p2/a/$Number$.m4s" initialization="a/init.mp4" duration="2" timescale="1"/>
  <Representation id="a" bandwidth="1000"/>
</AdaptationSet>
  </Period>"#;
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 8);
    // The open p0 is planned through 14 s.
    for step in 0..4 {
        if step > 0 {
            fixture.advance(2);
        }
        assert_eq!(planned_periods(&fixture.plan(&mpd(""))).len(), 1);
        fixture.consume_all();
    }
    // At 14 s p1 (10–12 s) is revealed while its segment is available.
    let revealed = fixture.plan(&mpd(p1));
    assert_eq!(planned_periods(&revealed), [(4, Some("p1"), 1)]);
    fixture.consume_all();
    // At 16 s p1's segment has left the window; p2 has no video.
    fixture.advance(2);
    let later = fixture.plan(&mpd(&format!("{p1}{p2}")));
    assert_eq!(absent_periods(&later), [(Some("p2"), 5)]);
}

#[test]
fn a_vanished_representation_is_replaced_by_a_new_epoch() {
    let with = |id: &str| {
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="$RepresentationID$/$Number$.m4s" initialization="$RepresentationID$/init.mp4" duration="2" timescale="1"/>
  <Representation id="{id}" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#
        )
    };
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
    let first = fixture.plan(&with("hd"));
    assert_eq!(media(&first)[0].1.number, 5);
    fixture.consume_all();
    fixture.advance(2);
    let next = fixture.plan(&with("sd"));
    let planned = media(&next);
    assert_eq!(planned.len(), 1);
    assert_eq!((planned[0].0, planned[0].1.number), (1, 6));
    assert_eq!(&*planned[0].1.representation_id, "sd");
    assert_ne!(init(&next).key, init(&first).key);
}

#[test]
fn a_representation_whose_index_was_not_read_is_replaced() {
    let live = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <Representation id="low" bandwidth="100">
    <SegmentTemplate media="low/$Time$.m4s" initialization="low.mp4" timescale="1">
      <SegmentTimeline><S t="0" d="2" r="4"/></SegmentTimeline>
    </SegmentTemplate>
  </Representation>
</AdaptationSet>
  </Period>
</MPD>"#;
    // The final MPD moves both Representations to `SegmentBase`; only
    // the selected one's index is read. `low` keeps its initialization,
    // timescale and timeline, so its epoch's fingerprint is unchanged.
    let finished = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT14S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <Representation id="low" bandwidth="100"><BaseURL>low.mp4</BaseURL><SegmentBase indexRange="100-199"><Initialization/></SegmentBase></Representation>
  <Representation id="high" bandwidth="900"><BaseURL>high.mp4</BaseURL><SegmentBase indexRange="100-199"/></Representation>
</AdaptationSet>
  </Period>
</MPD>"#;
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
    assert!(!media(&fixture.plan(live)).is_empty());
    let next = fixture.plan_edited(finished, |manifest| {
        let subsegments: Vec<_> = (0..7)
            .map(|n| crate::dash::manifest::Subsegment {
                range: ByteRange {
                    first: 200 + n * 10,
                    last: 209 + n * 10,
                },
                duration: 2,
            })
            .collect();
        let index = crate::dash::manifest::SubsegmentIndex::new(1, 0, 100, &subsegments).unwrap();
        let high = &mut manifest.periods[0].adaptation_sets[0].representations[1];
        high.apply_segment_index(&index).unwrap();
    });
    // Everything is planned, so the static presentation ends; it does
    // not fail on the unread `low`.
    assert!(
        matches!(next.terminal, Some(TerminalCause::End)),
        "{:?}",
        next.terminal
    );
    let planned = media(&next);
    assert!(!planned.is_empty());
    assert!(
        planned
            .iter()
            .all(|(_, meta, _)| &*meta.representation_id == "high")
    );
}

#[test]
fn an_unread_index_the_planner_does_not_need_is_ignored() {
    let template = r#"<SegmentTemplate media="low/$Number$.m4s" initialization="low/init.mp4" duration="2" timescale="1"/>"#;
    let live = format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S"><AdaptationSet mimeType="video/mp4">
<Representation id="low" bandwidth="100">{template}</Representation>
  </AdaptationSet></Period>
</MPD>"#
    );
    // The final MPD adds `high`, whose index could not be read; the
    // recording keeps `low` to its end.
    let finished = format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT14S">
  <Period id="p0" start="PT0S"><AdaptationSet mimeType="video/mp4">
<Representation id="low" bandwidth="100">{template}</Representation>
<Representation id="high" bandwidth="900"><BaseURL>high.mp4</BaseURL><SegmentBase indexRange="100-199"/></Representation>
  </AdaptationSet></Period>
</MPD>"#
    );
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
    assert!(!media(&fixture.plan(&live)).is_empty());
    fixture.consume_all();
    let next = fixture.plan(&finished);
    assert!(
        matches!(next.terminal, Some(TerminalCause::End)),
        "{:?}",
        next.terminal
    );
    let planned = media(&next);
    assert_eq!(
        planned
            .iter()
            .map(|(_, meta, _)| (&*meta.representation_id, meta.number))
            .collect::<Vec<_>>(),
        [("low", 6), ("low", 7)]
    );
}

#[test]
fn work_planned_before_a_switch_to_indexed_addressing_keeps_its_urls() {
    let live = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S"><AdaptationSet mimeType="video/mp4">
<Representation id="low" bandwidth="100">
  <SegmentTemplate media="low/$Time$.m4s" initialization="low.mp4" timescale="1">
    <SegmentTimeline><S t="0" d="2" r="4"/></SegmentTimeline>
  </SegmentTemplate>
</Representation>
  </AdaptationSet></Period>
</MPD>"#;
    // Same Representation, initialization, timescale and timeline, but
    // now byte ranges of `low.mp4`.
    let finished = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT14S">
  <Period id="p0" start="PT0S"><AdaptationSet mimeType="video/mp4">
<Representation id="low" bandwidth="100"><BaseURL>low.mp4</BaseURL><SegmentBase indexRange="100-199"><Initialization/></SegmentBase></Representation>
  </AdaptationSet></Period>
</MPD>"#;
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
    // Planned, not yet downloaded: reissued on the next MPD.
    assert!(!media(&fixture.plan(live)).is_empty());
    let next = fixture.plan_edited(finished, |manifest| {
        let subsegments: Vec<_> = (0..7)
            .map(|n| crate::dash::manifest::Subsegment {
                range: ByteRange {
                    first: 200 + n * 10,
                    last: 209 + n * 10,
                },
                duration: 2,
            })
            .collect();
        let index = crate::dash::manifest::SubsegmentIndex::new(1, 0, 100, &subsegments).unwrap();
        let low = &mut manifest.periods[0].adaptation_sets[0].representations[0];
        low.apply_segment_index(&index).unwrap();
    });
    let indexed: Vec<_> = next
        .descriptors
        .iter()
        .filter(|descriptor| {
            descriptor.key.kind == SegmentKind::Media
                && descriptor.parsed_url.path().ends_with("/low.mp4")
        })
        .collect();
    assert!(!indexed.is_empty());
    // Never the whole resource under a key planned for a template URL.
    assert!(
        indexed
            .iter()
            .all(|descriptor| descriptor.key.byte_range.is_some())
    );
}

#[test]
fn an_unread_index_in_a_later_period_waits_for_earlier_work() {
    let template = r#"<SegmentTemplate media="low/$Number$.m4s" initialization="low/init.mp4" duration="2" timescale="1"/>"#;
    let live = format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S"><AdaptationSet mimeType="video/mp4">
<Representation id="low" bandwidth="100">{template}</Representation>
  </AdaptationSet></Period>
</MPD>"#
    );
    // `p1`'s only Representation has an unread index.
    let finished = format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT20S">
  <Period id="p0" start="PT0S" duration="PT10S"><AdaptationSet mimeType="video/mp4">
<Representation id="low" bandwidth="100">{template}</Representation>
  </AdaptationSet></Period>
  <Period id="p1" start="PT10S"><AdaptationSet mimeType="video/mp4">
<Representation id="v" bandwidth="100"><BaseURL>v.mp4</BaseURL><SegmentBase indexRange="100-199"/></Representation>
  </AdaptationSet></Period>
</MPD>"#
    );
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 6);
    assert!(!media(&fixture.plan(&live)).is_empty());
    fixture.consume_all();
    let next = fixture.plan(&finished);
    assert_eq!(next.terminal, None);
    assert_eq!(
        media(&next)
            .iter()
            .map(|(_, meta, _)| meta.number)
            .collect::<Vec<_>>(),
        [4, 5]
    );
    fixture.consume_all();
    let last = fixture.plan(&finished);
    assert!(
        matches!(&last.terminal, Some(TerminalCause::Failed(error)) if error.to_string().contains("has not been read")),
        "{:?}",
        last.terminal
    );
    // When the read failed, the stream ends with that failure.
    let failed = fixture.plan_edited(&finished, |manifest| {
        manifest.periods[1].adaptation_sets[0].representations[0].fail_segment_index(
            SharedError::new(DownloadError::InvalidContent {
                protocol: "dash",
                reason: "corrupt sidx".to_owned(),
            }),
        );
    });
    let Some(TerminalCause::Failed(error)) = failed.terminal else {
        panic!("expected a failure, got {:?}", failed.terminal);
    };
    let error = error.to_error();
    assert!(
        matches!(&error, DownloadError::InvalidContent { reason, .. } if reason == "corrupt sidx"),
        "{error:?}"
    );
}

#[test]
fn static_presentation_plans_everything_and_ends() {
    let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT6S">
  <Period>
<AdaptationSet mimeType="audio/mp4">
  <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="2" timescale="1"/>
  <Representation id="a" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#;
    let mut fixture = Fixture::new(MediaTrack::Audio, LiveStartPolicy::LiveEdge, 0);
    let outcome = fixture.plan(mpd);
    assert_eq!(
        media(&outcome)
            .iter()
            .map(|(msn, _, _)| *msn)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(outcome.terminal, Some(TerminalCause::End));
    assert_eq!(
        outcome.prune,
        Some(PruneWindow {
            start: 0,
            len: usize::MAX
        }),
        "static backlog is never dropped"
    );
}

#[test]
fn availability_end_time_ends_the_stream_after_the_last_segments() {
    let mpd = number_mpd(r#"availabilityEndTime="2024-01-01T00:00:12Z""#);
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 9);
    let first = fixture.plan(&mpd);
    assert_eq!(media(&first)[0].1.number, 4);
    assert!(first.terminal.is_none());
    fixture.consume_all();
    fixture.advance(30);
    let last = fixture.plan(&mpd);
    let numbers: Vec<u64> = media(&last)
        .iter()
        .map(|(_, meta, _)| meta.number)
        .collect();
    assert_eq!(numbers, [5, 6]);
    assert_eq!(last.terminal, Some(TerminalCause::End));
}

#[test]
fn failed_snapshots_are_not_planned() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 20);
    fixture.plan(&number_mpd(""));
    let url = Url::parse(MPD_URL).unwrap();
    let snapshot = MpdSnapshot {
        generation: 7,
        manifest_generation: 1,
        manifest: Arc::new(Manifest::parse(number_mpd("").as_bytes(), &url).unwrap()),
        document_url: Arc::new(url),
        parent_params: Vec::new().into(),
        fetched_at: fixture.clock.now(),
        clock_offset: crate::dash::manifest::ClockOffset::ZERO,
        terminal: Some(TerminalCause::Failed(SharedError::new(
            DownloadError::Manifest {
                reason: "gone".to_owned(),
            },
        ))),
    };
    let planner = fixture.planner.as_mut().unwrap();
    let outcome = planner.plan(&snapshot);
    assert!(outcome.descriptors.is_empty());
    assert_eq!(outcome.terminal, snapshot.terminal);
}

fn reissued(outcome: &PlanOutcome) -> Vec<(u64, SegmentKey, String)> {
    outcome
        .descriptors
        .iter()
        .filter(|descriptor| descriptor.key.kind == SegmentKind::Media)
        .map(|descriptor| {
            (
                descriptor.msn,
                descriptor.key.clone(),
                descriptor.parsed_url.to_string(),
            )
        })
        .collect()
}

#[test]
fn an_mpd_update_reissues_unconsumed_work_with_fresh_urls() {
    let with_media = |media: &str| {
        number_mpd("").replace(r#"media="v/$Number$.m4s""#, &format!(r#"media="{media}""#))
    };
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 20);
    let first = reissued(&fixture.plan(&with_media("v/$Number$.m4s")));
    assert_eq!(first.len(), 3);
    // The assembler emitted the first segment; two are still queued.
    fixture.frontier.advance_to(1);

    let update = fixture.plan(&with_media("edge2/v/$Number$.m4s"));
    let again = reissued(&update);
    assert_eq!(
        again.iter().map(|(msn, ..)| *msn).collect::<Vec<_>>(),
        [1, 2],
        "unconsumed work keeps its ordinals; nothing new is available"
    );
    for ((_, key, _), (_, old_key, _)) in again.iter().zip(&first[1..]) {
        assert_eq!(key, old_key, "the store refreshes the record in place");
    }
    assert!(again.iter().all(|(_, _, url)| url.contains("/edge2/v/")));
    let init = init(&update);
    assert!(init.parsed_url.as_str().contains("/v/init.mp4"));

    // Republishing the same MPD issues nothing again.
    assert!(reissued(&fixture.plan(&with_media("edge2/v/$Number$.m4s"))).is_empty());
}

#[test]
fn an_init_change_under_the_same_ids_opens_a_new_epoch() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 20);
    let first = fixture.plan(&number_mpd(""));
    let first_init = init(&first).key.clone();
    fixture.consume_all();
    fixture.advance(2);
    let changed = number_mpd("").replace(
        r#"initialization="v/init.mp4""#,
        r#"initialization="v/init-2.mp4""#,
    );
    let next = fixture.plan(&changed);
    let planned = media(&next);
    assert_eq!(
        planned.iter().map(|(msn, ..)| *msn).collect::<Vec<_>>(),
        [1]
    );
    assert_eq!(planned[0].1.number, 11, "continues after the last segment");
    let media_init = next
        .descriptors
        .iter()
        .find(|descriptor| descriptor.msn == 1 && descriptor.key.kind == SegmentKind::Media)
        .and_then(|descriptor| descriptor.init_key.clone())
        .unwrap();
    assert_ne!(media_init, first_init);
    assert!(media_init.uri.contains("init-2.mp4"));

    // A new presentation time offset is a new epoch too, even with an
    // unchanged init URL: downstream must see a new init to re-time.
    fixture.consume_all();
    fixture.advance(2);
    let shifted = changed.replace(
        r#"duration="2000" timescale="1000" startNumber="1"/>
  <Representation id="v1""#,
        r#"duration="2000" timescale="1000" startNumber="1" presentationTimeOffset="5000"/>
  <Representation id="v1""#,
    );
    assert_ne!(shifted, changed);
    let third = fixture.plan(&shifted);
    let third_init = third
        .descriptors
        .iter()
        .find(|descriptor| descriptor.key.kind == SegmentKind::Init)
        .unwrap();
    assert_eq!(third_init.key.uri, media_init.uri);
    assert_ne!(third_init.key, media_init, "same URL, new epoch scope");
}

fn many_periods(count: usize) -> String {
    let periods: String = (0..count)
        .map(|index| {
            format!(
                r#"<Period id="p{index}" duration="PT2S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="p{index}/$Number$.m4s" initialization="init.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>"#
            )
        })
        .collect();
    format!(r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">{periods}</MPD>"#)
}

#[test]
fn epochs_are_admitted_one_ahead_of_consumption() {
    let mpd = many_periods(10);
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    let mut inits = 0;
    let mut planned = Vec::new();
    for _ in 0..20 {
        let outcome = fixture.plan(&mpd);
        let opened = outcome
            .descriptors
            .iter()
            .filter(|descriptor| descriptor.key.kind == SegmentKind::Init)
            .count();
        assert!(opened <= 2, "at most the current and the next epoch");
        inits += opened;
        planned.extend(media(&outcome).iter().map(|(msn, ..)| *msn));
        if outcome.terminal == Some(TerminalCause::End) {
            break;
        }
        fixture.consume_all();
    }
    assert_eq!(inits, 10);
    assert_eq!(planned, (0..10).collect::<Vec<_>>());
}

#[test]
fn huge_windows_are_planned_in_pages() {
    let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="P365D">
  <Period>
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="1" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#;
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    let first = fixture.plan(mpd);
    assert_eq!(media(&first).len(), PLANNING_LOOKAHEAD as usize);
    assert!(first.terminal.is_none());
    assert!(media(&fixture.plan(mpd)).is_empty(), "the window is full");
    fixture.frontier.advance_to(10);
    let next = media(&fixture.plan(mpd))
        .iter()
        .map(|(msn, ..)| *msn)
        .collect::<Vec<_>>();
    assert_eq!(next, (256..266).collect::<Vec<_>>());
}

#[test]
fn a_later_unusable_period_fails_typed_after_the_earlier_work() {
    let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="clear" duration="PT4S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="c/$Number$.m4s" initialization="c/init.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
  <Period id="drm" duration="PT4S">
<AdaptationSet mimeType="video/mp4">
  <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
  <SegmentTemplate media="d/$Number$.m4s" initialization="d/init.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#;
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    let first = fixture.plan(mpd);
    assert_eq!(media(&first).len(), 2);
    assert!(
        first.terminal.is_none(),
        "the clear Period is delivered first"
    );
    fixture.consume_all();
    let failed = fixture.plan(mpd);
    let Some(TerminalCause::Failed(error)) = failed.terminal else {
        panic!("expected a failure, got {:?}", failed.terminal);
    };
    let error = error.to_error();
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
}

#[test]
fn the_start_is_decided_once_for_every_track() {
    let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="p0/$Number$.m4s" initialization="v.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
<AdaptationSet mimeType="audio/mp4">
  <SegmentTemplate media="p0/a$Number$.m4s" initialization="a.mp4" duration="2" timescale="1"/>
  <Representation id="a" bandwidth="1000"/>
</AdaptationSet>
  </Period>
  <Period id="p1" start="PT10S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="p1/$Number$.m4s" initialization="v.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#;
    let url = Url::parse(MPD_URL).unwrap();
    let manifest = Manifest::parse(mpd.as_bytes(), &url).unwrap();
    // 12 s in, 6 s behind: the start lies in p0, which has audio, even
    // though the newest Period that has begun (p1) has none.
    let start = decide_start(
        &manifest,
        ast() + jiff::SignedDuration::from_secs(12),
        LiveStartPolicy::PresentationDelay(Some(Duration::from_secs(6))),
        &DashSelectionConfig::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(start.period.id.as_deref(), Some("p0"));
    assert_eq!(start.after_nanos, Some(6 * NANOS_PER_SEC));
}

#[test]
fn reissue_looks_pending_segments_up_without_scanning_the_gap() {
    let timeline = |entries: &str| {
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT1000001S">
  <Period id="p0">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Time$.m4s" initialization="v/init.mp4" timescale="1000">
    <SegmentTimeline>{entries}</SegmentTimeline>
  </SegmentTemplate>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#
        )
    };
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    let first = fixture.plan(&timeline(r#"<S t="0" d="1"/><S t="1000000000" d="1"/>"#));
    assert_eq!(media(&first).len(), 2);
    // The update fills the gap with a billion one-tick segments.
    let started = std::time::Instant::now();
    let update = fixture.plan(&timeline(r#"<S t="0" d="1" r="1000000000"/>"#));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "refresh work is proportional to the pending segments"
    );
    let mut reissued: Vec<(u64, u64)> = media(&update)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.time))
        .collect();
    reissued.sort_unstable();
    assert_eq!(reissued, [(0, 0), (1, 1_000_000_000)]);
}

const CLEAR_VIDEO: &str = r#"<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="2" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>"#;
const CLEAR_AUDIO: &str = r#"<AdaptationSet mimeType="audio/mp4">
  <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="2" timescale="1"/>
  <Representation id="a" bandwidth="1000"/>
</AdaptationSet>"#;
const PROTECTED_AUDIO: &str = r#"<AdaptationSet mimeType="audio/mp4">
  <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
  <SegmentTemplate media="pa/$Number$.m4s" initialization="pa/init.mp4" duration="2" timescale="1"/>
  <Representation id="pa" bandwidth="2000"/>
</AdaptationSet>"#;
const PROTECTED_VIDEO: &str = r#"<AdaptationSet mimeType="video/mp4">
  <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
  <SegmentTemplate media="pv/$Number$.m4s" initialization="pv/init.mp4" duration="2" timescale="1"/>
  <Representation id="pv" bandwidth="2000"/>
</AdaptationSet>"#;

fn static_periods(periods: &[&str]) -> String {
    let periods: String = periods
        .iter()
        .enumerate()
        .map(|(index, sets)| format!(r#"<Period id="p{index}" duration="PT4S">{sets}</Period>"#))
        .collect();
    format!(r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">{periods}</MPD>"#)
}

#[test]
fn rejections_count_only_against_their_own_track() {
    let second = format!("{CLEAR_AUDIO}{PROTECTED_AUDIO}");
    let first = format!("{CLEAR_VIDEO}{CLEAR_AUDIO}");
    let mpd = static_periods(&[&first, &second]);
    let url = Url::parse(MPD_URL).unwrap();
    let manifest = Manifest::parse(mpd.as_bytes(), &url).unwrap();
    let later = &manifest.periods[1];
    assert_eq!(rejection_for(later, MediaTrack::Video), None);
    assert!(rejection_for(later, MediaTrack::Audio).is_some());

    // The video track is merely absent from the second Period.
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    let outcome = fixture.plan(&mpd);
    assert_eq!(media(&outcome).len(), 2);
    assert_eq!(outcome.terminal, Some(TerminalCause::End));
}

#[test]
fn a_static_presentation_starts_at_its_first_period_even_if_unusable() {
    let first = format!("{PROTECTED_VIDEO}{CLEAR_AUDIO}");
    let second = format!("{CLEAR_VIDEO}{CLEAR_AUDIO}");
    let mpd = static_periods(&[&first, &second]);
    let url = Url::parse(MPD_URL).unwrap();
    let manifest = Manifest::parse(mpd.as_bytes(), &url).unwrap();
    let start = decide_start(
        &manifest,
        ast(),
        LiveStartPolicy::LiveEdge,
        &DashSelectionConfig::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(start.period.id.as_deref(), Some("p0"));
    assert!(matches!(
        rejection_for(&manifest.periods[0], MediaTrack::Video),
        Some(UnsupportedFeature::ContentProtection)
    ));
}

#[test]
fn rejected_text_image_and_metadata_belong_to_no_media_track() {
    let sets = [
        r#"<AdaptationSet contentType="text" mimeType="text/vtt"><Representation id="t1" bandwidth="1"><BaseURL>t.vtt</BaseURL></Representation></AdaptationSet>"#,
        r#"<AdaptationSet mimeType="application/mp4" codecs="stpp"><Representation id="t2" bandwidth="1"><BaseURL>t.mp4</BaseURL></Representation></AdaptationSet>"#,
        r#"<AdaptationSet contentType="image" mimeType="image/jpeg"><Representation id="i" bandwidth="1"><BaseURL>i.jpg</BaseURL></Representation></AdaptationSet>"#,
        r#"<AdaptationSet mimeType="application/x-unknown"><Representation id="u" bandwidth="1"><BaseURL>u</BaseURL></Representation></AdaptationSet>"#,
    ];
    let mpd = static_periods(&[&format!("{CLEAR_VIDEO}{}", sets.concat())]);
    let url = Url::parse(MPD_URL).unwrap();
    let manifest = Manifest::parse(mpd.as_bytes(), &url).unwrap();
    let contents: Vec<RejectedContent> = manifest.periods[0]
        .rejected
        .iter()
        .map(|rejected| rejected.content)
        .collect();
    assert_eq!(
        contents,
        [
            RejectedContent::Other,
            RejectedContent::Other,
            RejectedContent::Other,
            RejectedContent::Unknown
        ]
    );
    // ISO BMFF text is rejected as text, not for its container.
    let features: Vec<&UnsupportedFeature> = manifest.periods[0]
        .rejected
        .iter()
        .map(|rejected| &rejected.feature)
        .collect();
    assert_eq!(
        features,
        [
            &UnsupportedFeature::NotAudioVideo("text/vtt".into()),
            &UnsupportedFeature::NotAudioVideo("application/mp4".into()),
            &UnsupportedFeature::NotAudioVideo("image/jpeg".into()),
            &UnsupportedFeature::MimeType("application/x-unknown".into()),
        ]
    );
    let known = static_periods(&[&format!("{CLEAR_VIDEO}{}", sets[..3].concat())]);
    let manifest = Manifest::parse(known.as_bytes(), &url).unwrap();
    assert_eq!(rejection_for(&manifest.periods[0], MediaTrack::Audio), None);
    assert_eq!(rejection_for(&manifest.periods[0], MediaTrack::Video), None);
}

#[test]
fn a_video_gap_is_announced_only_after_overlapping_video_is_planned() {
    // p0 ends at 10 s with 4 s video segments, so its last one, [8, 12),
    // becomes available only at 12 s; p1 (audio only) starts at 10 s.
    let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT1S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="4" timescale="1"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
  <Period id="p1" start="PT10S">
<AdaptationSet mimeType="audio/mp4">
  <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="1" timescale="1"/>
  <Representation id="a" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#;
    let absent = |outcome: &PlanOutcome| -> Vec<u64> {
        outcome
            .notices
            .iter()
            .filter_map(|notice| match notice {
                EngineNotice::TrackAbsent { after_sequence, .. } => Some(*after_sequence),
                _ => None,
            })
            .collect()
    };
    let mut fixture = Fixture::new(
        MediaTrack::Video,
        LiveStartPolicy::PresentationDelay(Some(Duration::from_secs(11))),
        11,
    );
    let early = fixture.plan(mpd);
    assert_eq!(
        media(&early)
            .iter()
            .map(|(_, meta, _)| meta.number)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(
        absent(&early).is_empty(),
        "segment 3 of p0 is still to come"
    );
    fixture.consume_all();
    fixture.advance(1);
    let complete = fixture.plan(mpd);
    assert_eq!(
        media(&complete)
            .iter()
            .map(|(msn, meta, _)| (*msn, meta.number))
            .collect::<Vec<_>>(),
        [(2, 3)]
    );
    assert_eq!(absent(&complete), [3], "after every p0 video segment");
}

#[test]
fn a_period_covered_exactly_by_fractional_segments_is_complete() {
    // Nine 100100/30000 s segments end exactly at 30.03 s, where summing
    // separately truncated nanoseconds would fall 1 ns short.
    let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="p0" duration="PT30.03S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4" duration="100100" timescale="30000"/>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
  <Period id="p1" duration="PT2S">
<AdaptationSet mimeType="audio/mp4">
  <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4" duration="1" timescale="1"/>
  <Representation id="a" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#;
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    let outcome = fixture.plan(mpd);
    assert_eq!(media(&outcome).len(), 9);
    let absent: Vec<u64> = outcome
        .notices
        .iter()
        .filter_map(|notice| match notice {
            EngineNotice::TrackAbsent { after_sequence, .. } => Some(*after_sequence),
            _ => None,
        })
        .collect();
    assert_eq!(absent, [9], "p1 is announced after p0's nine segments");
}

#[test]
fn segment_bounds_floor_negative_offsets_exactly() {
    // PTO 2 ticks at 3 ticks/s: the first segment starts 2/3 s before
    // the Period (at 10 s) and ends 2/3 s after it.
    let mpd = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT20S">
  <Period id="p0" start="PT10S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Time$.m4s" initialization="v/init.mp4" timescale="3" presentationTimeOffset="2">
    <SegmentTimeline><S t="0" d="4"/><S d="4"/></SegmentTimeline>
  </SegmentTemplate>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#;
    let url = Url::parse(MPD_URL).unwrap();
    let manifest = Manifest::parse(mpd.as_bytes(), &url).unwrap();
    let period = &manifest.periods[0];
    let representation = &period.adaptation_sets[0].representations[0];
    let window = representation.segments(ast()).unwrap();
    let bounds: Vec<(i128, i128)> = window
        .iter()
        .map(|segment| segment_bounds(&segment.unwrap(), representation, period))
        .collect();
    assert_eq!(
        bounds,
        [
            // 10 s − 666 666 666.67 ns floors to −…667; +666 666 666.67 to …666.
            (9_333_333_333, 10_666_666_666),
            (10_666_666_666, 12_000_000_000),
        ]
    );
}

/// Consecutive live MPDs from a TikTok origin (sanitised; see the
/// fixtures' `served` comments for each response's `Date`). Its edges
/// alternate between two descriptions of the same stream: one moves
/// `availabilityStartTime` to the response time and lists only the
/// newest segment at `t="0"`; the other lists real media times that
/// lie ~45 minutes past its `availabilityStartTime`. Numbers agree.
const TIKTOK_LIVE: [&str; 18] = [
    include_str!("../../../tests/fixtures/dash/tiktok-live/01.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/02.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/03.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/04.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/05.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/06.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/07.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/08.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/09.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/10.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/11.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/12.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/13.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/14.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/15.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/16.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/17.mpd"),
    include_str!("../../../tests/fixtures/dash/tiktok-live/18.mpd"),
];

/// The server time a fixture MPD was served at.
fn served(mpd: &str) -> Timestamp {
    let (_, rest) = mpd.split_once("<!-- served: ").expect("served comment");
    let (time, _) = rest.split_once(" -->").expect("served comment end");
    jiff::fmt::temporal::DateTimeParser::new()
        .parse_timestamp(time)
        .expect("served time")
}

/// `(ordinal, number)` of every media descriptor, checking that a
/// reissued ordinal keeps its number.
fn record_numbers(planned: &mut std::collections::BTreeMap<u64, u64>, outcome: &PlanOutcome) {
    for (msn, meta, _) in media(outcome) {
        if let Some(previous) = planned.insert(msn, meta.number) {
            assert_eq!(previous, meta.number, "ordinal {msn} changed its segment");
        }
    }
}

fn init_scopes(outcome: &PlanOutcome) -> Vec<Option<Arc<str>>> {
    outcome
        .descriptors
        .iter()
        .filter(|descriptor| descriptor.key.kind == SegmentKind::Init)
        .map(|descriptor| descriptor.key.scope.clone())
        .collect()
}

#[test]
fn tiktok_live_updates_are_followed_by_segment_number() {
    for track in [MediaTrack::Video, MediaTrack::Audio] {
        let mut fixture = Fixture::new(track, LiveStartPolicy::default(), 0);
        let mut planned = std::collections::BTreeMap::new();
        let mut scopes = std::collections::HashSet::new();
        let mut anchors = Vec::new();
        for mpd in TIKTOK_LIVE {
            fixture
                .clock
                .set(served(mpd) + jiff::SignedDuration::from_millis(500));
            let outcome = fixture.plan(mpd);
            assert!(
                outcome.terminal.is_none(),
                "{track}: {:?}",
                outcome.terminal
            );
            record_numbers(&mut planned, &outcome);
            scopes.extend(init_scopes(&outcome));
            anchors.extend(
                media(&outcome)
                    .iter()
                    .map(|(msn, meta, _)| (*msn, wall_clock(meta))),
            );
            fixture.consume_all();
        }
        // Every number once, in order, under consecutive ordinals: 1330
        // is only ever listed by the far-ahead description.
        let ordinals: Vec<u64> = planned.keys().copied().collect();
        let numbers: Vec<u64> = planned.values().copied().collect();
        assert_eq!(ordinals, (0..13).collect::<Vec<_>>(), "{track}");
        assert_eq!(numbers, (1319..=1331).collect::<Vec<_>>(), "{track}");
        assert_eq!(scopes.len(), 1, "{track}: one epoch, one init");
        // Wall-clock anchoring stops with the first contradiction.
        assert!(anchors.first().is_some_and(|(_, anchor)| anchor.is_some()));
        assert!(anchors.last().is_some_and(|(_, anchor)| anchor.is_none()));
    }
}

/// A video-only `$Number$` timeline shaped like TikTok's: 2 s segments
/// (`t` in milliseconds) from `start_number`, published with
/// `availabilityStartTime` `ast_secs` after [`ast`].
fn number_timeline(ast_secs: i64, start_number: u64, times: &[u64]) -> String {
    timeline_variant(ast_secs, start_number, times, "v/init.mp4", "v")
}

/// [`number_timeline`] with another initialization URL or
/// Representation id.
fn timeline_variant(
    ast_secs: i64,
    start_number: u64,
    times: &[u64],
    init: &str,
    representation: &str,
) -> String {
    relocated_timeline(ast_secs, start_number, times, init, "v", representation)
}

/// [`timeline_variant`] with its media under `media_dir`.
fn relocated_timeline(
    ast_secs: i64,
    start_number: u64,
    times: &[u64],
    init: &str,
    media_dir: &str,
    representation: &str,
) -> String {
    let ast = (ast() + jiff::SignedDuration::from_secs(ast_secs)).to_string();
    let entries: String = times
        .iter()
        .map(|time| format!(r#"<S t="{time}" d="2000"/>"#))
        .collect();
    format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic" minimumUpdatePeriod="PT1S"
 timeShiftBufferDepth="PT12S" availabilityStartTime="{ast}" publishTime="{ast}">
  <Period id="dash" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="{media_dir}/$Number$.m4s" initialization="{init}" timescale="1000"
      availabilityTimeOffset="1.95" startNumber="{start_number}">
    <SegmentTimeline>{entries}</SegmentTimeline>
  </SegmentTemplate>
  <Representation id="{representation}" bandwidth="1000000"/>
</AdaptationSet>
  </Period>
</MPD>"#
    )
}

fn planned_numbers(outcome: &PlanOutcome) -> Vec<(u64, u64)> {
    media(outcome)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.number))
        .collect()
}

#[test]
fn contradicting_updates_never_regress_or_replan() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
    let at = |fixture: &Fixture, secs: i64| {
        fixture
            .clock
            .set(ast() + jiff::SignedDuration::from_millis(secs * 1000 + 500));
    };
    // The newest segment starts at the anchor, as the response time.
    at(&fixture, 100);
    let first = fixture.plan(&number_timeline(100, 50, &[0]));
    assert_eq!(planned_numbers(&first), [(0, 50)]);
    assert_eq!(
        wall_clock(media(&first)[0].1),
        Some(ast() + jiff::SignedDuration::from_secs(100))
    );

    // Real media times, ~16 minutes past the anchor: followed anyway.
    at(&fixture, 101);
    let ahead = fixture.plan(&number_timeline(101, 50, &[1_000_000, 1_002_000]));
    assert_eq!(
        planned_numbers(&ahead),
        [(0, 50), (1, 51)],
        "50 is reissued"
    );
    assert!(
        media(&ahead)
            .iter()
            .all(|(_, meta, _)| wall_clock(meta).is_none()),
        "neither new nor reissued work keeps the contradicted anchor"
    );
    assert_eq!(
        wall_clock(media(&first)[0].1),
        None,
        "work planned before the contradiction loses it too"
    );

    // `t` back at 0 for an already planned number: nothing new.
    at(&fixture, 102);
    let behind = fixture.plan(&number_timeline(102, 51, &[0]));
    assert!(planned_numbers(&behind).iter().all(|(msn, _)| *msn < 2));
    assert!(behind.terminal.is_none());

    // The next number, again at `t="0"`.
    at(&fixture, 103);
    fixture.consume_all();
    let next = fixture.plan(&number_timeline(103, 52, &[0]));
    assert_eq!(planned_numbers(&next), [(2, 52)]);

    // The anchor moves backwards and the times forwards again.
    at(&fixture, 104);
    fixture.consume_all();
    let later = fixture.plan(&number_timeline(
        100,
        51,
        &[1_002_000, 1_004_000, 1_006_000],
    ));
    assert_eq!(planned_numbers(&later), [(3, 53)]);
    fixture.consume_all();

    // Cached copies, their newest number 5 and 10 behind #53: not a
    // restart, so nothing is planned again.
    let mut outcomes = vec![first, ahead, behind, next, later];
    for (start_number, times) in [
        (46, [992_000, 994_000, 996_000]),
        (41, [982_000, 984_000, 986_000]),
    ] {
        let stale = fixture.plan(&number_timeline(101, start_number, &times));
        assert!(media(&stale).is_empty(), "from {start_number}");
        outcomes.push(stale);
    }
    // The next current description continues after #53.
    let current = fixture.plan(&number_timeline(
        100,
        52,
        &[1_004_000, 1_006_000, 1_008_000],
    ));
    assert_eq!(planned_numbers(&current), [(4, 54)]);
    outcomes.push(current);

    let scopes: std::collections::HashSet<_> = outcomes.iter().flat_map(init_scopes).collect();
    assert_eq!(scopes.len(), 1, "one epoch throughout");
}

/// Plans #50 of [`number_timeline`] described at its anchor (100 s),
/// consumes it, then contradicts the Period's timing: #50 and #51
/// described ~16 minutes past an anchor moved by a second. Returns both
/// outcomes; #51 is not consumed.
fn contradict_number_timeline(fixture: &mut Fixture) -> (PlanOutcome, PlanOutcome) {
    set_clock(fixture, 100_500);
    let first = fixture.plan(&number_timeline(100, 50, &[0]));
    fixture.consume_all();
    set_clock(fixture, 101_500);
    let ahead = fixture.plan(&number_timeline(101, 50, &[1_000_000, 1_002_000]));
    (first, ahead)
}

#[test]
fn each_timing_conflict_is_warned_once_per_track() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
    let (_, ahead) = contradict_number_timeline(&mut fixture);
    assert_eq!(planned_numbers(&ahead), [(1, 51)]);
    // All three kinds were seen, so none is reported again.
    let planner = fixture.planner.as_mut().unwrap();
    for conflict in [
        TimingConflict::AnchorMoved,
        TimingConflict::AheadOfClock,
        TimingConflict::TimesRewritten,
    ] {
        assert!(!planner.first_conflict(conflict), "{conflict:?}");
    }
}

#[test]
fn a_number_timeline_whose_entries_expire_is_followed_by_number() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    // Consistent with the clock: segment n (from 1) ends at 2n s.
    fixture
        .clock
        .set(ast() + jiff::SignedDuration::from_millis(8_100));
    let first = fixture.plan(&number_timeline(0, 1, &[0, 2000, 4000, 6000]));
    assert_eq!(planned_numbers(&first), [(0, 4)]);
    fixture.consume_all();
    fixture.advance(4);
    // Two entries expired; `startNumber` follows.
    let next = fixture.plan(&number_timeline(0, 3, &[4000, 6000, 8000, 10000]));
    assert_eq!(planned_numbers(&next), [(1, 5), (2, 6)]);
    assert!(anchored(&next));
}

#[test]
fn a_number_timeline_jumping_forward_continues_after_the_gap() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    // #50–#52 end at 2–6 s.
    set_clock(&fixture, 6_500);
    let first = fixture.plan(&number_timeline(0, 50, &[0, 2000, 4000]));
    assert_eq!(planned_numbers(&first), [(0, 52)]);
    fixture.consume_all();
    // #53–#59 left the MPD before a refresh: #60 follows #52 under the
    // next ordinal, still anchored.
    set_clock(&fixture, 22_500);
    let jumped = fixture.plan(&number_timeline(0, 60, &[20_000]));
    assert_eq!(planned_numbers(&jumped), [(1, 60)]);
    assert!(anchored(&jumped));
    fixture.consume_all();
    set_clock(&fixture, 24_500);
    let next = fixture.plan(&number_timeline(0, 60, &[20_000, 22_000]));
    assert_eq!(planned_numbers(&next), [(2, 61)]);
}

#[test]
fn a_time_template_timeline_stays_keyed_by_time() {
    let timeline = |start_number: u64, entries: &str| {
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Time$.m4s" initialization="v/init.mp4" timescale="1000" startNumber="{start_number}">
    <SegmentTimeline>{entries}</SegmentTimeline>
  </SegmentTemplate>
  <Representation id="v1" bandwidth="1000000"/>
</AdaptationSet>
  </Period>
</MPD>"#
        )
    };
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
    let first = fixture.plan(&timeline(1, r#"<S t="0" d="2000" r="4"/>"#));
    let times: Vec<(u64, u64)> = media(&first)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.time))
        .collect();
    assert_eq!(times, [(0, 8000)]);
    fixture.consume_all();
    fixture.advance(4);
    // The numbers jump; the media times carry on and decide.
    let next = fixture.plan(&timeline(100, r#"<S t="6000" d="2000" r="3"/>"#));
    let times: Vec<(u64, u64)> = media(&next)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.time))
        .collect();
    assert_eq!(times, [(1, 10000), (2, 12000)]);
    assert!(anchored(&next));
}

#[test]
fn a_timeline_listing_the_segment_in_production_waits_for_the_clock() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    // Available segments end by 9 + 1.95 s: 1–5 end at 2–10 s.
    fixture
        .clock
        .set(ast() + jiff::SignedDuration::from_secs(9));
    let mpd = number_timeline(0, 1, &[0, 2000, 4000, 6000, 8000]);
    let first = fixture.plan(&mpd);
    assert_eq!(planned_numbers(&first), [(0, 5)]);
    fixture.consume_all();
    let mpd = number_timeline(0, 1, &[0, 2000, 4000, 6000, 8000, 10000]);
    let early = fixture.plan(&mpd);
    // Segment 6 ends 1.05 s past the window: a segment in production,
    // not a contradiction.
    assert!(media(&early).is_empty());
    fixture.advance(2);
    let next = fixture.plan(&mpd);
    assert_eq!(planned_numbers(&next), [(1, 6)]);
    assert!(anchored(&next));
}

/// Sets the clock `millis` after [`ast`].
fn set_clock(fixture: &Fixture, millis: i64) {
    fixture
        .clock
        .set(ast() + jiff::SignedDuration::from_millis(millis));
}

#[test]
fn a_first_mpd_far_ahead_of_the_clock_waits_for_a_consistent_one() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    // One MPD cannot show that its timing is contradicted: the clock
    // decides, and nothing listed is available by it.
    set_clock(&fixture, 500);
    let ahead = fixture.plan(&number_timeline(
        0,
        1321,
        &[2_669_356, 2_671_356, 2_673_356],
    ));
    assert!(media(&ahead).is_empty());
    // The next refresh describes the newest segment as starting now.
    set_clock(&fixture, 1_500);
    let consistent = fixture.plan(&number_timeline(1, 1324, &[0]));
    assert_eq!(planned_numbers(&consistent), [(0, 1324)]);
    fixture.consume_all();
    // Once the anchor moved, the far-ahead description is followed.
    set_clock(&fixture, 2_500);
    let followed = fixture.plan(&number_timeline(2, 1324, &[2_675_356, 2_677_356]));
    assert_eq!(planned_numbers(&followed), [(1, 1325)]);
    assert_eq!(wall_clock(media(&followed)[0].1), None);
}

#[test]
fn an_anchor_moving_between_far_ahead_descriptions_lets_them_through() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    // An origin that only ever describes media far ahead of an anchor
    // it moves: the second description shows the contradiction.
    set_clock(&fixture, 500);
    let first = fixture.plan(&number_timeline(0, 1321, &[2_669_356, 2_671_356]));
    assert!(media(&first).is_empty());
    set_clock(&fixture, 1_500);
    let second = fixture.plan(&number_timeline(1, 1322, &[2_671_356, 2_673_356]));
    // Nothing was available at the start, so whatever is listed now.
    assert_eq!(planned_numbers(&second), [(0, 1322), (1, 1323)]);
    assert_eq!(wall_clock(media(&second)[0].1), None);
}

#[test]
fn times_rewritten_under_a_fixed_anchor_let_far_ahead_descriptions_through() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    // #50 starts at the anchor, then the same anchor describes #50 and
    // #51 ~16 minutes later: nothing the clock calls available shows
    // the contradiction, the listing does.
    set_clock(&fixture, 500);
    let first = fixture.plan(&number_timeline(0, 50, &[0]));
    assert_eq!(planned_numbers(&first), [(0, 50)]);
    fixture.consume_all();
    set_clock(&fixture, 1_500);
    let rewritten = fixture.plan(&number_timeline(0, 50, &[1_000_000, 1_002_000]));
    assert_eq!(planned_numbers(&rewritten), [(1, 51)]);
    assert_eq!(wall_clock(media(&first)[0].1), None);
    assert_eq!(wall_clock(media(&rewritten)[0].1), None);
}

#[test]
fn a_running_period_listing_only_future_segments_waits_for_the_clock() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    set_clock(&fixture, 4_500);
    let first = fixture.plan(&number_timeline(0, 1, &[0, 2000]));
    assert_eq!(planned_numbers(&first), [(0, 2)]);
    fixture.consume_all();
    // After a gap, only a segment from 100 s on is listed. Nothing
    // contradicts the timing, so it is not requested before then.
    set_clock(&fixture, 20_000);
    let mpd = number_timeline(0, 3, &[100_000]);
    assert!(media(&fixture.plan(&mpd)).is_empty());
    set_clock(&fixture, 98_000);
    assert!(media(&fixture.plan(&mpd)).is_empty());
    set_clock(&fixture, 100_100);
    let next = fixture.plan(&mpd);
    assert_eq!(planned_numbers(&next), [(1, 3)]);
    assert!(anchored(&next));
}

#[test]
fn a_switch_to_another_representation_never_compares_numbers() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    // `v` #51 covers 8–10 s.
    set_clock(&fixture, 10_500);
    let first = fixture.plan(&steady_timeline(50, &[6000, 8000], "v/init.mp4", "v"));
    assert_eq!(planned_numbers(&first), [(0, 51)]);
    fixture.consume_all();
    // `w` numbers the same interval differently: #50 is 8–10 s, #51 is
    // 10–12 s. That is no rewritten time, and `w` #51 is next.
    set_clock(&fixture, 12_500);
    let switched = fixture.plan(&steady_timeline(50, &[8000, 10000], "w/init.mp4", "w"));
    let planned: Vec<(u64, u64, u64)> = media(&switched)
        .iter()
        .map(|(msn, meta, _)| (*msn, meta.number, meta.time))
        .collect();
    assert_eq!(planned, [(1, 51, 10000)]);
    assert!(anchored(&switched));
}

#[test]
fn a_moved_anchor_reaches_superseded_epochs() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    set_clock(&fixture, 10_500);
    let first = fixture.plan(&steady_timeline(50, &[6000, 8000], "v/init.mp4", "v"));
    assert_eq!(planned_numbers(&first), [(0, 51)]);
    assert_eq!(wall_clock(media(&first)[0].1), Some(ast()));
    // #51 is still buffered when an update changes both the
    // initialization and the anchor, with media times moving on.
    set_clock(&fixture, 12_500);
    let next = fixture.plan(&timeline_variant(1, 51, &[8000, 10000], "v/init2.mp4", "v"));
    assert_eq!(planned_numbers(&next), [(1, 52)]);
    assert_eq!(wall_clock(media(&first)[0].1), None);
    assert_eq!(wall_clock(media(&next)[0].1), None);
}

/// A live MPD at 20 s whose current Period `p0` runs to 100 s, with
/// `p1` preannounced from 100 s.
fn preannounced_period(media: &str) -> String {
    format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="p0/{media}" initialization="p0/init.mp4" timescale="1">
    <SegmentTimeline><S t="0" d="2" r="49"/></SegmentTimeline>
  </SegmentTemplate>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
  <Period id="p1" start="PT100S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="p1/{media}" initialization="p1/init.mp4" timescale="1">
    <SegmentTimeline><S t="0" d="2" r="4"/></SegmentTimeline>
  </SegmentTemplate>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#
    )
}

#[test]
fn a_preannounced_period_never_becomes_the_live_edge() {
    for media in ["$Number$.m4s", "$Time$.m4s"] {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 20);
        let mpd = preannounced_period(media);
        let first = fixture.plan(&mpd);
        let planned: Vec<(Option<&str>, u64, u64)> = media_of(&first);
        // The segment ending at 20 s: number 10, time 18.
        assert_eq!(planned, [(Some("p0"), 10, 18)], "{media}");
        fixture.consume_all();
        fixture.advance(2);
        let next = fixture.plan(&mpd);
        assert_eq!(media_of(&next), [(Some("p0"), 11, 20)], "{media}");
        assert!(anchored(&next), "{media}");
    }
}

fn media_of(outcome: &PlanOutcome) -> Vec<(Option<&str>, u64, u64)> {
    media(outcome)
        .iter()
        .map(|(_, meta, _)| (meta.period_id.as_deref(), meta.number, meta.time))
        .collect()
}

#[test]
fn a_timeline_scheduling_future_segments_is_not_trusted_over_the_clock() {
    let timeline = |media: &str, entries: &str| {
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/{media}" initialization="v/init.mp4" timescale="1">
    <SegmentTimeline>{entries}</SegmentTimeline>
  </SegmentTemplate>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#
        )
    };
    // Listed up to 200 s at 20 s: the clock decides, whatever the key.
    for media in ["$Number$.m4s", "$Time$.m4s"] {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 20);
        let outcome = fixture.plan(&timeline(media, r#"<S t="0" d="2" r="99"/>"#));
        assert_eq!(media_of(&outcome), [(Some("p0"), 10, 18)], "{media}");
        assert!(anchored(&outcome), "{media}");
    }
    // Everything listed lies ahead, but `$Time$` URLs are built from
    // those very times: wait for the clock.
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 20);
    let outcome = fixture.plan(&timeline("$Time$.m4s", r#"<S t="1000" d="2" r="4"/>"#));
    assert!(media(&outcome).is_empty());
}

#[test]
fn epoch_changes_during_contradicting_updates_continue_by_number() {
    let variants = [("v/init2.mp4", "v"), ("v/init.mp4", "v2")];
    for (init, representation) in variants {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
        let (first, ahead) = contradict_number_timeline(&mut fixture);
        assert_eq!(planned_numbers(&ahead), [(1, 51)]);
        fixture.consume_all();
        // A new init or Representation, described at `t="0"` again.
        set_clock(&fixture, 102_500);
        let switched = fixture.plan(&timeline_variant(102, 52, &[0], init, representation));
        assert_eq!(
            planned_numbers(&switched),
            [(2, 52)],
            "{init} {representation}"
        );
        let scopes: std::collections::HashSet<_> = [first, ahead, switched]
            .iter()
            .flat_map(init_scopes)
            .collect();
        assert_eq!(scopes.len(), 2, "a new epoch");
        fixture.consume_all();
        // The new epoch keeps following numbers across descriptions.
        set_clock(&fixture, 103_500);
        let later = fixture.plan(&timeline_variant(
            103,
            52,
            &[1_004_000, 1_006_000],
            init,
            representation,
        ));
        assert_eq!(planned_numbers(&later), [(3, 53)]);
        assert!(
            media(&later)
                .iter()
                .all(|(_, meta, _)| wall_clock(meta).is_none())
        );
    }
}

/// A video `$Number$` timeline consistent with the clock: 2 s segments
/// from `start_number` at the given media times (ms), anchored at
/// [`ast`].
fn steady_timeline(start_number: u64, times: &[u64], init: &str, representation: &str) -> String {
    timeline_variant(0, start_number, times, init, representation)
}

#[test]
fn a_switch_that_first_contradicts_the_media_times_continues_by_number() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 10);
    // #50 and #51 end at 8 and 10 s.
    let first = fixture.plan(&steady_timeline(50, &[6000, 8000], "v/init.mp4", "v"));
    assert_eq!(planned_numbers(&first), [(0, 51)]);
    assert!(anchored(&first));
    fixture.consume_all();
    // The initialization changes and #52 is described at `t="0"`, with
    // the anchor unchanged: the first contradiction is at the switch.
    let switched = steady_timeline(52, &[0], "v/init2.mp4", "v");
    let outcome = fixture.plan(&switched);
    assert_eq!(planned_numbers(&outcome), [(1, 52)]);
    assert_ne!(init_scopes(&first), init_scopes(&outcome), "a new epoch");
    assert_eq!(wall_clock(media(&first)[0].1), None);
    assert_eq!(wall_clock(media(&outcome)[0].1), None);
    fixture.consume_all();
    // Repeated, it offers nothing new; the next number follows.
    assert!(media(&fixture.plan(&switched)).is_empty());
    let next = fixture.plan(&steady_timeline(52, &[0, 2000], "v/init2.mp4", "v"));
    assert_eq!(planned_numbers(&next), [(2, 53)]);
}

#[test]
fn a_switch_to_independent_numbers_starts_like_a_recording() {
    // Another Representation's own numbering: far below the previous
    // newest (#51), or overlapping it for a different interval.
    for (start_number, expected) in [(1, [4, 5, 6]), (48, [51, 52, 53])] {
        let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
        // Contradicting descriptions establish distrust.
        contradict_number_timeline(&mut fixture);
        fixture.consume_all();
        // Six segments ending at 2–12 s after an anchor 12.5 s ago.
        set_clock(&fixture, 112_500);
        let switched = fixture.plan(&timeline_variant(
            100,
            start_number,
            &[0, 2000, 4000, 6000, 8000, 10000],
            "w/init.mp4",
            "w",
        ));
        // Three segments behind the edge (the default delay): numbers
        // of another sequence say nothing about where #51 left off.
        let numbers: Vec<u64> = planned_numbers(&switched)
            .iter()
            .map(|(_, number)| *number)
            .collect();
        assert_eq!(numbers, expected, "from {start_number}");
    }
}

#[test]
fn media_times_are_compared_across_timescales() {
    let timeline = |init: &str, timescale: u64, start_number: u64, entries: &str| {
        format!(
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT1S">
  <Period id="p0" start="PT0S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="v/$Number$.m4s" initialization="{init}" timescale="{timescale}" startNumber="{start_number}">
    <SegmentTimeline>{entries}</SegmentTimeline>
  </SegmentTemplate>
  <Representation id="v" bandwidth="1000"/>
</AdaptationSet>
  </Period>
</MPD>"#
        )
    };
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    // #51 is 8–10 s at 90 kHz.
    set_clock(&fixture, 10_500);
    let first = fixture.plan(&timeline(
        "v/init.mp4",
        90_000,
        50,
        r#"<S t="540000" d="180000" r="1"/>"#,
    ));
    assert_eq!(planned_numbers(&first), [(0, 51)]);
    fixture.consume_all();
    // A new init at 1 kHz announces #52 at 100 s: later, not rewritten,
    // so nothing is requested before the clock reaches it.
    set_clock(&fixture, 11_000);
    let next = fixture.plan(&timeline(
        "v/init2.mp4",
        1000,
        52,
        r#"<S t="100000" d="2000"/>"#,
    ));
    assert!(media(&next).is_empty());
    assert!(anchored(&first), "a later time is no contradiction");
}

/// A live MPD of `$Number$` timelines in seconds, anchored at [`ast`].
fn periods_mpd(periods: &[(&str, u64, &str, u64, &str)]) -> String {
    let periods: String = periods
        .iter()
        .map(|(id, start, representation, start_number, entries)| {
            format!(
                r#"<Period id="{id}" start="PT{start}S">
<AdaptationSet mimeType="video/mp4">
  <SegmentTemplate media="{id}/{representation}/$Number$.m4s" initialization="{id}/{representation}/init.mp4" timescale="1" startNumber="{start_number}">
    <SegmentTimeline>{entries}</SegmentTimeline>
  </SegmentTemplate>
  <Representation id="{representation}" bandwidth="1000"/>
</AdaptationSet>
  </Period>"#
            )
        })
        .collect();
    format!(
        r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic"
 availabilityStartTime="2024-01-01T00:00:00Z" minimumUpdatePeriod="PT2S" timeShiftBufferDepth="PT30S">
  {periods}
</MPD>"#
    )
}

#[test]
fn a_contradiction_in_one_period_leaves_the_next_trusted() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    let p1 = ("p1", 10, "v", 1, r#"<S t="0" d="2" r="4"/>"#);
    set_clock(&fixture, 6_000);
    let first = fixture.plan(&periods_mpd(&[
        ("p0", 0, "v", 1, r#"<S t="0" d="2" r="2"/>"#),
        p1,
    ]));
    assert_eq!(planned_numbers(&first), [(0, 3)]);
    fixture.consume_all();
    // p0 re-describes #3 at another time.
    set_clock(&fixture, 7_000);
    fixture.plan(&periods_mpd(&[
        ("p0", 0, "v", 3, r#"<S t="100" d="2"/>"#),
        p1,
    ]));
    fixture.consume_all();
    // p1 is recorded to 20 s.
    set_clock(&fixture, 20_000);
    let p1_first = fixture.plan(&periods_mpd(&[p1]));
    assert_eq!(
        planned_numbers(&p1_first).last().map(|(_, number)| *number),
        Some(5)
    );
    assert_eq!(
        wall_clock(media(&p1_first)[0].1),
        Some(ast()),
        "p1 stays anchored"
    );
    fixture.consume_all();
    // A Representation switch in p1, whose timing nothing contradicted,
    // continues where p1's coverage ended.
    set_clock(&fixture, 24_000);
    let switched = fixture.plan(&periods_mpd(&[(
        "p1",
        10,
        "w",
        1,
        r#"<S t="0" d="2" r="6"/>"#,
    )]));
    let numbers: Vec<u64> = planned_numbers(&switched)
        .iter()
        .map(|(_, number)| *number)
        .collect();
    assert_eq!(numbers, [6, 7]);
}

#[test]
fn a_contradiction_reaches_retired_epochs_of_its_period() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    set_clock(&fixture, 10_500);
    let first = fixture.plan(&steady_timeline(50, &[6000, 8000], "v/init.mp4", "v"));
    assert_eq!(planned_numbers(&first), [(0, 51)]);
    fixture.consume_all();
    // A new initialization: the first epoch is retired once consumed,
    // while #51 may still be buffered downstream.
    set_clock(&fixture, 12_500);
    let second = fixture.plan(&steady_timeline(51, &[8000, 10000], "v/init2.mp4", "v"));
    assert_eq!(planned_numbers(&second), [(1, 52)]);
    fixture.consume_all();
    assert_eq!(wall_clock(media(&first)[0].1), Some(ast()));
    // #52 re-described at another time contradicts the Period.
    set_clock(&fixture, 14_500);
    fixture.plan(&steady_timeline(52, &[0, 2000], "v/init2.mp4", "v"));
    assert_eq!(wall_clock(media(&first)[0].1), None);
    assert_eq!(wall_clock(media(&second)[0].1), None);
}

#[test]
fn period_trust_is_forgotten_with_its_periods() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    for index in 0..50_u64 {
        // Each MPD carries the next Period and moves the anchor.
        let ast_secs = index % 2;
        let start = 10 * index;
        let mpd = periods_mpd(&[("p", start, "v", 1, r#"<S t="0" d="2" r="4"/>"#)])
            .replace(r#"id="p""#, &format!(r#"id="p{index}""#))
            .replace(
                "2024-01-01T00:00:00Z",
                &(ast() + jiff::SignedDuration::from_secs(ast_secs as i64)).to_string(),
            );
        set_clock(&fixture, (start as i64 + 11) * 1000);
        let outcome = fixture.plan(&mpd);
        assert!(!media(&outcome).is_empty(), "Period {index}");
        fixture.consume_all();
        let periods = fixture.planner.as_ref().unwrap().periods.len();
        assert!(periods <= 3, "{periods} Periods remembered at {index}");
    }
}

#[test]
fn a_cdn_switch_in_a_contradicted_period_continues_by_number() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
    let (_, ahead) = contradict_number_timeline(&mut fixture);
    assert_eq!(planned_numbers(&ahead), [(1, 51)]);
    fixture.consume_all();
    // Another CDN: initialization and media elsewhere, #50–#52 listed.
    set_clock(&fixture, 102_500);
    let moved = fixture.plan(&relocated_timeline(
        102,
        50,
        &[1_000_000, 1_002_000, 1_004_000],
        "edge2/init.mp4",
        "edge2/v",
        "v",
    ));
    // Neither replayed nor skipped.
    assert_eq!(planned_numbers(&moved), [(2, 52)]);
}

/// [`number_timeline`] as one open `S@r=-1` run from `t="0"` with a
/// 30 s time-shift buffer.
fn open_timeline(ast_secs: i64, start_number: u64, time: u64) -> String {
    number_timeline(ast_secs, start_number, &[time])
        .replace(r#"d="2000"/>"#, r#"d="2000" r="-1"/>"#)
        .replace(
            r#"timeShiftBufferDepth="PT12S""#,
            r#"timeShiftBufferDepth="PT30S""#,
        )
}

#[test]
fn an_open_timeline_refreshed_for_minutes_plans_each_segment_once() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 100);
    let mpd = open_timeline(0, 1, 0);
    let mut numbers = Vec::new();
    for _ in 0..90 {
        let outcome = fixture.plan(&mpd);
        assert_eq!(outcome.terminal, None);
        numbers.extend(planned_numbers(&outcome).iter().map(|(_, number)| *number));
        fixture.consume_all();
        fixture.advance(2);
    }
    // One new segment per 2 s, each once.
    assert_eq!(numbers, (50..=139).collect::<Vec<_>>());
}

#[test]
fn open_timelines_in_contradicting_descriptions_are_followed_by_number() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::default(), 100);
    let mut numbers = Vec::new();
    for step in 0..20_u64 {
        let secs = 100 + 2 * step as i64;
        set_clock(&fixture, secs * 1000 + 500);
        // The response-time description of the newest segment, then an
        // open run far ahead of its anchor from a few numbers back.
        let current = 50 + step;
        for mpd in [
            open_timeline(secs, current, 0),
            open_timeline(secs + 1, current.saturating_sub(5), 2_000_000 + 2000 * step),
        ] {
            let outcome = fixture.plan(&mpd);
            assert_eq!(outcome.terminal, None, "step {step}");
            numbers.extend(planned_numbers(&outcome).iter().map(|(_, number)| *number));
            fixture.consume_all();
        }
    }
    numbers.dedup();
    assert_eq!(numbers, (50..=69).collect::<Vec<_>>());
}

#[test]
fn an_epoch_queued_from_before_an_anchor_move_leaves_later_periods_anchored() {
    let mut fixture = Fixture::new(MediaTrack::Video, LiveStartPolicy::LiveEdge, 0);
    // p0's #3 is planned and still queued.
    set_clock(&fixture, 6_000);
    let first = fixture.plan(&periods_mpd(&[(
        "p0",
        0,
        "v",
        1,
        r#"<S t="0" d="2" r="2"/>"#,
    )]));
    assert_eq!(planned_numbers(&first), [(0, 3)]);
    assert_eq!(wall_clock(media(&first)[0].1), Some(ast()));
    // The anchor moves by a second, and p1 begins under it.
    set_clock(&fixture, 21_000);
    let moved = periods_mpd(&[("p1", 10, "v", 1, r#"<S t="0" d="2" r="4"/>"#)])
        .replace("2024-01-01T00:00:00Z", "2024-01-01T00:00:01Z");
    let p1 = fixture.plan(&moved);
    let anchor = Some(ast() + jiff::SignedDuration::from_secs(1));
    assert!(!media(&p1).is_empty());
    assert_eq!(wall_clock(media(&first)[0].1), None, "p0's anchor moved");
    assert_eq!(wall_clock(media(&p1)[0].1), anchor);
    // Refreshed unchanged while p0's epoch is still queued: p1 keeps its
    // anchor.
    set_clock(&fixture, 21_500);
    fixture.plan(&moved);
    assert_eq!(wall_clock(media(&p1)[0].1), anchor);
}
