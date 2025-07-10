use crate::data::FlvData;
use bytes::{BufMut, BytesMut};
use std::io;
use tokio_util::codec::Encoder;

/// Maximum allowed data size for a single FLV tag payload (24 bits).
const MAX_TAG_DATA_SIZE: usize = 0xFFFFFF; // 16,777,215 bytes
const FLV_HEADER_SIZE: usize = 9;
const PREV_TAG_FIELD_SIZE: usize = 4;
const TAG_HEADER_SIZE: usize = 11;

/// Encodes `FlvData` (Header or Tag) into the FLV byte format.
///
/// This encoder maintains the necessary state (`last_tag_size_written`)
/// to correctly write the `PreviousTagSize` field before each tag.
/// It ensures the header is written only once and validates tag data size.
///
/// Use with `tokio_util::codec::FramedWrite` for buffered asynchronous writing.
#[derive(Debug, Default)]
pub struct FlvEncoder {
    /// Stores the total size (PreviousTagSize field + Header + Data)
    /// of the *last* tag structure written to the buffer.
    /// This is needed to write the correct `PreviousTagSize` for the *next* tag.
    last_tag_size_written: u32,
    /// Tracks if the FLV header has already been written.
    header_written: bool,
}

impl Encoder<FlvData> for FlvEncoder {
    type Error = io::Error;

    /// Encodes an `FlvData` item into the provided `BytesMut` buffer.
    ///
    /// - For `FlvData::Header`, writes the 9-byte FLV header. Must be the first item.
    /// - For `FlvData::Tag`, writes the 4-byte `PreviousTagSize`, the 11-byte tag header,
    ///   and the tag data payload. Requires the header to have been written previously.
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

                dst.reserve(FLV_HEADER_SIZE);

                // 1. FLV Signature (3 bytes)
                dst.put_slice(b"FLV");
                // 2. Version (1 byte)
                dst.put_u8(header.version);
                // 3. Flags (TypeFlags) (1 byte)
                let mut flags = 0u8;
                // Bit 0: Video tags are present
                if header.has_video {
                    flags |= 0x01;
                }
                // Bit 2: Audio tags are present
                if header.has_audio {
                    flags |= 0x04;
                }
                // Other bits (1, 3-7) must be 0
                dst.put_u8(flags);
                // 4. Data Offset (4 bytes, BigEndian)
                // Specifies the size of the header, usually 9.
                dst.put_u32(FLV_HEADER_SIZE as u32);

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

                let data_len = tag.data.len();

                // Validate tag data size fits within 24 bits
                if data_len > MAX_TAG_DATA_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "FLV tag data size ({data_len}) exceeds 24-bit limit ({MAX_TAG_DATA_SIZE})"
                        ),
                    ));
                }
                // Cast safely after check
                let data_len_u32 = data_len as u32;

                // Calculate total size for the entire structure being written NOW
                // (PrevTagSize field + Tag Header + Tag Data)
                let current_tag_structure_size = PREV_TAG_FIELD_SIZE + TAG_HEADER_SIZE + data_len;
                dst.reserve(current_tag_structure_size);

                // 1. Write PreviousTagSize Field (4 bytes, BigEndian)
                // This contains the size of the complete previous tag structure.
                dst.put_u32(self.last_tag_size_written);

                // 2. Write Tag Header (11 bytes)
                //    a. Tag Type (1 byte)
                dst.put_u8(tag.tag_type.into());

                //    b. Data Size (3 bytes, BigEndian)
                //       Size of the tag data payload *only*.
                dst.put_u8((data_len_u32 >> 16) as u8); // MSB
                dst.put_u8((data_len_u32 >> 8) as u8); // Middle
                dst.put_u8(data_len_u32 as u8); // LSB

                //    c. Timestamp (3 bytes, BigEndian) + Timestamp Extended (1 byte) = 4 bytes total
                //       Combined to form a 32-bit millisecond timestamp.
                let timestamp = tag.timestamp_ms;
                dst.put_u8((timestamp >> 16) as u8); // Timestamp lower bytes [16:23]
                dst.put_u8((timestamp >> 8) as u8); // Timestamp lower bytes [8:15]
                dst.put_u8(timestamp as u8); // Timestamp lower bytes [0:7]
                dst.put_u8((timestamp >> 24) as u8); // Timestamp upper bytes [24:31] (Extended)

                //    d. Stream ID (3 bytes, BigEndian, usually 0)
                dst.put_u8((tag.stream_id >> 16) as u8);
                dst.put_u8((tag.stream_id >> 8) as u8);
                dst.put_u8(tag.stream_id as u8);

                // 3. Write Tag Data (Variable size)
                //    Append the raw bytes payload efficiently.
                dst.put(tag.data);

                // --- Update State for Next Tag ---
                // The *next* tag's PreviousTagSize field needs the total size
                // of the structure we just finished writing.
                self.last_tag_size_written = current_tag_structure_size as u32;
                Ok(())
            }
            // Handle other FlvData variants if they exist
            #[allow(unreachable_patterns)] // Silence warning if FlvData only has Header/Tag
            _ => {
                // Example: Could error, log, or ignore unknown variants
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Unsupported FlvData variant for encoding",
                ))
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
        assert_eq!(buf.len(), FLV_HEADER_SIZE);
        assert_eq!(
            &buf[..],
            &[
                // FLV
                0x46, 0x4C, 0x56, // Version 1
                0x01, // Flags (Audio + Video)
                0x05, // Data Offset 9 (BigEndian)
                0x00, 0x00, 0x00, 0x09,
            ]
        );
        assert!(encoder.header_written);
        assert_eq!(encoder.last_tag_size_written, 0); // Reset after header
    }

    #[test]
    fn test_encode_tag_without_header_fails() {
        let mut encoder = FlvEncoder::default();
        let tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 100,
            stream_id: 0,
            data: Bytes::from_static(&[0x01, 0x02]),
        };
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
        assert_eq!(buf.len(), FLV_HEADER_SIZE);

        // Second encode should fail
        let result = encoder.encode(FlvData::Header(header), &mut buf);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("written once"));
        assert_eq!(buf.len(), FLV_HEADER_SIZE); // Buffer unchanged by second call
    }

    #[test]
    fn test_encode_first_tag() {
        let mut encoder = FlvEncoder::default();
        let header = default_header();
        let tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 100, // 0x64
            stream_id: 0,
            data: Bytes::from_static(&[0xAA, 0xBB, 0xCC, 0xDD]), // 4 bytes data
        };
        let data_len = 4;
        let expected_tag_structure_size = PREV_TAG_FIELD_SIZE + TAG_HEADER_SIZE + data_len; // 4 + 11 + 4 = 19

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
        assert_eq!(
            encoder.last_tag_size_written,
            expected_tag_structure_size as u32
        );
    }

    #[test]
    fn test_encode_second_tag() {
        let mut encoder = FlvEncoder::default();
        let header = default_header();
        let tag1 = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 100,
            stream_id: 0,
            data: Bytes::from_static(&[0xAA, 0xBB, 0xCC, 0xDD]), // 4 bytes data
        };
        let tag1_structure_size = PREV_TAG_FIELD_SIZE + TAG_HEADER_SIZE + 4; // 19

        let tag2 = FlvTag {
            tag_type: FlvTagType::Audio,
            timestamp_ms: 120, // 0x78
            stream_id: 0,
            data: Bytes::from_static(&[0xEE, 0xFF]), // 2 bytes data
        };
        let tag2_data_len = 2;
        let expected_tag2_structure_size = PREV_TAG_FIELD_SIZE + TAG_HEADER_SIZE + tag2_data_len; // 4 + 11 + 2 = 17

        let mut buf = BytesMut::new();

        // Encode Header and Tag 1
        assert!(encoder.encode(FlvData::Header(header), &mut buf).is_ok());
        assert!(encoder.encode(FlvData::Tag(tag1), &mut buf).is_ok());
        assert_eq!(encoder.last_tag_size_written, tag1_structure_size as u32); // State updated correctly
        buf.clear(); // Clear buffer to only test the second tag part

        // Encode Tag 2
        let result = encoder.encode(FlvData::Tag(tag2), &mut buf);
        assert!(result.is_ok());

        assert_eq!(buf.len(), expected_tag2_structure_size);

        let expected_bytes = [
            // PreviousTagSize (should be size of tag1 structure = 19 = 0x13)
            0x00, 0x00, 0x00, 0x13, // Tag Header (11 bytes)
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
            expected_tag2_structure_size as u32
        );
    }

    #[test]
    fn test_encode_tag_data_too_large_fails() {
        let mut encoder = FlvEncoder::default();
        let header = default_header();

        // Create data larger than 24 bits can represent
        let large_data = vec![0u8; MAX_TAG_DATA_SIZE + 1];
        let tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 100,
            stream_id: 0,
            data: Bytes::from(large_data),
        };

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
        let tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: large_timestamp,
            stream_id: 0,
            data: Bytes::from_static(&[0x01]), // 1 byte data
        };
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
            expected_tag_structure_size as u32
        );
    }
}

// --- Example Usage (Conceptual - Requires async runtime and dependencies) ---
/*
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter}; // Need AsyncWriteExt for BufWriter flush
use tokio_util::codec::FramedWrite;
use futures::stream::{self, StreamExt};
use futures::sink::SinkExt;
use std::error::Error;

async fn write_example() -> Result<(), Box<dyn Error>> {
    // 1. Create some sample data
    let header = FlvHeader { version: 1, has_audio: true, has_video: true, data_offset: 9 };
    let tags = vec![
        FlvTag {
            tag_type: FlvTagType::ScriptData, // Usually the first tag
            timestamp_ms: 0,
            stream_id: 0,
            // A minimal valid onMetaData structure
            data: Bytes::from_static(&[
                0x02, 0x00, 0x0A, // String "onMetaData"
                b'o', b'n', b'M', b'e', b't', b'a', b'D', b'a', b't', b'a',
                0x08, 0x00, 0x00, 0x00, 0x00, // ECMA Array (0 elements)
                0x00, 0x00, 0x09 // End marker
            ]),
        },
        FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 40,
            stream_id: 0,
            data: Bytes::from_static(&[0x17, 0x01, 0x00, 0x00, 0x00, 0xde, 0xad, 0xbe, 0xef]), // Example video data
        },
        FlvTag {
            tag_type: FlvTagType::Audio,
            timestamp_ms: 42,
            stream_id: 0,
            data: Bytes::from_static(&[0xaf, 0x01, 0xfe, 0xed]), // Example audio data
        },
    ];

    // Convert Vec into a Stream of Result<FlvData, io::Error>
    // Must be Box<dyn Error + Send + Sync> for send_all if error type isn't Unpin+Sized
    type BoxedError = Box<dyn Error + Send + Sync>;
    let header_item = FlvData::Header(header);
    let tag_items = tags.into_iter().map(FlvData::Tag);
    let data_stream = stream::iter(std::iter::once(Ok(header_item)).chain(tag_items.map(Ok)))
                          .map(|res| res.map_err(|e| Box::new(e) as BoxedError)); // Map error type if needed


    // 2. Set up the writer
    let file = File::create("output_example.flv").await?;
    // Using BufWriter is often beneficial even with FramedWrite for underlying OS efficiency
    let writer = BufWriter::new(file);

    // 3. Create the FramedWrite sink using our FlvEncoder
    // Map the error type of the encoder if it doesn't match the stream's error type
    let mut framed_writer = FramedWrite::new(writer, FlvEncoder::default())
                               .with(|err: io::Error| Box::new(err) as BoxedError); // Map sink error type


    // 4. Send the stream data into the sink
    // `send_all` drives the stream and calls the encoder internally.
    // FramedWrite handles buffering.
    println!("Writing FLV data to output_example.flv...");
    match framed_writer.send_all(&mut data_stream.boxed()).await {
         Ok(_) => {
             println!("Stream finished successfully.");
         }
         Err(e) => {
             eprintln!("Error writing FLV stream: {}", e);
             // Handle error appropriately
             return Err(e);
         }
     }

    // 5. Crucially, flush the underlying writer (especially BufWriter)
    // `into_inner` gets the BufWriter back from FramedWrite.
    println!("Flushing writer...");
    let mut inner_writer = framed_writer.into_inner();
    inner_writer.flush().await?; // Flush BufWriter's internal buffer
    println!("Write complete.");

    Ok(())
}

// To run this example:
// #[tokio::main]
// async fn main() {
//     if let Err(e) = write_example().await {
//         eprintln!("Example failed: {}", e);
//     }
// }
*/
