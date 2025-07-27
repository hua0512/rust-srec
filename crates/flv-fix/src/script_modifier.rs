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
    io::{self, BufReader, BufWriter, Seek, Write},
    path::{Path, PathBuf},
};

use amf0::{Amf0Encoder, Amf0Marker, Amf0Value, write_amf_property_key};
use byteorder::{BigEndian, WriteBytesExt};
use flv::tag::{FlvTagData, FlvTagType::ScriptData};
use time::OffsetDateTime;
use tracing::{debug, info, trace, warn};

use crate::{
    analyzer::FlvStats,
    operators::NATURAL_METADATA_KEY_ORDER,
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
pub fn inject_stats_into_script_data(
    file_path: &Path,
    stats: &FlvStats,
) -> Result<(), ScriptModifierError> {
    let file_path_clone = file_path.to_path_buf();
    update_script_metadata(&file_path_clone, stats)
}

/// Write keyframes section with adjusted file positions
/// Precompute the size of the keyframes section without actually writing it
fn compute_keyframes_section_size(keyframes: &[crate::analyzer::Keyframe]) -> usize {
    let mut size = 0;

    // "keyframes" property key + object marker
    size += "keyframes".len() + 2 + 1; // string length (2 bytes) + string + object marker

    let keyframes_length = keyframes.len() as u32;

    // "times" property key + strict array marker + array length
    size += "times".len() + 2 + 1 + 4; // string length + string + array marker + length

    // Each time value is encoded as a number (8 bytes + 1 byte marker)
    size += keyframes_length as usize * 9;

    // "filepositions" property key + strict array marker + array length
    size += "filepositions".len() + 2 + 1 + 4; // string length + string + array marker + length

    // Each position value is encoded as u64 (8 bytes + 1 byte marker)
    size += keyframes_length as usize * 9;

    // Object EOF marker (3 bytes)
    size += 3;

    size
}

/// Write keyframes section with adjusted file positions
fn write_keyframes_section(
    buffer: &mut Vec<u8>,
    keyframes: &[crate::analyzer::Keyframe],
    position_adjustment: i64,
) -> Result<(), ScriptModifierError> {
    write_amf_property_key!(buffer, "keyframes");
    buffer.write_u8(Amf0Marker::Object as u8)?;

    let keyframes_length = keyframes.len() as u32;
    debug!("Injecting {} keyframes", keyframes_length);

    // Write times array
    write_amf_property_key!(buffer, "times");
    buffer.write_u8(Amf0Marker::StrictArray as u8)?;
    buffer.write_u32::<BigEndian>(keyframes_length)?;

    for keyframe in keyframes.iter() {
        let keyframe_time = keyframe.timestamp_s;
        trace!("Injecting keyframe at time {}", keyframe_time);
        amf0::Amf0Encoder::encode_number(buffer, keyframe_time as f64)?;
    }

    // Write filepositions array with adjusted positions
    write_amf_property_key!(buffer, "filepositions");
    buffer.push(Amf0Marker::StrictArray as u8);
    buffer.write_u32::<BigEndian>(keyframes_length)?;

    for keyframe in keyframes.iter() {
        // Adjust keyframe position based on script data size change
        let adjusted_keyframe_pos = if position_adjustment != 0 {
            let adjusted_pos = keyframe.file_position as i64 + position_adjustment;
            if adjusted_pos < 0 {
                warn!(
                    "Keyframe position adjustment resulted in negative position: {} + {} = {}",
                    keyframe.file_position, position_adjustment, adjusted_pos
                );
                keyframe.file_position // Keep original if adjustment is invalid
            } else {
                adjusted_pos as u64
            }
        } else {
            keyframe.file_position
        };

        trace!(
            "Injecting keyframe at position {} (adjusted from {} with diff {})",
            adjusted_keyframe_pos, keyframe.file_position, position_adjustment
        );
        amf0::Amf0Encoder::encode_u64(buffer, adjusted_keyframe_pos)?;
    }

    // close keyframes object
    amf0::Amf0Encoder::object_eof(buffer)?;

    Ok(())
}

/// Implementation function that actually does the metadata update work
/// This is not async as it performs blocking I/O operations
fn update_script_metadata(
    file_path: &PathBuf,
    stats: &FlvStats,
) -> Result<(), ScriptModifierError> {
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
        let mut buffer: Vec<u8> = Vec::with_capacity(original_payload_data as usize);
        Amf0Encoder::encode_string(&mut buffer, crate::AMF0_ON_METADATA)?;

        for key in NATURAL_METADATA_KEY_ORDER.iter() {
            match *key {
                "duration" => {
                    let duration = stats.duration;
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_number(&mut buffer, duration as f64)?;
                }
                "width" => {
                    write_amf_property_key!(&mut buffer, key);
                    if let Some(resolution) = stats.resolution {
                        Amf0Encoder::encode_number(&mut buffer, resolution.width as f64)?;
                    } else {
                        let original_value =
                            props.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
                        Amf0Encoder::encode(
                            &mut buffer,
                            &original_value.unwrap_or(Amf0Value::Number(0.0)),
                        )?;
                    }
                }
                "height" => {
                    write_amf_property_key!(&mut buffer, key);
                    if let Some(resolution) = stats.resolution {
                        Amf0Encoder::encode_number(&mut buffer, resolution.height as f64)?;
                    } else {
                        let original_value =
                            props.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
                        Amf0Encoder::encode(
                            &mut buffer,
                            &original_value.unwrap_or(Amf0Value::Number(0.0)),
                        )?;
                    }
                }
                "framerate" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_number(&mut buffer, stats.video_frame_rate as f64)?;
                }
                "videocodecid" => {
                    write_amf_property_key!(&mut buffer, key);

                    if let Some(codec_id) = stats.video_codec {
                        Amf0Encoder::encode_number(&mut buffer, codec_id as u8 as f64)?;
                    } else {
                        let original_value =
                            props.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
                        Amf0Encoder::encode(
                            &mut buffer,
                            &original_value.unwrap_or(Amf0Value::Number(0.0)),
                        )?;
                    }
                }
                "audiocodecid" => {
                    write_amf_property_key!(&mut buffer, key);
                    if let Some(codec_id) = stats.audio_codec {
                        Amf0Encoder::encode_number(&mut buffer, codec_id as u8 as f64)?;
                    } else {
                        let original_value =
                            props.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
                        Amf0Encoder::encode(
                            &mut buffer,
                            &original_value.unwrap_or(Amf0Value::Number(0.0)),
                        )?;
                    }
                }
                "hasAudio" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_bool(&mut buffer, stats.has_audio)?;
                }
                "hasVideo" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_bool(&mut buffer, stats.has_video)?;
                }
                "hasMetadata" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_bool(&mut buffer, true)?;
                }
                "hasKeyframes" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_bool(&mut buffer, !stats.keyframes.is_empty())?;
                }
                "canSeekToEnd" => {
                    write_amf_property_key!(&mut buffer, key);

                    Amf0Encoder::encode_bool(
                        &mut buffer,
                        stats.last_keyframe_timestamp == stats.last_timestamp,
                    )?;
                }
                "datasize" => {
                    write_amf_property_key!(&mut buffer, key);
                    let data_size = stats.audio_data_size + stats.video_data_size;
                    Amf0Encoder::encode_u64(&mut buffer, data_size)?;
                }
                "filesize" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_u64(&mut buffer, stats.file_size)?;
                }
                "audiosize" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_u64(&mut buffer, stats.audio_data_size)?;
                }
                "audiodatarate" => {
                    write_amf_property_key!(&mut buffer, key);
                    let audio_bitrate = stats.audio_sample_rate as f64;
                    Amf0Encoder::encode_number(&mut buffer, audio_bitrate)?;
                }
                "audiosamplerate" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_number(&mut buffer, stats.audio_sample_rate as f64)?;
                }
                "audiosamplesize" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_number(&mut buffer, stats.audio_sample_size as f64)?;
                }
                "stereo" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_bool(&mut buffer, stats.audio_stereo)?;
                }
                "videosize" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_u64(&mut buffer, stats.video_data_size)?;
                }
                "videodatarate" => {
                    write_amf_property_key!(&mut buffer, key);
                    let video_bitrate = stats.video_data_rate as f64;
                    Amf0Encoder::encode_number(&mut buffer, video_bitrate)?;
                }
                "lasttimestamp" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_u32(&mut buffer, stats.last_timestamp)?;
                }
                "lastkeyframelocation" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_u64(&mut buffer, stats.last_keyframe_position)?;
                }
                "lastkeyframetimestamp" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_u32(&mut buffer, stats.last_keyframe_timestamp)?;
                }
                "metadatacreator" => {
                    write_amf_property_key!(&mut buffer, key);
                    Amf0Encoder::encode_string(&mut buffer, "Srec")?;
                }
                "metadatadate" => {
                    write_amf_property_key!(&mut buffer, key);
                    let value = OffsetDateTime::now_local()
                        .unwrap_or_else(|_| OffsetDateTime::now_utc())
                        .format(&time::format_description::well_known::Rfc3339)
                        .map_err(|_| ScriptModifierError::ScriptData("Failed to format date"))?;

                    Amf0Encoder::encode_string(&mut buffer, &value)?;
                }
                _ => {}
            }
        }

        let mut count = 0;
        for (key, value) in props.iter() {
            if !NATURAL_METADATA_KEY_ORDER.contains(&key.as_ref()) {
                debug!("Adding custom property: {} with value: {:?}", key, value);
                write_amf_property_key!(buffer, key);
                amf0::Amf0Encoder::encode(&mut buffer, value)?;
                count += 1;
            }
        }
        debug!("Injected {} custom metadata keys", count);

        // Calculate size difference for the entire script data payload (metadata + keyframes)
        // This is how much all subsequent content in the file will shift
        let metadata_size_without_keyframes = buffer.len() + 3; // +3 for object_eof
        let keyframes_size = compute_keyframes_section_size(&stats.keyframes);
        let estimated_new_payload_size = metadata_size_without_keyframes + keyframes_size;
        let metadata_size_diff = estimated_new_payload_size as i64 - original_payload_data as i64;

        debug!(
            "Script data size difference: {} (original: {}, metadata only: {}, keyframes: {}, total new: {})",
            metadata_size_diff,
            original_payload_data,
            metadata_size_without_keyframes,
            keyframes_size,
            estimated_new_payload_size
        );

        // Add keyframes with positions adjusted by metadata size difference
        write_keyframes_section(&mut buffer, &stats.keyframes, metadata_size_diff)?;

        // close script data object
        amf0::Amf0Encoder::object_eof(&mut buffer)?;

        buffer.flush()?;

        let new_payload_size = buffer.len();
        debug!("New script data size: {}", new_payload_size);

        drop(reader); // Close the reader before opening the writer

        if new_payload_size == original_payload_data as usize {
            // Same size case - simple overwrite
            debug!("Script data size is same as original size, writing directly.");
            let mut writer = BufWriter::new(fs::OpenOptions::new().write(true).open(file_path)?);
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

            // Calculate size difference
            let size_diff = new_payload_size as i64 - original_payload_data as i64;

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
