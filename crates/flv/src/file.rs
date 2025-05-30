use byteorder::{BigEndian, ReadBytesExt};
use bytes::{Buf, Bytes};

use crate::tag::FlvUtil;

use super::header::FlvHeader;
use super::tag::FlvTagOwned;

/// An FLV file is a combination of a [`FlvHeader`] followed by the
/// `FLVFileBody` (which is just a series of [`FlvTag`]s)
///
/// The `FLVFileBody` is defined by:
/// - video_file_format_spec_v10.pdf (Chapter 1 - The FLV File Format - Page 8)
/// - video_file_format_spec_v10_1.pdf (Annex E.3 - The FLV File Body)
#[derive(Debug, Clone, PartialEq)]
pub struct FlvFile {
    pub header: FlvHeader,
    pub tags: Vec<FlvTagOwned>,
}

impl FlvFile {
    /// Demux an FLV file from a reader.
    /// The reader needs to be a [`std::io::Cursor`] with a [`Bytes`] buffer because we
    /// take advantage of zero-copy reading.
    #[allow(dead_code)]
    pub fn demux(reader: &mut std::io::Cursor<Bytes>) -> std::io::Result<Self> {
        let header = FlvHeader::parse(reader)?;

        let mut tags = Vec::new();
        while reader.has_remaining() {
            // We don't care about the previous tag size, its only really used for seeking
            // backwards.
            reader.read_u32::<BigEndian>()?;

            // If there is no more data, we can stop reading.
            if !reader.has_remaining() {
                break;
            }

            // Demux the tag from the reader.
            let tag = FlvTagOwned::demux(reader)?;
            tags.push(tag);
        }

        Ok(FlvFile { header, tags })
    }
}
