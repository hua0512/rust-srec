//! Shared fMP4 test builders.
//!
//! This module is available for local mp4 tests and optionally for downstream
//! crate tests when the `test-utils` feature is enabled.

use bytes::Bytes;

/// Bytes to skip in a visual sample entry body before child boxes begin.
const VISUAL_SAMPLE_ENTRY_HEADER: usize = 70;

/// Bytes to skip in an audio sample entry body before child boxes begin.
const AUDIO_SAMPLE_ENTRY_HEADER: usize = 20;

pub fn make_box(fourcc: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let size = (8 + body.len()) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(fourcc);
    out.extend_from_slice(body);
    out
}

pub fn make_full_box(fourcc: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + payload.len());
    body.push(version);
    body.push(((flags >> 16) & 0xFF) as u8);
    body.push(((flags >> 8) & 0xFF) as u8);
    body.push((flags & 0xFF) as u8);
    body.extend_from_slice(payload);
    make_box(fourcc, &body)
}

pub fn make_fullbox_body(content: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 4];
    out.extend_from_slice(content);
    out
}

pub fn make_visual_sample_entry(fourcc: &[u8; 4], children: &[u8]) -> Vec<u8> {
    let body_len = VISUAL_SAMPLE_ENTRY_HEADER + children.len();
    let total = 8 + body_len;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&[0u8; VISUAL_SAMPLE_ENTRY_HEADER]);
    out.extend_from_slice(children);
    out
}

pub fn make_audio_sample_entry(fourcc: &[u8; 4], children: &[u8]) -> Vec<u8> {
    let body_len = AUDIO_SAMPLE_ENTRY_HEADER + children.len();
    let total = 8 + body_len;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&[0u8; AUDIO_SAMPLE_ENTRY_HEADER]);
    out.extend_from_slice(children);
    out
}

pub fn make_init_with_video_sample_entry(track_id: u32, sample_entry: [u8; 4]) -> Bytes {
    let mut tkhd_payload = Vec::new();
    tkhd_payload.extend_from_slice(&0u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&0u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&track_id.to_be_bytes());
    tkhd_payload.extend_from_slice(&0u32.to_be_bytes());
    let tkhd = make_full_box(b"tkhd", 0, 0, &tkhd_payload);

    let sample_entry_box = make_box(&sample_entry, &[]);

    let mut stsd_payload = Vec::new();
    stsd_payload.extend_from_slice(&1u32.to_be_bytes());
    stsd_payload.extend_from_slice(&sample_entry_box);
    let stsd = make_full_box(b"stsd", 0, 0, &stsd_payload);

    let stbl = make_box(b"stbl", &stsd);
    let minf = make_box(b"minf", &stbl);
    let mdia = make_box(b"mdia", &minf);

    let mut trak_body = Vec::new();
    trak_body.extend_from_slice(&tkhd);
    trak_body.extend_from_slice(&mdia);
    let trak = make_box(b"trak", &trak_body);

    let moov = make_box(b"moov", &trak);
    Bytes::from(moov)
}

pub fn make_media_segment_for_track(track_id: u32, sample: &[u8]) -> Bytes {
    let mut tfhd_payload = Vec::new();
    tfhd_payload.extend_from_slice(&track_id.to_be_bytes());
    let tfhd = make_full_box(b"tfhd", 0, 0, &tfhd_payload);

    // trun flags: data_offset_present + sample_size_present
    let trun_flags = 0x000001 | 0x000200;
    let mut trun_payload = Vec::new();
    trun_payload.extend_from_slice(&1u32.to_be_bytes()); // sample_count
    trun_payload.extend_from_slice(&0i32.to_be_bytes()); // placeholder data_offset
    trun_payload.extend_from_slice(&(sample.len() as u32).to_be_bytes());
    let mut trun = make_full_box(b"trun", 0, trun_flags, &trun_payload);

    // Compute data_offset from the sizes we already know (no need to build moof twice).
    // moof = box header (8) + traf
    // traf = box header (8) + tfhd + trun
    // data_offset points past moof to the mdat payload, i.e. moof.len() + 8 (mdat header).
    let moof_len = 8 + 8 + tfhd.len() + trun.len();
    let data_offset = (moof_len + 8) as i32;

    let trun_data_offset_pos = 8 /* box header */ + 4 /* fullbox flags */ + 4 /* sample_count */;
    trun[trun_data_offset_pos..trun_data_offset_pos + 4]
        .copy_from_slice(&data_offset.to_be_bytes());

    let mut traf_body = Vec::new();
    traf_body.extend_from_slice(&tfhd);
    traf_body.extend_from_slice(&trun);
    let traf = make_box(b"traf", &traf_body);
    let moof = make_box(b"moof", &traf);
    let mdat = make_box(b"mdat", sample);

    let mut out = Vec::new();
    out.extend_from_slice(&moof);
    out.extend_from_slice(&mdat);
    Bytes::from(out)
}
