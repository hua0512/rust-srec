use crate::data::FlvData;
use crate::encode;
use bytes::{BufMut, BytesMut};
use std::io;
use tokio_util::codec::Encoder;

/// Maximum allowed data size for a single FLV tag payload (24 bits).
const MAX_TAG_DATA_SIZE: usize = 0xFFFFFF; // 16,777,215 bytes
const PREV_TAG_FIELD_SIZE: usize = encode::PREV_TAG_SIZE_FIELD_SIZE;
const TAG_HEADER_SIZE: usize = encode::TAG_HEADER_SIZE;

/// Encodes `FlvData` (Header or Tag) into the FLV byte format.
///
/// This encoder maintains the necessary state (`last_tag_size_written`)
/// to correctly write the `PreviousTagSize` field before each tag.
/// It ensures the header is written only once and validates tag data size.
///
/// Use with `tokio_util::codec::FramedWrite` for buffered asynchronous writing.
#[derive(Debug, Default)]
pub struct FlvEncoder {
    /// Stores the size of the *previous* tag (tag header + tag payload), in bytes.
    ///
    /// FLV `PreviousTagSize` values do NOT include the 4-byte `PreviousTagSize` field itself.
    /// The value for an FLV v1 tag is `11 + DataSize`.
    ///
    /// We write `PreviousTagSize` before each tag, which matches the on-wire layout:
    /// `Header(9) + PrevTagSize0(4) + Tag1 + PrevTagSize1 + Tag2 + ...`.
    /// This field tracks the value to write for the next tag.
    last_tag_size_written: u32,
    /// Tracks if the FLV header has already been written.
    header_written: bool,
}

impl Encoder<FlvData> for FlvEncoder {
    type Error = io::Error;

    /// Encodes an `FlvData` item into the provided `BytesMut` buffer.
    ///
    /// - For `FlvData::Header`, writes the 9-byte FLV header followed by `PreviousTagSize0` (=0).
    ///   This must be the first item.
    /// - For `FlvData::Tag`, writes the 4-byte `PreviousTagSize`, the 11-byte tag header,
    ///   and the tag data payload. Requires the header to have been written previously.
    /// - For `FlvData::EndOfSequence`, does nothing (control signal only).
    fn encode(&mut self, item: FlvData, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            FlvData::Header(header) => {
                // --- Encode FLV Header ---
                if self.header_written {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "FLV header can only be written once",
                    ));
                }

                let bytes = encode::encode_header_bytes(&header)?;
                dst.reserve(bytes.len());
                dst.put_slice(&bytes);

                // Update state: Header is now written, next tag's prev size is 0.
                self.last_tag_size_written = 0;
                self.header_written = true;
                Ok(())
            }
            FlvData::Tag(tag) => {
                // --- Encode FLV Tag ---
                if !self.header_written {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Cannot write FLV tag before header",
                    ));
                }

                let data_len = tag.data().len();

                // FLV tag StreamID is defined as UI24 and, for FLV files, SHALL always be 0.
                // (Multitrack in E-RTMP is signaled in the payload, not via StreamID.)
                if tag.stream_id != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("FLV tag stream_id must be 0 (got {})", tag.stream_id),
                    ));
                }

                // Validate tag data size fits within 24 bits
                if data_len > MAX_TAG_DATA_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "FLV tag data size ({data_len}) exceeds 24-bit limit ({MAX_TAG_DATA_SIZE})"
                        ),
                    ));
                }
                let data_len_u32 = data_len as u32;

                // Calculate total bytes written for this call:
                // `PreviousTagSize` field (4) + Tag Header (11) + Tag Data.
                let current_write_size = PREV_TAG_FIELD_SIZE + TAG_HEADER_SIZE + data_len;
                dst.reserve(current_write_size);

                // 1. PreviousTagSize
                let prev_bytes = encode::encode_prev_tag_size_bytes(self.last_tag_size_written);
                dst.put_slice(&prev_bytes);

                // 2. Tag header
                let header_bytes = encode::encode_tag_header_bytes(
                    tag.tag_type(),
                    tag.is_filtered(),
                    data_len_u32,
                    tag.timestamp_ms,
                    0,
                )?;
                dst.put_slice(&header_bytes);

                // 3. Write Tag Data (Variable size)
                //    Append the raw bytes payload efficiently.
                dst.put(tag.into_data());

                // --- Update State for Next Tag ---
                // The *next* tag's PreviousTagSize field needs the size of the tag we just wrote
                // (tag header + tag payload), excluding the PreviousTagSize field itself.
                self.last_tag_size_written = (TAG_HEADER_SIZE + data_len) as u32;
                Ok(())
            }
            // Handle control variants without an on-disk FLV representation.
            _ => {
                // `FlvData::EndOfSequence` is a control signal in our pipeline and does not have a
                // corresponding on-disk FLV representation. Treat it as a no-op.
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*; // Import encoder and constants
    use crate::data::FlvData;
    use crate::header::FlvHeader;
    use crate::tag::{FlvTag, FlvTagType};
    use bytes::{Bytes, BytesMut};
    use tokio_util::codec::Encoder; // Bring trait into scope

    const FLV_HEADER_SIZE: usize = encode::FLV_HEADER_SIZE;

    // Helper to create a default valid header
    fn default_header() -> FlvHeader {
        FlvHeader::new(true, true)
    }

    #[test]
    fn test_encode_header() {
        let mut encoder = FlvEncoder::default();
        let header = default_header();
        let mut buf = BytesMut::new();

        let result = encoder.encode(FlvData::Header(header), &mut buf);

        assert!(result.is_ok());
        assert_eq!(buf.len(), FLV_HEADER_SIZE + PREV_TAG_FIELD_SIZE);
        assert_eq!(
            &buf[..],
            &[
                // FLV
                0x46, 0x4C, 0x56, // Version 1
                0x01, // Flags (Audio + Video)
                0x05, // Data Offset 9 (BigEndian)
                0x00, 0x00, 0x00, 0x09, // PreviousTagSize0
                0x00, 0x00, 0x00, 0x00,
            ]
        );
        assert!(encoder.header_written);
        assert_eq!(encoder.last_tag_size_written, 0); // Reset after header
    }

    #[test]
    fn test_encode_tag_without_header_fails() {
        let mut encoder = FlvEncoder::default();
        let tag = FlvTag::new(
            100,
            0,
            FlvTagType::Video,
            false,
            Bytes::from_static(&[0x01, 0x02]),
        );
        let mut buf = BytesMut::new();

        let result = encoder.encode(FlvData::Tag(tag), &mut buf);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("before header"));
    }

    #[test]
    fn test_encode_header_twice_fails() {
        let mut encoder = FlvEncoder::default();
        let header = default_header();
        let mut buf = BytesMut::new();

        // First encode is ok
        assert!(
            encoder
                .encode(FlvData::Header(header.clone()), &mut buf)
                .is_ok()
        );
        assert_eq!(buf.len(), FLV_HEADER_SIZE + PREV_TAG_FIELD_SIZE);

        // Second encode should fail
        let result = encoder.encode(FlvData::Header(header), &mut buf);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("written once"));
        assert_eq!(buf.len(), FLV_HEADER_SIZE + PREV_TAG_FIELD_SIZE); // Buffer unchanged by second call
    }

    #[test]
    fn test_encode_first_tag() {
        let mut encoder = FlvEncoder::default();
        let header = default_header();
        let tag = FlvTag::new(
            100,
            0,
            FlvTagType::Video,
            false,
            Bytes::from_static(&[0xAA, 0xBB, 0xCC, 0xDD]),
        );
        let data_len = 4;
        let expected_tag_structure_size = PREV_TAG_FIELD_SIZE + TAG_HEADER_SIZE + data_len; // 4 + 11 + 4 = 19
        let expected_tag_size = TAG_HEADER_SIZE + data_len; // 11 + 4 = 15

        let mut buf = BytesMut::new();

        // Encode Header first
        assert!(encoder.encode(FlvData::Header(header), &mut buf).is_ok());
        buf.clear(); // Clear buffer to only test the tag part

        // Encode the Tag
        let result = encoder.encode(FlvData::Tag(tag), &mut buf);
        assert!(result.is_ok());

        assert_eq!(buf.len(), expected_tag_structure_size);

        let expected_bytes = [
            // PreviousTagSize (should be 0 after header)
            0x00, 0x00, 0x00, 0x00, // Tag Header (11 bytes)
            0x09, // Type: Video
            0x00, 0x00, 0x04, // Data Size: 4
            0x00, 0x00, 0x64, // Timestamp: 100 (lower 24 bits)
            0x00, // Timestamp Extended: 0 (upper 8 bits)
            0x00, 0x00, 0x00, // Stream ID: 0
            // Tag Data (4 bytes)
            0xAA, 0xBB, 0xCC, 0xDD,
        ];
        assert_eq!(&buf[..], &expected_bytes);

        // Check state update
        assert_eq!(encoder.last_tag_size_written, expected_tag_size as u32);
    }

    #[test]
    fn test_encode_second_tag() {
        let mut encoder = FlvEncoder::default();
        let header = default_header();
        let tag1 = FlvTag::new(
            100,
            0,
            FlvTagType::Video,
            false,
            Bytes::from_static(&[0xAA, 0xBB, 0xCC, 0xDD]),
        );
        let tag1_size = TAG_HEADER_SIZE + 4; // 15

        let tag2 = FlvTag::new(
            120,
            0,
            FlvTagType::Audio,
            false,
            Bytes::from_static(&[0xEE, 0xFF]),
        );
        let tag2_data_len = 2;
        let expected_tag2_structure_size = PREV_TAG_FIELD_SIZE + TAG_HEADER_SIZE + tag2_data_len; // 4 + 11 + 2 = 17

        let mut buf = BytesMut::new();

        // Encode Header and Tag 1
        assert!(encoder.encode(FlvData::Header(header), &mut buf).is_ok());
        assert!(encoder.encode(FlvData::Tag(tag1), &mut buf).is_ok());
        assert_eq!(encoder.last_tag_size_written, tag1_size as u32); // State updated correctly
        buf.clear(); // Clear buffer to only test the second tag part

        // Encode Tag 2
        let result = encoder.encode(FlvData::Tag(tag2), &mut buf);
        assert!(result.is_ok());

        assert_eq!(buf.len(), expected_tag2_structure_size);

        let expected_bytes = [
            // PreviousTagSize (should be size of tag1 = 15 = 0x0F)
            0x00, 0x00, 0x00, 0x0F, // Tag Header (11 bytes)
            0x08, // Type: Audio
            0x00, 0x00, 0x02, // Data Size: 2
            0x00, 0x00, 0x78, // Timestamp: 120 (lower 24 bits)
            0x00, // Timestamp Extended: 0 (upper 8 bits)
            0x00, 0x00, 0x00, // Stream ID: 0
            // Tag Data (2 bytes)
            0xEE, 0xFF,
        ];
        assert_eq!(&buf[..], &expected_bytes);

        // Check state update after tag 2
        assert_eq!(
            encoder.last_tag_size_written,
            (TAG_HEADER_SIZE + tag2_data_len) as u32
        );
    }

    #[test]
    fn test_encode_tag_data_too_large_fails() {
        let mut encoder = FlvEncoder::default();
        let header = default_header();

        // Create data larger than 24 bits can represent
        let large_data = vec![0u8; MAX_TAG_DATA_SIZE + 1];
        let tag = FlvTag::new(100, 0, FlvTagType::Video, false, Bytes::from(large_data));

        let mut buf = BytesMut::new();
        assert!(encoder.encode(FlvData::Header(header), &mut buf).is_ok()); // Header is fine

        // Encode the large tag
        let result = encoder.encode(FlvData::Tag(tag), &mut buf);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("exceeds 24-bit limit"));
    }

    #[test]
    fn test_timestamp_extended() {
        let mut encoder = FlvEncoder::default();
        let header = default_header();
        let large_timestamp = 0x12345678; // Example timestamp needing extended byte
        let tag = FlvTag::new(
            large_timestamp,
            0,
            FlvTagType::Video,
            false,
            Bytes::from_static(&[0x01]),
        );
        let data_len = 1;
        let expected_tag_structure_size = PREV_TAG_FIELD_SIZE + TAG_HEADER_SIZE + data_len; // 4 + 11 + 1 = 16

        let mut buf = BytesMut::new();
        assert!(encoder.encode(FlvData::Header(header), &mut buf).is_ok());
        buf.clear();

        assert!(encoder.encode(FlvData::Tag(tag), &mut buf).is_ok());
        assert_eq!(buf.len(), expected_tag_structure_size);

        let expected_bytes = [
            // PreviousTagSize
            0x00, 0x00, 0x00, 0x00, // Bytes 0-3
            // Tag Header
            0x09, // Type                     Byte 4
            0x00, 0x00, 0x01, // Size         Bytes 5-7
            // Timestamp (Lower 24 bits: 0x345678) + Extended (Upper 8 bits: 0x12)
            // Order: TS[16-23], TS[8-15], TS[0-7], TS Extended[24-31]
            0x34, // Timestamp[16-23]         Byte 8  (Decimal 52) <-- CORRECTED
            0x56, // Timestamp[8-15]          Byte 9  (Decimal 86) <-- CORRECTED
            0x78, // Timestamp[0-7]           Byte 10 (Decimal 120)
            0x12, // TimestampExtended[24-31] Byte 11 (Decimal 18)
            // Stream ID
            0x00, 0x00, 0x00, //                 Bytes 12-14
            // Data
            0x01, //                         Byte 15
        ];
        assert_eq!(&buf[..], &expected_bytes);
        assert_eq!(
            encoder.last_tag_size_written,
            (TAG_HEADER_SIZE + data_len) as u32
        );
    }
}
