use crate::tag::FlvTagType;
use std::io;

pub const PREV_TAG_SIZE_FIELD_SIZE: usize = 4;
pub const TAG_HEADER_SIZE: usize = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedTagHeader {
    pub tag_type: FlvTagType,
    pub is_filtered: bool,
    pub data_size: u32,
    pub timestamp_ms: u32,
    pub stream_id: u32,
}

pub fn parse_prev_tag_size(bytes: [u8; PREV_TAG_SIZE_FIELD_SIZE]) -> u32 {
    u32::from_be_bytes(bytes)
}

pub fn parse_tag_header_bytes(bytes: [u8; TAG_HEADER_SIZE]) -> io::Result<ParsedTagHeader> {
    let (tag_type, is_filtered) = {
        let tag_type_byte = bytes[0];
        let tag_type = FlvTagType::from(tag_type_byte & 0x1F);
        let is_filtered = (tag_type_byte & 0x20) != 0;
        (tag_type, is_filtered)
    };

    let data_size = ((bytes[1] as u32) << 16) | ((bytes[2] as u32) << 8) | (bytes[3] as u32);

    let timestamp_ms = ((bytes[7] as u32) << 24)
        | ((bytes[4] as u32) << 16)
        | ((bytes[5] as u32) << 8)
        | (bytes[6] as u32);

    let stream_id = ((bytes[8] as u32) << 16) | ((bytes[9] as u32) << 8) | (bytes[10] as u32);

    Ok(ParsedTagHeader {
        tag_type,
        is_filtered,
        data_size,
        timestamp_ms,
        stream_id,
    })
}
