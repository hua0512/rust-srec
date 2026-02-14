//! Minimal ISOBMFF (ISO Base Media File Format) box parsing for fMP4 init segments.
//!
//! This module provides just enough parsing to detect codec types from fMP4
//! initialization segments used in HLS/CMAF delivery. It walks the box tree
//! to find sample entries in the `stsd` box and identifies codecs by FourCC.

use bytes::Bytes;
use tracing::debug;

use crate::Resolution;
use crate::box_utils::{box_at, find_first_box_payload};

#[cfg(test)]
use crate::box_utils::read_box_header;

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

/// Options for parsing and profiling fMP4 init segments.
#[derive(Debug, Clone, Copy, Default)]
pub struct ParseOptions {
    /// If `true`, attempt to parse codec configuration data to populate
    /// [`InitSegmentInfo::video_resolution`].
    pub include_resolution: bool,
}

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
    /// Raw `avcC` box payload (`AVCDecoderConfigurationRecord` bytes), if found.
    pub avcc_data: Option<Bytes>,
    /// Raw `hvcC` box payload (`HEVCDecoderConfigurationRecord` bytes), if found.
    pub hvcc_data: Option<Bytes>,
    /// Best-effort video resolution extracted from codec configuration, if available.
    pub video_resolution: Option<Resolution>,
}

/// Parse an fMP4 init segment.
///
/// This API is intentionally minimal and uses `Bytes` throughout to allow
/// zero-copy slicing of codec configuration boxes.
pub fn parse_init_segment(data: &Bytes) -> InitSegmentInfo {
    parse_init_segment_with_options(data, ParseOptions::default())
}

/// Parse an fMP4 init segment with options.
pub fn parse_init_segment_with_options(data: &Bytes, options: ParseOptions) -> InitSegmentInfo {
    let mut info = InitSegmentInfo::default();
    walk_boxes_bytes(data, 0, data.len(), &mut info);
    if options.include_resolution {
        info.video_resolution = info.video_resolution_from_codec_config();
    }
    info
}

fn walk_boxes_bytes(data: &Bytes, start: usize, end: usize, info: &mut InitSegmentInfo) {
    let mut offset = start;
    while offset < end {
        let Some(parsed) = box_at(data, offset, end) else {
            break;
        };

        if CONTAINER_BOXES.contains(&parsed.fourcc) {
            walk_boxes_bytes(data, parsed.body_start, parsed.body_end, info);
        } else if parsed.fourcc == *b"stsd" {
            parse_stsd_bytes(data, parsed.body_start, parsed.body_end, info);
        }

        offset = parsed.end;
    }
}

/// Parse the `stsd` (Sample Description) box to identify codec sample entries.
///
/// `stsd` is a FullBox: 4 bytes (version + flags) + 4 bytes (entry_count),
/// followed by sample entry boxes.
fn parse_stsd_bytes(data: &Bytes, start: usize, end: usize, info: &mut InitSegmentInfo) {
    if end - start < 8 {
        return;
    }

    let header = &data[start..end];
    let entry_count = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let mut offset = start + 8;

    for _ in 0..entry_count {
        if offset + 8 > end {
            break;
        }

        let Some(parsed) = box_at(data, offset, end) else {
            break;
        };

        debug!(
            "Found sample entry: {} (size: {})",
            fourcc_to_string(&parsed.fourcc),
            parsed.size
        );

        match &parsed.fourcc {
            b"av01" => {
                info.has_av1 = true;
                let inner_offset = parsed.header_size + VISUAL_SAMPLE_ENTRY_HEADER;
                if inner_offset < parsed.size {
                    let inner_start = offset + inner_offset;
                    let inner_end = parsed.end;
                    info.av1c_data = find_box_bytes(data, inner_start, inner_end, b"av1C");
                }
            }
            b"avc1" | b"avc3" => {
                info.has_h264 = true;
                let inner_offset = parsed.header_size + VISUAL_SAMPLE_ENTRY_HEADER;
                if inner_offset < parsed.size {
                    let inner_start = offset + inner_offset;
                    let inner_end = parsed.end;
                    info.avcc_data = find_box_bytes(data, inner_start, inner_end, b"avcC");
                }
            }
            b"hvc1" | b"hev1" => {
                info.has_h265 = true;
                let inner_offset = parsed.header_size + VISUAL_SAMPLE_ENTRY_HEADER;
                if inner_offset < parsed.size {
                    let inner_start = offset + inner_offset;
                    let inner_end = parsed.end;
                    info.hvcc_data = find_box_bytes(data, inner_start, inner_end, b"hvcC");
                }
            }
            b"mp4a" => {
                info.has_aac = true;
            }
            b"ac-3" | b"ec-3" => {
                info.has_ac3 = true;
            }
            b"Opus" => {}
            _ => {
                debug!(
                    "Unknown sample entry FourCC: {}",
                    fourcc_to_string(&parsed.fourcc)
                );
            }
        }

        offset = parsed.end;
    }
}

fn find_box_bytes(data: &Bytes, start: usize, end: usize, target: &[u8; 4]) -> Option<Bytes> {
    find_first_box_payload(data, start, end, *target)
}

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

impl InitSegmentInfo {
    fn video_resolution_from_codec_config(&self) -> Option<Resolution> {
        if self.has_av1 {
            let av1c = self.av1c_data.as_ref()?;
            let config_obu = av1::AV1CodecConfigurationRecord::config_obu_bytes(av1c).ok()?;
            if config_obu.is_empty() {
                return None;
            }

            let mut obu_cursor = std::io::Cursor::new(config_obu);
            let header = av1::ObuHeader::parse(&mut obu_cursor).ok()?;
            let seq = av1::seq::SequenceHeaderObu::parse(header, &mut obu_cursor).ok()?;
            return Some(Resolution::new(
                seq.max_frame_width as u32,
                seq.max_frame_height as u32,
            ));
        }

        if self.has_h265 {
            let hvcc = self.hvcc_data.as_ref()?;
            let sps_bytes =
                h265::HEVCDecoderConfigurationRecord::first_sps_nalu_bytes(hvcc).ok()?;
            let sps = h265::SpsNALUnit::parse(std::io::Cursor::new(sps_bytes.as_ref())).ok()?;
            return Some(Resolution::new(
                sps.rbsp.cropped_width() as u32,
                sps.rbsp.cropped_height() as u32,
            ));
        }

        if self.has_h264 {
            let avcc = self.avcc_data.as_ref()?;
            let sps_bytes = h264::AVCDecoderConfigurationRecord::first_sps_nalu_bytes(avcc).ok()?;
            let sps = h264::Sps::parse_with_emulation_prevention(std::io::Cursor::new(
                sps_bytes.as_ref(),
            ))
            .ok()?;
            return Some(Resolution::new(sps.width() as u32, sps.height() as u32));
        }

        None
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use super::*;
    use crate::test_support::{
        make_audio_sample_entry, make_box, make_fullbox_body, make_visual_sample_entry,
    };

    #[test]
    fn test_read_box_header_basic() {
        let data = [
            0x00, 0x00, 0x00, 0x10, b'f', b't', b'y', b'p', 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let (size, fourcc, header_size) = read_box_header(&data).unwrap();
        assert_eq!(size, 16);
        assert_eq!(&fourcc, b"ftyp");
        assert_eq!(header_size, 8);
    }

    #[test]
    fn test_read_box_header_extended_size() {
        let mut data = vec![0x00, 0x00, 0x00, 0x01, b'm', b'o', b'o', b'v'];
        data.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 24]);
        data.extend_from_slice(&[0u8; 8]);
        let (size, fourcc, header_size) = read_box_header(&data).unwrap();
        assert_eq!(size, 24);
        assert_eq!(&fourcc, b"moov");
        assert_eq!(header_size, 16);
    }

    #[test]
    fn test_read_box_header_size_zero() {
        let data = [0x00, 0x00, 0x00, 0x00, b't', b'e', b's', b't', 1, 2, 3];
        let (size, fourcc, header_size) = read_box_header(&data).unwrap();
        assert_eq!(size, 11);
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
        let data = Bytes::new();
        let info = parse_init_segment(&data);
        assert!(!info.has_av1);
        assert!(!info.has_h264);
        assert!(!info.has_h265);
        assert!(!info.has_aac);
        assert!(!info.has_ac3);
        assert!(info.av1c_data.is_none());
        assert!(info.avcc_data.is_none());
        assert!(info.hvcc_data.is_none());
        assert!(info.video_resolution.is_none());
    }

    #[test]
    fn test_parse_init_segment_with_h264() {
        let sample_entry = make_visual_sample_entry(b"avc1", &[]);
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

        let data = Bytes::from(moov);
        let info = parse_init_segment(&data);
        assert!(info.has_h264);
        assert!(!info.has_av1);
        assert!(!info.has_h265);
        assert!(info.avcc_data.is_none());
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

        let data = Bytes::from(moov);
        let info = parse_init_segment(&data);
        assert!(info.has_h265);
        assert!(!info.has_av1);
        assert!(!info.has_h264);
        assert!(info.hvcc_data.is_none());
    }

    #[test]
    fn test_parse_init_segment_with_av1_and_av1c() {
        let av1c_payload = vec![0x81, 0x04, 0x0C, 0x00];
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

        let data = Bytes::from(moov);
        let info = parse_init_segment(&data);
        assert!(info.has_av1);
        assert!(!info.has_h264);
        assert!(!info.has_h265);
        assert!(info.av1c_data.is_some());
        assert_eq!(info.av1c_data.unwrap().as_ref(), &av1c_payload);
    }

    #[test]
    fn test_parse_init_segment_with_h264_and_avcc() {
        let avcc_payload = b"\x01d\0\x1f\xff\xe1\0\x19\x67\x64\x00\x1F\xAC\xD9\x41\xE0\x6D\xF9\xE6\xA0\x20\x20\x28\x00\x00\x03\x00\x08\x00\x00\x03\x01\xE0\x01\0\x06h\xeb\xe3\xcb\"\xc0\xfd\xf8\xf8\0";
        let avcc_box = make_box(b"avcC", avcc_payload);
        let sample_entry = make_visual_sample_entry(b"avc1", &avcc_box);

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

        let data = Bytes::from(moov);
        let info = parse_init_segment(&data);
        assert!(info.has_h264);
        assert!(info.avcc_data.is_some());
        assert_eq!(info.avcc_data.unwrap().as_ref(), avcc_payload);
    }

    #[test]
    fn test_parse_init_segment_with_h265_and_hvcc() {
        let hvcc_payload = b"\x01\x01@\0\0\0\x90\0\0\0\0\0\x99\xf0\0\xfc\xfd\xf8\xf8\0\0\x0f\x03 \0\x01\0\x18@\x01\x0c\x01\xff\xff\x01@\0\0\x03\0\x90\0\0\x03\0\0\x03\0\x99\x95@\x90!\0\x01\0=B\x01\x01\x01@\0\0\x03\0\x90\0\0\x03\0\0\x03\0\x99\xa0\x01@ \x05\xa1e\x95R\x90\x84d_\xf8\xc0Z\x80\x80\x80\x82\0\0\x03\0\x02\0\0\x03\x01 \xc0\x0b\xbc\xa2\0\x02bX\0\x011-\x08\"\0\x01\0\x07D\x01\xc0\x93|\x0c\xc9";
        let hvcc_box = make_box(b"hvcC", hvcc_payload);
        let sample_entry = make_visual_sample_entry(b"hvc1", &hvcc_box);

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

        let data = Bytes::from(moov);
        let info = parse_init_segment(&data);
        assert!(info.has_h265);
        assert!(info.hvcc_data.is_some());
        assert_eq!(info.hvcc_data.unwrap().as_ref(), hvcc_payload);
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

        let data = Bytes::from(moov);
        let info = parse_init_segment(&data);
        assert!(info.has_aac);
        assert!(!info.has_av1);
    }

    #[test]
    fn test_parse_init_segment_with_options_resolution_opt_in_h264() {
        let avcc_payload = Bytes::from_static(
            b"\x01d\0\x1f\xff\xe1\0\x19\x67\x64\x00\x1F\xAC\xD9\x41\xE0\x6D\xF9\xE6\xA0\x20\x20\x28\x00\x00\x03\x00\x08\x00\x00\x03\x01\xE0\x01\0\x06h\xeb\xe3\xcb\"\xc0\xfd\xf8\xf8\0",
        );
        let avcc_box = make_box(b"avcC", avcc_payload.as_ref());
        let sample_entry = make_visual_sample_entry(b"avc1", &avcc_box);
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

        let data = Bytes::from(moov);
        let fast = parse_init_segment_with_options(
            &data,
            ParseOptions {
                include_resolution: false,
            },
        );
        assert!(fast.video_resolution.is_none());

        let full = parse_init_segment_with_options(
            &data,
            ParseOptions {
                include_resolution: true,
            },
        );
        assert!(full.video_resolution.is_some());

        let avcc = h264::AVCDecoderConfigurationRecord::parse(&mut std::io::Cursor::new(
            avcc_payload.clone(),
        ))
        .unwrap();
        let sps =
            h264::Sps::parse_with_emulation_prevention(std::io::Cursor::new(avcc.sps[0].as_ref()))
                .unwrap();
        assert_eq!(
            full.video_resolution.unwrap(),
            Resolution::new(sps.width() as u32, sps.height() as u32)
        );
    }

    #[test]
    fn test_parse_init_segment_with_options_resolution_opt_in_h265() {
        let hvcc_payload = Bytes::from_static(
            b"\x01\x01@\0\0\0\x90\0\0\0\0\0\x99\xf0\0\xfc\xfd\xf8\xf8\0\0\x0f\x03 \0\x01\0\x18@\x01\x0c\x01\xff\xff\x01@\0\0\x03\0\x90\0\0\x03\0\0\x03\0\x99\x95@\x90!\0\x01\0=B\x01\x01\x01@\0\0\x03\0\x90\0\0\x03\0\0\x03\0\x99\xa0\x01@ \x05\xa1e\x95R\x90\x84d_\xf8\xc0Z\x80\x80\x80\x82\0\0\x03\0\x02\0\0\x03\x01 \xc0\x0b\xbc\xa2\0\x02bX\0\x011-\x08\"\0\x01\0\x07D\x01\xc0\x93|\x0c\xc9",
        );
        let hvcc_box = make_box(b"hvcC", hvcc_payload.as_ref());
        let sample_entry = make_visual_sample_entry(b"hvc1", &hvcc_box);
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

        let data = Bytes::from(moov);
        let fast = parse_init_segment_with_options(
            &data,
            ParseOptions {
                include_resolution: false,
            },
        );
        assert!(fast.video_resolution.is_none());

        let full = parse_init_segment_with_options(
            &data,
            ParseOptions {
                include_resolution: true,
            },
        );
        assert!(full.video_resolution.is_some());

        let hvcc = h265::HEVCDecoderConfigurationRecord::demux(&mut std::io::Cursor::new(
            hvcc_payload.clone(),
        ))
        .unwrap();
        let sps_array = hvcc
            .arrays
            .iter()
            .find(|a| a.nal_unit_type == h265::NALUnitType::SpsNut)
            .unwrap();
        let sps =
            h265::SpsNALUnit::parse(std::io::Cursor::new(sps_array.nalus[0].as_ref())).unwrap();
        assert_eq!(
            full.video_resolution.unwrap(),
            Resolution::new(
                sps.rbsp.cropped_width() as u32,
                sps.rbsp.cropped_height() as u32
            )
        );
    }
}
