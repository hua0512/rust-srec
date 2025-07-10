use crate::data::FlvData;
use crate::error::FlvError;
use crate::header::FlvHeader;
use crate::tag::FlvTag;
use bytes::{Buf, BytesMut};
use futures::{Stream, StreamExt};
use std::{
    io::Cursor,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::AsyncRead;
use tokio_util::codec::{Decoder, FramedRead};
use tracing::{debug, error, trace, warn};

// 8 KiB buffer size for reading
const BUFFER_SIZE: usize = 8 * 1024;
// 16 MiB sanity limit for tag data size
const MAX_TAG_DATA_SIZE: u32 = 16 * 1024 * 1024;
const FLV_HEADER_SIZE: usize = 9;
const PREV_TAG_SIZE_FIELD_SIZE: usize = 4;
const TAG_HEADER_SIZE: usize = 11;
// Need at least a tag header and the *next* prev tag size
const MIN_REQUIRED_AFTER_RESYNC: usize = TAG_HEADER_SIZE + PREV_TAG_SIZE_FIELD_SIZE;

/// An FLV format decoder that implements Tokio's Decoder trait
#[derive(Default)]
pub struct FlvDecoder {
    header_parsed: bool,

    // Tracks if we are expecting the 4-byte PreviousTagSize field before the next tag header.
    // True initially (after header) and after successfully reading a PreviousTagSize.
    // False after successfully parsing a tag (meaning the *next* item should be PreviousTagSize).
    expecting_tag_header: bool,
    // Stores the size of the last successfully parsed tag, useful for potential validation
    last_tag_size: u32,
}

impl FlvDecoder {
    // Helper function to attempt resynchronization by finding the next potential tag start
    // Returns true if resync advanced the buffer, false otherwise.
    fn try_resync(&mut self, src: &mut BytesMut) -> bool {
        // Look for the next potential tag start (type 8, 9, or 18)
        if let Some(pos) = src.iter().position(|&b| b == 8 || b == 9 || b == 18) {
            // Discard bytes before the potential tag start
            src.advance(pos);
            debug!(
                "Resync: Found potential tag start after skipping {} bytes. Remaining buffer: {}",
                pos,
                src.len()
            );
            // After skipping, we are positioned at a potential tag type byte.
            // We implicitly skipped whatever was before it, including any PreviousTagSize field.
            // We now expect a tag header directly.
            self.expecting_tag_header = true;
            // We lost context, so reset last tag size knowledge
            self.last_tag_size = 0;
            true
        } else {
            // No potential tag start found in the current buffer, discard it all.
            let discarded_len = src.len();
            src.clear();
            // Request more data by reserving a minimal amount
            src.reserve(BUFFER_SIZE);
            warn!(
                "Resync: No potential tag start found. Discarded {} bytes.",
                discarded_len
            );
            // Still expecting a tag header when new data arrives
            self.expecting_tag_header = true;
            // We lost context
            self.last_tag_size = 0;
            false
        }
    }
}

impl Decoder for FlvDecoder {
    type Item = FlvData;
    // Use our custom FlvError type instead of std::io::Error
    type Error = FlvError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        trace!(
            "Decode called with {} bytes in buffer. State: header_parsed={}, expecting_tag_header={}",
            src.len(),
            self.header_parsed,
            self.expecting_tag_header
        );

        // --- 1. Parse Header (if needed) ---
        if !self.header_parsed {
            let expected_size = FLV_HEADER_SIZE + PREV_TAG_SIZE_FIELD_SIZE;
            if src.len() < expected_size {
                trace!("Awaiting FLV header ({} bytes needed)", FLV_HEADER_SIZE);
                src.reserve(FLV_HEADER_SIZE - src.len());
                return Ok(None);
            }

            let header_bytes = src.split_to(FLV_HEADER_SIZE);
            let mut cursor = Cursor::new(&header_bytes[..]); // Borrow slice temporarily

            match FlvHeader::parse(&mut cursor) {
                Ok(header) => {
                    debug!("Successfully parsed FLV header: {:?}", header);
                    // Skip PrevTagSize field (4 bytes) after header
                    src.advance(PREV_TAG_SIZE_FIELD_SIZE);
                    self.header_parsed = true;
                    self.expecting_tag_header = true; // After header, expect first tag
                    self.last_tag_size = 0; // Header is preceded by 0 size
                    return Ok(Some(FlvData::Header(header)));
                }
                Err(e) => {
                    error!("Failed to parse FLV header: {:?}", e);
                    // Header is fundamental, failure here is critical.
                    return Err(FlvError::InvalidHeader);
                }
            }
        }

        // --- Loop to handle multiple tags/skips within the available buffer ---

        trace!(
            "Decode loop iteration. Buffer size: {}. State: expecting_tag_header={}",
            src.len(),
            self.expecting_tag_header
        );

        // --- 2. Handle Previous Tag Size (if needed) ---
        if !self.expecting_tag_header {
            if src.len() < PREV_TAG_SIZE_FIELD_SIZE {
                trace!(
                    "Awaiting PreviousTagSize field ({} bytes needed)",
                    PREV_TAG_SIZE_FIELD_SIZE
                );
                src.reserve(PREV_TAG_SIZE_FIELD_SIZE - src.len());
                return Ok(None); // Need more data for the prev tag size
            }

            // Read and potentially validate PreviousTagSize
            // We create a slice without consuming yet, in case validation fails later
            let prev_tag_size_bytes = &src[..PREV_TAG_SIZE_FIELD_SIZE];
            let prev_tag_size = u32::from_be_bytes([
                prev_tag_size_bytes[0],
                prev_tag_size_bytes[1],
                prev_tag_size_bytes[2],
                prev_tag_size_bytes[3],
            ]);

            // Optional validation: Check if it matches the size of the *last* parsed tag.
            // FLV muxers *should* set this correctly. Discrepancies indicate potential issues.
            if self.last_tag_size > 0 && prev_tag_size != self.last_tag_size {
                warn!(
                    "PreviousTagSize mismatch: Expected {}, found {}. Stream might be corrupted.",
                    self.last_tag_size, prev_tag_size
                );
                // Decide whether to bail out or try to continue. Let's try to continue for now.
                // If we wanted strict checking, we could return Err here or trigger resync.
                // Investigation shows the some streams have this mismatch (script tags with incorrect size).
            } else {
                trace!("Read PreviousTagSize: {}", prev_tag_size);
            }

            // Consume the PreviousTagSize field
            src.advance(PREV_TAG_SIZE_FIELD_SIZE);
            self.expecting_tag_header = true; // Now we expect the tag header
        }

        // --- 3. Parse Tag Header ---
        if src.len() < TAG_HEADER_SIZE {
            trace!("Awaiting Tag Header ({} bytes needed)", TAG_HEADER_SIZE);
            // Try to reserve enough for header and potentially a small tag + next prev size
            src.reserve(MIN_REQUIRED_AFTER_RESYNC.saturating_sub(src.len()));
            return Ok(None); // Need more data for the tag header
        }

        // Peek at the tag header without consuming yet
        let tag_type_byte = src[0];
        let data_size_bytes = &src[1..4];
        let data_size = u32::from_be_bytes([
            0,
            data_size_bytes[0],
            data_size_bytes[1],
            data_size_bytes[2],
        ]);

        // --- 4. Validate Tag Header ---
        let tag_type = tag_type_byte & 0x1F; // Lower 5 bits for type (ignore filter bit for now)
        if tag_type != 8 && tag_type != 9 && tag_type != 18 {
            warn!(
                "Invalid tag type encountered: {}. Attempting resync.",
                tag_type_byte
            );
            // Discard the single invalid byte and try resyncing
            // src.advance(1);
            // Now attempt resync on the rest
            if !self.try_resync(src) {
                // Resync advanced the buffer. Return None to signal progress
                // but no complete frame yet from *this* specific call point.
                // The next call to decode will attempt parsing from the new position.
                return Ok(None);
            } else {
                // Resync cleared the buffer or couldn't find anything.
                // Need more data. try_resync already reserved space.
                trace!("Resync failed or cleared buffer, returning None for more data.");
                return Ok(None);
            }
        }

        if data_size > MAX_TAG_DATA_SIZE {
            warn!(
                "Unusually large tag data size: {} (max allowed: {}). Skipping tag header and attempting resync.",
                data_size, MAX_TAG_DATA_SIZE
            );
            // Discard the invalid tag header
            src.advance(TAG_HEADER_SIZE);
            self.last_tag_size = 0; // Lost context
            // Return None here as well to indicate progress (skipping header)
            // without producing a full item. Let the next call handle PreviousTagSize.
            trace!("Skipped large tag header, returning None to yield.");
            return Ok(None);
        }

        // --- 5. Check for Full Tag Data ---
        let total_tag_size = TAG_HEADER_SIZE + data_size as usize;
        if src.len() < total_tag_size {
            trace!(
                "Awaiting full tag data ({} bytes needed, have {})",
                total_tag_size,
                src.len()
            );
            src.reserve(total_tag_size - src.len());
            return Ok(None); // Need more data for the tag body
        }

        // --- 6. Demux Tag ---
        // We have the full tag. Create a Bytes slice containing the *entire* tag.
        let tag_bytes = src.split_to(total_tag_size).freeze();
        // Cursor now owns the tag's Bytes
        let mut cursor = Cursor::new(tag_bytes);

        match FlvTag::demux(&mut cursor) {
            // Use the FlvUtil<FlvTag> implementation
            Ok(tag) => {
                trace!(
                    "Successfully parsed FLV tag: Type={}, Timestamp={}, Size={}",
                    tag.tag_type, tag.timestamp_ms, data_size
                );
                // Store the *full* size of the tag (header + data) for the next PreviousTagSize check
                self.last_tag_size = total_tag_size as u32;
                // After a successful tag, we expect the PreviousTagSize field next
                self.expecting_tag_header = false;
                Ok(Some(FlvData::Tag(tag))) // Successfully decoded a tag
            }
            Err(e) => {
                // Demux failed (e.g., bad data *within* the tag body, or unexpected EOF *within* demux)
                warn!(
                    "Failed to demux FLV tag (type: {}, data_size: {}): {:?}. Discarded {} bytes.",
                    tag_type, data_size, e, total_tag_size
                );
                // `split_to` already removed the bytes from `src`.
                // We failed parsing, so the next item should be PreviousTagSize, but we don't trust the stream.
                self.expecting_tag_header = false; // Expect PreviousTagSize next, potentially bad one
                self.last_tag_size = 0; // Can't trust the size
                // Return None to signal progress (discarded bad tag)
                // without producing a full item. Let the next call handle PreviousTagSize.
                trace!("Demux failed, returning None to yield after discarding tag.");
                Ok(None)
            }
        }
    }
}

pub struct FlvDecoderStream<R> {
    framed: FramedRead<R, FlvDecoder>,
}

impl<R: AsyncRead + Unpin> FlvDecoderStream<R> {
    pub fn new(reader: R) -> Self {
        Self {
            framed: FramedRead::with_capacity(reader, FlvDecoder::default(), BUFFER_SIZE),
        }
    }

    pub fn with_capacity(reader: R, capacity: usize) -> Self {
        Self {
            framed: FramedRead::with_capacity(reader, FlvDecoder::default(), capacity),
        }
    }
}

impl<R: AsyncRead + Unpin> Stream for FlvDecoderStream<R> {
    type Item = Result<FlvData, FlvError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.framed.poll_next_unpin(cx)
    }
}

pub struct FlvParser;

impl FlvParser {
    /// Create a stream using Tokio's Decoder interface
    pub async fn create_decoder_stream(
        path: &Path,
    ) -> Result<impl Stream<Item = Result<FlvData, FlvError>>, std::io::Error> {
        let file = tokio::fs::File::open(path).await?;
        // Consider using BufReader for potentially better performance with File I/O
        let reader = tokio::io::BufReader::new(file);

        Ok(FlvDecoderStream::new(reader))
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::tag::{FlvTagType, FlvUtil};
    use bytes::BytesMut;
    use futures::TryStreamExt;
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::time::Instant;

    // Helper to initialize tracing for tests
    fn init_tracing() {
        let _ = tracing_subscriber::fmt::try_init();
    }

    #[test]
    fn test_decode_header_ok() {
        init_tracing();
        let mut decoder = FlvDecoder::default();
        let mut buffer = BytesMut::from(
            &[
                0x46, 0x4C, 0x56, // "FLV"
                0x01, // Version 1
                0x05, // Flags: Audio + Video
                0x00, 0x00, 0x00, 0x09, // Header size 9
                // Next part (start of first tag prev size)
                0x00, 0x00, 0x00, 0x00,
            ][..],
        );

        let result = decoder.decode(&mut buffer);
        assert!(result.is_ok());
        let item = result.unwrap();
        assert!(item.is_some());
        match item.unwrap() {
            FlvData::Header(h) => {
                assert_eq!(h.version, 1);
                assert!(h.has_audio);
                assert!(h.has_video);
            }
            _ => panic!("Expected Header"),
        }
        assert!(decoder.header_parsed);
        assert!(decoder.expecting_tag_header); // Expecting first tag header next
        assert_eq!(buffer.len(), 0); // Header + PrevTagSize consumed
    }

    #[test]
    fn test_decode_header_incomplete() {
        init_tracing();
        let mut decoder = FlvDecoder::default();
        let mut buffer = BytesMut::from(
            &[
                0x46, 0x4C, 0x56, 0x01, 0x05, 0x00, 0x00, 0x00, // Only 8 bytes
            ][..],
        );

        let result = decoder.decode(&mut buffer);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none()); // Expect None (need more data)
        assert!(!decoder.header_parsed);
        assert!(buffer.capacity() >= FLV_HEADER_SIZE); // Should have reserved
    }

    #[test]
    fn test_decode_header_invalid() {
        init_tracing();
        let mut decoder = FlvDecoder::default();
        let mut buffer = BytesMut::from(
            &[
                0x46, 0x4C, 0x57, // Invalid signature 'W'
                0x01, 0x05, 0x00, 0x00, 0x00, 0x09,
                // Next part (start of first tag prev size)
                0x00, 0x00, 0x00, 0x00,
            ][..],
        );

        let result = decoder.decode(&mut buffer);
        assert!(result.is_err());
        match result.err().unwrap() {
            FlvError::InvalidHeader => {} // Expected error
            e => panic!("Unexpected error type: {e:?}"),
        }
    }

    #[test]
    fn test_flv_decoder_stream() {
        let data = vec![
            0x46, 0x4C, 0x56, // "FLV" signature
            0x01, // version
            0x05, // flags (audio and video)
            0x00, 0x00, 0x00, 0x09, // data offset (9 bytes)
            // FLV tag header (11 bytes)
            0x09, // tag type (video)
            0x00, 0x00, 0x00, 0x00, // data size (0 bytes for header)
            0x00, 0x00, 0x00, 0x00, // timestamp (0 ms)
            0x00, // stream ID (always 0)
        ];

        let cursor = Cursor::new(data);
        let mut decoder_stream = FlvDecoderStream::new(cursor);

        let result = futures::executor::block_on(decoder_stream.next());

        assert!(result.is_some());
        if let Some(Ok(FlvData::Header(header))) = result {
            assert_eq!(header.version, 1);
        } else {
            panic!("Expected FLV header");
        }
    }

    #[test]
    fn test_decode_first_tag_ok() {
        init_tracing();
        let mut decoder = FlvDecoder::default();
        let mut buffer = BytesMut::new();

        // 1. Provide Header
        buffer.extend_from_slice(&[
            0x46, 0x4C, 0x56, 0x01, 0x05, 0x00, 0x00, 0x00, 0x09, // Header
            // Next part (start of first tag prev size)
            0x00, 0x00, 0x00, 0x00,
        ]);
        let header_res = decoder.decode(&mut buffer).unwrap().unwrap();
        assert!(matches!(header_res, FlvData::Header(_)));
        assert_eq!(buffer.len(), 0); // Consumed header

        // 2. Provide First PreviousTagSize (always 0) + Tag Header + Tag Data + Next PreviousTagSize
        buffer.extend_from_slice(&[
            // Previous Tag Size (0 for header) - This should be handled implicitly after header
            //0x00, 0x00, 0x00, 0x00, // <-- Decoder logic skips reading this after header

            // Tag 1: Script Data (type 18), 15 bytes data, timestamp 0
            0x12, // Type 18 (Script)
            0x00, 0x00, 0x0F, // Data Size 15
            0x00, 0x00, 0x00, // Timestamp 0 (lower 24 bits)
            0x00, // Timestamp extension
            0x00, 0x00, 0x00, // Stream ID 0
            // Data (15 bytes, example AMF data)
            0x02, 0x00, 0x0A, b'o', b'n', b'M', b'e', b't', b'a', b'D', b'a', b't', b'a', 0x08,
            0x00, // End marker omitted for simplicity, assuming demux handles it
            // Previous Tag Size for Tag 1 (Header=11 + Data=15 = 26 = 0x1A)
            0x00, 0x00, 0x00, 0x1A,
        ]);

        let tag1_res = decoder.decode(&mut buffer);
        assert!(tag1_res.is_ok());
        let tag1_opt = tag1_res.unwrap();
        assert!(tag1_opt.is_some());

        match tag1_opt.unwrap() {
            FlvData::Tag(tag) => {
                assert_eq!(tag.tag_type, FlvTagType::ScriptData);
                assert_eq!(tag.timestamp_ms, 0);
                assert_eq!(tag.data.len(), 15);
                assert_eq!(decoder.last_tag_size, 11 + 15); // Header + Data
            }
            _ => panic!("Expected Tag"),
        }
        assert!(!decoder.expecting_tag_header); // Expecting prev tag size next
        assert_eq!(buffer.len(), 4); // Remaining bytes should be the next PreviousTagSize
        assert_eq!(&buffer[..], &[0x00, 0x00, 0x00, 0x1A]);
    }

    #[tokio::test]
    async fn test_flv_parser_invalid() {
        let path = Path::new("invalid.flv");
        let parser = FlvParser::create_decoder_stream(path).await;
        assert!(parser.is_err());
    }

    #[test]
    fn test_decode_second_tag_after_prev_size() {
        init_tracing();
        let mut decoder = FlvDecoder::default();
        let mut buffer = BytesMut::new();

        // --- Simulate state after first tag ---
        decoder.header_parsed = true;
        decoder.expecting_tag_header = false; // Expecting prev tag size
        decoder.last_tag_size = 11 + 15; // Size of the "first" tag (ScriptData example)
        buffer.extend_from_slice(&[
            // Previous Tag Size for Tag 1 (Header=11 + Data=15 = 26 = 0x1A)
            0x00, 0x00, 0x00, 0x1A,
            // Tag 2: Video Data (type 9), 5 bytes data, timestamp 100ms
            0x09, // Type 9 (Video)
            0x00, 0x00, 0x05, // Data Size 5
            0x00, 0x00, 0x64, // Timestamp 100ms (lower 24 bits)
            0x00, // Timestamp extension
            0x00, 0x00, 0x00, // Stream ID 0
            // Data (5 bytes)
            0x17, 0x01, 0x02, 0x03, 0x04,
            // Previous Tag Size for Tag 2 (Header=11 + Data=5 = 16 = 0x10)
            0x00, 0x00, 0x00, 0x10,
        ]);
        // --- End Simulate ---

        let tag2_res = decoder.decode(&mut buffer);
        assert!(tag2_res.is_ok(), "Decode failed: {:?}", tag2_res.err());
        let tag2_opt = tag2_res.unwrap();
        assert!(tag2_opt.is_some(), "Decoder returned None unexpectedly");

        match tag2_opt.unwrap() {
            FlvData::Tag(tag) => {
                assert_eq!(tag.tag_type, FlvTagType::Video);
                assert_eq!(tag.timestamp_ms, 100);
                assert_eq!(tag.data.len(), 5);
                assert_eq!(decoder.last_tag_size, 11 + 5); // Header + Data
            }
            _ => panic!("Expected Tag"),
        }
        assert!(!decoder.expecting_tag_header); // Expecting next prev tag size
        assert_eq!(buffer.len(), 4); // Remaining bytes should be the next PreviousTagSize
        assert_eq!(&buffer[..], &[0x00, 0x00, 0x00, 0x10]);
    }

    #[test]
    fn test_decode_invalid_tag_type_resync() {
        init_tracing();
        let mut decoder = FlvDecoder::default();
        let mut buffer = BytesMut::new();

        // Simulate state after header
        decoder.header_parsed = true;
        decoder.expecting_tag_header = true;
        decoder.last_tag_size = 0;

        buffer.extend_from_slice(&[
            // Garbage bytes then invalid tag type
            0xFF, 0xFF, 0x07, // Invalid Tag Type (7)
            0x00, 0x00, 0x0A, // Fake Size
            0x00, 0x00, 0x00, 0x00, 0x00, // Fake Header Rest
            // Some more garbage
            0xAA, 0xBB, 0xCC, // Valid Tag Header Start (Video)
            0x09, // Type 9 (Video)
            0x00, 0x00, 0x05, // Data Size 5
            0x00, 0x00, 0xC8, // Timestamp 200ms
            0x00, // Timestamp extension
            0x00, 0x00, 0x00, // Stream ID 0
            // Data (5 bytes)
            0x11, 0x22, 0x33, 0x44, 0x55, // Next Prev Tag Size (16 = 0x10)
            0x00, 0x00, 0x00, 0x10,
        ]);

        // First decode attempt should hit invalid type and resync
        let resync_res = decoder.decode(&mut buffer);
        assert!(resync_res.is_ok());
        // Resync should skip garbage and the bad type, returning None for this call
        assert!(resync_res.unwrap().is_none());
        // Buffer should now start at the valid tag header (0x09)
        assert_eq!(buffer[0], 0x09);
        assert!(decoder.expecting_tag_header); // Resync should set this

        // Second decode attempt should parse the valid tag
        let tag_res = decoder.decode(&mut buffer);
        assert!(tag_res.is_ok(), "Decode failed: {:?}", tag_res.err());
        let tag_opt = tag_res.unwrap();
        assert!(tag_opt.is_some(), "Decoder returned None unexpectedly");

        match tag_opt.unwrap() {
            FlvData::Tag(tag) => {
                assert_eq!(tag.tag_type, FlvTagType::Video);
                assert_eq!(tag.timestamp_ms, 200);
                assert_eq!(tag.data.len(), 5);
                assert_eq!(decoder.last_tag_size, 11 + 5);
            }
            _ => panic!("Expected Tag"),
        }
        assert!(!decoder.expecting_tag_header); // Parsed tag, expect prev size next
        assert_eq!(buffer.len(), 4); // Should have next prev tag size remaining
        assert_eq!(&buffer[..], &[0x00, 0x00, 0x00, 0x10]);
    }

    #[test]
    fn test_decode_incomplete_tag_data_arrival() {
        init_tracing();
        let mut decoder = FlvDecoder::default();
        let mut buffer = BytesMut::new();

        // Simulate state after header is parsed
        decoder.header_parsed = true;
        decoder.expecting_tag_header = true; // Expecting first tag header
        decoder.last_tag_size = 0;

        // Tag Header indicates 5 bytes of data
        let tag_header = &[
            0x09, // Type 9 (Video)
            0x00, 0x00, 0x05, // Data Size 5
            0x00, 0x01, 0x00, // Timestamp 256ms
            0x00, // Timestamp extension
            0x00, 0x00, 0x00, // Stream ID 0
        ];
        let expected_data_size = 5;
        let total_tag_size = tag_header.len() + expected_data_size; // 11 + 5 = 16 bytes

        // 1. Provide only the tag header (11 bytes)
        buffer.extend_from_slice(tag_header);
        trace!("Test buffer state 1: {:?}", buffer);

        let res1 = decoder.decode(&mut buffer);
        assert!(
            res1.is_ok(),
            "Decode failed on header only: {:?}",
            res1.err()
        );
        // Expect None because data size (5) > available data (0 after header)
        assert!(res1.unwrap().is_none(), "Expected None (need tag data)");
        // Check that it reserved space (capacity check can be brittle, focus on need more data)
        // assert!(buffer.capacity() >= total_tag_size);
        assert_eq!(
            buffer.len(),
            tag_header.len(),
            "Buffer length should be unchanged"
        ); // Buffer unchanged

        // 2. Simulate providing *some* but not all data (e.g., 3 bytes)
        let partial_data1 = &[0x01, 0x02, 0x03];
        buffer.extend_from_slice(partial_data1); // Now buffer has 11 + 3 = 14 bytes
        trace!("Test buffer state 2: {:?}", buffer);

        let res2 = decoder.decode(&mut buffer);
        assert!(
            res2.is_ok(),
            "Decode failed on partial data: {:?}",
            res2.err()
        );
        // Still needs 16 bytes total, only has 14
        assert!(
            res2.unwrap().is_none(),
            "Expected None (still need more tag data)"
        );
        assert_eq!(
            buffer.len(),
            tag_header.len() + partial_data1.len(),
            "Buffer length should reflect added data"
        );

        // 3. Simulate providing the rest of the data (2 more bytes)
        let partial_data2 = &[0x04, 0x05];
        buffer.extend_from_slice(partial_data2); // Now buffer has 14 + 2 = 16 bytes
        trace!("Test buffer state 3: {:?}", buffer);

        let res3 = decoder.decode(&mut buffer);
        assert!(
            res3.is_ok(),
            "Decode failed on complete data: {:?}",
            res3.err()
        );
        let tag_opt = res3.unwrap();
        assert!(
            tag_opt.is_some(),
            "Expected Some(Tag) now that data is complete"
        );

        // 4. Verify the tag was parsed correctly
        match tag_opt.unwrap() {
            FlvData::Tag(tag) => {
                assert_eq!(tag.tag_type, FlvTagType::Video);
                assert_eq!(tag.timestamp_ms, 256);
                assert_eq!(tag.data.len(), expected_data_size);
                // Verify the data content combines the partial arrivals
                assert_eq!(&tag.data[..], &[0x01, 0x02, 0x03, 0x04, 0x05]);
                assert_eq!(
                    decoder.last_tag_size, total_tag_size as u32,
                    "Last tag size mismatch"
                );
            }
            other => panic!("Expected FlvData::Tag, got {other:?}"),
        }

        // 5. Buffer should be empty now as the complete tag was consumed
        assert!(
            buffer.is_empty(),
            "Buffer should be empty after consuming tag"
        );
        // State should be ready for the next PreviousTagSize
        assert!(
            !decoder.expecting_tag_header,
            "Should be expecting prev tag size next"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_read_file_async() -> Result<(), Box<dyn std::error::Error>> {
        init_tracing(); // Initialize tracing
        let path = Path::new("D:/test/999/testHEVC.flv");

        if !path.exists() {
            println!(
                "Test file not found, skipping test_read_file_async: {}",
                path.display()
            );
            return Ok(());
        }

        let file_size = std::fs::metadata(path)?.len();
        let file_size_mb = file_size as f64 / (1024.0 * 1024.0);
        println!("Starting async parse. File size: {file_size_mb:.2} MB");

        let start = Instant::now();
        let stream = FlvDecoderStream::with_capacity(
            tokio::io::BufReader::new(tokio::fs::File::open(path).await?),
            32 * 1024,
        );

        // Consume the stream using try_fold for aggregation
        struct ParseStats {
            has_header: bool,
            tags_count: u64,
            video_tags: u64,
            audio_tags: u64,
            metadata_tags: u64,
            last_timestamp: u32,
        }

        let initial_stats = ParseStats {
            has_header: false,
            tags_count: 0,
            video_tags: 0,
            audio_tags: 0,
            metadata_tags: 0,
            last_timestamp: 0,
        };

        let final_stats = stream
            .try_fold(initial_stats, |mut stats, item| async {
                match item {
                    FlvData::Header(_) => {
                        stats.has_header = true;
                    }
                    FlvData::Tag(tag) => {
                        stats.tags_count += 1;
                        stats.last_timestamp = tag.timestamp_ms; // Track last timestamp
                        match tag.tag_type {
                            FlvTagType::Video => stats.video_tags += 1,
                            FlvTagType::Audio => stats.audio_tags += 1,
                            FlvTagType::ScriptData => stats.metadata_tags += 1,
                            FlvTagType::Unknown(_) => {}
                        }
                        if stats.tags_count % 50000 == 0 {
                            // Log progress less frequently
                            debug!(
                                "Processed {} tags... Last timestamp: {}",
                                stats.tags_count, stats.last_timestamp
                            );
                        }
                    }
                    FlvData::EndOfSequence(_) => {
                        // Handle end of sequence if needed
                    }
                }
                Ok(stats)
            })
            .await; // Handle potential stream error here

        let duration = start.elapsed();
        let seconds = duration.as_secs_f64();
        let speed_mbps = if seconds > 0.0 {
            file_size_mb / seconds
        } else {
            0.0
        };

        println!("-----------------------------------------");
        println!("Async Parse Results:");
        println!("Parsed FLV file asynchronously in {duration:?}");
        println!("File size: {file_size_mb:.2} MB");
        println!("Read speed: {speed_mbps:.2} MB/s");

        match final_stats {
            Ok(stats) => {
                println!("Header found: {}", stats.has_header);
                println!("Total tags parsed: {}", stats.tags_count);
                println!(
                    "Tag Types: Audio={}, Video={}, Metadata={}",
                    stats.audio_tags, stats.video_tags, stats.metadata_tags
                );
                println!("Last tag timestamp: {} ms", stats.last_timestamp);
                println!("Errors encountered during fold: 0"); // try_fold stops on first Err

                assert!(stats.has_header, "Expected to parse an FLV header");
                // Allow zero tags only if file is truly empty/corrupt
                if file_size > (FLV_HEADER_SIZE as u64) {
                    assert!(
                        stats.tags_count > 0,
                        "Expected to parse tags from non-empty file"
                    );
                }
            }
            Err(e) => {
                println!("Stream processing stopped due to error: {e:?}");
                // You might want to assert the error type or context depending on needs
                return Err(e.into()); // Propagate the error
            }
        }
        println!("-----------------------------------------");

        Ok(())
    }

    #[tokio::test]
    #[ignore] // This test requires a specific file to be present
    async fn test_compare_parsers() -> Result<(), Box<dyn std::error::Error>> {
        init_tracing(); // Initialize tracing
        let path = Path::new("D:/test/999/test.flv");

        // Skip the test if the file doesn't exist
        if !path.exists() {
            println!("Test file not found, skipping test");
            return Ok(());
        }

        // Parse using the synchronous parser
        let sync_start = Instant::now();
        let sync_tags_count = crate::parser::FlvParser::parse_file(path)?;
        let sync_duration = sync_start.elapsed();

        // Parse using the async parser and collect detailed tag information
        let async_start = Instant::now();
        let mut stream = FlvParser::create_decoder_stream(path).await?;

        // Track all tags with their timestamps - we'll use these to compare
        let mut async_tag_counts = HashMap::new();
        let mut async_tag_timestamps = Vec::new();
        let mut async_tags_count = 0;

        // Process each item in the stream
        while let Some(result) = stream.next().await {
            match result {
                Ok(FlvData::Tag(tag)) => {
                    async_tags_count += 1;

                    println!(
                        "Async parser: Tag Type: {}, Timestamp: {}, Size: {}",
                        tag.tag_type,
                        tag.timestamp_ms,
                        tag.data.len()
                    );

                    // Categorize the tag by type
                    let tag_type = if tag.is_video_tag() {
                        "video"
                    } else if tag.is_audio_tag() {
                        "audio"
                    } else if tag.is_script_tag() {
                        "metadata"
                    } else {
                        "unknown"
                    };

                    // Count by type and timestamp
                    *async_tag_counts.entry(tag_type).or_insert(0) += 1;

                    // Store timestamp for pattern analysis
                    if async_tags_count % 1000 == 0 {
                        // Sample every 1000th tag to avoid excessive memory use
                        async_tag_timestamps.push((tag.timestamp_ms, tag_type.to_string()));
                    }
                }
                Ok(FlvData::Header(_)) => {}
                Ok(FlvData::EndOfSequence(_)) => {}
                Err(e) => {
                    println!("Error processing tag: {e:?}");
                    // Don't break - continue processing
                }
            }
        }
        let async_duration = async_start.elapsed();

        // Now run a separate sync parser to collect timestamps
        let mut sync_tag_timestamps = Vec::new();
        let file = std::fs::File::open(path)?;
        let mut reader = std::io::BufReader::new(file);
        let mut buffer = BytesMut::with_capacity(9);

        // Read and skip header
        buffer.resize(9, 0);
        std::io::Read::read_exact(&mut reader, &mut buffer)?;

        // Create buffer for reading tags
        let mut tag_buffer = BytesMut::with_capacity(4 * 1024);
        let mut sync_tag_counter = 0;

        loop {
            // Skip previous tag size
            tag_buffer.resize(4, 0);
            if let Err(e) = std::io::Read::read_exact(&mut reader, &mut tag_buffer) {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    break;
                }
                return Err(e.into());
            }

            // Read tag header
            tag_buffer.resize(11, 0);
            if let Err(e) = std::io::Read::read_exact(&mut reader, &mut tag_buffer) {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    break;
                }
                return Err(e.into());
            }

            // Get tag type and size
            let tag_type = tag_buffer[0];
            let data_size = ((tag_buffer[1] as u32) << 16)
                | ((tag_buffer[2] as u32) << 8)
                | (tag_buffer[3] as u32);

            // Get timestamp
            let timestamp = ((tag_buffer[7] as u32) << 24)
                | ((tag_buffer[4] as u32) << 16)
                | ((tag_buffer[5] as u32) << 8)
                | (tag_buffer[6] as u32);

            // Record timestamp for sampled tags
            sync_tag_counter += 1;
            if sync_tag_counter % 1000 == 0 {
                let tag_type_str = match tag_type {
                    8 => "audio",
                    9 => "video",
                    18 => "metadata",
                    _ => "unknown",
                };
                sync_tag_timestamps.push((timestamp, tag_type_str.to_string()));
            }

            // Skip the tag data
            let mut data_buffer = vec![0; data_size as usize];
            if let Err(e) = std::io::Read::read_exact(&mut reader, &mut data_buffer) {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    break;
                }
                return Err(e.into());
            }
        }

        // Print comparison results
        println!("\n=== PARSER COMPARISON RESULTS ===");
        println!("Sync parser: {sync_tags_count} tags in {sync_duration:?}");
        println!("Async parser: {async_tags_count} tags in {async_duration:?}");
        println!(
            "Difference: {} tags",
            sync_tags_count as i64 - async_tags_count as i64
        );
        println!("\nAsync parser tag types:");
        for (tag_type, count) in &async_tag_counts {
            println!("  {tag_type}: {count}");
        }

        // Find timestamp discrepancies
        println!("\nTimestamp comparison (sample of every 1000th tag):");
        let min_samples = std::cmp::min(sync_tag_timestamps.len(), async_tag_timestamps.len());
        let mut first_discrepancy = None;

        for i in 0..min_samples {
            if sync_tag_timestamps[i] != async_tag_timestamps[i] {
                first_discrepancy = Some(i);
                break;
            }
        }

        if let Some(idx) = first_discrepancy {
            println!("First timestamp discrepancy at tag #{}:", idx * 1000);
            println!("  Sync: {:?}", sync_tag_timestamps[idx]);
            println!("  Async: {:?}", async_tag_timestamps[idx]);

            // Print surrounding context
            let start = idx.saturating_sub(2);
            let end = if idx + 3 < min_samples {
                idx + 3
            } else {
                min_samples
            };

            println!("\nContext around discrepancy:");
            for i in start..end {
                println!(
                    "  Tag #{}: Sync {:?}, Async {:?}",
                    i * 1000,
                    sync_tag_timestamps[i],
                    async_tag_timestamps[i]
                );
            }
        } else {
            println!("No timestamp discrepancies found in sampled tags");
        }

        Ok(())
    }
}
