//! Media the fake platform serves, made by ffmpeg (see
//! `tests/fixtures/media/README.md`). The default mesio engine repairs FLV as
//! it records, so recordings need real media rather than placeholder bytes.

use bytes::{BufMut, Bytes, BytesMut};

/// Two seconds of 32x32, 1 fps H.264 with AAC audio, every video frame a
/// keyframe.
const TINY_FLV: &[u8] = include_bytes!("../../fixtures/media/tiny.flv");

/// FLV file header (9 bytes) plus the first PreviousTagSize (4 bytes).
const FLV_PREAMBLE_LEN: usize = 13;
const TAG_HEADER_LEN: usize = 11;
const PREVIOUS_TAG_SIZE_LEN: usize = 4;
const AUDIO_TAG: u8 = 8;
const VIDEO_TAG: u8 = 9;
const SCRIPT_TAG: u8 = 18;
/// Duration of one AAC frame in the fixture (1024 samples at 16 kHz), the gap
/// left between the last tag of one repetition and the first of the next.
const FRAME_GAP_MS: u32 = 64;

struct Tag {
    timestamp_ms: u32,
    /// The whole tag: header, body and the PreviousTagSize after it.
    bytes: Bytes,
}

/// An endless FLV stream made by repeating [`TINY_FLV`]'s media tags, each
/// repetition shifted by the fixture's length so timestamps keep rising.
///
/// Each [`Self::stream`] starts with the file header, the script tag and the
/// codec sequence headers, as a new connection does; every repetition, from
/// either method, continues the timestamps where the previous one stopped.
pub struct FlvLoop {
    header: Bytes,
    media: Vec<Tag>,
    period_ms: u32,
    loops: u32,
}

impl FlvLoop {
    pub fn tiny() -> Self {
        Self::parse(TINY_FLV)
    }

    fn parse(file: &[u8]) -> Self {
        assert_eq!(&file[..3], b"FLV", "not an FLV file");
        let mut header = BytesMut::from(&file[..FLV_PREAMBLE_LEN]);
        let mut media = Vec::new();
        let mut offset = FLV_PREAMBLE_LEN;
        while offset < file.len() {
            let tag_header = &file[offset..offset + TAG_HEADER_LEN];
            let kind = tag_header[0] & 0x1f;
            let size = u32::from_be_bytes([0, tag_header[1], tag_header[2], tag_header[3]]);
            let timestamp_ms =
                u32::from_be_bytes([tag_header[7], tag_header[4], tag_header[5], tag_header[6]]);
            let end = offset + TAG_HEADER_LEN + size as usize + PREVIOUS_TAG_SIZE_LEN;
            let tag = &file[offset..end];
            let body = &tag[TAG_HEADER_LEN..];
            // Byte 1 of an AVC or AAC body is its packet type: 0 is the
            // sequence header, 2 (AVC only) the end of the sequence.
            match (kind, body[1]) {
                (SCRIPT_TAG, _) | (VIDEO_TAG | AUDIO_TAG, 0) => header.extend_from_slice(tag),
                (VIDEO_TAG, 2) => {}
                (VIDEO_TAG | AUDIO_TAG, _) => media.push(Tag {
                    timestamp_ms,
                    bytes: Bytes::copy_from_slice(tag),
                }),
                (other, _) => panic!("unexpected FLV tag type {other}"),
            }
            offset = end;
        }
        let last = media.iter().map(|tag| tag.timestamp_ms).max().unwrap_or(0);
        Self {
            header: header.freeze(),
            media,
            period_ms: last + FRAME_GAP_MS,
            loops: 0,
        }
    }

    /// The next repetition of the media tags, to continue a connection that
    /// [`Self::stream`] started.
    pub fn next_loop(&mut self) -> Bytes {
        let offset = self.loops * self.period_ms;
        self.loops += 1;
        let mut out = BytesMut::with_capacity(self.media.iter().map(|tag| tag.bytes.len()).sum());
        for tag in &self.media {
            let timestamp = (tag.timestamp_ms + offset).to_be_bytes();
            out.put_slice(&tag.bytes[..4]);
            // FLV stores 24 timestamp bits, then the upper 8 bits.
            out.put_slice(&[timestamp[1], timestamp[2], timestamp[3], timestamp[0]]);
            out.put_slice(&tag.bytes[8..]);
        }
        out.freeze()
    }

    /// The header followed by the next `loops` repetitions.
    pub fn stream(&mut self, loops: u32) -> Bytes {
        let mut out = BytesMut::from(&self.header[..]);
        for _ in 0..loops {
            out.extend_from_slice(&self.next_loop());
        }
        out.freeze()
    }
}
