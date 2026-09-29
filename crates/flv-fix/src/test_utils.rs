//! # Test Utilities
//!
//! This module contains common utility functions and structs for testing FLV processing components.
//! These utilities help create consistent test environments and reduce code duplication across tests.
use amf0::Amf0Value;
use bytes::Bytes;
use flv::data::FlvData;
use flv::header::FlvHeader;
use flv::tag::{FlvTag, FlvTagType};
use std::borrow::Cow;

/// Create a standard FLV header for testing
pub fn create_test_header() -> FlvData {
    FlvData::Header(FlvHeader::new(true, true))
}

/// Create a generic FlvTag for testing
pub fn create_test_tag(tag_type: FlvTagType, timestamp: u32, data: Vec<u8>) -> FlvData {
    FlvData::Tag(FlvTag::new(
        timestamp,
        0,
        tag_type,
        false,
        Bytes::from(data),
    ))
}

/// Create a video tag with specified timestamp and keyframe flag
pub fn create_video_tag(timestamp: u32, is_keyframe: bool) -> FlvData {
    // First byte: 4 bits frame type (1=keyframe, 2=inter), 4 bits codec id (7=AVC)
    let frame_type = if is_keyframe { 1 } else { 2 };
    let first_byte = (frame_type << 4) | 7; // AVC codec
    create_test_tag(FlvTagType::Video, timestamp, vec![first_byte, 1, 0, 0, 0])
}

/// Create a video tag with specified size (for testing size limits)
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
pub fn create_audio_tag(timestamp: u32) -> FlvData {
    create_test_tag(
        FlvTagType::Audio,
        timestamp,
        vec![0xAF, 1, 0x21, 0x10, 0x04],
    )
}

/// Create a script data (metadata) tag
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
pub fn create_video_sequence_header(timestamp: u32, version: u8) -> FlvData {
    let data = vec![
        0x17, // Keyframe (1) + AVC (7)
        0x00, // AVC sequence header
        0x00, 0x00, 0x00,    // Composition time
        version, // AVC version
        0x64, 0x00, 0x28, // AVCC data
    ];
    create_test_tag(FlvTagType::Video, timestamp, data)
}

/// Create an audio sequence header with specified version
pub fn create_audio_sequence_header(timestamp: u32, version: u8) -> FlvData {
    let data = vec![
        0xAF,    // Audio format 10 (AAC) + sample rate 3 (44kHz) + sample size 1 (16-bit) + stereo
        0x00,    // AAC sequence header
        version, // AAC specific config
        0x10,
    ];
    create_test_tag(FlvTagType::Audio, timestamp, data)
}

/// Extract timestamps from processed items
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
pub fn print_tags(items: &[FlvData]) {
    println!("Tag sequence:");
    for (i, item) in items.iter().enumerate() {
        match item {
            FlvData::Header(_) => println!("  {i}: Header"),
            FlvData::Tag(tag) => {
                let type_str = match tag.tag_type() {
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
            FlvData::Split(reason) => println!("  {i}: Split({reason:?})"),
            FlvData::EndOfSequence(_) => println!("  {i}: EndOfSequence"),
        }
    }
}

/// Real codec payloads (FLV tag bodies) for operators that inspect codec data.
/// FFmpeg is only needed to regenerate them, not to run tests.
pub mod fixtures {
    /// Legacy AAC-LC sequence header: 44.1 kHz stereo AudioSpecificConfig.
    pub const AAC_LC_CONFIG: &[u8] = b"\xaf\0\x12\x10";

    /// Legacy AAC-LC stereo silence, one 1024-sample raw_data_block at 44.1 kHz.
    /// Second audio packet of:
    /// `ffmpeg -f lavfi -i anullsrc=r=44100:cl=stereo -frames:a 2 -c:a aac -f flv silence.flv`
    pub const AAC_LC_SILENCE: &[u8] = b"\xaf\x01\x21\x10\x04\x60\x8c\x1c";
    /// `AAC_LC_SILENCE` in Enhanced-RTMP `CodedFrames` framing.
    pub const AAC_LC_SILENCE_ENHANCED: &[u8] = b"\x91mp4a\x21\x10\x04\x60\x8c\x1c";

    // Legacy AVC 16x16 black video, SEI omitted:
    // `ffmpeg -f lavfi -i color=c=black:s=16x16:r=10 -t 2 -c:v libx264
    //  -preset ultrafast -tune zerolatency -g 10 -f flv black.flv`
    /// AVCDecoderConfigurationRecord from the command above.
    pub const AVC_CONFIG_16X16: &[u8] = b"\x17\0\0\0\0\x01\x42\xc0\x0a\xff\xe1\0\x15\x67\x42\xc0\x0a\xda\x7b\x01\x10\0\0\x03\0\x10\0\0\x03\x01\x48\xf1\x22\x6a\x01\0\x04\x68\xce\x0f\xc8";
    /// IDR frame from the command above.
    pub const AVC_IDR_16X16: &[u8] =
        b"\x17\x01\0\0\0\0\0\0\x0a\x65\x88\x84\x3a\x26\x28\0\x09\x02\xe0";
    /// P frame from the command above.
    pub const AVC_P_16X16: &[u8] = b"\x27\x01\0\0\0\0\0\0\x05\x41\x9a\x20\x32\x94";

    // Enhanced-RTMP AV1 16x16 black keyframe:
    // `ffmpeg -f lavfi -i color=c=black:s=16x16:r=10 -frames:v 1
    //  -c:v libaom-av1 -cpu-used 8 -g 1 -crf 40 -f flv black.flv`
    /// AV1CodecConfigurationRecord (av1C) from the command above, without FLV framing.
    pub const AV1_CODEC_CONFIG: &[u8] = b"\x81\0\x0c\0\x0a\x0a\0\0\0\x01\x9f\xf9\xb5\xf2\0\x80";
    /// Keyframe (`CodedFrames`) tag body from the command above.
    pub const AV1_KEYFRAME: &[u8] = b"\x91av01\x12\0\x0a\x0a\0\0\0\x01\x9f\xf9\xb5\xf2\0\x80\x32\x0e\x10\0\xd0\0\0\x02\x80\0\0\0\xa9\x8e\x5e\xd0";
}
