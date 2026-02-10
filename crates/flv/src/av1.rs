use av1::{AV1CodecConfigurationRecord, ObuHeader, seq::SequenceHeaderObu};
use bytes::Bytes;

use crate::resolution::Resolution;

/// AV1 Packet
///
/// Container for AV1 data within an enhanced FLV video tag.
#[derive(Debug, Clone, PartialEq)]
pub enum Av1Packet {
    /// AV1 sequence start (codec configuration record).
    SequenceStart(AV1CodecConfigurationRecord),
    /// AV1 raw frame data (low-overhead OBU bitstream).
    Raw(Bytes),
    /// AV1 end of sequence.
    EndOfSequence,
}

impl Av1Packet {
    /// Extracts the video resolution from a [`SequenceStart`](Av1Packet::SequenceStart) packet.
    ///
    /// Parses the sequence header OBU embedded in the codec configuration record
    /// to obtain `max_frame_width` and `max_frame_height`.
    ///
    /// Returns `None` for non-`SequenceStart` variants or if parsing fails.
    pub fn get_video_resolution(&self) -> Option<Resolution> {
        let config = match self {
            Av1Packet::SequenceStart(config) => config,
            _ => return None,
        };

        if config.config_obu.is_empty() {
            return None;
        }

        let mut cursor = std::io::Cursor::new(config.config_obu.clone());
        let header = ObuHeader::parse(&mut cursor).ok()?;
        let seq = SequenceHeaderObu::parse(header, &mut cursor).ok()?;

        Some(Resolution {
            width: seq.max_frame_width as f32,
            height: seq.max_frame_height as f32,
        })
    }
}

impl std::fmt::Display for Av1Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Av1Packet::SequenceStart(config) => write!(
                f,
                "SequenceStart [Profile: {}, Level: {}]",
                config.seq_profile, config.seq_level_idx_0
            ),
            Av1Packet::Raw(data) => write!(f, "Data ({} bytes)", data.len()),
            Av1Packet::EndOfSequence => write!(f, "EndOfSequence"),
        }
    }
}
