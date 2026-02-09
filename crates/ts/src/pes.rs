use bytes::Bytes;

use crate::{Result, TsError};

/// Video stream ID range (0xE0..=0xEF)
pub const STREAM_ID_VIDEO_MIN: u8 = 0xE0;
/// Video stream ID range (0xE0..=0xEF)
pub const STREAM_ID_VIDEO_MAX: u8 = 0xEF;
/// Audio stream ID range (0xC0..=0xDF)
pub const STREAM_ID_AUDIO_MIN: u8 = 0xC0;
/// Audio stream ID range (0xC0..=0xDF)
pub const STREAM_ID_AUDIO_MAX: u8 = 0xDF;
/// Private stream 1
pub const STREAM_ID_PRIVATE_1: u8 = 0xBD;
/// Private stream 2
pub const STREAM_ID_PRIVATE_2: u8 = 0xBF;
/// Padding stream
pub const STREAM_ID_PADDING: u8 = 0xBE;

/// Parse a 33-bit PTS or DTS timestamp from 5 bytes.
///
/// Layout: `[marker(4) | ts32..30 | 1 | ts29..15 | 1 | ts14..0 | 1]`
fn parse_timestamp(data: &[u8]) -> Option<u64> {
    if data.len() < 5 {
        return None;
    }
    let ts = (((data[0] as u64 >> 1) & 0x07) << 30)
        | ((data[1] as u64) << 22)
        | (((data[2] as u64 >> 1) & 0x7F) << 15)
        | ((data[3] as u64) << 7)
        | ((data[4] as u64 >> 1) & 0x7F);
    Some(ts)
}

/// Check if a stream_id has an optional PES header (PTS/DTS fields).
fn has_optional_pes_header(stream_id: u8) -> bool {
    // Per ISO 13818-1 Table 2-18, these stream IDs do NOT have optional header:
    !matches!(
        stream_id,
        0xBC   // program_stream_map
        | 0xBE // padding_stream
        | 0xBF // private_stream_2
        | 0xF0 // ECM_stream
        | 0xF1 // EMM_stream
        | 0xFF // program_stream_directory
        | 0xF2 // DSMCC_stream
        | 0xF8 // ITU-T Rec. H.222.1 type E
    )
}

/// Owned PES header with parsed fields.
#[derive(Debug, Clone)]
pub struct PesHeader {
    pub stream_id: u8,
    pub pes_packet_length: u16,
    pub pts: Option<u64>,
    pub dts: Option<u64>,
    pub data_alignment_indicator: bool,
    pub pes_header_data_length: u8,
    /// Offset to elementary stream data (past the PES header)
    pub payload_offset: usize,
}

impl PesHeader {
    /// Parse PES header from a byte slice starting with the PES start code (0x000001).
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 6 {
            return Err(TsError::InsufficientData {
                expected: 6,
                actual: data.len(),
            });
        }

        // Verify start code prefix: 0x00 0x00 0x01
        if data[0] != 0x00 || data[1] != 0x00 || data[2] != 0x01 {
            return Err(TsError::InvalidPesStartCode);
        }

        let stream_id = data[3];
        let pes_packet_length = ((data[4] as u16) << 8) | data[5] as u16;

        if !has_optional_pes_header(stream_id) {
            return Ok(PesHeader {
                stream_id,
                pes_packet_length,
                pts: None,
                dts: None,
                data_alignment_indicator: false,
                pes_header_data_length: 0,
                payload_offset: 6,
            });
        }

        if data.len() < 9 {
            return Err(TsError::InsufficientData {
                expected: 9,
                actual: data.len(),
            });
        }

        let data_alignment_indicator = (data[6] & 0x04) != 0;
        let pts_dts_flags = (data[7] >> 6) & 0x03;
        let pes_header_data_length = data[8];
        let header_end = 9 + pes_header_data_length as usize;

        let (pts, dts) = match pts_dts_flags {
            0b00 => (None, None),
            0b01 => {
                return Err(TsError::InvalidPtsDtsFlags(pts_dts_flags));
            }
            0b10 => {
                // PTS only
                if data.len() < 14 {
                    return Err(TsError::InsufficientData {
                        expected: 14,
                        actual: data.len(),
                    });
                }
                (parse_timestamp(&data[9..14]), None)
            }
            0b11 => {
                // PTS + DTS
                if data.len() < 19 {
                    return Err(TsError::InsufficientData {
                        expected: 19,
                        actual: data.len(),
                    });
                }
                (
                    parse_timestamp(&data[9..14]),
                    parse_timestamp(&data[14..19]),
                )
            }
            _ => unreachable!(),
        };

        Ok(PesHeader {
            stream_id,
            pes_packet_length,
            pts,
            dts,
            data_alignment_indicator,
            pes_header_data_length,
            payload_offset: header_end,
        })
    }

    /// Convert PTS to seconds.
    pub fn pts_seconds(&self) -> Option<f64> {
        self.pts.map(|pts| pts as f64 / 90_000.0)
    }

    /// Convert DTS to seconds.
    pub fn dts_seconds(&self) -> Option<f64> {
        self.dts.map(|dts| dts as f64 / 90_000.0)
    }

    /// Get the elementary stream payload (after PES header).
    pub fn payload<'a>(&self, data: &'a [u8]) -> Option<&'a [u8]> {
        if self.payload_offset <= data.len() {
            Some(&data[self.payload_offset..])
        } else {
            None
        }
    }

    /// Check if this is a video stream.
    pub fn is_video(&self) -> bool {
        self.stream_id >= STREAM_ID_VIDEO_MIN && self.stream_id <= STREAM_ID_VIDEO_MAX
    }

    /// Check if this is an audio stream.
    pub fn is_audio(&self) -> bool {
        self.stream_id >= STREAM_ID_AUDIO_MIN && self.stream_id <= STREAM_ID_AUDIO_MAX
    }
}

/// Zero-copy PES header reference.
#[derive(Debug, Clone)]
pub struct PesHeaderRef {
    data: Bytes,
    pub stream_id: u8,
    pub pes_packet_length: u16,
    pub pts: Option<u64>,
    pub dts: Option<u64>,
    pub data_alignment_indicator: bool,
    pub pes_header_data_length: u8,
    payload_offset: usize,
}

impl PesHeaderRef {
    /// Parse PES header from Bytes starting with the PES start code (0x000001).
    pub fn parse(data: Bytes) -> Result<Self> {
        let header = PesHeader::parse(&data)?;
        Ok(PesHeaderRef {
            data,
            stream_id: header.stream_id,
            pes_packet_length: header.pes_packet_length,
            pts: header.pts,
            dts: header.dts,
            data_alignment_indicator: header.data_alignment_indicator,
            pes_header_data_length: header.pes_header_data_length,
            payload_offset: header.payload_offset,
        })
    }

    /// Get the elementary stream payload (after PES header).
    pub fn payload(&self) -> Bytes {
        if self.payload_offset <= self.data.len() {
            self.data.slice(self.payload_offset..)
        } else {
            Bytes::new()
        }
    }

    /// Convert PTS to seconds.
    pub fn pts_seconds(&self) -> Option<f64> {
        self.pts.map(|pts| pts as f64 / 90_000.0)
    }

    /// Convert DTS to seconds.
    pub fn dts_seconds(&self) -> Option<f64> {
        self.dts.map(|dts| dts as f64 / 90_000.0)
    }

    /// Check if this is a video stream.
    pub fn is_video(&self) -> bool {
        self.stream_id >= STREAM_ID_VIDEO_MIN && self.stream_id <= STREAM_ID_VIDEO_MAX
    }

    /// Check if this is an audio stream.
    pub fn is_audio(&self) -> bool {
        self.stream_id >= STREAM_ID_AUDIO_MIN && self.stream_id <= STREAM_ID_AUDIO_MAX
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pes_with_pts(stream_id: u8, pts: u64) -> Vec<u8> {
        let mut data = vec![
            0x00, 0x00, 0x01, // start code
            stream_id, 0x00, 0x00, // stream_id + length=0 (unbounded)
            0x80, // marker bits
            0x80, // PTS only (pts_dts_flags=0b10)
            0x05, // pes_header_data_length=5
        ];
        // Encode PTS in 5 bytes
        let mut pts_bytes = [0u8; 5];
        pts_bytes[0] = 0x21 | (((pts >> 30) as u8 & 0x07) << 1);
        pts_bytes[1] = (pts >> 22) as u8;
        pts_bytes[2] = ((pts >> 15) as u8 & 0x7F) << 1 | 0x01;
        pts_bytes[3] = (pts >> 7) as u8;
        pts_bytes[4] = ((pts as u8) & 0x7F) << 1 | 0x01;
        data.extend_from_slice(&pts_bytes);
        data.extend_from_slice(&[0xDE, 0xAD]); // payload
        data
    }

    #[test]
    fn test_pes_header_pts_only() {
        let data = make_pes_with_pts(0xE0, 90000); // 1 second
        let header = PesHeader::parse(&data).unwrap();
        assert_eq!(header.stream_id, 0xE0);
        assert!(header.is_video());
        assert!(!header.is_audio());
        assert_eq!(header.pts, Some(90000));
        assert!(header.dts.is_none());
        let seconds = header.pts_seconds().unwrap();
        assert!((seconds - 1.0).abs() < 1e-9);
        assert_eq!(header.payload_offset, 14);
    }

    #[test]
    fn test_pes_header_pts_dts() {
        let pts: u64 = 180000; // 2 seconds
        let dts: u64 = 90000; // 1 second
        let mut data = vec![
            0x00, 0x00, 0x01, // start code
            0xE0, 0x00, 0x00, // video stream, length=0
            0x80, // marker bits
            0xC0, // PTS + DTS (pts_dts_flags=0b11)
            0x0A, // pes_header_data_length=10
        ];
        // Encode PTS
        let mut pts_bytes = [0u8; 5];
        pts_bytes[0] = 0x31 | (((pts >> 30) as u8 & 0x07) << 1);
        pts_bytes[1] = (pts >> 22) as u8;
        pts_bytes[2] = ((pts >> 15) as u8 & 0x7F) << 1 | 0x01;
        pts_bytes[3] = (pts >> 7) as u8;
        pts_bytes[4] = ((pts as u8) & 0x7F) << 1 | 0x01;
        data.extend_from_slice(&pts_bytes);
        // Encode DTS
        let mut dts_bytes = [0u8; 5];
        dts_bytes[0] = 0x11 | (((dts >> 30) as u8 & 0x07) << 1);
        dts_bytes[1] = (dts >> 22) as u8;
        dts_bytes[2] = ((dts >> 15) as u8 & 0x7F) << 1 | 0x01;
        dts_bytes[3] = (dts >> 7) as u8;
        dts_bytes[4] = ((dts as u8) & 0x7F) << 1 | 0x01;
        data.extend_from_slice(&dts_bytes);
        data.push(0xFF); // payload

        let header = PesHeader::parse(&data).unwrap();
        assert_eq!(header.pts, Some(180000));
        assert_eq!(header.dts, Some(90000));
        let pts_sec = header.pts_seconds().unwrap();
        assert!((pts_sec - 2.0).abs() < 1e-9);
        let dts_sec = header.dts_seconds().unwrap();
        assert!((dts_sec - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_pes_header_no_timestamps() {
        let data = vec![
            0x00, 0x00, 0x01, // start code
            0xC0, 0x00, 0x05, // audio stream, length=5
            0x80, // marker bits
            0x00, // no PTS/DTS (pts_dts_flags=0b00)
            0x00, // pes_header_data_length=0
            0xAA, 0xBB, // payload
        ];
        let header = PesHeader::parse(&data).unwrap();
        assert_eq!(header.stream_id, 0xC0);
        assert!(header.is_audio());
        assert!(header.pts.is_none());
        assert!(header.dts.is_none());
        assert_eq!(header.payload_offset, 9);
    }

    #[test]
    fn test_pes_header_max_pts() {
        // Max 33-bit value: 0x1FFFFFFFF = 8589934591
        let data = make_pes_with_pts(0xE0, 0x1_FFFF_FFFF);
        let header = PesHeader::parse(&data).unwrap();
        assert_eq!(header.pts, Some(0x1_FFFF_FFFF));
    }

    #[test]
    fn test_pes_header_zero_pts() {
        let data = make_pes_with_pts(0xE0, 0);
        let header = PesHeader::parse(&data).unwrap();
        assert_eq!(header.pts, Some(0));
    }

    #[test]
    fn test_pes_invalid_start_code() {
        let data = vec![0x00, 0x00, 0x00, 0xE0, 0x00, 0x00];
        assert!(matches!(
            PesHeader::parse(&data),
            Err(TsError::InvalidPesStartCode)
        ));
    }

    #[test]
    fn test_pes_invalid_pts_dts_flags() {
        let data = vec![
            0x00, 0x00, 0x01, // start code
            0xE0, 0x00, 0x00, // video stream
            0x80, // marker bits
            0x40, // pts_dts_flags=0b01 (forbidden)
            0x00, // pes_header_data_length=0
        ];
        assert!(matches!(
            PesHeader::parse(&data),
            Err(TsError::InvalidPtsDtsFlags(0x01))
        ));
    }

    #[test]
    fn test_pes_header_ref() {
        let data = make_pes_with_pts(0xE0, 90000);
        let header_ref = PesHeaderRef::parse(Bytes::from(data)).unwrap();
        assert_eq!(header_ref.pts, Some(90000));
        assert!(header_ref.is_video());
        let payload = header_ref.payload();
        assert_eq!(&payload[..], &[0xDE, 0xAD]);
    }

    #[test]
    fn test_pes_padding_stream() {
        // Padding stream has no optional PES header
        let data = vec![
            0x00,
            0x00,
            0x01, // start code
            STREAM_ID_PADDING,
            0x00,
            0x04, // length=4
            0xFF,
            0xFF,
            0xFF,
            0xFF, // padding bytes
        ];
        let header = PesHeader::parse(&data).unwrap();
        assert_eq!(header.stream_id, STREAM_ID_PADDING);
        assert!(header.pts.is_none());
        assert_eq!(header.payload_offset, 6);
    }
}
