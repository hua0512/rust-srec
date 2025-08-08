//! # Script Data Modifier Module
//!
//! This module provides functionality for modifying FLV script data (metadata) sections
//! based on collected statistics and analysis.
//!
//! ## Key Features:
//!
//! - Updates metadata in FLV files with accurate statistics
//! - Handles both direct replacement and file rewriting when metadata size changes
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
    io::{self, BufReader, Seek, Write},
    path::Path,
};

use amf0::Amf0Value;
use flv::tag::{FlvTagData, FlvTagType::ScriptData};
use tracing::{debug, info, warn};

use crate::{
    amf::{builder::OnMetaDataBuilder, model::AmfScriptData},
    analyzer::FlvStats,
    utils::{self, shift_content_backward, shift_content_forward, write_flv_tag},
};

/// Error type for script modification operations
#[derive(Debug, thiserror::Error)]
pub enum ScriptModifierError {
    #[error("IO Error: {0}")]
    Io(#[from] io::Error),
    #[error("FLV Error: {0}")]
    Flv(#[from] flv::error::FlvError),
    #[error("AMF0 Read Error: {0}")]
    Amf0Read(#[from] amf0::Amf0ReadError),
    #[error("AMF0 Write Error: {0}")]
    Amf0Write(#[from] amf0::Amf0WriteError),
    #[error("Script data error: {0}")]
    ScriptData(&'static str),
}

/// Main function to update FLV file metadata based on collected statistics
/// This is an async wrapper around the actual implementation
#[inline]
pub fn inject_stats_into_script_data(
    file_path: &Path,
    stats: &FlvStats,
) -> Result<(), ScriptModifierError> {
    update_script_metadata(file_path, stats)
}

/// Implementation function that actually does the metadata update work
/// This is not async as it performs blocking I/O operations
fn update_script_metadata(file_path: &Path, stats: &FlvStats) -> Result<(), ScriptModifierError> {
    debug!("Injecting stats into script data section.");

    // Create a backup of the file
    // create_backup(file_path)?;

    // Parse the script data section and inject stats
    let mut reader = BufReader::new(fs::File::open(file_path)?);

    // Seek to the script data section (9 bytes header + 4 bytes PreviousTagSize)
    reader.seek(io::SeekFrom::Start(13))?;

    let start_pos = reader.stream_position()?;

    debug!(
        "Seeking to script data section. Start position: {}",
        start_pos
    );

    // Read the script data tag
    let script_tag = match flv::parser::FlvParser::parse_tag(&mut reader)? {
        Some((tag, _)) => tag,
        None => {
            warn!("No script tag found in file, skipping stats injection.");
            return Ok(());
        }
    };

    let script_data = match script_tag.data {
        FlvTagData::ScriptData(data) => data,
        FlvTagData::Audio(_) => {
            return Err(ScriptModifierError::ScriptData(
                "Expected script data tag but found audio data tag instead",
            ));
        }
        FlvTagData::Video(_) => {
            return Err(ScriptModifierError::ScriptData(
                "Expected script data tag but found video data tag instead",
            ));
        }
        FlvTagData::Unknown {
            tag_type: _,
            data: _,
        } => {
            return Err(ScriptModifierError::ScriptData(
                "Expected script data tag but found unknown tag type instead",
            ));
        }
    };

    debug!("Script data: {:?}", script_data);

    // Get the size of the payload of the script data tag
    // After reading the tag entirely, we are at the end of the payload
    // The script data size is the size of the tag minus the header (11 bytes)
    let end_script_pos = reader.stream_position()?;

    let original_payload_data = (end_script_pos - start_pos - 11) as u32;
    debug!(
        "Original script data payload size: {}",
        original_payload_data
    );
    debug!("End of original script data position: {}", end_script_pos);

    if script_data.name != crate::AMF0_ON_METADATA {
        return Err(ScriptModifierError::ScriptData(
            "First script tag is not onMetaData",
        ));
    }

    let amf_data = script_data.data;
    if amf_data.is_empty() {
        return Err(ScriptModifierError::ScriptData("Script data is empty"));
    }

    // Generate new script data buffer
    if let Amf0Value::Object(props) = &amf_data[0] {
        // current script data model
        let script_data_model = AmfScriptData::from_amf_object_ref(props)?;

        let (times, filepositions): (Vec<f64>, Vec<u64>) = stats
            .keyframes
            .iter()
            .map(|k| (k.timestamp_s as f64, k.file_position))
            .unzip();

        // new script data buffer and size diff
        let (buffer, size_diff) = OnMetaDataBuilder::from_script_data(script_data_model)
            .with_stats(stats)
            .with_final_keyframes(times, filepositions)
            .build_bytes(original_payload_data)?;

        let new_payload_size = buffer.len();
        debug!("New script data size: {}", new_payload_size);

        drop(reader); // Close the reader before opening the writer

        if size_diff == 0 {
            // Same size case - simple overwrite
            debug!("Script data size is same as original size, writing directly.");
            let mut writer =
                std::io::BufWriter::new(fs::OpenOptions::new().write(true).open(file_path)?);
            // Skip the header + 4 bytes for PreviousTagSize + 11 bytes for tag header
            writer.seek(io::SeekFrom::Start(start_pos + 11))?;
            writer.write_all(&buffer)?;
            writer.flush()?;
        } else {
            debug!(
                "Script data size changed (original: {}, new: {}).",
                original_payload_data, new_payload_size
            );

            // This position is where the next tag starts after the script data tag
            let next_tag_pos = end_script_pos + 4; // +4 for PreviousTagSize

            // Get the file size
            let total_file_size = fs::metadata(file_path)?.len();

            // Open the file for both reading and writing
            let mut file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(file_path)?;

            if size_diff > 0 {
                // New data is larger - need to shift content forward
                shift_content_forward(&mut file, next_tag_pos, total_file_size, size_diff)?;

                // Write the new script tag header and data
                write_flv_tag(&mut file, start_pos, ScriptData, &buffer, 0)?;

                info!(
                    "Successfully rewrote file with expanded script data. New file size: {}",
                    total_file_size + size_diff as u64
                );
            } else {
                // New data is smaller - need to shift content backward

                // Write the new script tag header and data
                write_flv_tag(&mut file, start_pos, ScriptData, &buffer, 0)?;

                // Calculate new position for the next tag
                let new_next_tag_pos = start_pos
                    + utils::FLV_TAG_HEADER_SIZE as u64
                    + new_payload_size as u64
                    + utils::FLV_PREVIOUS_TAG_SIZE as u64;

                // Now shift all remaining content backward
                shift_content_backward(&mut file, next_tag_pos, new_next_tag_pos, total_file_size)?;

                // Truncate the file to the new size
                let new_file_size = total_file_size - (-size_diff) as u64;
                file.set_len(new_file_size)?;

                info!(
                    "Successfully rewrote file with reduced script data. New file size: {}",
                    new_file_size
                );
            }
        }
    } else {
        return Err(ScriptModifierError::ScriptData(
            "First script tag data is not an object",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use flv::{FlvTagType, FlvUtil, parser::FlvParserRef, script::ScriptData};
    use std::{fs::File, io::Cursor};
    use tracing::trace;
    use tracing_subscriber::fmt;

    use crate::{FlvAnalyzer, analyzer::Keyframe, operators::MIN_INTERVAL_BETWEEN_KEYFRAMES_MS};

    use super::*;

    #[tokio::test]
    #[ignore]
    async fn validate_keyframes_extraction() {
        let log_file = File::create("test_run.log").expect("Failed to create log file.");
        let subscriber = fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(log_file)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");

        // Source and destination paths
        let input_path = Path::new(
            "D:/Develop/hua0512/stream-rec/rust-srec/fix/19_40_40-WE前职业第八赛季国服第一芮娜王赋能局游龙_p000.flv",
        );

        // Skip if test file doesn't exist
        if !input_path.exists() {
            info!(path = %input_path.display(), "Test file not found, skipping test");
            return;
        }

        let mut analyzer = FlvAnalyzer::default();
        let mut keyframes = Vec::new();
        let mut last_keyframe_timestamp = 0;

        // First, analyze the header
        let file = std::fs::File::open(input_path).unwrap();
        let mut reader = std::io::BufReader::new(file);
        let header = FlvParserRef::parse_header(&mut reader).unwrap();
        analyzer.analyze_header(&header).unwrap();

        // The position after the header
        let current_position = 9u64;

        // Parse tags using the same reader
        FlvParserRef::parse_tags(
            &mut reader,
            |tag, tag_type, position| {
                analyzer.analyze_tag(tag).unwrap();

                if tag.is_script_tag() {
                    let mut script_data = Cursor::new(tag.data.clone());
                    let data = ScriptData::demux(&mut script_data).unwrap();
                    println!("Script data: {data:?}");
                }

                if tag.is_key_frame() && tag_type == FlvTagType::Video {
                    let timestamp = tag.timestamp_ms;
                    let add_keyframe = last_keyframe_timestamp == 0
                        || (timestamp.saturating_sub(last_keyframe_timestamp)
                            >= MIN_INTERVAL_BETWEEN_KEYFRAMES_MS);

                    trace!(
                        "Test: Checking keyframe. Current timestamp: {}, Last keyframe timestamp: {}, Condition: {}",
                        tag.timestamp_ms,
                        last_keyframe_timestamp,
                        tag.timestamp_ms.saturating_sub(last_keyframe_timestamp) >= MIN_INTERVAL_BETWEEN_KEYFRAMES_MS
                    );
                    if add_keyframe {
                        let keyframe = Keyframe {
                            timestamp_s: timestamp as f32 / 1000.0,
                            file_position: position,
                        };
                        keyframes.push(keyframe);
                        trace!("Test: Adding keyframe. New count: {}", keyframes.len());
                        last_keyframe_timestamp = timestamp;
                    }
                }
            },
            current_position,
        )
        .unwrap();

        // Build the stats to get FlvStats
        let stats = analyzer.build_stats().unwrap();
        let analyzed_keyframes = stats.keyframes.clone();

        assert_eq!(
            analyzed_keyframes.len(),
            keyframes.len(),
            "Mismatch in the number of keyframes"
        );

        for (analyzed, manual) in analyzed_keyframes.iter().zip(keyframes.iter()) {
            assert_eq!(
                manual.timestamp_s, analyzed.timestamp_s,
                "Timestamp mismatch"
            );
            assert_eq!(
                manual.file_position, analyzed.file_position,
                "File position mismatch"
            );
        }
    }
}
