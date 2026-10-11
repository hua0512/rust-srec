use bytes::Bytes;

use super::*;
use crate::FourCc;
use crate::box_utils::top_level_boxes;
use crate::independent::IndependentFragmentCheck;
use crate::isobmff::{ParseOptions, parse_init_segment_with_options};
use crate::moof::Moof;
use crate::test_support::{make_box, make_full_box};

macro_rules! fixture {
    ($name:literal) => {
        Bytes::from_static(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/dash/",
            $name
        )))
    };
}

fn video_fixture() -> (Bytes, Vec<Bytes>) {
    (
        fixture!("video-init.mp4"),
        vec![fixture!("video-1.m4s"), fixture!("video-2.m4s")],
    )
}

fn audio_fixture() -> (Bytes, Vec<Bytes>) {
    (
        fixture!("audio-init.mp4"),
        vec![
            fixture!("audio-1.m4s"),
            fixture!("audio-2.m4s"),
            fixture!("audio-3.m4s"),
        ],
    )
}

/// Structural facts about a merged file, gathered by an independent walk
/// over the bytes the merger wrote.
#[derive(Debug, Default)]
struct Summary {
    sequence_numbers: Vec<u32>,
    /// Per merged track: decode times, sample count and media bytes.
    decode_times: [Vec<u64>; 2],
    samples: [u64; 2],
    media_bytes: [u64; 2],
}

/// Walks `ftyp`, `moov`, then `moof`/`mdat` pairs and checks every run's
/// samples lie inside the `mdat` right after its `moof`.
fn inspect(file: &Bytes) -> Summary {
    let boxes: Vec<_> = top_level_boxes(file).collect::<Result<_, _>>().unwrap();
    assert_eq!(boxes[0].fourcc, *b"ftyp");
    assert_eq!(boxes[1].fourcc, *b"moov");
    let mut summary = Summary::default();
    let init = MergedInitView::parse(&file.slice(..boxes[1].end));
    for pair in boxes[2..].chunks(2) {
        let [moof_box, mdat] = pair else {
            panic!("dangling box at the end of the file");
        };
        assert_eq!((moof_box.fourcc, mdat.fourcc), (*b"moof", *b"mdat"));
        let moof = Moof::decode(&file.slice(moof_box.start..moof_box.end)).unwrap();
        summary.sequence_numbers.push(moof.sequence_number);
        let [traf] = moof.trafs.as_slice() else {
            panic!("expected one traf per moof");
        };
        assert!(traf.tfhd.default_base_is_moof);
        assert_eq!(traf.tfhd.base_data_offset, None);
        let track = (traf.tfhd.track_id - 1) as usize;
        let (default_duration, default_size) = init.defaults[track];
        summary.decode_times[track].push(traf.tfdt().unwrap().base_media_decode_time());
        for trun in traf.truns() {
            let start = moof_box.start as u64 + trun.data_offset.unwrap() as u64;
            let totals = trun.totals(
                traf.tfhd
                    .default_sample_duration
                    .unwrap_or(default_duration),
                traf.tfhd.default_sample_size.unwrap_or(default_size),
            );
            assert!(start >= mdat.body_start as u64);
            assert!(start + totals.size <= mdat.end as u64);
            summary.samples[track] += u64::from(trun.sample_count());
            summary.media_bytes[track] += totals.size;
        }
    }
    summary
}

/// The complete box at `path` (each element a child of the previous one).
fn find_nested(data: &Bytes, path: &[&[u8; 4]]) -> Bytes {
    let (mut start, mut end) = (0, data.len());
    let mut found = None;
    for fourcc in path {
        let item = crate::box_utils::find_first_box(data, start, end, **fourcc).unwrap();
        (start, end) = (item.body_start, item.end);
        found = Some(item);
    }
    let item = found.unwrap();
    data.slice(item.start..item.end)
}

/// The `trex` defaults of a merged init, read back for [`inspect`].
struct MergedInitView {
    defaults: Vec<(u32, u32)>,
}

impl MergedInitView {
    fn parse(init: &Bytes) -> Self {
        // One trex per track, in track order.
        let moov = top_level_boxes(init).nth(1).unwrap().unwrap();
        let mvex =
            crate::box_utils::find_first_box(init, moov.body_start, moov.end, *b"mvex").unwrap();
        let mut defaults = Vec::new();
        let mut offset = mvex.body_start;
        while let Some(trex) = crate::box_utils::box_at(init, offset, mvex.end) {
            let word = |at: usize| {
                u32::from_be_bytes(
                    init[trex.body_start + at..trex.body_start + at + 4]
                        .try_into()
                        .unwrap(),
                )
            };
            assert_eq!(word(4) as usize, defaults.len() + 1, "trex in track order");
            defaults.push((word(12), word(16)));
            offset = trex.end;
        }
        Self { defaults }
    }
}

/// Merge the fixtures the way the DASH muxer will: each video fragment
/// followed by the audio fragments that start before it ends.
fn merge_fixtures() -> (MergedInit, Bytes, [u64; 2]) {
    let (video_init, video_segments) = video_fixture();
    let (audio_init, audio_segments) = audio_fixture();
    let merged = merge_init(&[video_init, audio_init]).unwrap();

    let parse = |track, segments: &[Bytes]| -> Vec<SourceFragment> {
        segments
            .iter()
            .flat_map(|segment| merged.parse_media_segment(track, segment, 0).unwrap())
            .collect()
    };
    let video = parse(0, &video_segments);
    let audio = parse(1, &audio_segments);
    let source_samples = [
        video.iter().map(SourceFragment::sample_count).sum(),
        audio.iter().map(SourceFragment::sample_count).sum(),
    ];

    let seconds = |fragment: &SourceFragment, ticks: u64| {
        ticks as f64 / f64::from(merged.tracks()[fragment.track_index()].timescale())
    };
    let mut audio = audio.into_iter().peekable();
    let mut writer = FragmentWriter::new();
    let mut out = merged.data().to_vec();
    for fragment in video {
        let end = seconds(&fragment, fragment.decode_time() + fragment.duration());
        writer.write(fragment, &mut out).unwrap();
        while let Some(next) = audio.next_if(|next| seconds(next, next.decode_time()) < end) {
            writer.write(next, &mut out).unwrap();
        }
    }
    for fragment in audio {
        writer.write(fragment, &mut out).unwrap();
    }
    (merged, Bytes::from(out), source_samples)
}

#[test]
fn merged_init_describes_both_tracks() {
    let (merged, file, _) = merge_fixtures();
    let tracks = merged.tracks();
    assert_eq!(tracks.len(), 2);
    assert_eq!(
        tracks
            .iter()
            .map(|track| (track.track_id(), track.source_track_id(), track.handler()))
            .collect::<Vec<_>>(),
        [(1, 1, FourCc::new(b"vide")), (2, 1, FourCc::new(b"soun"))]
    );
    assert_eq!(tracks[0].timescale(), 12288);
    assert_eq!(tracks[1].timescale(), 48000);

    let info = parse_init_segment_with_options(
        merged.data(),
        ParseOptions {
            include_resolution: true,
        },
    );
    assert!(info.has_h264 && info.has_aac);
    assert_eq!(info.video_resolution, Some(crate::Resolution::new(64, 64)));
    // Sample descriptions (codec configuration included) are copied verbatim.
    let (video_init, _) = video_fixture();
    let (audio_init, _) = audio_fixture();
    for source in [video_init, audio_init] {
        let stsd = find_nested(
            &source,
            &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"],
        );
        assert!(
            merged
                .data()
                .windows(stsd.len())
                .any(|window| window == stsd.as_ref()),
            "stsd is preserved byte for byte"
        );
    }

    // The merged init is recognised as two tracks: the first item (video then
    // the audio that overlaps it) is a valid cut point only if both are seen.
    let check = IndependentFragmentCheck::from_init(merged.data()).unwrap();
    let boxes: Vec<_> = top_level_boxes(&file).collect::<Result<_, _>>().unwrap();
    let first_audio_mdat = boxes
        .iter()
        .skip(2)
        .step_by(2)
        .position(|moof| {
            let moof = Moof::decode(&file.slice(moof.start..moof.end)).unwrap();
            moof.trafs[0].tfhd.track_id == 2
        })
        .unwrap();
    let item_end = boxes[2 + 2 * first_audio_mdat + 1].end;
    assert!(check.is_independent(&file.slice(boxes[2].start..item_end)));
    assert!(
        !check.is_independent(&file.slice(boxes[2].start..boxes[3].end)),
        "video alone is not every track"
    );
}

#[test]
fn merged_fragments_are_consistently_addressed() {
    let (_, file, source_samples) = merge_fixtures();
    let summary = inspect(&file);
    let count = summary.sequence_numbers.len() as u32;
    assert_eq!(summary.sequence_numbers, (1..=count).collect::<Vec<_>>());
    assert_eq!(
        summary.samples, source_samples,
        "no sample lost or duplicated"
    );
    for times in &summary.decode_times {
        assert!(
            times.windows(2).all(|pair| pair[0] < pair[1]),
            "decode times increase per track"
        );
    }
}

#[test]
fn media_data_and_decode_times_are_unchanged() {
    let (video_init, video_segments) = video_fixture();
    let (audio_init, _) = audio_fixture();
    let merged = merge_init(&[video_init, audio_init]).unwrap();
    for segment in &video_segments {
        let source = top_level_boxes(segment)
            .map(Result::unwrap)
            .find(|item| item.fourcc == *b"moof")
            .unwrap();
        let source_moof = Moof::decode(&segment.slice(source.start..source.end)).unwrap();
        let mdat = top_level_boxes(segment)
            .map(Result::unwrap)
            .find(|item| item.fourcc == *b"mdat")
            .unwrap();

        let [fragment] = merged
            .parse_media_segment(0, segment, 0)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(
            fragment.decode_time(),
            source_moof.trafs[0]
                .tfdt()
                .unwrap()
                .base_media_decode_time()
        );
        let mut out = Vec::new();
        FragmentWriter::new().write(fragment, &mut out).unwrap();
        let out = Bytes::from(out);
        let written = top_level_boxes(&out)
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(
            out[written[1].body_start..],
            segment[mdat.body_start..mdat.end]
        );
    }
}

#[test]
fn time_shift_moves_decode_times_and_rejects_negative_results() {
    let (video_init, video_segments) = video_fixture();
    let merged = merge_init(&[video_init]).unwrap();
    let original = merged
        .parse_media_segment(0, &video_segments[1], 0)
        .unwrap()[0]
        .decode_time();
    assert!(original > 0);

    let shift = i64::from(u32::MAX);
    let shifted = merged
        .parse_media_segment(0, &video_segments[1], shift)
        .unwrap();
    assert_eq!(shifted[0].decode_time(), original + u64::from(u32::MAX));
    assert_eq!(
        merged
            .parse_media_segment(0, &video_segments[1], -(original as i64) - 1)
            .unwrap_err(),
        MergeError::Overflow("decode time after time shift")
    );
}

#[test]
fn tfdt_version_0_is_upgraded_when_the_shifted_time_needs_64_bits() {
    let merged = merge_init(&[InitSpec::default().build()]).unwrap();
    let segment = SegmentSpec {
        tfdt_version: 0,
        ..Default::default()
    }
    .build();
    let tfdt_of = |fragment: SourceFragment| {
        let mut out = Vec::new();
        FragmentWriter::new().write(fragment, &mut out).unwrap();
        let at = out.windows(4).position(|window| window == b"tfdt").unwrap();
        // Version byte, then a 32- or 64-bit decode time.
        let time = match out[at + 4] {
            0 => u64::from(u32::from_be_bytes(out[at + 8..at + 12].try_into().unwrap())),
            _ => u64::from_be_bytes(out[at + 8..at + 16].try_into().unwrap()),
        };
        (out[at + 4], time)
    };

    let [unshifted] = merged
        .parse_media_segment(0, &segment, 0)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(
        tfdt_of(unshifted),
        (0, 9000),
        "version kept when the value fits"
    );
    let shift = i64::from(u32::MAX);
    let [shifted] = merged
        .parse_media_segment(0, &segment, shift)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(tfdt_of(shifted), (1, 9000 + u64::from(u32::MAX)));
}

#[test]
fn truncated_or_corrupted_fixtures_never_panic() {
    let (video_init, video_segments) = video_fixture();
    for end in 0..video_init.len() {
        assert!(
            merge_init(&[video_init.slice(..end)]).is_err(),
            "init prefix of {end} bytes"
        );
    }
    let merged = merge_init(std::slice::from_ref(&video_init)).unwrap();
    let segment = &video_segments[0];
    for end in 0..segment.len() {
        let _ = merged.parse_media_segment(0, &segment.slice(..end), 0);
    }
    for index in 0..video_init.len() {
        for value in [0x00, 0x01, 0x80, 0xff] {
            let mut corrupted = video_init.to_vec();
            corrupted[index] = value;
            let _ = merge_init(&[Bytes::from(corrupted)]);
        }
    }
    // The sample table is large; corrupt the box headers and the start of it.
    let mut writer = FragmentWriter::new();
    for index in 0..segment.len().min(1500) {
        for value in [0x00, 0x01, 0x80, 0xff] {
            let mut corrupted = segment.to_vec();
            corrupted[index] = value;
            if let Ok(fragments) = merged.parse_media_segment(0, &Bytes::from(corrupted), 0) {
                for fragment in fragments {
                    let mut out = Vec::new();
                    let _ = writer.write(fragment, &mut out);
                }
            }
        }
    }
}

// Synthetic inputs for the rules the fixtures do not exercise.

struct InitSpec {
    track_id: u32,
    handler: &'static [u8; 4],
    movie_timescale: u32,
    edit_duration: Option<u32>,
    sample_entry: &'static [u8; 4],
    alternate_group: u16,
    trex_sample_description_index: u32,
    /// Flags of the single `url ` data reference; 1 means "this file".
    data_reference_flags: u32,
    chunk_offsets: u32,
    extra_trak: Vec<u8>,
    extra_mdia: Vec<u8>,
    extra_moov: Vec<u8>,
    traks: usize,
}

impl Default for InitSpec {
    fn default() -> Self {
        Self {
            track_id: 1,
            handler: b"vide",
            movie_timescale: 1000,
            edit_duration: None,
            sample_entry: b"avc1",
            alternate_group: 0,
            trex_sample_description_index: 1,
            data_reference_flags: 1,
            chunk_offsets: 0,
            extra_trak: Vec::new(),
            extra_mdia: Vec::new(),
            extra_moov: Vec::new(),
            traks: 1,
        }
    }
}

impl InitSpec {
    fn audio() -> Self {
        Self {
            handler: b"soun",
            sample_entry: b"mp4a",
            ..Default::default()
        }
    }

    /// A fragmented-track initialization segment with the structure
    /// ISO/IEC 14496-12 requires: sample entries of their full size, a
    /// self-contained data reference and empty sample tables.
    fn build(&self) -> Bytes {
        let mut mvhd = vec![0; 8];
        mvhd.extend_from_slice(&self.movie_timescale.to_be_bytes());
        mvhd.extend_from_slice(&5000u32.to_be_bytes());
        mvhd.extend_from_slice(&[0; 76]);
        mvhd.extend_from_slice(&2u32.to_be_bytes());
        let mvhd = make_full_box(b"mvhd", 0, 0, &mvhd);

        let mut tkhd = vec![0; 8];
        tkhd.extend_from_slice(&self.track_id.to_be_bytes());
        tkhd.extend_from_slice(&[0; 4]);
        tkhd.extend_from_slice(&5000u32.to_be_bytes());
        tkhd.extend_from_slice(&[0; 10]);
        tkhd.extend_from_slice(&self.alternate_group.to_be_bytes());
        tkhd.extend_from_slice(&[0; 48]);
        let tkhd = make_full_box(b"tkhd", 0, 7, &tkhd);

        let edts = self
            .edit_duration
            .map(|duration| {
                let mut elst = 1u32.to_be_bytes().to_vec();
                elst.extend_from_slice(&duration.to_be_bytes());
                elst.extend_from_slice(&1024u32.to_be_bytes());
                elst.extend_from_slice(&[0, 1, 0, 0]);
                make_box(b"edts", &make_full_box(b"elst", 0, 0, &elst))
            })
            .unwrap_or_default();

        let mut mdhd = vec![0; 8];
        mdhd.extend_from_slice(&90_000u32.to_be_bytes());
        mdhd.extend_from_slice(&[0; 8]);
        let mdhd = make_full_box(b"mdhd", 0, 0, &mdhd);
        let mut hdlr = vec![0; 4];
        hdlr.extend_from_slice(self.handler);
        hdlr.extend_from_slice(&[0; 13]);
        let hdlr = make_full_box(b"hdlr", 0, 0, &hdlr);

        let url = make_full_box(b"url ", 0, self.data_reference_flags, &[]);
        let dref = make_full_box(b"dref", 0, 0, &[&1u32.to_be_bytes()[..], &url].concat());
        let dinf = make_box(b"dinf", &dref);
        // SampleEntry: six reserved bytes and data_reference_index 1, then
        // the visual (70 more bytes) or audio (20 more bytes) fields.
        let entry_fields = if self.handler == b"soun" { 20 } else { 70 };
        let entry = make_box(
            self.sample_entry,
            &[&[0, 0, 0, 0, 0, 0, 0, 1][..], &vec![0; entry_fields]].concat(),
        );
        let stsd = make_full_box(b"stsd", 0, 0, &[&1u32.to_be_bytes()[..], &entry].concat());
        let mut chunk_offsets = self.chunk_offsets.to_be_bytes().to_vec();
        chunk_offsets.extend(std::iter::repeat_n(0u8, 4 * self.chunk_offsets as usize));
        let stbl = make_box(
            b"stbl",
            &[
                stsd,
                make_full_box(b"stts", 0, 0, &0u32.to_be_bytes()),
                make_full_box(b"stsc", 0, 0, &0u32.to_be_bytes()),
                make_full_box(b"stsz", 0, 0, &[0; 8]),
                make_full_box(b"stco", 0, 0, &chunk_offsets),
            ]
            .concat(),
        );
        let minf = make_box(b"minf", &[dinf, stbl].concat());
        let mdia = make_box(
            b"mdia",
            &[mdhd, hdlr, self.extra_mdia.clone(), minf].concat(),
        );

        let trak = make_box(
            b"trak",
            &[tkhd, edts, self.extra_trak.clone(), mdia].concat(),
        );
        let mut trex = self.track_id.to_be_bytes().to_vec();
        trex.extend_from_slice(&self.trex_sample_description_index.to_be_bytes());
        trex.extend_from_slice(&3000u32.to_be_bytes());
        trex.extend_from_slice(&0u32.to_be_bytes());
        trex.extend_from_slice(&0u32.to_be_bytes());
        let mvex = make_box(b"mvex", &make_full_box(b"trex", 0, 0, &trex));
        let moov = make_box(
            b"moov",
            &[mvhd, trak.repeat(self.traks), mvex, self.extra_moov.clone()].concat(),
        );
        Bytes::from([make_box(b"ftyp", b"iso5\0\0\0\0iso5"), moov].concat())
    }
}

struct SegmentSpec {
    track_id: u32,
    decode_time: u64,
    tfdt_version: u8,
    sample_sizes: Vec<u32>,
    tfhd_flags: u32,
    tfhd_extra: Vec<u8>,
    /// Boxes placed between the `moof` and the `mdat`.
    between: Vec<u8>,
    /// Extra bytes inside `mdat` before the samples.
    mdat_prefix: usize,
    /// Second run without a data offset, continuing the first.
    continued_run: Option<Vec<u32>>,
    open_ended_mdat: bool,
}

impl Default for SegmentSpec {
    fn default() -> Self {
        Self {
            track_id: 1,
            decode_time: 9000,
            tfdt_version: 1,
            sample_sizes: vec![5, 7],
            tfhd_flags: 0x02_0000,
            tfhd_extra: Vec::new(),
            between: Vec::new(),
            mdat_prefix: 0,
            continued_run: None,
            open_ended_mdat: false,
        }
    }
}

impl SegmentSpec {
    fn build(&self) -> Bytes {
        let trun = |sizes: &[u32], data_offset: Option<i32>| {
            let mut payload = (sizes.len() as u32).to_be_bytes().to_vec();
            if let Some(offset) = data_offset {
                payload.extend_from_slice(&offset.to_be_bytes());
            }
            for size in sizes {
                payload.extend_from_slice(&size.to_be_bytes());
            }
            make_full_box(
                b"trun",
                0,
                0x200 | u32::from(data_offset.is_some()),
                &payload,
            )
        };
        let moof = |data_offset: i32| {
            let mut tfhd = self.track_id.to_be_bytes().to_vec();
            tfhd.extend_from_slice(&self.tfhd_extra);
            let mut traf = make_full_box(b"tfhd", 0, self.tfhd_flags, &tfhd);
            let decode_time = if self.tfdt_version == 0 {
                (self.decode_time as u32).to_be_bytes().to_vec()
            } else {
                self.decode_time.to_be_bytes().to_vec()
            };
            traf.extend(make_full_box(b"tfdt", self.tfdt_version, 0, &decode_time));
            traf.extend(trun(&self.sample_sizes, Some(data_offset)));
            if let Some(sizes) = &self.continued_run {
                traf.extend(trun(sizes, None));
            }
            let mfhd = make_full_box(b"mfhd", 0, 0, &7u32.to_be_bytes());
            make_box(b"moof", &[mfhd, make_box(b"traf", &traf)].concat())
        };
        let styp = make_box(b"styp", b"msdh\0\0\0\0msdh");
        let probe = moof(0);
        let data_offset = (probe.len() + self.between.len() + 8 + self.mdat_prefix) as i32;
        let samples: Vec<u8> = self
            .sample_sizes
            .iter()
            .chain(self.continued_run.iter().flatten())
            .enumerate()
            .flat_map(|(index, &size)| vec![index as u8 + 1; size as usize])
            .collect();
        let mut mdat = make_box(b"mdat", &[vec![0xee; self.mdat_prefix], samples].concat());
        if self.open_ended_mdat {
            mdat[..4].copy_from_slice(&[0; 4]);
        }
        let moof = moof(data_offset);
        Bytes::from([styp, moof, self.between.clone(), mdat].concat())
    }
}

fn merge_one(init: InitSpec, segment: SegmentSpec) -> Result<Vec<SourceFragment>, MergeError> {
    merge_init(&[init.build()])?.parse_media_segment(0, &segment.build(), 0)
}

#[test]
fn layouts_that_relocate_cleanly_are_accepted() {
    let emsg = make_full_box(b"emsg", 0, 0, &[0; 24]);
    for spec in [
        SegmentSpec::default(),
        // Legacy addressing without default-base-is-moof.
        SegmentSpec {
            tfhd_flags: 0,
            ..Default::default()
        },
        // A box between moof and mdat shifts the source offsets.
        SegmentSpec {
            between: emsg.clone(),
            ..Default::default()
        },
        SegmentSpec {
            mdat_prefix: 3,
            ..Default::default()
        },
        SegmentSpec {
            continued_run: Some(vec![2, 4]),
            ..Default::default()
        },
        SegmentSpec {
            open_ended_mdat: true,
            ..Default::default()
        },
    ] {
        let expected_samples: Vec<u8> = spec
            .sample_sizes
            .iter()
            .chain(spec.continued_run.iter().flatten())
            .enumerate()
            .flat_map(|(index, &size)| vec![index as u8 + 1; size as usize])
            .collect();
        let merged = merge_init(&[InitSpec::default().build()]).unwrap();
        let fragments = merged.parse_media_segment(0, &spec.build(), 0).unwrap();
        let mut out = merged.data().to_vec();
        let mut writer = FragmentWriter::new();
        for fragment in fragments {
            writer.write(fragment, &mut out).unwrap();
        }
        let out = Bytes::from(out);
        let summary = inspect(&out);
        assert_eq!(summary.media_bytes[0], expected_samples.len() as u64);

        // Read the samples back through the rewritten offsets.
        let boxes: Vec<_> = top_level_boxes(&out).map(Result::unwrap).collect();
        let moof = Moof::decode(&out.slice(boxes[2].start..boxes[2].end)).unwrap();
        let mut samples = Vec::new();
        for trun in moof.trafs[0].truns() {
            let start = boxes[2].start + trun.data_offset.unwrap() as usize;
            let size = trun.totals(0, 0).size as usize;
            samples.extend_from_slice(&out[start..start + size]);
        }
        assert_eq!(samples, expected_samples);
        assert_eq!(boxes[3].size, boxes[3].end - boxes[3].start);
        assert!(!boxes[3].open_ended, "mdat gets an explicit size");
    }
}

#[test]
fn layouts_that_cannot_be_relocated_are_rejected() {
    let base_offset = SegmentSpec {
        tfhd_flags: 0x01,
        tfhd_extra: 0u64.to_be_bytes().to_vec(),
        ..Default::default()
    };
    assert_eq!(
        merge_one(InitSpec::default(), base_offset).unwrap_err(),
        MergeError::unsupported(b"tfhd", "explicit base-data-offset is not supported")
    );
    assert_eq!(
        merge_one(
            InitSpec::default(),
            SegmentSpec {
                track_id: 2,
                ..Default::default()
            }
        )
        .unwrap_err(),
        MergeError::UnknownTrack { track_id: 2 }
    );
    assert_eq!(
        merge_one(
            InitSpec::default(),
            SegmentSpec {
                tfhd_flags: 0x02_0002,
                tfhd_extra: 2u32.to_be_bytes().to_vec(),
                ..Default::default()
            }
        )
        .unwrap_err(),
        MergeError::malformed(
            b"tfhd",
            "sample description index refers to no sample entry"
        )
    );
    assert_eq!(
        merge_one(
            InitSpec::default(),
            SegmentSpec {
                tfhd_flags: 0x03_0000,
                ..Default::default()
            }
        )
        .unwrap_err(),
        MergeError::malformed(
            b"tfhd",
            "duration-is-empty is set but the fragment has samples"
        )
    );
    // The declared sizes reach past the end of the mdat.
    let mut overrun = SegmentSpec::default().build().to_vec();
    let mdat_at = overrun.len() - 12 - 8;
    overrun[mdat_at..mdat_at + 4].copy_from_slice(&16u32.to_be_bytes());
    overrun.truncate(mdat_at + 16);
    let merged = merge_init(&[InitSpec::default().build()]).unwrap();
    assert_eq!(
        merged
            .parse_media_segment(0, &Bytes::from(overrun), 0)
            .unwrap_err(),
        MergeError::malformed(b"trun", "sample data lies outside the following mdat")
    );
    assert_eq!(
        merged
            .parse_media_segment(1, &SegmentSpec::default().build(), 0)
            .unwrap_err(),
        MergeError::UnknownSource {
            index: 1,
            sources: 1
        }
    );
}

#[test]
fn source_inits_outside_the_contract_are_rejected() {
    let pssh = make_full_box(b"pssh", 0, 0, &[0; 20]);
    let tref = make_box(b"tref", &make_box(b"vdep", &2u32.to_be_bytes()));
    let cases = [
        (
            InitSpec {
                sample_entry: b"encv",
                ..Default::default()
            },
            MergeError::Protected {
                fourcc: FourCc::new(b"encv"),
            },
        ),
        (
            InitSpec {
                extra_moov: pssh,
                ..Default::default()
            },
            MergeError::Protected {
                fourcc: FourCc::new(b"pssh"),
            },
        ),
        (
            InitSpec {
                extra_trak: tref,
                ..Default::default()
            },
            MergeError::unsupported(
                b"tref",
                "track references point outside a single-track source",
            ),
        ),
        (
            InitSpec {
                traks: 2,
                ..Default::default()
            },
            MergeError::unsupported(b"moov", "each source must contain exactly one track"),
        ),
        (
            InitSpec {
                trex_sample_description_index: 2,
                ..Default::default()
            },
            MergeError::malformed(
                b"trex",
                "default sample description index refers to no sample entry",
            ),
        ),
        (
            InitSpec {
                data_reference_flags: 0,
                ..Default::default()
            },
            MergeError::unsupported(b"url ", "media data in another file is not supported"),
        ),
        (
            InitSpec {
                chunk_offsets: 1,
                ..Default::default()
            },
            MergeError::unsupported(
                b"stco",
                "initialization segment describes samples outside fragments",
            ),
        ),
        (
            InitSpec {
                extra_mdia: make_full_box(b"mdhd", 0, 0, &[0; 20]),
                ..Default::default()
            },
            MergeError::malformed(b"mdhd", "box appears more than once in its parent"),
        ),
    ];
    for (spec, expected) in cases {
        assert_eq!(merge_init(&[spec.build()]).unwrap_err(), expected);
    }
    assert_eq!(merge_init(&[]).unwrap_err(), MergeError::NoSources);
}

#[test]
fn track_ids_are_assigned_by_source_order_and_track_headers_reset() {
    // Both sources claim alternate group 1, which would make the merged
    // tracks alternatives of each other.
    let video = InitSpec {
        track_id: 2,
        alternate_group: 1,
        ..Default::default()
    };
    let audio = InitSpec {
        track_id: 1,
        alternate_group: 1,
        ..InitSpec::audio()
    };
    let merged = merge_init(&[video.build(), audio.build()]).unwrap();
    let ids: Vec<_> = merged
        .tracks()
        .iter()
        .map(|t| (t.track_id(), t.source_track_id()))
        .collect();
    assert_eq!(ids, [(1, 2), (2, 1)]);

    let data = merged.data();
    let moov = top_level_boxes(data).nth(1).unwrap().unwrap();
    let mvhd = crate::box_utils::find_first_box(data, moov.body_start, moov.end, *b"mvhd").unwrap();
    let mvhd = &data[mvhd.body_start..mvhd.end];
    assert_eq!(mvhd[16..20], [0; 4], "movie duration cleared");
    assert_eq!(mvhd[mvhd.len() - 4..], 3u32.to_be_bytes(), "next_track_ID");
    // tkhd version 0 body: track_ID at 12, duration at 20, alternate_group at 34.
    let track_headers: Vec<_> = data
        .windows(4)
        .enumerate()
        .filter(|(_, window)| *window == b"tkhd")
        .map(|(at, _)| &data[at + 4..])
        .map(|body| {
            (
                body[12..16].to_vec(),
                body[20..24].to_vec(),
                body[34..36].to_vec(),
            )
        })
        .collect();
    assert_eq!(
        track_headers,
        [
            (1u32.to_be_bytes().to_vec(), vec![0; 4], vec![0; 2]),
            (2u32.to_be_bytes().to_vec(), vec![0; 4], vec![0; 2]),
        ]
    );

    // Each source's segments are re-addressed to its merged track.
    let fragments = merged
        .parse_media_segment(
            1,
            &SegmentSpec {
                track_id: 1,
                ..Default::default()
            }
            .build(),
            0,
        )
        .unwrap();
    let mut out = Vec::new();
    FragmentWriter::new()
        .write(fragments.into_iter().next().unwrap(), &mut out)
        .unwrap();
    let out = Bytes::from(out);
    let moof =
        Moof::decode(&out.slice(..top_level_boxes(&out).next().unwrap().unwrap().end)).unwrap();
    assert_eq!(moof.trafs[0].tfhd.track_id, 2);
}

#[test]
fn edit_lists_are_rescaled_to_the_movie_timescale() {
    let video = InitSpec {
        movie_timescale: 1000,
        edit_duration: Some(2000),
        ..Default::default()
    };
    let audio = InitSpec {
        movie_timescale: 48000,
        edit_duration: Some(96_000),
        ..InitSpec::audio()
    };
    let merged = merge_init(&[video.build(), audio.build()]).unwrap();
    let data = merged.data();
    let durations: Vec<u32> = data
        .windows(4)
        .enumerate()
        .filter(|(_, window)| *window == b"elst")
        .map(|(at, _)| u32::from_be_bytes(data[at + 12..at + 16].try_into().unwrap()))
        .collect();
    // 2 s in both cases, expressed in the first source's movie timescale.
    assert_eq!(durations, [2000, 2000]);

    let too_long = InitSpec {
        movie_timescale: 1,
        edit_duration: Some(u32::MAX),
        ..InitSpec::audio()
    };
    assert_eq!(
        merge_init(&[video.build(), too_long.build()]).unwrap_err(),
        MergeError::Overflow("edit list segment duration")
    );
}
