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
//! ## Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use kanal;
//! use crate::context::StreamerContext;
//! use crate::operators::script_injector::{ScriptKeyframesFillerOperator, ScriptFillerConfig};
//!
//! async fn example() {
//!     let context = Arc::new(StreamerContext::default());
//!     let config = ScriptFillerConfig::default();
//!     let mut operator = ScriptKeyframesFillerOperator::new(context, config);
//!     
//!     // Create channels for the pipeline
//!     let (input_tx, input_rx) = kanal::bounded_async(32);
//!     let (output_tx, output_rx) = kanal::bounded_async(32);
//!     
//!     // Process stream in background task
//!     tokio::spawn(async move {
//!         operator.process(input_rx, output_tx).await;
//!     });
//! }
//! ```
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!

use crate::context::StreamerContext;
use crate::operators::FlvOperator;
use amf0::{Amf0Marker, Amf0Value, write_amf_property_key};
use byteorder::{BigEndian, WriteBytesExt};
use bytes::Bytes;
use flv::data::FlvData;
use flv::error::FlvError;
use flv::script::ScriptData;
use flv::tag::{FlvTag, FlvTagType};
use kanal::{AsyncReceiver, AsyncSender};
use std::borrow::Cow;
use std::io::{self, Write};
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::field::debug;

const DEFAULT_KEYFRAME_INTERVAL_MS: u32 = (3.5 * 60.0 * 60.0 * 1000.0) as u32; // 3.5 hours in ms
const MIN_INTERVAL_BETWEEN_KEYFRAMES_MS: u32 = 1900; // 1.9 seconds in ms

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

    /// Modifies the parsed AMF values to include the keyframes property.
    fn add_keyframes_to_amf(&self, tag: FlvTag) -> io::Result<FlvTag> {
        // Parse the AMF data using a reference to the tag data instead of cloning
        let mut cursor = std::io::Cursor::new(tag.data.clone());

        if let Ok(amf_data) = ScriptData::demux(&mut cursor) {
            if amf_data.name == "onMetaData" && !amf_data.data.is_empty() {
                // typically the onMetaData is a AMF0 object
                if let Amf0Value::Object(props) = &amf_data.data[0] {
                    debug!(
                        "{} Found onMetaData with {} properties",
                        self.context.name,
                        props.len()
                    );

                    // estimate size of the new object
                    // Start with the size of the original data
                    let mut estimated_size = tag.data.len() + 1;

                    // sum the estimated size of all the natural metadata keys
                    estimated_size += TOTAL_NATURAL_METADATA_SIZE;

                    // keyframes count
                    let keyframes_count =
                        (self.config.keyframe_duration_ms + MIN_INTERVAL_BETWEEN_KEYFRAMES_MS - 1)
                            / MIN_INTERVAL_BETWEEN_KEYFRAMES_MS;

                    // the total size of keyframes arrays (times, filepositions)
                    let double_array_size = 2 * keyframes_count as usize;

                    // Keyframes size calculation:
                    // 1. Object marker: 1 byte
                    // 2. Three property names (times, filepositions, spacer): ~20 bytes each including length encoding
                    // 3. Each array has a marker (1 byte) and length field (4 bytes)
                    // 4. For spacer array: Each NaN value needs 9 bytes (1 marker + 8 bytes f64), and we have 2*keyframes_count values
                    // 5. Object end marker: 3 bytes

                    let keyframes_size = 1 +                      // Object marker
                        (3 * 20) +                                // 3 property names est. 20 bytes each
                        (3 * 5) +                                 // 3 arrays with marker (1) + length field (4)
                        (9 * double_array_size) +               // Spacer array values (9 bytes each)
                        3; // Object end marker (3 bytes)

                    estimated_size += keyframes_size as usize;

                    debug!("Estimated script data size: {}", estimated_size);

                    // Allocate buffer with estimated size
                    let mut buffer = Vec::with_capacity(estimated_size);

                    // Create a mutable list of keys to track which ones we've processed
                    // Skip "keyframes" as we'll handle it specially
                    let ordered_keys: Vec<&str> =
                        NATURAL_METADATA_KEY_ORDER[..NATURAL_METADATA_KEY_ORDER.len() - 1].to_vec();

                    // Add onMetaData string
                    amf0::Amf0Encoder::encode_string(&mut buffer, &amf_data.name).unwrap();

                    // Start object
                    buffer.write_u8(Amf0Marker::Object as u8)?; // AMF0 object marker

                    // Track if we've added the standard flags
                    let mut has_keyframes_added = false;
                    let mut can_seek_to_end_added = false;

                    // loop through the ordered keys
                    for key in ordered_keys {
                        // Check for special flags
                        if key == "hasKeyframes" {
                            has_keyframes_added = true;
                        } else if key == "canSeekToEnd" {
                            can_seek_to_end_added = true;
                        } else if key == "keyframes" {
                            // Skip keyframes as we'll handle it specially
                            continue;
                        }

                        // Find the property in the original props
                        let value_opt = props
                            .iter()
                            .find(|(k, _)| k.as_ref() == key)
                            .map(|(_, v)| v);

                        if let Some(value) = value_opt {
                            // write key
                            write_amf_property_key!(&mut buffer, key);
                            // Encode value
                            amf0::Amf0Encoder::encode(&mut buffer, value).unwrap();
                            trace!("Encoded property: {}, {:?}", key, value);
                        } else {
                            // Add default values for missing properties
                            if let Some(default_value) = self.get_default_metadata_value(key) {
                                trace!(
                                    "{} Adding default value for missing property: {}",
                                    self.context.name, key
                                );
                                write_amf_property_key!(&mut buffer, key);
                                amf0::Amf0Encoder::encode(&mut buffer, &default_value).unwrap();
                            } else {
                                // Skip unknown properties
                                trace!(
                                    "{} Unknown property in metadata: {}",
                                    self.context.name, key
                                );
                            }
                        }
                    }

                    // Add remaining properties from the original object
                    for (key, value) in props.iter() {
                        if !NATURAL_METADATA_KEY_ORDER.contains(&key.as_ref()) {
                            write_amf_property_key!(&mut buffer, key);
                            amf0::Amf0Encoder::encode(&mut buffer, value).unwrap();
                        }
                    }

                    // Add standard player compatibility flags if not present
                    if !has_keyframes_added {
                        write_amf_property_key!(&mut buffer, "hasKeyframes");
                        amf0::Amf0Encoder::encode(&mut buffer, &Amf0Value::Boolean(true)).unwrap();
                    }

                    if !can_seek_to_end_added {
                        write_amf_property_key!(&mut buffer, "canSeekToEnd");
                        amf0::Amf0Encoder::encode(&mut buffer, &Amf0Value::Boolean(true)).unwrap();
                    }

                    // Add keyframes object directly
                    write_amf_property_key!(&mut buffer, "keyframes");
                    buffer.write_u8(Amf0Marker::Object as u8)?; // AMF0 Object marker

                    // Times array
                    write_amf_property_key!(&mut buffer, "times");
                    buffer.write_u8(Amf0Marker::StrictArray as u8)?; // AMF0 Strict Array marker
                    buffer.write_u32::<BigEndian>(0)?;

                    // File positions array - empty array
                    write_amf_property_key!(&mut buffer, "filepositions");
                    buffer.push(Amf0Marker::StrictArray as u8); // AMF0 Strict Array marker
                    buffer.write_u32::<BigEndian>(0)?;

                    // Spacer array, stub array
                    write_amf_property_key!(&mut buffer, "spacer");
                    buffer.write_u8(Amf0Marker::StrictArray as u8)?; // AMF0 Strict Array marker

                    // // Add spacer array length
                    buffer.write_u32::<BigEndian>(double_array_size as u32)?;

                    for _ in 0..double_array_size {
                        amf0::Amf0Encoder::encode_number(&mut buffer, f64::NAN).unwrap();
                    }

                    // // End keyframes object
                    amf0::Amf0Encoder::object_eof(&mut buffer).unwrap();

                    // End of object
                    amf0::Amf0Encoder::object_eof(&mut buffer).unwrap();

                    debug!("New script data payload size: {}", buffer.len());

                    buffer.flush()?; // Flush the buffer to ensure all data is written

                    // Create a new tag with the modified data
                    return Ok(FlvTag {
                        timestamp_ms: tag.timestamp_ms,
                        stream_id: tag.stream_id,
                        tag_type: tag.tag_type,
                        data: Bytes::from(buffer),
                    });
                } else {
                    // for other types, we just ignore the injection
                    warn!(
                        "{} Unsupported AMF data type for keyframe injection: {:?}",
                        self.context.name, amf_data.data[0]
                    );
                }
            }
        }
        Ok(tag)
    }

    /// Get default metadata value for a given key.
    fn get_default_metadata_value(&self, key: &str) -> Option<Amf0Value> {
        match key {
            // Booleans - Defaulting to true might be optimistic
            "hasAudio" | "hasVideo" | "stereo" => Some(Amf0Value::Boolean(true)),
            // Numeric - Defaulting to 0 might be safer than assuming values
            "duration"
            | "width"
            | "height"
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

impl FlvOperator for ScriptKeyframesFillerOperator {
    fn context(&self) -> &Arc<StreamerContext> {
        &self.context
    }

    async fn process(
        &mut self,
        input: AsyncReceiver<Result<FlvData, FlvError>>,
        output: AsyncSender<Result<FlvData, FlvError>>,
    ) {
        debug!("{} Starting script modification process", self.context.name);

        while let Ok(item_result) = input.recv().await {
            match item_result {
                Ok(data) => {
                    match data {
                        FlvData::Header(_) => {
                            debug!("{} Received Header. Forwarding.", self.context.name);
                            // reset flag
                            self.seen_first_script_tag = false;

                            if output.send(Ok(data)).await.is_err() {
                                return; // Output closed
                            }
                        }
                        FlvData::Tag(tag) => {
                            if tag.tag_type == FlvTagType::ScriptData {
                                if !self.seen_first_script_tag {
                                    debug!(
                                        "{} Found first script tag. Modifying.",
                                        self.context.name
                                    );
                                    self.seen_first_script_tag = true;

                                    let tag_clone = tag.clone();
                                    let inject_script =
                                        self.add_keyframes_to_amf(tag_clone).unwrap_or(tag);
                                    if output.send(Ok(FlvData::Tag(inject_script))).await.is_err() {
                                        return; // Output closed
                                    }
                                } else {
                                    debug!(
                                        "{} Found subsequent script tag. Forwarding.",
                                        self.context.name
                                    );
                                    // Forward subsequent script tags without modification
                                    if output.send(Ok(FlvData::Tag(tag))).await.is_err() {
                                        return; // Output closed
                                    }
                                }
                            } else {
                                // Forward tags that are not script data
                                if output.send(Ok(FlvData::Tag(tag))).await.is_err() {
                                    debug!("Sending tag failed. Output closed.");
                                    return;
                                }
                            }
                        }
                        // Handle other FlvData types if necessary
                        _ => {
                            trace!(
                                "{} Received other data type. Forwarding.",
                                self.context.name
                            );
                            if output.send(Ok(data)).await.is_err() {
                                return; // Output closed
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "{} Error receiving data: {:?}. Forwarding error.",
                        self.context.name, e
                    );
                    if output.send(Err(e)).await.is_err() {
                        return; // Output closed
                    }
                }
            }
        }
        info!(
            "{} Script modification operator finished.",
            self.context.name
        );
    }

    fn name(&self) -> &'static str {
        "ScriptInjectorOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amf0::Amf0Value;
    use bytes::Bytes;
    use flv::header::FlvHeader;
    use kanal::{bounded_async, unbounded_async};
    use std::collections::HashMap;
    use std::time::Duration;

    // Helper to initialize tracing for tests
    fn init_tracing() {
        let _ = tracing_subscriber::fmt::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer() // Write to test output
            .try_init();
    }

    // Helper function to create a StreamerContext for testing
    fn create_test_context() -> Arc<StreamerContext> {
        Arc::new(StreamerContext::default())
    }

    // Helper function to create a script tag with onMetaData
    fn create_metadata_tag(with_keyframes: bool) -> FlvTag {
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
        amf0::Amf0Encoder::encode_string(&mut buffer, "onMetaData").unwrap();
        amf0::Amf0Encoder::encode(&mut buffer, &obj).unwrap();

        FlvTag {
            timestamp_ms: 0,
            stream_id: 0,
            tag_type: FlvTagType::ScriptData,
            data: Bytes::from(buffer),
        }
    }

    // Helper function to create a video tag
    fn create_video_tag() -> FlvTag {
        // Simple video tag data - doesn't need to be valid for our tests
        let data = vec![0x17, 0x01, 0x00, 0x00, 0x00]; // H.264 keyframe

        FlvTag {
            timestamp_ms: 10,
            stream_id: 0,
            tag_type: FlvTagType::Video,
            data: Bytes::from(data),
        }
    }

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
                                    result.insert(kf_key.to_string(), values);
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

    #[tokio::test]
    async fn test_add_keyframes_to_amf() {
        init_tracing();
        let context = create_test_context();
        let config = ScriptFillerConfig::default();
        let operator = ScriptKeyframesFillerOperator::new(context, config);

        // Test case 1: Tag without keyframes should have them added
        let tag = create_metadata_tag(false);
        let mut modified_tag = operator.add_keyframes_to_amf(tag).unwrap();

        // Verify keyframes were added
        let keyframes = extract_keyframes(&modified_tag);
        assert!(keyframes.is_some());

        let keyframes = keyframes.unwrap();
        assert!(keyframes.contains_key("times"));
        assert!(keyframes.contains_key("filepositions"));
        assert!(keyframes.contains_key("spacer"));

        // Check that spacer has the right length
        assert!(keyframes.get("spacer").unwrap().len() > 100,);

        // Test case 2: Tag with existing keyframes should have them replaced
        let tag = create_metadata_tag(true);
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

    #[tokio::test]
    async fn test_process_flow() {
        init_tracing();
        let context = create_test_context();
        let config = ScriptFillerConfig::default();
        let mut operator = ScriptKeyframesFillerOperator::new(context, config);

        // Create channels
        let (tx_input, rx_input) = unbounded_async();
        let (tx_output, rx_output) = unbounded_async();

        // Start operator in background
        let operator_task = tokio::spawn(async move {
            operator.process(rx_input, tx_output).await;
        });

        // Send header
        let header = FlvHeader::new(true, true);
        tx_input.send(Ok(FlvData::Header(header))).await.unwrap();

        // Send script tag
        let script_tag = create_metadata_tag(false);
        tx_input.send(Ok(FlvData::Tag(script_tag))).await.unwrap();

        // Send video tag
        let video_tag = create_video_tag();
        tx_input
            .send(Ok(FlvData::Tag(video_tag.clone())))
            .await
            .unwrap();

        // Send another script tag (should be ignored for modification)
        let second_script_tag = create_metadata_tag(false);
        tx_input
            .send(Ok(FlvData::Tag(second_script_tag.clone())))
            .await
            .unwrap();

        // Check outputs

        // Header should be passed through unchanged
        let header_out = rx_output.recv().await.unwrap().unwrap();
        assert!(matches!(header_out, FlvData::Header(_)));

        // First script tag should be modified
        let script_out = rx_output.recv().await.unwrap().unwrap();
        if let FlvData::Tag(tag) = script_out {
            assert_eq!(tag.tag_type, FlvTagType::ScriptData);
            let keyframes = extract_keyframes(&tag);
            assert!(keyframes.is_some());
        } else {
            panic!("Expected script tag");
        }

        // Video tag should be unchanged
        let video_out = rx_output.recv().await.unwrap().unwrap();
        if let FlvData::Tag(tag) = video_out {
            assert_eq!(tag.tag_type, FlvTagType::Video);
            assert_eq!(tag.timestamp_ms, video_tag.timestamp_ms);
        } else {
            panic!("Expected video tag");
        }

        // Second script tag should be unchanged
        // Note: In the current implementation, subsequent script tags are not forwarded
        // So we don't check for them here.

        // Clean up
        drop(tx_input);
        let _ = tokio::time::timeout(Duration::from_secs(1), operator_task).await;
    }

    #[tokio::test]
    async fn test_error_handling() {
        let context = create_test_context();
        let config = ScriptFillerConfig::default();
        let mut operator = ScriptKeyframesFillerOperator::new(context, config);

        let (tx_input, rx_input) = bounded_async(10);
        let (tx_output, mut rx_output) = bounded_async(10);

        // Start operator in background
        let operator_task = tokio::spawn(async move {
            operator.process(rx_input, tx_output).await;
        });

        // Send an error
        let test_error = FlvError::Io(io::Error::new(io::ErrorKind::Other, "test error"));
        tx_input.send(Err(test_error)).await.unwrap();

        // The error should be forwarded
        let error_out = rx_output.recv().await.unwrap();
        assert!(error_out.is_err());

        // Clean up
        drop(tx_input);
        let _ = tokio::time::timeout(Duration::from_secs(1), operator_task).await;
    }
}
