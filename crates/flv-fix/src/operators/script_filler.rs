//! # ScriptKeyframesFillerOperator
//!
//! The `ScriptKeyframesFillerOperator` prepares FLV streams for improved seeking by injecting
//! placeholder keyframe metadata structures that can be populated later in the stream processing pipeline.
//!
//!
//! ## Purpose
//!
//! Many FLV streams lack proper keyframe metadata, which can result in:
//!
//! 1. Slow or inaccurate seeking in players
//! 2. Inability to use advanced player features
//! 3. Compatibility issues with certain players that expect standard metadata
//! 4. Poor streaming performance when keyframe information is needed for adapting quality
//!
//! This operator ensures the first script tag in the FLV stream contains properly
//! structured metadata including:
//!
//! - Standard metadata properties like duration, dimensions, codec info
//! - Empty keyframe index structure with pre-allocated space
//! - Player compatibility flags
//!
//! ## Operation
//!
//! The operator:
//! - Processes only the first script tag in the stream
//! - Preserves any existing metadata values when possible
//! - Adds default values for missing but important metadata
//! - Constructs empty keyframe arrays that can be filled by later operators
//! - Ensures proper ordering of metadata properties for maximum compatibility
//!
//! ## Configuration
//!
//! The operator supports configuration for:
//! - Target keyframe interval in milliseconds
//! - (Default to 3.5 hours for long recording sessions)
//!
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!

use amf0::{Amf0Marker, Amf0Value, write_amf_property_key};
use byteorder::{BigEndian, WriteBytesExt};
use bytes::Bytes;
use flv::data::FlvData;
use flv::script::ScriptData;
use flv::tag::{FlvTag, FlvTagType};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::borrow::Cow;
use std::io::{self, Write};
use std::sync::Arc;
use tracing::{debug, info, trace, warn};

const DEFAULT_KEYFRAME_INTERVAL_MS: u32 = (3.5 * 60.0 * 60.0 * 1000.0) as u32; // 3.5 hours in ms
pub const MIN_INTERVAL_BETWEEN_KEYFRAMES_MS: u32 = 1900; // 1.9 seconds in ms

pub static NATURAL_METADATA_KEY_ORDER: &[&str] = &[
    "duration",
    "width",
    "height",
    "framerate",
    "videocodecid",
    "audiocodecid",
    "hasAudio",
    "hasVideo",
    "hasMetadata",
    "hasKeyframes",
    "canSeekToEnd",
    "datasize",
    "filesize",
    "audiosize",
    "audiodatarate",
    "audiosamplerate",
    "audiosamplesize",
    "stereo",
    "videosize",
    "videodatarate",
    "lasttimestamp",
    "lastkeyframelocation",
    "lastkeyframetimestamp",
    "metadatacreator",
    "metadatadate",
    "keyframes",
];

/// Estimated size of the total metadata object
/// ~474 bytes for standard metadata properties
const TOTAL_NATURAL_METADATA_SIZE: usize = 500;

/// Configuration for the ScriptInjectorOperator
#[derive(Clone, Debug)]
pub struct ScriptFillerConfig {
    /// The target maximum duration of keyframes in milliseconds.
    /// Defaults to 3.5 hours.
    pub keyframe_duration_ms: u32,
}

impl Default for ScriptFillerConfig {
    fn default() -> Self {
        Self {
            keyframe_duration_ms: DEFAULT_KEYFRAME_INTERVAL_MS,
        }
    }
}

/// Operator to modify the first script tag with keyframe information.
pub struct ScriptKeyframesFillerOperator {
    context: Arc<StreamerContext>,
    config: ScriptFillerConfig,
    seen_first_script_tag: bool,
}

impl ScriptKeyframesFillerOperator {
    /// Creates a new ScriptInjectorOperator with the given configuration.
    pub fn new(context: Arc<StreamerContext>, config: ScriptFillerConfig) -> Self {
        Self {
            context,
            config,
            seen_first_script_tag: false,
        }
    }

    fn create_script_tag_payload() -> Bytes {
        // Create a new script tag with empty data
        let mut buffer = Vec::new();
        amf0::Amf0Encoder::encode_string(&mut buffer, crate::AMF0_ON_METADATA).unwrap();
        amf0::Amf0Encoder::encode(
            &mut buffer,
            &Amf0Value::Object(Cow::Owned(vec![
                // add minimal duration property
                (Cow::Borrowed("duration"), Amf0Value::Number(0.0)),
            ])),
        )
        .unwrap();
        Bytes::from(buffer)
    }

    /// Creates a fallback tag with the same metadata as the original but with default script payload
    fn create_fallback_tag(&self, original_tag: &FlvTag) -> FlvTag {
        FlvTag {
            timestamp_ms: original_tag.timestamp_ms,
            stream_id: original_tag.stream_id,
            tag_type: original_tag.tag_type,
            data: Self::create_script_tag_payload(),
        }
    }

    /// Processes the AMF data for a valid onMetaData object
    fn process_onmeta_object(
        &self,
        props: &[(Cow<'_, str>, Amf0Value)],
        tag: &FlvTag,
        amf_data_name: &str,
    ) -> io::Result<FlvTag> {
        debug!(
            "{} Found onMetaData with {} properties",
            self.context.name,
            props.len()
        );

        // Calculate buffer sizes
        let keyframes_count = self
            .config
            .keyframe_duration_ms
            .div_ceil(MIN_INTERVAL_BETWEEN_KEYFRAMES_MS);
        debug!(
            "{} Keyframes count: {}, keyframe duration: {}ms",
            self.context.name, keyframes_count, self.config.keyframe_duration_ms
        );
        let double_array_size = 2 * keyframes_count as usize;

        // Pre-allocate with a reasonable size estimate
        // Base size + metadata + keyframes structure
        let estimated_size = tag.data.len()
            + TOTAL_NATURAL_METADATA_SIZE
            + (1 + (3 * 25) + (3 * 5) + (9 * double_array_size) + 3);

        debug!("Estimated script data size: {}", estimated_size);
        let mut buffer = Vec::with_capacity(estimated_size);

        // Write metadata name (e.g. "onMetaData")
        amf0::Amf0Encoder::encode_string(&mut buffer, amf_data_name).unwrap();

        // Start object
        buffer.write_u8(Amf0Marker::Object as u8)?;

        // Get all keys except "keyframes" to process first
        let ordered_keys: Vec<&str> =
            NATURAL_METADATA_KEY_ORDER[..NATURAL_METADATA_KEY_ORDER.len() - 1].to_vec();

        // Track if we've added standard flags
        let flags_added = self.write_metadata_properties(&mut buffer, props, &ordered_keys)?;

        // Add non-standard properties
        self.write_custom_properties(&mut buffer, props)?;

        // Add compatibility flags if missing
        self.write_compatibility_flags(&mut buffer, flags_added)?;

        // Add keyframes structure
        self.write_keyframes_section(&mut buffer, double_array_size)?;

        // End of object
        amf0::Amf0Encoder::object_eof(&mut buffer).unwrap();

        buffer.flush()?;

        debug!("New script data payload size: {}", buffer.len());

        // Create a new tag with the modified data
        Ok(FlvTag {
            timestamp_ms: tag.timestamp_ms,
            stream_id: tag.stream_id,
            tag_type: tag.tag_type,
            data: Bytes::from(buffer),
        })
    }

    /// Write standard metadata properties in the correct order
    fn write_metadata_properties(
        &self,
        buffer: &mut Vec<u8>,
        props: &[(Cow<'_, str>, Amf0Value)],
        ordered_keys: &[&str],
    ) -> io::Result<(bool, bool)> {
        let mut has_keyframes_added = false;
        let mut can_seek_to_end_added = false;

        for key in ordered_keys {
            // Check for special flags
            if *key == "hasKeyframes" {
                has_keyframes_added = true;
            } else if *key == "canSeekToEnd" {
                can_seek_to_end_added = true;
            } else if *key == "keyframes" {
                // Skip keyframes as we'll handle it specially
                continue;
            }

            // Find property in the original props
            let value_opt = props
                .iter()
                .find(|(k, _)| k.as_ref() == *key)
                .map(|(_, v)| v);

            if let Some(value) = value_opt {
                // Write existing property
                write_amf_property_key!(buffer, key);
                amf0::Amf0Encoder::encode(buffer, value).unwrap();
                trace!("Encoded property: {}, {:?}", key, value);
            } else if let Some(default_value) = self.get_default_metadata_value(key) {
                // Add default value for missing property
                trace!(
                    "{} Adding default value for missing property: {}",
                    self.context.name, key
                );
                write_amf_property_key!(buffer, key);
                amf0::Amf0Encoder::encode(buffer, &default_value).unwrap();
            }
        }

        Ok((has_keyframes_added, can_seek_to_end_added))
    }

    /// Write custom properties not in the standard ordered list
    fn write_custom_properties(
        &self,
        buffer: &mut Vec<u8>,
        props: &[(Cow<'_, str>, Amf0Value)],
    ) -> io::Result<()> {
        let mut count = 0;
        // Add any remaining custom properties from the original object
        for (key, value) in props.iter() {
            if !NATURAL_METADATA_KEY_ORDER.contains(&key.as_ref()) {
                debug!(
                    "{} Adding custom property: {} with value: {:?}",
                    self.context.name, key, value
                );
                write_amf_property_key!(buffer, key);
                amf0::Amf0Encoder::encode(buffer, value).unwrap();
                count += 1;
            }
        }
        debug!(
            "{} Added {} custom properties to the script tag",
            self.context.name, count
        );
        Ok(())
    }

    /// Write standard compatibility flags if they weren't already present
    fn write_compatibility_flags(
        &self,
        buffer: &mut Vec<u8>,
        (has_keyframes_added, can_seek_to_end_added): (bool, bool),
    ) -> io::Result<()> {
        // Add standard player compatibility flags if not present
        if !has_keyframes_added {
            write_amf_property_key!(buffer, "hasKeyframes");
            amf0::Amf0Encoder::encode(buffer, &Amf0Value::Boolean(true)).unwrap();
        }

        if !can_seek_to_end_added {
            write_amf_property_key!(buffer, "canSeekToEnd");
            amf0::Amf0Encoder::encode(buffer, &Amf0Value::Boolean(true)).unwrap();
        }

        Ok(())
    }

    /// Write the keyframes section with times, filepositions and spacer arrays
    fn write_keyframes_section(
        &self,
        buffer: &mut Vec<u8>,
        double_array_size: usize,
    ) -> io::Result<()> {
        // Add keyframes object directly
        write_amf_property_key!(buffer, "keyframes");
        buffer.write_u8(Amf0Marker::Object as u8)?;

        // Times array - empty array
        write_amf_property_key!(buffer, "times");
        buffer.write_u8(Amf0Marker::StrictArray as u8)?;
        buffer.write_u32::<BigEndian>(0)?;

        // File positions array - empty array
        write_amf_property_key!(buffer, "filepositions");
        buffer.push(Amf0Marker::StrictArray as u8);
        buffer.write_u32::<BigEndian>(0)?;

        // Spacer array with pre-allocated NaN values
        write_amf_property_key!(buffer, "spacer");
        buffer.write_u8(Amf0Marker::StrictArray as u8)?;
        buffer.write_u32::<BigEndian>(double_array_size as u32)?;

        for _ in 0..double_array_size {
            amf0::Amf0Encoder::encode_number(buffer, f64::NAN).unwrap();
        }

        // End keyframes object
        amf0::Amf0Encoder::object_eof(buffer).unwrap();

        Ok(())
    }

    /// Modifies the parsed AMF values to include the keyframes property.
    fn add_keyframes_to_amf(&self, tag: FlvTag) -> io::Result<FlvTag> {
        // Parse the AMF data using a reference to the tag data
        let mut cursor = std::io::Cursor::new(tag.data.clone());

        // Try to parse the AMF data
        match ScriptData::demux(&mut cursor) {
            Ok(amf_data) => {
                debug!(
                    "{} Script tag name: '{}', data length: {}, timestamp: {}ms",
                    self.context.name,
                    amf_data.name,
                    tag.data.len(),
                    tag.timestamp_ms
                );

                // Verify we have "onMetaData" with non-empty data array
                if amf_data.name != crate::AMF0_ON_METADATA {
                    warn!(
                        "{} Script tag name is not 'onMetaData', found: '{}'. Creating fallback.",
                        self.context.name, amf_data.name
                    );
                    return self.add_keyframes_to_amf(self.create_fallback_tag(&tag));
                }

                if amf_data.data.is_empty() {
                    warn!(
                        "{} onMetaData script tag has empty data array. Creating fallback.",
                        self.context.name
                    );
                    return self.add_keyframes_to_amf(self.create_fallback_tag(&tag));
                }

                // Check if first data item is an Object
                if let Amf0Value::Object(props) = &amf_data.data[0] {
                    self.process_onmeta_object(props, &tag, &amf_data.name)
                } else {
                    warn!(
                        "{} Unsupported AMF data type for keyframe injection: {:?}. Expected Object but found different type.",
                        self.context.name, amf_data.data[0]
                    );
                    self.add_keyframes_to_amf(self.create_fallback_tag(&tag))
                }
            }
            Err(err) => {
                // Log parsing error details
                warn!(
                    "{} Failed to parse AMF data for keyframe injection: {}. \
                    Tag data length: {}, timestamp: {}ms, first few bytes: {:?}",
                    self.context.name,
                    err,
                    tag.data.len(),
                    tag.timestamp_ms,
                    tag.data.iter().take(16).collect::<Vec<_>>()
                );

                // Use fallback
                self.add_keyframes_to_amf(self.create_fallback_tag(&tag))
            }
        }
    }

    /// Get default metadata value for a given key.
    fn get_default_metadata_value(&self, key: &str) -> Option<Amf0Value> {
        match key {
            // Booleans - Defaulting to true might be optimistic
            "hasAudio" | "hasVideo" | "hasMetadata" | "stereo" => Some(Amf0Value::Boolean(true)),
            // Numeric - Defaulting to 0 might be safer than assuming values
            "duration"
            | "width"
            | "height"
            | "datasize"
            | "videosize"
            | "audiosize"
            | "lasttimestamp"
            | "lastkeyframelocation"
            | "lastkeyframetimestamp"
            | "filesize" => Some(Amf0Value::Number(0.0)),
            // Codecs - Common defaults
            "videocodecid" => Some(Amf0Value::Number(7.0)), // H264 (AVC)
            "audiocodecid" => Some(Amf0Value::Number(10.0)), // AAC
            // Audio params - Common defaults
            "audiosamplerate" => Some(Amf0Value::Number(44100.0)),
            "audiosamplesize" => Some(Amf0Value::Number(16.0)),
            // Data rates - Arbitrary defaults, might need adjustment
            "audiodatarate" => Some(Amf0Value::Number(128.0)), // kbps
            "videodatarate" => Some(Amf0Value::Number(1000.0)), // kbps
            "framerate" => Some(Amf0Value::Number(30.0)),
            // Strings
            "metadatacreator" => Some(Amf0Value::String(Cow::Borrowed("Srec"))),
            "metadatadate" => {
                // Generate current date dynamically
                Some(Amf0Value::String(Cow::Owned(
                    chrono::Utc::now().to_rfc3339(),
                )))
            }
            _ => None, // No default for unknown keys
        }
    }
}

impl Processor<FlvData> for ScriptKeyframesFillerOperator {
    fn process(
        &mut self,
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        match input {
            FlvData::Header(_) => {
                debug!("{} Received Header. Forwarding.", self.context.name);
                // reset flag
                self.seen_first_script_tag = false;

                output(input)
            }
            FlvData::Tag(tag) if tag.tag_type == FlvTagType::ScriptData => {
                if !self.seen_first_script_tag {
                    debug!("{} Found first script tag. Modifying.", self.context.name);
                    self.seen_first_script_tag = true;

                    let inject_script = self.add_keyframes_to_amf(tag)?;
                    output(FlvData::Tag(inject_script))
                } else {
                    debug!(
                        "{} Found subsequent script tag. Forwarding.",
                        self.context.name
                    );
                    // Forward subsequent script tags without modification
                    output(FlvData::Tag(tag))
                }
            }
            // Handle other FlvData types if necessary
            _ => {
                trace!(
                    "{} Received other data type. Forwarding.",
                    self.context.name
                );
                output(input)
            }
        }
    }

    fn finish(
        &mut self,
        _output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        info!(
            "{} Script modification operator finished.",
            self.context.name
        );
        Ok(())
    }

    fn name(&self) -> &'static str {
        "ScriptInjectorOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{self, create_script_tag};
    use amf0::Amf0Value;
    use bytes::Bytes;
    use flv::{header::FlvHeader, tag::FlvTagType};
    use pipeline_common::StreamerContext;
    use pipeline_common::init_test_tracing;

    use std::collections::HashMap;

    // Helper function to extract keyframes object from tag data
    fn extract_keyframes(tag: &FlvTag) -> Option<HashMap<String, Vec<f64>>> {
        let mut cursor = std::io::Cursor::new(tag.data.clone());
        if let Ok(amf_data) = ScriptData::demux(&mut cursor) {
            if let Amf0Value::Object(props) = &amf_data.data[0] {
                for (key, value) in props.iter() {
                    if key == "keyframes" {
                        if let Amf0Value::Object(keyframe_props) = value {
                            let mut result = HashMap::new();
                            for (kf_key, kf_value) in keyframe_props.iter() {
                                if let Amf0Value::StrictArray(array) = kf_value {
                                    let values: Vec<f64> = array
                                        .iter()
                                        .filter_map(|v| {
                                            if let Amf0Value::Number(num) = v {
                                                Some(*num)
                                            } else {
                                                None
                                            }
                                        })
                                        .collect();
                                    result.insert(kf_key.as_ref().to_owned(), values);
                                }
                            }
                            return Some(result);
                        }
                    }
                }
            }
        }
        None
    }

    #[test]
    fn test_add_keyframes_to_amf() {
        init_test_tracing!();
        let context = StreamerContext::arc_new();
        let config = ScriptFillerConfig::default();
        let operator = ScriptKeyframesFillerOperator::new(context, config);

        // Test case 1: Tag without keyframes should have them added
        let tag = create_script_tag(0, false);
        let tag = match tag {
            FlvData::Tag(tag) => tag,
            _ => panic!("Expected FlvData::Tag but got something else"),
        };
        let modified_tag = operator.add_keyframes_to_amf(tag).unwrap();

        // Verify keyframes were added
        let keyframes = extract_keyframes(&modified_tag);
        assert!(keyframes.is_some());

        let keyframes = keyframes.unwrap();
        assert!(keyframes.contains_key("times"));
        assert!(keyframes.contains_key("filepositions"));
        assert!(keyframes.contains_key("spacer"));

        // Check that spacer has the right length
        assert!(keyframes.get("spacer").unwrap().len() > 100);

        // Test case 2: Tag with existing keyframes should have them replaced
        let tag = create_script_tag(0, true);
        let tag = match tag {
            FlvData::Tag(tag) => tag,
            _ => panic!("Expected FlvData::Tag but got something else"),
        };
        let modified_tag = operator.add_keyframes_to_amf(tag).unwrap();

        // Verify keyframes were modified
        let keyframes = extract_keyframes(&modified_tag).unwrap();
        assert!(keyframes.contains_key("times"));
        assert!(keyframes.contains_key("filepositions"));
        assert!(keyframes.contains_key("spacer")); // New field added

        // Original arrays should be empty now
        assert!(keyframes.get("times").unwrap().is_empty());
        assert!(keyframes.get("filepositions").unwrap().is_empty());
    }

    #[test]
    fn test_process_flow() {
        init_test_tracing!();
        let context = StreamerContext::arc_new();
        let config = ScriptFillerConfig::default();
        let mut operator = ScriptKeyframesFillerOperator::new(context, config);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send header
        operator
            .process(FlvData::Header(FlvHeader::new(true, true)), &mut output_fn)
            .unwrap();

        // Send script tag
        let script_tag = create_script_tag(0, false);
        operator.process(script_tag, &mut output_fn).unwrap();

        // Send video tag
        let video_tag = test_utils::create_video_tag(10, true);
        let video_tag_clone = video_tag.clone();
        operator.process(video_tag, &mut output_fn).unwrap();

        // Send another script tag (should be forwarded without modification)
        let second_script_tag = create_script_tag(0, false);
        operator.process(second_script_tag, &mut output_fn).unwrap();

        // Check outputs
        assert_eq!(output_items.len(), 4, "Should have 4 items in output");

        // Header should be passed through unchanged
        if let FlvData::Header(_) = &output_items[0] {
            // Header passed through correctly
        } else {
            panic!("Expected header as first output item");
        }

        // First script tag should be modified
        if let FlvData::Tag(tag) = &output_items[1] {
            assert_eq!(tag.tag_type, FlvTagType::ScriptData);
            let keyframes = extract_keyframes(tag);
            assert!(
                keyframes.is_some(),
                "First script tag should have keyframes added"
            );
        } else {
            panic!("Expected script tag as second output item");
        }

        // Video tag should be unchanged
        if let FlvData::Tag(tag) = &output_items[2] {
            assert_eq!(tag.tag_type, FlvTagType::Video);
            if let FlvData::Tag(video) = &video_tag_clone {
                assert_eq!(tag.timestamp_ms, video.timestamp_ms);
            } else {
                panic!("Expected video_tag to be FlvData::Tag");
            }
        } else {
            panic!("Expected video tag as third output item");
        }

        // Second script tag should be unchanged
        if let FlvData::Tag(tag) = &output_items[3] {
            assert_eq!(tag.tag_type, FlvTagType::ScriptData);
            // It should be a different object than the first script tag
            if let FlvData::Tag(first_script) = &output_items[1] {
                assert_ne!(
                    tag.data, first_script.data,
                    "Second script tag should not be modified"
                );
            }
        } else {
            panic!("Expected script tag as fourth output item");
        }
    }

    #[test]
    fn test_malformed_script_data() {
        init_test_tracing!();
        let context = StreamerContext::arc_new();
        let config = ScriptFillerConfig::default();
        let mut operator = ScriptKeyframesFillerOperator::new(context, config);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send header
        operator
            .process(FlvData::Header(FlvHeader::new(true, true)), &mut output_fn)
            .unwrap();

        // Create a malformed script tag with invalid AMF data
        let invalid_script_tag = FlvTag {
            timestamp_ms: 0,
            stream_id: 0,
            tag_type: FlvTagType::ScriptData,
            data: Bytes::from(vec![
                0x02, 0x00, 0x0A, 0x6E, 0x6F, 0x74, 0x4D, 0x65, 0x74, 0x61, 0x44, 0x61, 0x74, 0x61,
            ]), // "notMetaData" without proper AMF structure
        };

        // Process should handle malformed data and create a fallback
        operator
            .process(FlvData::Tag(invalid_script_tag), &mut output_fn)
            .unwrap();

        // Check that we got a valid script tag back
        if let FlvData::Tag(tag) = &output_items[1] {
            assert_eq!(tag.tag_type, FlvTagType::ScriptData);
            // Should contain valid AMF data now
            let keyframes = extract_keyframes(tag);
            assert!(
                keyframes.is_some(),
                "Should have valid keyframes structure even with invalid input"
            );
        } else {
            panic!("Expected script tag as second output item");
        }
    }
}
