use crate::header::FlvHeader;
use crate::tag::FlvTagType;
use std::io;

pub const FLV_HEADER_SIZE: usize = 9;
pub const PREV_TAG_SIZE_FIELD_SIZE: usize = 4;
pub const TAG_HEADER_SIZE: usize = 11;

const MAX_TAG_DATA_SIZE: u32 = 0xFF_FFFF;

pub fn encode_header_bytes(
    header: &FlvHeader,
) -> io::Result<[u8; FLV_HEADER_SIZE + PREV_TAG_SIZE_FIELD_SIZE]> {
    let mut out = [0u8; FLV_HEADER_SIZE + PREV_TAG_SIZE_FIELD_SIZE];

    // Signature: "FLV"
    out[0] = 0x46;
    out[1] = 0x4C;
    out[2] = 0x56;

    // Version
    out[3] = header.version;

    // Flags: bit 2 = audio, bit 0 = video
    let mut flags = 0u8;
    if header.has_video {
        flags |= 0x01;
    }
    if header.has_audio {
        flags |= 0x04;
    }
    out[4] = flags;

    // DataOffset (BE u32)
    // We only emit the standard 9-byte header. If a decoded stream had an extended header
    // (DataOffset > 9), those bytes are not preserved, so we canonicalize on write.
    let data_offset = (FLV_HEADER_SIZE as u32).to_be_bytes();
    out[5..9].copy_from_slice(&data_offset);

    // PreviousTagSize0 (BE u32)
    out[9..13].copy_from_slice(&0u32.to_be_bytes());

    Ok(out)
}

pub fn encode_prev_tag_size_bytes(prev_tag_size: u32) -> [u8; PREV_TAG_SIZE_FIELD_SIZE] {
    prev_tag_size.to_be_bytes()
}

pub fn encode_tag_header_bytes(
    tag_type: FlvTagType,
    is_filtered: bool,
    data_size: u32,
    timestamp_ms: u32,
    stream_id: u32,
) -> io::Result<[u8; TAG_HEADER_SIZE]> {
    let mut out = [0u8; TAG_HEADER_SIZE];

    if data_size > MAX_TAG_DATA_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("FLV tag data size ({data_size}) exceeds 24-bit limit ({MAX_TAG_DATA_SIZE})"),
        ));
    }

    if stream_id > 0xFF_FFFF {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("FLV tag stream_id out of range for UI24: {stream_id}"),
        ));
    }

    out[0] = (if is_filtered { 0x20 } else { 0x00 }) | u8::from(tag_type);

    // DataSize is UI24.
    out[1] = (data_size >> 16) as u8;
    out[2] = (data_size >> 8) as u8;
    out[3] = data_size as u8;

    // Timestamp: lower 24 bits + extended 8 bits.
    out[4] = (timestamp_ms >> 16) as u8;
    out[5] = (timestamp_ms >> 8) as u8;
    out[6] = timestamp_ms as u8;
    out[7] = (timestamp_ms >> 24) as u8;

    // StreamID is UI24, typically 0.
    out[8] = (stream_id >> 16) as u8;
    out[9] = (stream_id >> 8) as u8;
    out[10] = stream_id as u8;

    Ok(out)
}
