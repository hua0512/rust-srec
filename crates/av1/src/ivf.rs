//! IVF container format parsing and writing.
//!
//! IVF (Indeo Video Format) is a simple container format commonly used
//! for AV1, VP8, and VP9 test files. It consists of a 32-byte file
//! header followed by 12-byte frame headers with raw codec data.
//!
//! All multi-byte integers are little-endian.

use std::io;

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use bytes::Bytes;
use bytes_util::BytesCursorExt;

use crate::error::{Av1Error, Result};

/// IVF file signature: `"DKIF"`.
const IVF_SIGNATURE: [u8; 4] = *b"DKIF";

/// AV1 codec FourCC: `"av01"`.
const AV1_FOURCC: [u8; 4] = *b"av01";

/// IVF file header (32 bytes, all little-endian).
///
/// ```text
/// Offset  Size  Field
/// 0       4     signature: "DKIF"
/// 4       2     version: 0
/// 6       2     header_size: 32
/// 8       4     codec_fourcc: "av01"
/// 12      2     width
/// 14      2     height
/// 16      4     timebase_denominator (rate)
/// 20      4     timebase_numerator (scale)
/// 24      4     frame_count
/// 28      4     reserved
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IvfHeader {
    /// IVF version (must be 0).
    pub version: u16,
    /// Video width in pixels.
    pub width: u16,
    /// Video height in pixels.
    pub height: u16,
    /// Timebase numerator.
    ///
    /// Together with `timebase_denominator`, defines the time base as
    /// `numerator / denominator` seconds per tick. For 30fps video,
    /// typically numerator=1, denominator=30.
    ///
    /// Stored at byte offset 20 in the IVF header (after denominator).
    pub timebase_numerator: u32,
    /// Timebase denominator.
    ///
    /// Stored at byte offset 16 in the IVF header (before numerator).
    pub timebase_denominator: u32,
    /// Total number of frames (may be 0 if unknown).
    pub frame_count: u32,
}

impl IvfHeader {
    /// Size of the IVF file header in bytes.
    pub const SIZE: usize = 32;

    /// Demuxes an IVF file header from the given reader.
    pub fn demux<R: io::Read>(reader: &mut R) -> Result<Self> {
        let mut signature = [0u8; 4];
        reader.read_exact(&mut signature)?;
        if signature != IVF_SIGNATURE {
            return Err(Av1Error::InvalidIvfSignature(signature));
        }

        let version = reader.read_u16::<LittleEndian>()?;
        if version != 0 {
            return Err(Av1Error::UnsupportedIvfVersion(version));
        }

        let _header_size = reader.read_u16::<LittleEndian>()?;

        let mut codec = [0u8; 4];
        reader.read_exact(&mut codec)?;
        if !codec.eq_ignore_ascii_case(&AV1_FOURCC) {
            return Err(Av1Error::InvalidIvfCodec(codec));
        }

        let width = reader.read_u16::<LittleEndian>()?;
        let height = reader.read_u16::<LittleEndian>()?;
        let timebase_denominator = reader.read_u32::<LittleEndian>()?;
        let timebase_numerator = reader.read_u32::<LittleEndian>()?;

        if timebase_numerator == 0 || timebase_denominator == 0 {
            return Err(Av1Error::InvalidIvfTimebase {
                numerator: timebase_numerator,
                denominator: timebase_denominator,
            });
        }

        let frame_count = reader.read_u32::<LittleEndian>()?;
        let _reserved = reader.read_u32::<LittleEndian>()?;

        Ok(IvfHeader {
            version,
            width,
            height,
            timebase_numerator,
            timebase_denominator,
            frame_count,
        })
    }

    /// Muxes this IVF file header to the given writer.
    pub fn mux<W: io::Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_all(&IVF_SIGNATURE)?;
        writer.write_u16::<LittleEndian>(self.version)?;
        writer.write_u16::<LittleEndian>(Self::SIZE as u16)?;
        writer.write_all(&AV1_FOURCC)?;
        writer.write_u16::<LittleEndian>(self.width)?;
        writer.write_u16::<LittleEndian>(self.height)?;
        writer.write_u32::<LittleEndian>(self.timebase_denominator)?;
        writer.write_u32::<LittleEndian>(self.timebase_numerator)?;
        writer.write_u32::<LittleEndian>(self.frame_count)?;
        writer.write_u32::<LittleEndian>(0)?; // reserved
        Ok(())
    }
}

/// IVF frame header (12 bytes, all little-endian).
///
/// ```text
/// Offset  Size  Field
/// 0       4     frame_size (bytes of payload following)
/// 4       8     pts (presentation timestamp in timebase units)
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub struct IvfFrameHeader {
    /// Size of the frame data in bytes.
    pub frame_size: u32,
    /// Presentation timestamp in timebase units.
    pub pts: u64,
}

impl IvfFrameHeader {
    /// Size of each IVF frame header in bytes.
    pub const SIZE: usize = 12;

    /// Demuxes an IVF frame header from the given reader.
    pub fn demux<R: io::Read>(reader: &mut R) -> Result<Self> {
        let frame_size = reader.read_u32::<LittleEndian>()?;
        let pts = reader.read_u64::<LittleEndian>()?;
        Ok(IvfFrameHeader { frame_size, pts })
    }

    /// Muxes this IVF frame header to the given writer.
    pub fn mux<W: io::Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_u32::<LittleEndian>(self.frame_size)?;
        writer.write_u64::<LittleEndian>(self.pts)?;
        Ok(())
    }
}

/// A parsed IVF frame with zero-copy data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IvfFrame {
    /// Frame header.
    pub header: IvfFrameHeader,
    /// Raw frame data (OBU sequence). Zero-copy `Bytes` slice.
    pub data: Bytes,
}

impl IvfFrame {
    /// Demuxes a full IVF frame (header + data) from a `Cursor<Bytes>`.
    ///
    /// Uses zero-copy slicing for the frame data.
    pub fn demux(reader: &mut io::Cursor<Bytes>) -> Result<Self> {
        let header = IvfFrameHeader::demux(reader)?;
        let data = reader
            .extract_bytes(header.frame_size as usize)
            .map_err(|_| Av1Error::UnexpectedEof {
                expected: header.frame_size as usize,
                actual: (reader.get_ref().len() as u64 - reader.position()) as usize,
            })?;
        Ok(IvfFrame { header, data })
    }
}

/// IVF file writer.
///
/// Writes an IVF file header followed by frames. Call [`finalize`](IvfWriter::finalize)
/// (requires `Write + Seek`) to update the frame count in the file header after all
/// frames have been written.
pub struct IvfWriter<W: io::Write> {
    writer: W,
    frame_count: u32,
}

impl<W: io::Write> IvfWriter<W> {
    /// Creates a new IVF writer and writes the file header.
    pub fn new(mut writer: W, header: &IvfHeader) -> Result<Self> {
        header.mux(&mut writer)?;
        Ok(IvfWriter {
            writer,
            frame_count: 0,
        })
    }

    /// Writes a single frame with the given presentation timestamp and data.
    pub fn write_frame(&mut self, pts: u64, data: &[u8]) -> Result<()> {
        let frame_header = IvfFrameHeader {
            frame_size: data.len() as u32,
            pts,
        };
        frame_header.mux(&mut self.writer)?;
        self.writer.write_all(data)?;
        self.frame_count += 1;
        Ok(())
    }

    /// Returns the number of frames written so far.
    pub fn frame_count(&self) -> u32 {
        self.frame_count
    }

    /// Consumes the writer and returns the inner writer.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: io::Write + io::Seek> IvfWriter<W> {
    /// Seeks back to the file header and updates the frame count field.
    pub fn finalize(&mut self) -> Result<()> {
        self.writer.seek(io::SeekFrom::Start(24))?;
        self.writer.write_u32::<LittleEndian>(self.frame_count)?;
        self.writer.seek(io::SeekFrom::End(0))?;
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(all(coverage_nightly, test), coverage(off))]
mod tests {
    use super::*;

    fn test_header() -> IvfHeader {
        IvfHeader {
            version: 0,
            width: 1920,
            height: 1080,
            timebase_numerator: 1,
            timebase_denominator: 30,
            frame_count: 0,
        }
    }

    #[test]
    fn test_ivf_header_round_trip() {
        let header = test_header();
        let mut buf = Vec::new();
        header.mux(&mut buf).unwrap();
        assert_eq!(buf.len(), IvfHeader::SIZE);

        let parsed = IvfHeader::demux(&mut io::Cursor::new(buf)).unwrap();
        assert_eq!(parsed, header);
    }

    #[test]
    fn test_ivf_header_byte_layout() {
        // Verify byte-level compatibility with libvpx/ffmpeg IVF format.
        // For 30fps 1920x1080: denominator=30 at offset 16, numerator=1 at offset 20.
        let header = test_header();
        let mut buf = Vec::new();
        header.mux(&mut buf).unwrap();

        // Signature
        assert_eq!(&buf[0..4], b"DKIF");
        // Version
        assert_eq!(u16::from_le_bytes([buf[4], buf[5]]), 0);
        // Header size
        assert_eq!(u16::from_le_bytes([buf[6], buf[7]]), 32);
        // Codec FourCC (lowercase av01)
        assert_eq!(&buf[8..12], b"av01");
        // Width
        assert_eq!(u16::from_le_bytes([buf[12], buf[13]]), 1920);
        // Height
        assert_eq!(u16::from_le_bytes([buf[14], buf[15]]), 1080);
        // Offset 16: denominator (rate) = 30
        assert_eq!(u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]), 30);
        // Offset 20: numerator (scale) = 1
        assert_eq!(u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]), 1);
        // Frame count
        assert_eq!(u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]), 0);
        // Reserved
        assert_eq!(u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]), 0);
    }

    #[test]
    fn test_ivf_header_invalid_signature() {
        let data = b"XXXX\x00\x00\x20\x00av01\x80\x07\x38\x04\x01\x00\x00\x00\x1e\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        let err = IvfHeader::demux(&mut io::Cursor::new(data.as_slice())).unwrap_err();
        assert!(matches!(err, Av1Error::InvalidIvfSignature(_)));
    }

    #[test]
    fn test_ivf_header_invalid_codec() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"DKIF");
        buf.extend_from_slice(&0u16.to_le_bytes()); // version
        buf.extend_from_slice(&32u16.to_le_bytes()); // header_size
        buf.extend_from_slice(b"VP80"); // wrong codec
        buf.extend_from_slice(&[0; 16]); // remaining fields
        let err = IvfHeader::demux(&mut io::Cursor::new(buf)).unwrap_err();
        assert!(matches!(err, Av1Error::InvalidIvfCodec(_)));
    }

    #[test]
    fn test_ivf_header_accepts_uppercase_fourcc() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"DKIF");
        buf.extend_from_slice(&0u16.to_le_bytes()); // version
        buf.extend_from_slice(&32u16.to_le_bytes()); // header_size
        buf.extend_from_slice(b"AV01"); // uppercase (FFmpeg convention)
        buf.extend_from_slice(&1920u16.to_le_bytes()); // width
        buf.extend_from_slice(&1080u16.to_le_bytes()); // height
        buf.extend_from_slice(&30u32.to_le_bytes()); // timebase_denominator
        buf.extend_from_slice(&1u32.to_le_bytes()); // timebase_numerator
        buf.extend_from_slice(&0u32.to_le_bytes()); // frame_count
        buf.extend_from_slice(&0u32.to_le_bytes()); // reserved
        let header = IvfHeader::demux(&mut io::Cursor::new(buf)).unwrap();
        assert_eq!(header.width, 1920);
        assert_eq!(header.height, 1080);
    }

    #[test]
    fn test_ivf_header_unsupported_version() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"DKIF");
        buf.extend_from_slice(&1u16.to_le_bytes()); // bad version
        buf.extend_from_slice(&[0; 24]); // remaining fields
        let err = IvfHeader::demux(&mut io::Cursor::new(buf)).unwrap_err();
        assert!(matches!(err, Av1Error::UnsupportedIvfVersion(1)));
    }

    #[test]
    fn test_ivf_frame_header_round_trip() {
        let header = IvfFrameHeader {
            frame_size: 1234,
            pts: 42,
        };
        let mut buf = Vec::new();
        header.mux(&mut buf).unwrap();
        assert_eq!(buf.len(), IvfFrameHeader::SIZE);

        let parsed = IvfFrameHeader::demux(&mut io::Cursor::new(buf)).unwrap();
        assert_eq!(parsed, header);
    }

    #[test]
    fn test_ivf_frame_demux() {
        let payload = b"OBU data here";
        let mut buf = Vec::new();
        let frame_header = IvfFrameHeader {
            frame_size: payload.len() as u32,
            pts: 100,
        };
        frame_header.mux(&mut buf).unwrap();
        buf.extend_from_slice(payload);

        let mut cursor = io::Cursor::new(Bytes::from(buf));
        let frame = IvfFrame::demux(&mut cursor).unwrap();
        assert_eq!(frame.header.frame_size, payload.len() as u32);
        assert_eq!(frame.header.pts, 100);
        assert_eq!(frame.data.as_ref(), payload);
    }

    #[test]
    fn test_ivf_writer_round_trip() {
        let header = test_header();
        let mut buf = io::Cursor::new(Vec::new());
        let mut writer = IvfWriter::new(&mut buf, &header).unwrap();

        writer.write_frame(0, b"frame0").unwrap();
        writer.write_frame(1, b"frame1").unwrap();
        assert_eq!(writer.frame_count(), 2);

        writer.finalize().unwrap();

        let data = Bytes::from(buf.into_inner());
        let mut cursor = io::Cursor::new(data.clone());

        let parsed_header = IvfHeader::demux(&mut cursor).unwrap();
        assert_eq!(parsed_header.width, 1920);
        assert_eq!(parsed_header.height, 1080);
        assert_eq!(parsed_header.frame_count, 2);

        let frame0 = IvfFrame::demux(&mut cursor).unwrap();
        assert_eq!(frame0.header.pts, 0);
        assert_eq!(frame0.data.as_ref(), b"frame0");

        let frame1 = IvfFrame::demux(&mut cursor).unwrap();
        assert_eq!(frame1.header.pts, 1);
        assert_eq!(frame1.data.as_ref(), b"frame1");
    }

    #[test]
    fn test_ivf_writer_zero_frames() {
        let header = test_header();
        let mut buf = io::Cursor::new(Vec::new());
        let writer = IvfWriter::new(&mut buf, &header).unwrap();
        assert_eq!(writer.frame_count(), 0);

        let data = buf.into_inner();
        assert_eq!(data.len(), IvfHeader::SIZE);

        let parsed = IvfHeader::demux(&mut io::Cursor::new(data)).unwrap();
        assert_eq!(parsed.frame_count, 0);
    }

    #[test]
    fn test_ivf_frame_truncated_data() {
        let mut buf = Vec::new();
        let frame_header = IvfFrameHeader {
            frame_size: 100, // claims 100 bytes
            pts: 0,
        };
        frame_header.mux(&mut buf).unwrap();
        buf.extend_from_slice(b"short"); // only 5 bytes

        let mut cursor = io::Cursor::new(Bytes::from(buf));
        let err = IvfFrame::demux(&mut cursor).unwrap_err();
        assert!(matches!(
            err,
            Av1Error::UnexpectedEof {
                expected: 100,
                actual: 5
            }
        ));
    }

    #[test]
    fn test_ivf_header_zero_timebase_denominator() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"DKIF");
        buf.extend_from_slice(&0u16.to_le_bytes()); // version
        buf.extend_from_slice(&32u16.to_le_bytes()); // header_size
        buf.extend_from_slice(b"av01");
        buf.extend_from_slice(&1920u16.to_le_bytes()); // width
        buf.extend_from_slice(&1080u16.to_le_bytes()); // height
        buf.extend_from_slice(&0u32.to_le_bytes()); // denominator = 0
        buf.extend_from_slice(&1u32.to_le_bytes()); // numerator
        buf.extend_from_slice(&0u32.to_le_bytes()); // frame_count
        buf.extend_from_slice(&0u32.to_le_bytes()); // reserved
        let err = IvfHeader::demux(&mut io::Cursor::new(buf)).unwrap_err();
        assert!(matches!(err, Av1Error::InvalidIvfTimebase { .. }));
    }

    #[test]
    fn test_ivf_header_zero_timebase_numerator() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"DKIF");
        buf.extend_from_slice(&0u16.to_le_bytes()); // version
        buf.extend_from_slice(&32u16.to_le_bytes()); // header_size
        buf.extend_from_slice(b"av01");
        buf.extend_from_slice(&1920u16.to_le_bytes()); // width
        buf.extend_from_slice(&1080u16.to_le_bytes()); // height
        buf.extend_from_slice(&30u32.to_le_bytes()); // denominator
        buf.extend_from_slice(&0u32.to_le_bytes()); // numerator = 0
        buf.extend_from_slice(&0u32.to_le_bytes()); // frame_count
        buf.extend_from_slice(&0u32.to_le_bytes()); // reserved
        let err = IvfHeader::demux(&mut io::Cursor::new(buf)).unwrap_err();
        assert!(matches!(err, Av1Error::InvalidIvfTimebase { .. }));
    }
}
