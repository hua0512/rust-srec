//! # FLV Script Module
//!
//! Implementation of FLV script tag data parsing for metadata and other scripting information.
//!
//! This module handles parsing of script data from FLV (Flash Video) files, primarily used
//! for metadata such as video dimensions, duration, framerate, and other properties. Script
//! tags contain ActionScript objects serialized in AMF0 format.
//!
//! ## Features
//!
//! - Parses AMF0-encoded script data from FLV tags
//! - Extracts metadata fields like duration, width, height, framerate, etc.
//! - Supports common script tag names like "onMetaData"
//! - Handles complex nested data structures through the AMF0 format
//!
//! ## Common Script Tags
//!
//! - `onMetaData`: Contains metadata about the media file (dimensions, duration, etc.)
//!
//! ## Specifications
//!
//! - [Flash Video File Format Specification v10](https://www.adobe.com/content/dam/acom/en/devnet/flv/video_file_format_spec_v10.pdf)
//! - [Action Message Format -- AMF 0](https://www.adobe.com/content/dam/acom/en/devnet/pdf/amf0-file-format-specification.pdf)
//!
//! ## Usage
//!
//! ```no_run
//! use flv::script::ScriptData;
//! use bytes::Bytes;
//! use std::io::Cursor;
//!
//! // Parse script data from an FLV tag body
//! let data = vec![/* FLV script tag data */];
//! let mut cursor = Cursor::new(Bytes::from(data));
//! let script = ScriptData::demux(&mut cursor).unwrap();
//!
//! // Check script name and access data
//! if script.name == "onMetaData" {
//!     println!("Found metadata script tag with {} values", script.data.len());
//! }
//! ```
//! ## Credits
//!
//! Based on specifications from Adobe and the E-RTMP project.
//!
//! Based on the work of [ScuffleCloud project](https://github.com/ScuffleCloud/scuffle/blob/main/crates/flv/src/script.rs)
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - ScuffleCloud project contributors
//! - hua0512

use std::{fmt, io};

use amf0::{Amf0Decoder, Amf0Marker, Amf0Value};
use bytes::Bytes;
use bytes_util::BytesCursorExt;
use tracing::warn;

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptData {
    /// The name of the script data
    pub name: String,
    /// The data of the script data
    pub data: Vec<Amf0Value<'static>>,
}

impl ScriptData {
    /// Creates a new empty ScriptData instance with the specified name.
    ///
    /// # Example
    ///
    /// ```
    /// use flv::script::ScriptData;
    ///
    /// let script = ScriptData::new("onMetaData");
    /// assert_eq!(script.name, "onMetaData");
    /// assert!(script.data.is_empty());
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data: Vec::new(),
        }
    }

    pub fn demux(reader: &mut io::Cursor<Bytes>) -> io::Result<Self> {
        let buf = reader.extract_remaining();
        let mut amf0_reader = Amf0Decoder::new(&buf);

        let name = match amf0_reader.decode_with_type(Amf0Marker::String) {
            Ok(Amf0Value::String(name)) => name,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Invalid script data name, expected String but got {:?}",
                        other.unwrap_err()
                    ),
                ));
            }
        };

        let (data, error) = amf0_reader.decode_all();

        // If data is empty and we have an error, return the error
        if data.is_empty() && error.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse script data: {:?}", error.unwrap()),
            ));
        } else if error.is_some() {
            // If we have data but also an error, log the error but continue
            warn!(
                "Partial script data parsed with error: {:?}",
                error.unwrap()
            );
        }

        Ok(Self {
            name: name.into_owned(),
            data: data.into_iter().map(|v| v.to_owned()).collect(),
        })
    }
}

impl fmt::Display for ScriptData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} values", self.data.len())
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use amf0::Amf0Encoder;
    use byteorder::{BigEndian, WriteBytesExt};

    use super::*;

    use std::borrow::Cow;

    #[test]
    fn test_script_data() {
        let mut reader = io::Cursor::new(Bytes::from_static(&[
            0x02, // String marker
            0x00, 0x0A, // Length (10 bytes)
            b'o', b'n', b'M', b'e', b't', b'a', b'D', b'a', b't', b'a', // "onMetaData"
            0x05, // null marker
            0x05, // null marker
        ]));
        let script_data = ScriptData::demux(&mut reader).unwrap();
        assert_eq!(script_data.name, "onMetaData");
        assert_eq!(script_data.data.len(), 2);
        assert_eq!(script_data.data[0], Amf0Value::Null);
        assert_eq!(script_data.data[1], Amf0Value::Null);
    }

    #[test]
    fn test_onmetadata_with_basic_info() {
        // Create a buffer to hold our AMF data
        let mut buffer = Vec::new();

        // Write the name "onMetaData"
        Amf0Encoder::encode_string(&mut buffer, "onMetaData").unwrap();

        // Create an object with typical FLV metadata properties
        let properties = vec![
            (Cow::Borrowed("duration"), Amf0Value::Number(120.5)),
            (Cow::Borrowed("width"), Amf0Value::Number(1280.0)),
            (Cow::Borrowed("height"), Amf0Value::Number(720.0)),
            (Cow::Borrowed("videodatarate"), Amf0Value::Number(1000.0)),
            (Cow::Borrowed("framerate"), Amf0Value::Number(29.97)),
            (Cow::Borrowed("videocodecid"), Amf0Value::Number(7.0)), // AVC
            (Cow::Borrowed("audiodatarate"), Amf0Value::Number(128.0)),
            (Cow::Borrowed("audiosamplerate"), Amf0Value::Number(44100.0)),
            (Cow::Borrowed("audiosamplesize"), Amf0Value::Number(16.0)),
            (Cow::Borrowed("stereo"), Amf0Value::Boolean(true)),
            (Cow::Borrowed("audiocodecid"), Amf0Value::Number(10.0)), // AAC
            (
                Cow::Borrowed("major_brand"),
                Amf0Value::String("mp42".into()),
            ),
            (
                Cow::Borrowed("encoder"),
                Amf0Value::String("Lavf58.29.100".into()),
            ),
        ];

        // Write as object (common format for onMetaData)
        Amf0Encoder::encode_object(&mut buffer, &properties).unwrap();

        // Test parsing
        let mut reader = io::Cursor::new(Bytes::from(buffer));
        let script_data = ScriptData::demux(&mut reader).unwrap();

        // Verify basic structure
        assert_eq!(script_data.name, "onMetaData");
        assert_eq!(script_data.data.len(), 1); // The object is one value

        // Verify it's an object
        if let Amf0Value::Object(obj) = &script_data.data[0] {
            // Check specific values
            assert_eq!(
                obj.iter().find(|(k, _)| k == "duration").map(|(_, v)| v),
                Some(&Amf0Value::Number(120.5))
            );
            assert_eq!(
                obj.iter().find(|(k, _)| k == "width").map(|(_, v)| v),
                Some(&Amf0Value::Number(1280.0))
            );
            assert_eq!(
                obj.iter().find(|(k, _)| k == "height").map(|(_, v)| v),
                Some(&Amf0Value::Number(720.0))
            );
            assert_eq!(
                obj.iter().find(|(k, _)| k == "framerate").map(|(_, v)| v),
                Some(&Amf0Value::Number(29.97))
            );
            assert_eq!(
                obj.iter()
                    .find(|(k, _)| k == "audiocodecid")
                    .map(|(_, v)| v),
                Some(&Amf0Value::Number(10.0))
            );
            assert_eq!(
                obj.iter().find(|(k, _)| k == "stereo").map(|(_, v)| v),
                Some(&Amf0Value::Boolean(true))
            );
            assert_eq!(
                obj.iter().find(|(k, _)| k == "encoder").map(|(_, v)| v),
                Some(&Amf0Value::String("Lavf58.29.100".into()))
            );
        } else {
            panic!("Expected Object but got: {:?}", script_data.data[0]);
        }

        // Test display format
        let display = format!("{script_data}");
        assert_eq!(display, "1 values");
    }

    #[test]
    fn test_onmetadata_with_strict_array() {
        // Create a buffer to hold our AMF data
        let mut buffer = Vec::new();

        // Write the name "onMetaData"
        Amf0Encoder::encode_string(&mut buffer, "onMetaData").unwrap();

        // First create some values to put in the strict array
        let mut strict_array_buffer = Vec::new();

        // Add a number value to the array
        Amf0Encoder::encode_number(&mut strict_array_buffer, 29.97).unwrap();

        // Add a string value to the array
        Amf0Encoder::encode_string(&mut strict_array_buffer, "Video Title").unwrap();

        // Add a boolean value to the array
        Amf0Encoder::encode_bool(&mut strict_array_buffer, true).unwrap();

        // Write array marker and length
        buffer.push(0x0A); // StrictArray marker
        buffer.write_u32::<BigEndian>(3).unwrap(); // Array length (3 elements)

        // Write the array content
        buffer.extend_from_slice(&strict_array_buffer);

        // Test parsing
        let mut reader = io::Cursor::new(Bytes::from(buffer));
        let script_data = ScriptData::demux(&mut reader).unwrap();

        // Verify basic structure
        assert_eq!(script_data.name, "onMetaData");
        assert_eq!(script_data.data.len(), 1); // The array is one value

        // Verify it's a strict array
        if let Amf0Value::StrictArray(array) = &script_data.data[0] {
            // Check array length
            assert_eq!(array.len(), 3);

            // Check specific values
            assert_eq!(array[0], Amf0Value::Number(29.97));
            assert_eq!(array[1], Amf0Value::String("Video Title".into()));
            assert_eq!(array[2], Amf0Value::Boolean(true));
        } else {
            panic!("Expected StrictArray but got: {:?}", script_data.data[0]);
        }
    }
}
