//! fMP4 fragment helpers for AV1 sample validation.
//!
//! This module extracts AV1 track IDs from an fMP4 init segment and validates
//! AV1 sample payloads from corresponding media segments.

use std::io;

use av1::sample::{IsobmffSampleParseOptions, validate_isobmff_sample_bytes_with_options};
use bytes::Bytes;

use crate::box_utils::{box_at, find_first_box};

/// AV1 sample conformance policy for fMP4 validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1ValidationOptions {
    /// Reject OBU types that AV1 ISOBMFF marks as "SHOULD NOT".
    pub enforce_should_not_obus: bool,
    /// Reject reserved OBU types.
    pub enforce_reserved_obus: bool,
}

impl Default for Av1ValidationOptions {
    fn default() -> Self {
        Self {
            enforce_should_not_obus: true,
            enforce_reserved_obus: false,
        }
    }
}

/// Summary of AV1 sample validation work performed on a media segment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Av1MediaValidationSummary {
    /// Number of AV1 tracks in the fragment that were checked.
    pub checked_tracks: usize,
    /// Number of AV1 sample payloads validated.
    pub checked_samples: usize,
}

/// Validate AV1 sample payloads in an fMP4 media segment using an init segment.
///
/// This function:
/// - detects AV1 track IDs from the init segment (`av01` sample entries)
/// - parses the media fragment (`moof`/`traf`/`tfhd`/`trun` + `mdat`)
/// - validates each AV1 sample payload using AV1 ISOBMFF OBU conformance rules
pub fn validate_av1_media_segment_against_init(
    init_segment: &Bytes,
    media_segment: &Bytes,
    enforce_should_not_obus: bool,
) -> io::Result<Av1MediaValidationSummary> {
    validate_av1_media_segment_against_init_with_options(
        init_segment,
        media_segment,
        Av1ValidationOptions {
            enforce_should_not_obus,
            enforce_reserved_obus: false,
        },
    )
}

/// Validate AV1 sample payloads in an fMP4 media segment using an init segment,
/// with explicit conformance options.
pub fn validate_av1_media_segment_against_init_with_options(
    init_segment: &Bytes,
    media_segment: &Bytes,
    options: Av1ValidationOptions,
) -> io::Result<Av1MediaValidationSummary> {
    let av1_track_ids = extract_av1_track_ids_from_init(init_segment);
    validate_av1_media_segment_with_track_ids_and_options(media_segment, &av1_track_ids, options)
}

/// Validate AV1 sample payloads in an fMP4 media segment using pre-parsed AV1 track IDs.
///
/// This avoids repeated init-segment parsing across many media segments.
pub fn validate_av1_media_segment_with_track_ids(
    media_segment: &Bytes,
    av1_track_ids: &[u32],
    enforce_should_not_obus: bool,
) -> io::Result<Av1MediaValidationSummary> {
    validate_av1_media_segment_with_track_ids_and_options(
        media_segment,
        av1_track_ids,
        Av1ValidationOptions {
            enforce_should_not_obus,
            enforce_reserved_obus: false,
        },
    )
}

/// Validate AV1 sample payloads in an fMP4 media segment using pre-parsed AV1 track IDs,
/// with explicit conformance options.
pub fn validate_av1_media_segment_with_track_ids_and_options(
    media_segment: &Bytes,
    av1_track_ids: &[u32],
    options: Av1ValidationOptions,
) -> io::Result<Av1MediaValidationSummary> {
    if av1_track_ids.is_empty() {
        return Ok(Av1MediaValidationSummary::default());
    }

    let track_ids_sorted = av1_track_ids.windows(2).all(|pair| pair[0] <= pair[1]);
    validate_av1_tracks_in_fragment(media_segment, av1_track_ids, track_ids_sorted, options)
}

fn find_child_box_range(
    data: &Bytes,
    start: usize,
    end: usize,
    target: [u8; 4],
) -> Option<(usize, usize)> {
    let parsed = find_first_box(data, start, end, target)?;
    Some((parsed.body_start, parsed.body_end))
}

fn parse_tkhd_track_id(data: &Bytes, start: usize, end: usize) -> Option<u32> {
    let body = &data[start..end];
    if body.len() < 4 {
        return None;
    }

    let version = body[0];
    match version {
        0 if body.len() >= 16 => Some(u32::from_be_bytes([body[12], body[13], body[14], body[15]])),
        1 if body.len() >= 24 => Some(u32::from_be_bytes([body[20], body[21], body[22], body[23]])),
        _ => None,
    }
}

fn stsd_has_av01(data: &Bytes, start: usize, end: usize) -> bool {
    if end - start < 8 {
        return false;
    }

    let header = &data[start..end];
    let entry_count = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let mut offset = start + 8;

    for _ in 0..entry_count {
        let Some(parsed) = box_at(data, offset, end) else {
            break;
        };

        if parsed.fourcc == *b"av01" {
            return true;
        }

        offset = parsed.end;
    }

    false
}

fn track_is_av1(data: &Bytes, trak_start: usize, trak_end: usize) -> Option<u32> {
    let (tkhd_start, tkhd_end) = find_child_box_range(data, trak_start, trak_end, *b"tkhd")?;
    let track_id = parse_tkhd_track_id(data, tkhd_start, tkhd_end)?;

    let (mdia_start, mdia_end) = find_child_box_range(data, trak_start, trak_end, *b"mdia")?;
    let (minf_start, minf_end) = find_child_box_range(data, mdia_start, mdia_end, *b"minf")?;
    let (stbl_start, stbl_end) = find_child_box_range(data, minf_start, minf_end, *b"stbl")?;
    let (stsd_start, stsd_end) = find_child_box_range(data, stbl_start, stbl_end, *b"stsd")?;

    if stsd_has_av01(data, stsd_start, stsd_end) {
        Some(track_id)
    } else {
        None
    }
}

/// Extract AV1 track IDs from an fMP4 init segment (`moov` tree).
pub fn extract_av1_track_ids_from_init(data: &Bytes) -> Vec<u32> {
    let mut ids = Vec::new();

    let mut offset = 0;
    while offset < data.len() {
        let Some(parsed) = box_at(data, offset, data.len()) else {
            break;
        };

        if parsed.fourcc == *b"moov" {
            let mut moov_offset = parsed.body_start;
            let moov_end = parsed.end;

            while moov_offset < moov_end {
                let Some(child) = box_at(data, moov_offset, moov_end) else {
                    break;
                };

                if child.fourcc == *b"trak"
                    && let Some(track_id) = track_is_av1(data, child.body_start, child.end)
                {
                    ids.push(track_id);
                }

                moov_offset = child.end;
            }
        }

        offset = parsed.end;
    }

    ids.sort_unstable();
    ids.dedup();
    ids
}

#[derive(Debug, Clone, Copy)]
struct TfhdInfo {
    track_id: u32,
    base_data_offset: u64,
    default_sample_size: Option<u32>,
}

fn parse_tfhd(data: &Bytes, start: usize, end: usize, moof_start: usize) -> io::Result<TfhdInfo> {
    let body = &data[start..end];
    if body.len() < 8 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "tfhd box too short",
        ));
    }

    let flags = ((body[1] as u32) << 16) | ((body[2] as u32) << 8) | body[3] as u32;
    let track_id = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);

    let mut idx = 8;
    let mut base_data_offset = moof_start as u64;
    let mut default_sample_size = None;

    if flags & 0x000001 != 0 {
        if idx + 8 > body.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "tfhd missing base_data_offset",
            ));
        }
        base_data_offset = u64::from_be_bytes([
            body[idx],
            body[idx + 1],
            body[idx + 2],
            body[idx + 3],
            body[idx + 4],
            body[idx + 5],
            body[idx + 6],
            body[idx + 7],
        ]);
        idx += 8;
    }

    if flags & 0x000002 != 0 {
        idx += 4;
    }
    if flags & 0x000008 != 0 {
        idx += 4;
    }
    if flags & 0x000010 != 0 {
        if idx + 4 > body.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "tfhd missing default_sample_size",
            ));
        }
        default_sample_size = Some(u32::from_be_bytes([
            body[idx],
            body[idx + 1],
            body[idx + 2],
            body[idx + 3],
        ]));
        idx += 4;
    }
    if flags & 0x000020 != 0 {
        idx += 4;
    }

    if idx > body.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "tfhd fields exceed box size",
        ));
    }

    Ok(TfhdInfo {
        track_id,
        base_data_offset,
        default_sample_size,
    })
}

#[derive(Debug, Default)]
struct TrunValidationState {
    next_sample_offset: Option<usize>,
    checked_samples: usize,
}

fn parse_trun_and_validate_samples(
    data: &Bytes,
    start: usize,
    end: usize,
    tfhd_info: TfhdInfo,
    mdat_range: (usize, usize),
    options: Av1ValidationOptions,
    state: &mut TrunValidationState,
) -> io::Result<()> {
    let (mdat_start, mdat_end) = mdat_range;
    let body = &data[start..end];
    if body.len() < 8 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trun box too short",
        ));
    }

    let flags = ((body[1] as u32) << 16) | ((body[2] as u32) << 8) | body[3] as u32;
    let sample_count = u32::from_be_bytes([body[4], body[5], body[6], body[7]]) as usize;

    let mut idx = 8;
    let data_offset = if flags & 0x000001 != 0 {
        if idx + 4 > body.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "trun missing data_offset",
            ));
        }
        let value = i32::from_be_bytes([body[idx], body[idx + 1], body[idx + 2], body[idx + 3]]);
        idx += 4;
        Some(value)
    } else {
        None
    };

    if flags & 0x000004 != 0 {
        if idx + 4 > body.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "trun missing first_sample_flags",
            ));
        }
        idx += 4;
    }

    let has_sample_duration = flags & 0x000100 != 0;
    let has_sample_size = flags & 0x000200 != 0;
    let has_sample_flags = flags & 0x000400 != 0;
    let has_sample_cto = flags & 0x000800 != 0;

    let mut sample_offset = if let Some(data_offset) = data_offset {
        let offset = tfhd_info.base_data_offset as i64 + data_offset as i64;
        if offset < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "computed AV1 sample offset is negative",
            ));
        }
        offset as usize
    } else if let Some(offset) = state.next_sample_offset {
        offset
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "AV1 trun without data_offset and unknown running sample offset",
        ));
    };

    for _ in 0..sample_count {
        if has_sample_duration {
            if idx + 4 > body.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "trun sample duration overflows box",
                ));
            }
            idx += 4;
        }

        let sample_size = if has_sample_size {
            if idx + 4 > body.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "trun sample size overflows box",
                ));
            }
            let value =
                u32::from_be_bytes([body[idx], body[idx + 1], body[idx + 2], body[idx + 3]]);
            idx += 4;
            value
        } else {
            tfhd_info.default_sample_size.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "trun sample has no explicit size and tfhd has no default_sample_size",
                )
            })?
        };

        let sample_end = sample_offset + sample_size as usize;
        if sample_offset < mdat_start || sample_end > mdat_end {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "AV1 sample range [{sample_offset}..{sample_end}) is outside mdat [{mdat_start}..{mdat_end})"
                ),
            ));
        }

        validate_isobmff_sample_bytes_with_options(
            &data[sample_offset..sample_end],
            IsobmffSampleParseOptions {
                enforce_should_not_obus: options.enforce_should_not_obus,
                enforce_reserved_obus: options.enforce_reserved_obus,
            },
        )
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "AV1 sample conformance failure on track {}: {}",
                    tfhd_info.track_id, e
                ),
            )
        })?;

        state.checked_samples += 1;
        sample_offset = sample_end;

        if has_sample_flags {
            if idx + 4 > body.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "trun sample flags overflows box",
                ));
            }
            idx += 4;
        }

        if has_sample_cto {
            if idx + 4 > body.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "trun sample composition time overflows box",
                ));
            }
            idx += 4;
        }
    }

    state.next_sample_offset = Some(sample_offset);

    Ok(())
}

fn validate_av1_tracks_in_fragment(
    data: &Bytes,
    av1_track_ids: &[u32],
    track_ids_sorted: bool,
    options: Av1ValidationOptions,
) -> io::Result<Av1MediaValidationSummary> {
    let mut moof: Option<(usize, usize, usize)> = None;
    let mut mdat: Option<(usize, usize)> = None;

    let mut top_offset = 0;
    while top_offset < data.len() {
        let Some(parsed) = box_at(data, top_offset, data.len()) else {
            break;
        };

        if parsed.fourcc == *b"moof" && moof.is_none() {
            moof = Some((parsed.start, parsed.body_start, parsed.end));
        } else if parsed.fourcc == *b"mdat" && mdat.is_none() {
            mdat = Some((parsed.body_start, parsed.body_end));
        }

        top_offset = parsed.end;
    }

    let Some((moof_start, moof_body_start, moof_end)) = moof else {
        return Ok(Av1MediaValidationSummary::default());
    };
    let Some((mdat_start, mdat_end)) = mdat else {
        return Ok(Av1MediaValidationSummary::default());
    };

    let mut summary = Av1MediaValidationSummary::default();
    let mut moof_offset = moof_body_start;
    while moof_offset < moof_end {
        let Some(child) = box_at(data, moof_offset, moof_end) else {
            break;
        };

        if child.fourcc == *b"traf" {
            let traf_start = child.body_start;
            let traf_end = child.end;

            let mut tfhd: Option<TfhdInfo> = None;
            let mut is_av1_track = false;
            let mut counted_track = false;
            let mut trun_state = TrunValidationState {
                checked_samples: summary.checked_samples,
                ..TrunValidationState::default()
            };
            let mut pending_truns: Vec<(usize, usize)> = Vec::new();

            let mut traf_offset = traf_start;
            while traf_offset < traf_end {
                let Some(traf_child) = box_at(data, traf_offset, traf_end) else {
                    break;
                };

                if traf_child.fourcc == *b"tfhd" {
                    let parsed_tfhd =
                        parse_tfhd(data, traf_child.body_start, traf_child.end, moof_start)?;
                    is_av1_track = if track_ids_sorted {
                        av1_track_ids.binary_search(&parsed_tfhd.track_id).is_ok()
                    } else {
                        av1_track_ids.contains(&parsed_tfhd.track_id)
                    };

                    if is_av1_track && !counted_track {
                        summary.checked_tracks += 1;
                        counted_track = true;
                    }

                    if is_av1_track {
                        for (pending_start, pending_end) in pending_truns.drain(..) {
                            parse_trun_and_validate_samples(
                                data,
                                pending_start,
                                pending_end,
                                parsed_tfhd,
                                (mdat_start, mdat_end),
                                options,
                                &mut trun_state,
                            )?;
                        }
                    }

                    tfhd = Some(parsed_tfhd);
                } else if traf_child.fourcc == *b"trun" {
                    if let Some(tfhd_info) = tfhd {
                        if is_av1_track {
                            parse_trun_and_validate_samples(
                                data,
                                traf_child.body_start,
                                traf_child.end,
                                tfhd_info,
                                (mdat_start, mdat_end),
                                options,
                                &mut trun_state,
                            )?;
                        }
                    } else {
                        pending_truns.push((traf_child.body_start, traf_child.end));
                    }
                    traf_offset = traf_child.end;
                    continue;
                }

                traf_offset = traf_child.end;
            }

            if is_av1_track && let Some(tfhd_info) = tfhd {
                for (pending_start, pending_end) in pending_truns {
                    parse_trun_and_validate_samples(
                        data,
                        pending_start,
                        pending_end,
                        tfhd_info,
                        (mdat_start, mdat_end),
                        options,
                        &mut trun_state,
                    )?;
                }
            }

            summary.checked_samples = trun_state.checked_samples;
        }

        moof_offset = child.end;
    }

    Ok(summary)
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use super::*;
    use crate::test_support::{make_init_with_video_sample_entry, make_media_segment_for_track};

    #[test]
    fn test_validate_av1_media_segment_against_init_ok() {
        let init = make_init_with_video_sample_entry(1, *b"av01");

        let mut sample = Vec::new();
        av1::obu_stream::write_obu(&mut sample, av1::ObuType::Frame, None, &[0x11, 0x22]).unwrap();
        let media = make_media_segment_for_track(1, &sample);

        let summary = validate_av1_media_segment_against_init(&init, &media, true).unwrap();
        assert_eq!(summary.checked_tracks, 1);
        assert_eq!(summary.checked_samples, 1);
    }

    #[test]
    fn test_validate_av1_media_segment_rejects_disallowed_obu() {
        let init = make_init_with_video_sample_entry(1, *b"av01");

        let mut sample = Vec::new();
        av1::obu_stream::write_obu(&mut sample, av1::ObuType::TemporalDelimiter, None, &[])
            .unwrap();
        let media = make_media_segment_for_track(1, &sample);

        let err = validate_av1_media_segment_against_init(&init, &media, true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("OBU_TEMPORAL_DELIMITER"));
    }

    #[test]
    fn test_validate_av1_media_segment_no_av1_track_is_noop() {
        let init = make_init_with_video_sample_entry(7, *b"avc1");

        let mut sample = Vec::new();
        av1::obu_stream::write_obu(&mut sample, av1::ObuType::Frame, None, &[0xAA]).unwrap();
        let media = make_media_segment_for_track(7, &sample);

        let summary = validate_av1_media_segment_against_init(&init, &media, true).unwrap();
        assert_eq!(summary.checked_tracks, 0);
        assert_eq!(summary.checked_samples, 0);
    }
}
