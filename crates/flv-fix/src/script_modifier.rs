//! # Script Data Modifier Module
//!
//! This module provides functionality for modifying FLV script data (metadata) sections
//! based on collected statistics and analysis.
//!
//! ## Key Features:
//!
//! - Updates metadata in FLV files with accurate statistics
//! - Preserves the existing script-tag payload layout
//! - Manages keyframe indices for proper seeking functionality
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!

use std::{
    fs,
    io::{self, BufReader, Read, Seek, Write},
    path::Path,
};

use flv::tag::FlvTagType;
use tracing::{debug, trace, warn};

use crate::{
    amf::{
        builder::{FixedSizeMetadataError, OnMetaDataBuilder},
        model::AmfScriptData,
    },
    analyzer::FlvStats,
};

/// Error type for script modification operations
#[derive(Debug, thiserror::Error)]
pub enum ScriptModifierError {
    #[error("IO Error: {0}")]
    Io(#[from] io::Error),
    #[error("AMF0 Write Error: {0}")]
    Amf0Write(#[from] amf0::Amf0WriteError),
    #[error("Fixed-size metadata error: {0}")]
    FixedSizeMetadata(#[from] FixedSizeMetadataError),
    #[error("Script data error: {0}")]
    ScriptData(&'static str),
}

/// Injects stats into the script data section of an FLV file.
/// * `file_path` - The path to the FLV file.
/// * `stats` - The statistics to inject into the script data section.
/// * `low_latency_metadata` - Retained for API compatibility. Metadata updates are always
///   fixed-size and never shift the file tail.
pub fn inject_stats_into_script_data(
    file_path: &Path,
    stats: &FlvStats,
    _low_latency_metadata: bool,
) -> Result<(), ScriptModifierError> {
    debug!("Injecting stats into script data section.");

    // Find the first onMetaData script tag (not all FLVs place it immediately after the header).
    let mut reader = BufReader::new(fs::File::open(file_path)?);
    let header = flv::parser::FlvParser::parse_header(&mut reader)?;
    let first_tag_offset = u64::from(header.data_offset)
        .checked_add(crate::utils::FLV_PREVIOUS_TAG_SIZE as u64)
        .ok_or(ScriptModifierError::ScriptData(
            "FLV header data_offset overflow",
        ))?;
    reader.seek(io::SeekFrom::Start(first_tag_offset))?;

    let (start_pos, script_data, original_payload_data) = loop {
        let tag_start_pos = reader.stream_position()?;

        // Use the non-owned parser to avoid fully demuxing audio/video payloads while scanning.
        // Some upstream streams can have non-standard codec headers; we only need raw bytes until
        // we hit the script tag.
        let parsed = match flv::parser::FlvParser::parse_tag(&mut reader)? {
            Some(v) => v,
            None => {
                warn!("No onMetaData script tag found in file, skipping stats injection.");
                return Ok(());
            }
        };

        let (tag, tag_type) = parsed;

        // Skip PreviousTagSize for the tag we just parsed (4 bytes).
        let mut prev_size_buf = [0u8; 4];
        if let Err(e) = reader.read_exact(&mut prev_size_buf) {
            warn!(error = ?e, "Failed to read PreviousTagSize while scanning tags; skipping stats injection.");
            return Ok(());
        }
        if tag_type != FlvTagType::ScriptData || tag.is_filtered() {
            continue;
        }

        let mut cursor = std::io::Cursor::new(tag.data().clone());
        let data = flv::script::ScriptData::demux(&mut cursor)?;
        trace!("Script data: {:?}", data);

        if !crate::amf::is_on_metadata_script(&data) {
            continue;
        }

        let original_payload_data = tag.data().len() as u32;
        debug!("Found onMetaData at position: {tag_start_pos}");
        debug!("Original script data payload size: {original_payload_data}");

        break (tag_start_pos, data, original_payload_data);
    };

    let Some(metadata_value) = crate::amf::on_metadata_value(&script_data) else {
        return Err(ScriptModifierError::ScriptData("Script data is empty"));
    };

    // Generate new script data buffer
    if let Some(props) = metadata_value.as_object_properties() {
        // current script data model
        let script_data_model = AmfScriptData::from_amf_object_ref(props)?;

        debug!("script data model: {script_data_model:?}");

        // new script data buffer and size diff
        let mut builder = OnMetaDataBuilder::from_script_data(script_data_model).with_stats(stats);

        if let Some(video_stats) = &stats.video_stats {
            let (times, filepositions): (Vec<f64>, Vec<u64>) = video_stats
                .keyframes
                .iter()
                .map(|k| (k.timestamp_s, k.file_position))
                .unzip();
            builder = builder.with_final_keyframes(times, filepositions);
        }

        let fixed = match builder.build_fixed_size(original_payload_data as usize) {
            Ok(fixed) => fixed,
            Err(FixedSizeMetadataError::TooLarge { target, minimum }) => {
                warn!(
                    target,
                    minimum,
                    path = %file_path.display(),
                    "Metadata reservation is too small; leaving the script tag unchanged"
                );
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };

        if fixed.truncated {
            warn!(
                written = fixed.keyframes_written,
                available = stats
                    .video_stats
                    .as_ref()
                    .map_or(0, |video| video.keyframes.len()),
                path = %file_path.display(),
                "Truncated FLV keyframe index to preserve metadata layout"
            );
        }

        drop(reader); // Close the reader before opening the writer
        let mut writer =
            std::io::BufWriter::new(fs::OpenOptions::new().write(true).open(file_path)?);
        writer.seek(io::SeekFrom::Start(
            start_pos + flv::framing::TAG_HEADER_SIZE as u64,
        ))?;
        writer.write_all(&fixed.bytes)?;
        writer.flush()?;
    } else {
        return Err(ScriptModifierError::ScriptData(
            "First script tag data is not an object",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use amf0::Amf0Value;
    use flv::{FlvTagType, parser::FlvParser, script::ScriptData};
    use std::{fs::File, io::Cursor};

    use crate::analyzer::Keyframe;

    use super::*;

    #[test]
    fn inject_stats_finds_onmetadata_not_first_tag() {
        use pipeline_common::init_test_tracing;
        init_test_tracing!();
        use crate::analyzer::VideoStats;
        use crate::test_utils;
        use flv::{FlvData, FlvHeader, FlvTag, FlvWriter, tag::FlvTagType as RawTagType};
        use std::io::BufWriter;
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut path = std::env::temp_dir();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("flv_fix_script_modifier_{unique}.flv"));

        // Build a tiny FLV where the first tag after the header is NOT onMetaData.
        {
            let file = File::create(&path).unwrap();
            let mut writer = FlvWriter::new(BufWriter::new(file)).unwrap();
            writer.write_header(&FlvHeader::new(true, true)).unwrap();

            // First tag: video sequence header
            if let FlvData::Tag(tag) = test_utils::create_video_sequence_header(0, 1) {
                writer.write_tag_f(&tag).unwrap();
            } else {
                panic!("Expected video sequence header tag");
            }

            let (payload, _) = OnMetaDataBuilder::new()
                .with_placeholder_keyframes(20)
                .build_bytes(0, false)
                .unwrap();
            let tag = FlvTag::new(
                0,
                0,
                RawTagType::ScriptData,
                false,
                bytes::Bytes::from(payload),
            );
            writer.write_tag_f(&tag).unwrap();

            // Keep data after the script tag to verify that patching does not touch the file tail.
            if let FlvData::Tag(tag) = test_utils::create_video_tag(100, true) {
                writer.write_tag_f(&tag).unwrap();
            } else {
                panic!("Expected video tag");
            }

            writer.close().unwrap();
        }

        let before = std::fs::read(&path).unwrap();
        let payload_range = {
            let file = File::open(&path).unwrap();
            let mut reader = std::io::BufReader::new(file);
            reader.seek(io::SeekFrom::Start(13)).unwrap();
            loop {
                let tag_start = reader.stream_position().unwrap() as usize;
                let (tag, tag_type) = FlvParser::parse_tag(&mut reader).unwrap().unwrap();
                let mut previous_tag_size = [0u8; 4];
                reader.read_exact(&mut previous_tag_size).unwrap();
                if tag_type == RawTagType::ScriptData {
                    break tag_start + flv::framing::TAG_HEADER_SIZE
                        ..tag_start + flv::framing::TAG_HEADER_SIZE + tag.data().len();
                }
            }
        };

        let file_size = std::fs::metadata(&path).unwrap().len();
        let stats = FlvStats {
            file_size,
            duration: 0.1,
            has_video: true,
            last_timestamp: 100,
            video_stats: Some(VideoStats {
                first_video_timestamp: Some(0),
                last_video_timestamp: 100,
                keyframes: vec![Keyframe {
                    timestamp_s: 0.1,
                    file_position: 13,
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        inject_stats_into_script_data(&path, &stats, false).unwrap();

        let after = std::fs::read(&path).unwrap();
        assert_eq!(after.len(), before.len());
        for (index, (old, new)) in before.iter().zip(&after).enumerate() {
            if !payload_range.contains(&index) {
                assert_eq!(old, new, "byte outside metadata payload changed at {index}");
            }
        }

        // Re-parse file and validate that onMetaData exists and duration was updated.
        let file = File::open(&path).unwrap();
        let mut reader = std::io::BufReader::new(file);
        let _header = FlvParser::parse_header(&mut reader).unwrap();

        let mut found = None;
        FlvParser::parse_tags(
            &mut reader,
            |tag, tag_type, _position| {
                if tag_type == RawTagType::ScriptData && found.is_none() {
                    found = Some(tag.clone());
                }
            },
            9,
        )
        .unwrap();

        let script_tag = found.expect("Expected onMetaData script tag");
        let mut cursor = Cursor::new(script_tag.data().clone());
        let script = ScriptData::demux(&mut cursor).unwrap();
        assert_eq!(script.name, crate::AMF0_ON_METADATA);

        let Amf0Value::Object(props) = &script.data[0] else {
            panic!("Expected AMF object for onMetaData");
        };

        let duration = props
            .iter()
            .find(|(k, _)| k.as_ref() == "duration")
            .map(|(_, v)| v)
            .expect("Expected duration field");

        assert_eq!(*duration, Amf0Value::Number(stats.duration));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn inject_stats_does_not_rewrite_filtered_metadata() {
        use flv::{FlvHeader, FlvTag, FlvWriter};
        use std::io::BufWriter;

        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("filtered-metadata.flv");
        let (payload, _) = OnMetaDataBuilder::new()
            .with_placeholder_keyframes(20)
            .build_bytes(0, false)
            .unwrap();
        {
            let file = File::create(&path).unwrap();
            let mut writer = FlvWriter::new(BufWriter::new(file)).unwrap();
            writer.write_header(&FlvHeader::new(true, true)).unwrap();
            writer
                .write_tag_f(&FlvTag::new(
                    0,
                    0,
                    FlvTagType::ScriptData,
                    true,
                    bytes::Bytes::from(payload),
                ))
                .unwrap();
            writer.close().unwrap();
        }
        let before = std::fs::read(&path).unwrap();

        inject_stats_into_script_data(
            &path,
            &FlvStats {
                duration: 42.0,
                ..Default::default()
            },
            false,
        )
        .unwrap();

        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn inject_stats_handles_extended_header_cue_points_and_set_data_frame() {
        use std::borrow::Cow;
        use std::io::BufWriter;

        use flv::{FlvTag, FlvWriter};

        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("extended-setdataframe.flv");

        let mut cue_payload = Vec::new();
        amf0::Amf0Encoder::encode_string(&mut cue_payload, "onCuePoint").unwrap();
        amf0::Amf0Encoder::encode(
            &mut cue_payload,
            &Amf0Value::Object(Cow::Owned(vec![(
                Cow::Borrowed("name"),
                Amf0Value::String(Cow::Borrowed("chapter-1")),
            )])),
        )
        .unwrap();

        let (canonical_meta, _) = OnMetaDataBuilder::new()
            .with_placeholder_keyframes(20)
            .with_custom_property("encoder", Amf0Value::String(Cow::Borrowed("obs")))
            .build_bytes(0, false)
            .unwrap();
        let mut meta_cursor = Cursor::new(bytes::Bytes::from(canonical_meta));
        let parsed_canonical = ScriptData::demux(&mut meta_cursor).unwrap();
        let mut set_data_frame_payload = Vec::new();
        amf0::Amf0Encoder::encode_string(&mut set_data_frame_payload, "@setDataFrame").unwrap();
        amf0::Amf0Encoder::encode_string(&mut set_data_frame_payload, "onMetaData").unwrap();
        amf0::Amf0Encoder::encode(&mut set_data_frame_payload, &parsed_canonical.data[0]).unwrap();

        {
            let mut file = BufWriter::new(File::create(&path).unwrap());
            // Extended 25-byte FLV header (9-byte standard + 16-byte extension) + 4-byte PreviousTagSize0.
            let mut header_bytes = vec![b'F', b'L', b'V', 1, 0x05, 0, 0, 0, 25];
            header_bytes.extend_from_slice(&[0xAA; 16]);
            header_bytes.extend_from_slice(&0u32.to_be_bytes());
            file.write_all(&header_bytes).unwrap();

            let mut writer = FlvWriter::new(file).unwrap();
            writer
                .write_tag_f(&FlvTag::new(
                    0,
                    0,
                    FlvTagType::ScriptData,
                    false,
                    bytes::Bytes::from(cue_payload.clone()),
                ))
                .unwrap();
            writer
                .write_tag_f(&FlvTag::new(
                    0,
                    0,
                    FlvTagType::ScriptData,
                    false,
                    bytes::Bytes::from(set_data_frame_payload),
                ))
                .unwrap();
            writer.close().unwrap();
        }

        let before_len = std::fs::metadata(&path).unwrap().len();
        inject_stats_into_script_data(
            &path,
            &FlvStats {
                file_size: before_len,
                duration: 15.5,
                has_video: true,
                ..Default::default()
            },
            false,
        )
        .unwrap();

        assert_eq!(std::fs::metadata(&path).unwrap().len(), before_len);

        let mut reader = BufReader::new(File::open(&path).unwrap());
        let header = FlvParser::parse_header(&mut reader).unwrap();
        assert_eq!(header.data_offset, 25);
        reader.seek(io::SeekFrom::Start(29)).unwrap();

        let (cue_tag, _) = FlvParser::parse_tag(&mut reader).unwrap().unwrap();
        let mut prev = [0u8; 4];
        reader.read_exact(&mut prev).unwrap();
        assert_eq!(cue_tag.data().as_ref(), cue_payload.as_slice());

        let (meta_tag, _) = FlvParser::parse_tag(&mut reader).unwrap().unwrap();
        let mut cursor = Cursor::new(meta_tag.data().clone());
        let script = ScriptData::demux(&mut cursor).unwrap();
        let props = crate::amf::on_metadata_value(&script)
            .unwrap()
            .as_object_properties()
            .unwrap();
        assert_eq!(
            props
                .iter()
                .find(|(k, _)| k.as_ref() == "duration")
                .map(|(_, v)| v),
            Some(&Amf0Value::Number(15.5))
        );
        assert_eq!(
            props
                .iter()
                .find(|(k, _)| k.as_ref() == "encoder")
                .map(|(_, v)| v),
            Some(&Amf0Value::String(Cow::Borrowed("obs")))
        );
    }
}
