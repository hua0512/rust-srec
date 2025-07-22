//! # FLV Writer Module
//!
//! Implementation of FLV file writing functionality.
//!
//! This module provides capabilities to create and write FLV (Flash Video) files,
//! with support for video, audio, and script tags.
//!
//! ## Features
//!
//! - Creating FLV files with proper headers
//! - Writing video tags with various codecs (AVC/H.264, HEVC/H.265, AV1, etc.)
//! - Writing audio tags with various codecs (AAC, MP3, etc.)
//! - Writing script tags for metadata
//! - Support for both legacy and enhanced FLV formats
//!
//! ## Usage
//!
//! ```no_run
//! use flv::writer::FlvWriter;
//! use flv::header::FlvHeader;
//! use bytes::Bytes;
//! use std::fs::File;
//! use std::io::{BufWriter, Result};
//!
//! fn main() -> Result<()> {
//!     // Create a new FLV file with both audio and video
//!     let file = File::create("output.flv")?;
//!     let mut writer = FlvWriter::new(file)?;
//!
//!     // Write metadata
//!     writer.write_metadata("onMetaData", &[("duration", 60.0), ("width", 1280.0), ("height", 720.0)]);
//!
//!     // Write video and audio tags
//!     // ...
//!
//!     Ok(())
//! }
//! ```
//!
//! ## License
//!
//! MIT License

use crate::header::FlvHeader;
use crate::tag::{FlvTag, FlvTagType};
use amf0::{Amf0Encoder, Amf0Value, Amf0WriteError};
use byteorder::{BigEndian, WriteBytesExt};
use bytes::Bytes;
use std::borrow::Cow;
use std::io::{self, Seek, Write};

/// FLV Writer for creating FLV files
pub struct FlvWriter<W: Write + Seek> {
    pub writer: W,
    pub has_audio: bool,
    pub has_video: bool,
    pub timestamp: u32,
    pub previous_tag_size: u32,
}

impl<W: Write + Seek> FlvWriter<W> {
    /// Creates a new FLV writer with the specified output writer
    pub fn new(writer: W) -> io::Result<Self> {
        Ok(Self {
            writer,
            has_audio: false,
            has_video: false,
            timestamp: 0,
            previous_tag_size: 0,
        })
    }

    pub fn write_header(&mut self, header: &FlvHeader) -> io::Result<()> {
        // Write FLV signature ("FLV")
        self.writer.write_all(&[0x46, 0x4C, 0x56])?; // "FLV"

        // Write version (0x01)
        self.writer.write_u8(header.version)?;

        // Write flags (bit 2 for audio, bit 0 for video)
        let mut flags = 0_u8;
        if header.has_audio {
            flags |= 0x04;
        }
        if header.has_video {
            flags |= 0x01;
        }
        self.writer.write_u8(flags)?;

        // Write data offset (always 9 for standard FLV header)
        self.writer.write_u32::<BigEndian>(9)?;

        // Write initial previous tag size (0 before first tag)
        self.writer.write_u32::<BigEndian>(0)?;
        Ok(())
    }

    /// Writes an FLV tag header to the output
    ///
    /// # Arguments
    ///
    /// * `tag_type` - The type of the tag (audio, video, script)
    /// * `data_size` - The size of the tag data in bytes
    /// * `timestamp_ms` - The timestamp in milliseconds
    ///
    /// # Returns
    ///
    /// A Result indicating success or an IO error
    pub fn write_tag_header(
        &mut self,
        tag_type: FlvTagType,
        data_size: u32,
        timestamp_ms: u32,
    ) -> io::Result<()> {
        // Write tag type
        self.writer.write_u8(tag_type.into())?;

        // Write data size (3 bytes)
        self.writer.write_u24::<BigEndian>(data_size)?;

        // Write timestamp (3 bytes + 1 byte extended)
        self.writer
            .write_u24::<BigEndian>(timestamp_ms & 0xFFFFFF)?;
        self.writer.write_u8((timestamp_ms >> 24) as u8)?;

        // Write stream ID (always 0)
        self.writer.write_u24::<BigEndian>(0)?;

        Ok(())
    }

    /// Writes an FLV tag to the output
    ///
    /// # Arguments
    ///
    /// * `tag_type` - The type of the tag (audio, video, script)
    /// * `data` - The tag data
    /// * `timestamp_ms` - The timestamp in milliseconds
    ///
    /// # Returns
    ///
    /// A Result indicating success or an IO error
    pub fn write_tag(
        &mut self,
        tag_type: FlvTagType,
        data: Bytes,
        timestamp_ms: u32,
    ) -> io::Result<()> {
        let data_size = data.len() as u32;

        // Write tag header
        self.write_tag_header(tag_type, data_size, timestamp_ms)?;

        // Write tag data
        self.writer.write_all(&data)?;

        // Update previous tag size
        self.previous_tag_size = data_size + 11; // data size + tag header size

        // Write previous tag size
        self.writer.write_u32::<BigEndian>(self.previous_tag_size)?;

        // Update timestamp for sequential writing
        if timestamp_ms > self.timestamp {
            self.timestamp = timestamp_ms;
        }

        Ok(())
    }

    pub fn write_tag_f(&mut self, tag: &FlvTag) -> io::Result<()> {
        self.write_tag(tag.tag_type, tag.data.clone(), tag.timestamp_ms)?;
        Ok(())
    }

    /// Writes a script tag (metadata) to the output
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the script data (e.g., "onMetaData")
    /// * `properties` - A slice of tuples containing property names and values
    ///
    /// # Returns
    ///
    /// A Result indicating success or an IO error
    pub fn write_metadata(
        &mut self,
        name: &str,
        properties: &[(&str, f64)],
    ) -> Result<(), Amf0WriteError> {
        let mut buffer = Vec::new();

        // Write the script name (e.g., "onMetaData")
        Amf0Encoder::encode_string(&mut buffer, name)?;

        // Create object properties
        let amf_properties: Vec<(Cow<'_, str>, Amf0Value)> = properties
            .iter()
            .map(|(name, value)| (Cow::Borrowed(*name), Amf0Value::Number(*value)))
            .collect();

        // Encode the object
        Amf0Encoder::encode_object(&mut buffer, &amf_properties)?;

        // Write script tag
        self.write_tag(FlvTagType::ScriptData, Bytes::from(buffer), 0)
            .map_err(Amf0WriteError::Io)
    }

    /// Writes video data to the output
    ///
    /// # Arguments
    ///
    /// * `data` - The video data bytes
    /// * `timestamp_ms` - The timestamp in milliseconds
    ///
    /// # Returns
    ///
    /// A Result indicating success or an IO error
    pub fn write_video(&mut self, data: Bytes, timestamp_ms: u32) -> io::Result<()> {
        if !self.has_video {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "FLV file not configured for video",
            ));
        }

        self.write_tag(FlvTagType::Video, data, timestamp_ms)
    }

    /// Writes audio data to the output
    ///
    /// # Arguments
    ///
    /// * `data` - The audio data bytes
    /// * `timestamp_ms` - The timestamp in milliseconds
    ///
    /// # Returns
    ///
    /// A Result indicating success or an IO error
    pub fn write_audio(&mut self, data: Bytes, timestamp_ms: u32) -> io::Result<()> {
        if !self.has_audio {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "FLV file not configured for audio",
            ));
        }

        self.write_tag(FlvTagType::Audio, data, timestamp_ms)
    }

    /// Flushes any buffered data to the underlying writer
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    /// Returns the current timestamp of the writer
    pub fn timestamp(&self) -> u32 {
        self.timestamp
    }

    /// Closes the writer, ensuring all data is flushed
    ///
    /// This method flushes any buffered data and returns the inner writer.
    pub fn close(mut self) -> io::Result<W> {
        self.flush()?;
        Ok(self.writer)
    }

    /// Consumes the `FlvWriter`, returning the wrapped writer.
    ///
    /// Note that any leftover data in internal buffers will be written to the underlying writer
    /// before returning it.
    pub fn into_inner(mut self) -> io::Result<W> {
        self.flush()?;
        Ok(self.writer)
    }
}

impl<W: Write + Seek> Write for FlvWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_write_header() {
        let buffer = Cursor::new(Vec::new());

        let mut writer = FlvWriter::new(buffer).unwrap();
        writer.write_header(&FlvHeader::new(true, true)).unwrap();

        // Get the inner buffer
        let buffer = writer.writer.into_inner();

        // Check FLV signature
        assert_eq!(&buffer[0..3], b"FLV");
        // Check version
        assert_eq!(buffer[3], 0x01);
        // Check flags (audio + video = 0x05)
        assert_eq!(buffer[4], 0x05);
        // Check data offset
        assert_eq!(&buffer[5..9], &[0x00, 0x00, 0x00, 0x09]);
        // Check initial previous tag size
        assert_eq!(&buffer[9..13], &[0x00, 0x00, 0x00, 0x00]);
    }
}
