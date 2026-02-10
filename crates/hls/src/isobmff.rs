//! Minimal ISOBMFF (ISO Base Media File Format) box parsing for fMP4 init segments.
//!
//! This module provides just enough parsing to detect codec types from fMP4
//! initialization segments used in HLS/CMAF delivery. It walks the box tree
//! to find sample entries in the `stsd` box and identifies codecs by FourCC.

use bytes::Bytes;
use tracing::debug;

/// Well-known ISOBMFF container box FourCCs that we descend into.
const CONTAINER_BOXES: &[[u8; 4]] = &[*b"moov", *b"trak", *b"mdia", *b"minf", *b"stbl"];

/// Bytes to skip in a visual sample entry body before child boxes begin.
///
/// Layout (ISO 14496-12 VisualSampleEntry):
///   6 reserved + 2 data_ref_idx + 16 pre-defined/reserved +
///   2 width + 2 height + 4 horiz_res + 4 vert_res + 4 reserved +
///   2 frame_count + 32 compressor_name + 2 depth + 2 pre-defined = 78 bytes
///   minus 8-byte box header already consumed by `read_box_header` = 70
const VISUAL_SAMPLE_ENTRY_HEADER: usize = 70;

/// Bytes to skip in an audio sample entry body before child boxes begin.
///
/// Layout (ISO 14496-12 AudioSampleEntry):
///   6 reserved + 2 data_ref_idx + 8 reserved +
///   2 channel_count + 2 sample_size + 2 pre-defined + 2 reserved +
///   4 sample_rate = 28 bytes
///   minus 8-byte box header already consumed = 20
#[cfg(test)]
const AUDIO_SAMPLE_ENTRY_HEADER: usize = 20;

/// Result of parsing an fMP4 init segment for codec information.
#[derive(Debug, Clone, Default)]
pub struct InitSegmentInfo {
    pub has_av1: bool,
    pub has_h264: bool,
    pub has_h265: bool,
    pub has_aac: bool,
    pub has_ac3: bool,
    /// Raw `av1C` box payload (`AV1CodecConfigurationRecord` bytes), if found.
    pub av1c_data: Option<Bytes>,
}

/// Parse an fMP4 init segment to detect codecs and extract AV1 config.
pub fn parse_init_segment(data: &[u8]) -> InitSegmentInfo {
    let mut info = InitSegmentInfo::default();
    walk_boxes(data, &mut info);
    info
}

/// Read a box header: returns `(total_box_size, fourcc, header_size)`.
///
/// Handles 32-bit size, 64-bit extended size (`size == 1`),
/// and box-extends-to-EOF (`size == 0`).
fn read_box_header(data: &[u8]) -> Option<(usize, [u8; 4], usize)> {
    if data.len() < 8 {
        return None;
    }

    let size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64;
    let fourcc: [u8; 4] = [data[4], data[5], data[6], data[7]];

    if size == 1 {
        // 64-bit extended size
        if data.len() < 16 {
            return None;
        }
        let ext_size = u64::from_be_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]);
        Some((ext_size as usize, fourcc, 16))
    } else if size == 0 {
        // Box extends to end of data
        Some((data.len(), fourcc, 8))
    } else {
        Some((size as usize, fourcc, 8))
    }
}

/// Recursively walk ISOBMFF boxes looking for `stsd`.
fn walk_boxes(data: &[u8], info: &mut InitSegmentInfo) {
    let mut offset = 0;
    while offset < data.len() {
        let remaining = &data[offset..];
        let Some((box_size, fourcc, header_size)) = read_box_header(remaining) else {
            break;
        };

        if box_size < header_size || offset + box_size > data.len() {
            break;
        }

        let box_body = &remaining[header_size..box_size];

        if CONTAINER_BOXES.contains(&fourcc) {
            walk_boxes(box_body, info);
        } else if fourcc == *b"stsd" {
            parse_stsd(box_body, info);
        }

        offset += box_size;
    }
}

/// Parse the `stsd` (Sample Description) box to identify codec sample entries.
///
/// `stsd` is a FullBox: 4 bytes (version + flags) + 4 bytes (entry_count),
/// followed by sample entry boxes.
fn parse_stsd(data: &[u8], info: &mut InitSegmentInfo) {
    if data.len() < 8 {
        return;
    }

    let entry_count = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let mut offset = 8;

    for _ in 0..entry_count {
        if offset + 8 > data.len() {
            break;
        }
        let remaining = &data[offset..];
        let Some((entry_size, fourcc, header_size)) = read_box_header(remaining) else {
            break;
        };

        if entry_size < header_size || offset + entry_size > data.len() {
            break;
        }

        debug!(
            "Found sample entry: {} (size: {entry_size})",
            fourcc_to_string(&fourcc)
        );

        match &fourcc {
            b"av01" => {
                info.has_av1 = true;
                let inner_offset = header_size + VISUAL_SAMPLE_ENTRY_HEADER;
                if inner_offset < entry_size {
                    let inner_data = &remaining[inner_offset..entry_size];
                    info.av1c_data = find_box(inner_data, b"av1C");
                }
            }
            b"avc1" | b"avc3" => {
                info.has_h264 = true;
            }
            b"hvc1" | b"hev1" => {
                info.has_h265 = true;
            }
            b"mp4a" => {
                info.has_aac = true;
            }
            b"ac-3" | b"ec-3" => {
                info.has_ac3 = true;
            }
            b"Opus" => {
                // Opus audio — detected but not tracked separately
            }
            _ => {
                debug!(
                    "Unknown sample entry FourCC: {}",
                    fourcc_to_string(&fourcc)
                );
            }
        }

        offset += entry_size;
    }
}

/// Find a specific child box within a data region and return its body.
fn find_box(data: &[u8], target: &[u8; 4]) -> Option<Bytes> {
    let mut offset = 0;
    while offset < data.len() {
        let remaining = &data[offset..];
        let (box_size, fourcc, header_size) = read_box_header(remaining)?;

        if box_size < header_size || offset + box_size > data.len() {
            break;
        }

        if fourcc == *target {
            let body = &remaining[header_size..box_size];
            return Some(Bytes::copy_from_slice(body));
        }

        offset += box_size;
    }
    None
}

/// Convert a FourCC to a display string.
fn fourcc_to_string(fourcc: &[u8; 4]) -> String {
    fourcc
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '?'
            }
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use super::*;

    /// Build an ISOBMFF box: `[size_be32][fourcc][body...]`
    fn make_box(fourcc: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let size = (8 + body.len()) as u32;
        let mut out = Vec::with_capacity(size as usize);
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(fourcc);
        out.extend_from_slice(body);
        out
    }

    /// Build a FullBox body: `[version=0][flags=0x000000][content...]`
    fn make_fullbox_body(content: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; 4];
        out.extend_from_slice(content);
        out
    }

    /// Build a minimal visual sample entry box (just header + 70 zero bytes + child boxes).
    fn make_visual_sample_entry(fourcc: &[u8; 4], children: &[u8]) -> Vec<u8> {
        let body_len = VISUAL_SAMPLE_ENTRY_HEADER + children.len();
        let total = 8 + body_len;
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&(total as u32).to_be_bytes());
        out.extend_from_slice(fourcc);
        out.extend_from_slice(&[0u8; VISUAL_SAMPLE_ENTRY_HEADER]);
        out.extend_from_slice(children);
        out
    }

    /// Build a minimal audio sample entry box.
    fn make_audio_sample_entry(fourcc: &[u8; 4], children: &[u8]) -> Vec<u8> {
        let body_len = AUDIO_SAMPLE_ENTRY_HEADER + children.len();
        let total = 8 + body_len;
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&(total as u32).to_be_bytes());
        out.extend_from_slice(fourcc);
        out.extend_from_slice(&[0u8; AUDIO_SAMPLE_ENTRY_HEADER]);
        out.extend_from_slice(children);
        out
    }

    #[test]
    fn test_read_box_header_basic() {
        let data = [0x00, 0x00, 0x00, 0x10, b'f', b't', b'y', b'p', 0, 0, 0, 0, 0, 0, 0, 0];
        let (size, fourcc, header_size) = read_box_header(&data).unwrap();
        assert_eq!(size, 16);
        assert_eq!(&fourcc, b"ftyp");
        assert_eq!(header_size, 8);
    }

    #[test]
    fn test_read_box_header_extended_size() {
        let mut data = vec![0x00, 0x00, 0x00, 0x01, b'm', b'o', b'o', b'v'];
        // 64-bit size = 24
        data.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 24]);
        data.extend_from_slice(&[0u8; 8]); // padding to reach 24 bytes
        let (size, fourcc, header_size) = read_box_header(&data).unwrap();
        assert_eq!(size, 24);
        assert_eq!(&fourcc, b"moov");
        assert_eq!(header_size, 16);
    }

    #[test]
    fn test_read_box_header_size_zero() {
        let data = [0x00, 0x00, 0x00, 0x00, b't', b'e', b's', b't', 1, 2, 3];
        let (size, fourcc, header_size) = read_box_header(&data).unwrap();
        assert_eq!(size, 11); // extends to end of data
        assert_eq!(&fourcc, b"test");
        assert_eq!(header_size, 8);
    }

    #[test]
    fn test_read_box_header_too_short() {
        assert!(read_box_header(&[0; 7]).is_none());
        assert!(read_box_header(&[]).is_none());
    }

    #[test]
    fn test_parse_init_segment_empty() {
        let info = parse_init_segment(&[]);
        assert!(!info.has_av1);
        assert!(!info.has_h264);
        assert!(!info.has_h265);
        assert!(!info.has_aac);
        assert!(!info.has_ac3);
        assert!(info.av1c_data.is_none());
    }

    #[test]
    fn test_parse_init_segment_with_h264() {
        let sample_entry = make_visual_sample_entry(b"avc1", &[]);
        let stsd_body = make_fullbox_body(&{
            let mut content = 1u32.to_be_bytes().to_vec(); // entry_count = 1
            content.extend_from_slice(&sample_entry);
            content
        });
        let stsd = make_box(b"stsd", &stsd_body);
        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);
        let mdia = make_box(b"mdia", &minf);
        let trak = make_box(b"trak", &mdia);
        let moov = make_box(b"moov", &trak);

        let info = parse_init_segment(&moov);
        assert!(info.has_h264);
        assert!(!info.has_av1);
        assert!(!info.has_h265);
    }

    #[test]
    fn test_parse_init_segment_with_h265() {
        let sample_entry = make_visual_sample_entry(b"hvc1", &[]);
        let stsd_body = make_fullbox_body(&{
            let mut content = 1u32.to_be_bytes().to_vec();
            content.extend_from_slice(&sample_entry);
            content
        });
        let stsd = make_box(b"stsd", &stsd_body);
        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);
        let mdia = make_box(b"mdia", &minf);
        let trak = make_box(b"trak", &mdia);
        let moov = make_box(b"moov", &trak);

        let info = parse_init_segment(&moov);
        assert!(info.has_h265);
        assert!(!info.has_av1);
        assert!(!info.has_h264);
    }

    #[test]
    fn test_parse_init_segment_with_av1_and_av1c() {
        let av1c_payload = vec![0x81, 0x04, 0x0C, 0x00]; // minimal av1C data
        let av1c_box = make_box(b"av1C", &av1c_payload);
        let sample_entry = make_visual_sample_entry(b"av01", &av1c_box);
        let stsd_body = make_fullbox_body(&{
            let mut content = 1u32.to_be_bytes().to_vec();
            content.extend_from_slice(&sample_entry);
            content
        });
        let stsd = make_box(b"stsd", &stsd_body);
        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);
        let mdia = make_box(b"mdia", &minf);
        let trak = make_box(b"trak", &mdia);
        let moov = make_box(b"moov", &trak);

        let info = parse_init_segment(&moov);
        assert!(info.has_av1);
        assert!(!info.has_h264);
        assert!(!info.has_h265);
        assert!(info.av1c_data.is_some());
        assert_eq!(info.av1c_data.unwrap().as_ref(), &av1c_payload);
    }

    #[test]
    fn test_parse_init_segment_with_audio() {
        let sample_entry = make_audio_sample_entry(b"mp4a", &[]);
        let stsd_body = make_fullbox_body(&{
            let mut content = 1u32.to_be_bytes().to_vec();
            content.extend_from_slice(&sample_entry);
            content
        });
        let stsd = make_box(b"stsd", &stsd_body);
        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);
        let mdia = make_box(b"mdia", &minf);
        let trak = make_box(b"trak", &mdia);
        let moov = make_box(b"moov", &trak);

        let info = parse_init_segment(&moov);
        assert!(info.has_aac);
        assert!(!info.has_av1);
    }

    #[test]
    fn test_parse_init_segment_multiple_tracks() {
        // Video track (AV1)
        let av1c_box = make_box(b"av1C", &[0x81, 0x04, 0x0C, 0x00]);
        let video_entry = make_visual_sample_entry(b"av01", &av1c_box);
        let video_stsd_body = make_fullbox_body(&{
            let mut c = 1u32.to_be_bytes().to_vec();
            c.extend_from_slice(&video_entry);
            c
        });
        let video_stsd = make_box(b"stsd", &video_stsd_body);
        let video_stbl = make_box(b"stbl", &video_stsd);
        let video_minf = make_box(b"minf", &video_stbl);
        let video_mdia = make_box(b"mdia", &video_minf);
        let video_trak = make_box(b"trak", &video_mdia);

        // Audio track (AAC)
        let audio_entry = make_audio_sample_entry(b"mp4a", &[]);
        let audio_stsd_body = make_fullbox_body(&{
            let mut c = 1u32.to_be_bytes().to_vec();
            c.extend_from_slice(&audio_entry);
            c
        });
        let audio_stsd = make_box(b"stsd", &audio_stsd_body);
        let audio_stbl = make_box(b"stbl", &audio_stsd);
        let audio_minf = make_box(b"minf", &audio_stbl);
        let audio_mdia = make_box(b"mdia", &audio_minf);
        let audio_trak = make_box(b"trak", &audio_mdia);

        let mut moov_body = Vec::new();
        moov_body.extend_from_slice(&video_trak);
        moov_body.extend_from_slice(&audio_trak);
        let moov = make_box(b"moov", &moov_body);

        let info = parse_init_segment(&moov);
        assert!(info.has_av1);
        assert!(info.has_aac);
        assert!(info.av1c_data.is_some());
    }

    #[test]
    fn test_parse_init_segment_truncated_box() {
        // Box declares size 100 but data is only 12 bytes
        let data = [0x00, 0x00, 0x00, 0x64, b'm', b'o', b'o', b'v', 0, 0, 0, 0];
        let info = parse_init_segment(&data);
        // Should not panic, just return empty info
        assert!(!info.has_av1);
    }

    #[test]
    fn test_fourcc_to_string() {
        assert_eq!(fourcc_to_string(b"moov"), "moov");
        assert_eq!(fourcc_to_string(b"av01"), "av01");
        assert_eq!(fourcc_to_string(b"ac-3"), "ac-3");
    }

    #[test]
    fn test_find_box_present() {
        let inner = make_box(b"av1C", &[1, 2, 3]);
        let other = make_box(b"colr", &[4, 5]);
        let mut data = Vec::new();
        data.extend_from_slice(&other);
        data.extend_from_slice(&inner);

        let result = find_box(&data, b"av1C");
        assert!(result.is_some());
        assert_eq!(result.unwrap().as_ref(), &[1, 2, 3]);
    }

    #[test]
    fn test_find_box_absent() {
        let other = make_box(b"colr", &[4, 5]);
        let result = find_box(&other, b"av1C");
        assert!(result.is_none());
    }
}
