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
/// Each tag is followed by its `PreviousTagSize` back-pointer, including the
/// final tag (FLV specification, Annex E.3). The header includes PreviousTagSize0.
/// The encoder writes the header once and validates tag data size.
///
/// Use with `tokio_util::codec::FramedWrite` for buffered asynchronous writing.
#[derive(Debug, Default)]
pub struct FlvEncoder {
    /// Tracks if the FLV header has already been written.
    header_written: bool,
}

impl Encoder<FlvData> for FlvEncoder {
    type Error = io::Error;

    /// Encodes an `FlvData` item into the provided `BytesMut` buffer.
    ///
    /// - For `FlvData::Header`, writes the 9-byte FLV header followed by `PreviousTagSize0` (=0).
    ///   This must be the first item.
    /// - For `FlvData::Tag`, writes the 11-byte tag header, payload, and 4-byte
    ///   `PreviousTagSize`. Requires the header to have been written previously.
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

                let header_bytes = encode::encode_tag_header_bytes(
                    tag.tag_type(),
                    tag.is_filtered(),
                    data_len_u32,
                    tag.timestamp_ms,
                    0,
                )?;
                dst.put_slice(&header_bytes);

                dst.put(tag.into_data());

                // Back-pointers include the tag header and payload, but not themselves.
                let tag_size = (TAG_HEADER_SIZE + data_len) as u32;
                dst.put_slice(&encode::encode_prev_tag_size_bytes(tag_size));
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
    use std::io::Cursor;

    use bytes::{Bytes, BytesMut};
    use tokio_util::codec::{Decoder, Encoder};

    use crate::parser::{FlvParser, PrevTagSizeMode};
    use crate::parser_async::FlvDecoder;
    use crate::{FlvData, FlvHeader, FlvTag, FlvTagType, FlvWriter};

    use super::FlvEncoder;

    #[test]
    fn complete_stream_matches_spec_bytes_and_round_trips() {
        let header = FlvHeader::new(true, true);
        let tags = [
            FlvTag::new(
                0,
                0,
                FlvTagType::Audio,
                false,
                Bytes::from_static(b"\xaf\0\x12\x10"),
            ),
            // Legacy video command: end of client-side seeking.
            FlvTag::new(
                0x1234_5678,
                0,
                FlvTagType::Video,
                false,
                Bytes::from_static(b"\x57\x01"),
            ),
            // The container writer must preserve filtered bytes without decoding them.
            FlvTag::new(
                0x1234_5679,
                0,
                FlvTagType::Audio,
                true,
                Bytes::from_static(b"opaque"),
            ),
        ];
        // FLV Annex E.3/E.4 wire bytes, independent of either encoder's helpers.
        let expected = [
            b'F', b'L', b'V', 1, 5, 0, 0, 0, 9, 0, 0, 0, 0, 8, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0xaf,
            0, 0x12, 0x10, 0, 0, 0, 15, 9, 0, 0, 2, 0x34, 0x56, 0x78, 0x12, 0, 0, 0, 0x57, 1, 0, 0,
            0, 13, 0x28, 0, 0, 6, 0x34, 0x56, 0x79, 0x12, 0, 0, 0, b'o', b'p', b'a', b'q', b'u',
            b'e', 0, 0, 0, 17,
        ];
        let mut encoded = BytesMut::new();
        let mut encoder = FlvEncoder::default();
        encoder
            .encode(FlvData::Header(header.clone()), &mut encoded)
            .unwrap();
        let mut writer = FlvWriter::new(Cursor::new(Vec::new())).unwrap();
        writer.write_header(&header).unwrap();
        for tag in &tags {
            encoder
                .encode(FlvData::Tag(tag.clone()), &mut encoded)
                .unwrap();
            writer.write_tag_f(tag).unwrap();
        }
        assert_eq!(encoded.as_ref(), expected);
        assert_eq!(writer.writer.get_ref().as_slice(), expected);

        let mut reader = Cursor::new(&expected);
        assert_eq!(FlvParser::parse_header(&mut reader).unwrap(), header);
        let mut parsed = Vec::new();
        FlvParser::parse_tags_with_prev_tag_size_mode(
            &mut reader,
            &mut |tag, _, _| parsed.push(tag.clone()),
            9,
            PrevTagSizeMode::Strict,
        )
        .unwrap();
        assert_eq!(parsed, tags);
        assert_eq!(reader.position(), expected.len() as u64);

        let mut decoder = FlvDecoder::default();
        assert_eq!(
            decoder.decode_eof(&mut encoded).unwrap(),
            Some(FlvData::Header(header))
        );
        for tag in tags {
            assert_eq!(
                decoder.decode_eof(&mut encoded).unwrap(),
                Some(FlvData::Tag(tag))
            );
        }
        assert!(decoder.decode_eof(&mut encoded).unwrap().is_none());
        assert!(encoded.is_empty());
    }

    #[test]
    fn tag_before_header_is_rejected_without_writing_bytes() {
        let mut encoder = FlvEncoder::default();
        let mut buffer = BytesMut::new();
        let tag = FlvTag::new(
            0,
            0,
            FlvTagType::Audio,
            false,
            Bytes::from_static(b"\xaf\0\x12\x10"),
        );
        let error = encoder.encode(FlvData::Tag(tag), &mut buffer).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(buffer.is_empty());
    }

    #[test]
    fn invalid_items_leave_the_stream_usable() {
        let header = FlvHeader::new(true, false);
        let valid = FlvTag::new(
            0,
            0,
            FlvTagType::Audio,
            false,
            Bytes::from_static(b"\xaf\0\x12\x10"),
        );
        let invalid = [
            FlvData::Header(header.clone()),
            FlvData::Tag(FlvTag::new(
                0,
                1,
                FlvTagType::Audio,
                false,
                valid.data().clone(),
            )),
            // UI24 can represent at most 0xFF_FFFF bytes.
            FlvData::Tag(FlvTag::new(
                0,
                0,
                FlvTagType::Audio,
                false,
                Bytes::from(vec![0; 0x100_0000]),
            )),
        ];
        for item in invalid {
            let mut encoder = FlvEncoder::default();
            let mut buffer = BytesMut::new();
            encoder
                .encode(FlvData::Header(header.clone()), &mut buffer)
                .unwrap();
            let before = buffer.clone();
            let error = encoder.encode(item, &mut buffer).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert_eq!(buffer, before);
            encoder
                .encode(FlvData::Tag(valid.clone()), &mut buffer)
                .unwrap();
            assert_eq!(
                &buffer[13..],
                &[
                    8, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0xaf, 0, 0x12, 0x10, 0, 0, 0, 15
                ]
            );
        }
    }
}
