//! # Test Utilities
//!
//! This module contains common utility functions and structs for testing FLV processing components.
//! These utilities help create consistent test environments and reduce code duplication across tests.
#[cfg(test)]
use amf0::Amf0Value;
#[cfg(test)]
use bytes::Bytes;
#[cfg(test)]
use flv::data::FlvData;
#[cfg(test)]
use flv::header::FlvHeader;
#[cfg(test)]
use flv::tag::{FlvTag, FlvTagType, FlvUtil};
#[cfg(test)]
use std::borrow::Cow;

/// Create a standard FLV header for testing
#[cfg(test)]
pub fn create_test_header() -> FlvData {
    FlvData::Header(FlvHeader::new(true, true))
}

/// Create a generic FlvTag for testing
#[cfg(test)]
pub fn create_test_tag(tag_type: FlvTagType, timestamp: u32, data: Vec<u8>) -> FlvData {
    FlvData::Tag(FlvTag {
        timestamp_ms: timestamp,
        stream_id: 0,
        tag_type,
        data: Bytes::from(data),
    })
}

/// Create a video tag with specified timestamp and keyframe flag
#[cfg(test)]
pub fn create_video_tag(timestamp: u32, is_keyframe: bool) -> FlvData {
    // First byte: 4 bits frame type (1=keyframe, 2=inter), 4 bits codec id (7=AVC)
    let frame_type = if is_keyframe { 1 } else { 2 };
    let first_byte = (frame_type << 4) | 7; // AVC codec
    create_test_tag(FlvTagType::Video, timestamp, vec![first_byte, 1, 0, 0, 0])
}

/// Create a video tag with specified size (for testing size limits)
#[cfg(test)]
pub fn create_video_tag_with_size(timestamp: u32, is_keyframe: bool, size: usize) -> FlvData {
    let frame_type = if is_keyframe { 1 } else { 2 };
    let first_byte = (frame_type << 4) | 7; // AVC codec

    // Create a data buffer of specified size
    let mut data = vec![0u8; size];
    data[0] = first_byte;
    data[1] = 1; // AVC NALU

    create_test_tag(FlvTagType::Video, timestamp, data)
}

/// Create an audio tag with specified timestamp
#[cfg(test)]
pub fn create_audio_tag(timestamp: u32) -> FlvData {
    create_test_tag(
        FlvTagType::Audio,
        timestamp,
        vec![0xAF, 1, 0x21, 0x10, 0x04],
    )
}

/// Create a script data (metadata) tag
#[cfg(test)]
pub fn create_script_tag(timestamp: u32, with_keyframes: bool) -> FlvData {
    let mut properties = vec![
        (Cow::Borrowed("duration"), Amf0Value::Number(120.5)),
        (Cow::Borrowed("width"), Amf0Value::Number(1920.0)),
        (Cow::Borrowed("height"), Amf0Value::Number(1080.0)),
        (Cow::Borrowed("videocodecid"), Amf0Value::Number(7.0)),
        (Cow::Borrowed("audiocodecid"), Amf0Value::Number(10.0)),
    ];

    if with_keyframes {
        let keyframes_obj = vec![
            (
                Cow::Borrowed("times"),
                Amf0Value::StrictArray(Cow::Owned(vec![
                    Amf0Value::Number(0.0),
                    Amf0Value::Number(5.0),
                ])),
            ),
            (
                Cow::Borrowed("filepositions"),
                Amf0Value::StrictArray(Cow::Owned(vec![
                    Amf0Value::Number(100.0),
                    Amf0Value::Number(2500.0),
                ])),
            ),
        ];

        properties.push((
            Cow::Borrowed("keyframes"),
            Amf0Value::Object(Cow::Owned(keyframes_obj)),
        ));
    }

    let obj = Amf0Value::Object(Cow::Owned(properties));
    let mut buffer = Vec::new();
    amf0::Amf0Encoder::encode_string(&mut buffer, crate::AMF0_ON_METADATA).unwrap();
    amf0::Amf0Encoder::encode(&mut buffer, &obj).unwrap();

    create_test_tag(FlvTagType::ScriptData, timestamp, buffer)
}

/// Create a video sequence header with specified version
#[cfg(test)]
pub fn create_video_sequence_header(avc_version: u8) -> FlvData {
    let data = vec![
        0x17, // Keyframe (1) + AVC (7)
        0x00, // AVC sequence header
        0x00,
        0x00,
        0x00,        // Composition time
        avc_version, // AVC version, use different values to test detection
        0x64,
        0x00,
        0x28, // AVCC data
    ];
    create_test_tag(FlvTagType::Video, 0, data)
}

/// Create an audio sequence header with specified version
#[cfg(test)]
pub fn create_audio_sequence_header(aac_version: u8) -> FlvData {
    let data = vec![
        0xAF,        // Audio format 10 (AAC) + sample rate 3 (44kHz) + sample size 1 (16-bit) + stereo
        0x00,        // AAC sequence header
        aac_version, // AAC specific config, use different values to test detection
        0x10,
    ];
    create_test_tag(FlvTagType::Audio, 0, data)
}

/// Extract timestamps from processed items
#[cfg(test)]
pub fn extract_timestamps(items: &[FlvData]) -> Vec<u32> {
    items
        .iter()
        .filter_map(|item| match item {
            FlvData::Tag(tag) => Some(tag.timestamp_ms),
            _ => None,
        })
        .collect()
}

/// Print tag information for debugging
#[cfg(test)]
pub fn print_tags(items: &[FlvData]) {
    println!("Tag sequence:");
    for (i, item) in items.iter().enumerate() {
        match item {
            FlvData::Header(_) => println!("  {i}: Header"),
            FlvData::Tag(tag) => {
                let type_str = match tag.tag_type {
                    FlvTagType::Audio => {
                        if tag.is_audio_sequence_header() {
                            "Audio (Header)"
                        } else {
                            "Audio"
                        }
                    }
                    FlvTagType::Video => {
                        if tag.is_key_frame_nalu() {
                            "Video (Keyframe)"
                        } else if tag.is_video_sequence_header() {
                            "Video (Header)"
                        } else {
                            "Video"
                        }
                    }
                    FlvTagType::ScriptData => "Script",
                    _ => "Unknown",
                };
                println!("  {i}: {type_str} @ {ts}ms", ts = tag.timestamp_ms);
            }
            _ => println!("  {i}: Other"),
        }
    }
}
