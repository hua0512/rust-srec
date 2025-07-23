use crate::{Result, TsError};

/// Stream types defined in MPEG-2 and other standards
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StreamType {
    /// MPEG-1 Video
    Mpeg1Video = 0x01,
    /// MPEG-2 Video
    Mpeg2Video = 0x02,
    /// MPEG-1 Audio
    Mpeg1Audio = 0x03,
    /// MPEG-2 Audio
    Mpeg2Audio = 0x04,
    /// MPEG-2 Private sections
    Mpeg2PrivateSections = 0x05,
    /// MPEG-2 Private PES packets
    Mpeg2PrivatePes = 0x06,
    /// ISO/IEC 13522 MHEG
    Mheg = 0x07,
    /// ITU-T Rec. H.222.0 | ISO/IEC 13818-1 Annex A DSM-CC
    DsmCc = 0x08,
    /// ITU-T Rec. H.222.1
    H2221 = 0x09,
    /// ISO/IEC 13818-6 type A
    Iso13818_6TypeA = 0x0A,
    /// ISO/IEC 13818-6 type B
    Iso13818_6TypeB = 0x0B,
    /// ISO/IEC 13818-6 type C
    Iso13818_6TypeC = 0x0C,
    /// ISO/IEC 13818-6 type D
    Iso13818_6TypeD = 0x0D,
    /// ITU-T Rec. H.222.0 | ISO/IEC 13818-1 auxiliary
    Mpeg2Auxiliary = 0x0E,
    /// ADTS AAC Audio
    AdtsAac = 0x0F,
    /// MPEG-4 Visual
    Mpeg4Visual = 0x10,
    /// LATM AAC Audio
    LatmAac = 0x11,
    /// MPEG-4 SL-packetized stream or FlexMux stream carried in PES packets
    Mpeg4SlPes = 0x12,
    /// MPEG-4 SL-packetized stream or FlexMux stream carried in ISO/IEC 14496 sections
    Mpeg4SlSections = 0x13,
    /// ISO/IEC 13818-6 Synchronized Download Protocol
    Iso13818_6Sdp = 0x14,
    /// Metadata carried in PES packets
    MetadataPes = 0x15,
    /// Metadata carried in metadata sections
    MetadataSections = 0x16,
    /// Metadata carried in ISO/IEC 13818-6 Data Carousel
    MetadataDataCarousel = 0x17,
    /// Metadata carried in ISO/IEC 13818-6 Object Carousel
    MetadataObjectCarousel = 0x18,
    /// Metadata carried in ISO/IEC 13818-6 Synchronized Download Protocol
    MetadataSdp = 0x19,
    /// IPMP stream (defined in ISO/IEC 13818-11, MPEG-2 IPMP)
    Ipmp = 0x1A,
    /// AVC video stream (ITU-T Rec. H.264 | ISO/IEC 14496-10)
    H264 = 0x1B,
    /// MPEG-4 Audio, without using any additional transport syntax
    Mpeg4Audio = 0x1C,
    /// MPEG-4 Visual, without using any additional transport syntax
    Mpeg4VisualPlain = 0x1D,
    /// SVC video sub-bitstream (ITU-T Rec. H.264 | ISO/IEC 14496-10)
    Svc = 0x1E,
    /// MVC video sub-bitstream (ITU-T Rec. H.264 | ISO/IEC 14496-10)
    Mvc = 0x1F,
    /// Video stream conforming to one or more profiles defined in Annex A of ITU-T Rec. H.264 | ISO/IEC 14496-10
    H264Additional = 0x20,
    /// JPEG 2000 video stream (ITU-T Rec. T.800 | ISO/IEC 15444-1)
    Jpeg2000 = 0x21,
    /// Additional view H.262 video stream (ITU-T Rec. H.262 | ISO/IEC 13818-2)
    H262Additional = 0x22,
    /// Additional view H.264 video stream (ITU-T Rec. H.264 | ISO/IEC 14496-10)
    H264AdditionalView = 0x23,
    /// HEVC video stream (ITU-T Rec. H.265 | ISO/IEC 23008-2)
    H265 = 0x24,
    /// MVCD video sub-bitstream (ITU-T Rec. H.264 | ISO/IEC 14496-10)
    Mvcd = 0x25,
    /// Timeline and External Media Information Stream (ITU-T Rec. H.274 | ISO/IEC 23001-10)
    Timeline = 0x26,
    /// HEVC temporal video sub-bitstream (ITU-T Rec. H.265 | ISO/IEC 23008-2)
    H265Temporal = 0x27,
    /// HEVC enhancement layer video sub-bitstream (ITU-T Rec. H.265 | ISO/IEC 23008-2)
    H265Enhancement = 0x28,
    /// HEVC temporal enhancement layer video sub-bitstream (ITU-T Rec. H.265 | ISO/IEC 23008-2)
    H265TemporalEnhancement = 0x29,
    /// HEVC tile video sub-bitstream (ITU-T Rec. H.265 | ISO/IEC 23008-2)
    H265Tile = 0x2A,
    /// JPEG XS video stream (ISO/IEC 21122-2)
    JpegXs = 0x32,
    /// VVC video stream (ITU-T Rec. H.266 | ISO/IEC 23090-3)
    H266 = 0x33,
    /// EVC video stream (ISO/IEC 23094-1)
    Evc = 0x34,
    /// LCEVC video stream (ISO/IEC 23094-2)
    Lcevc = 0x35,
    /// Chinese AVS2-P2 video (GB/T 33475.2)
    Avs2 = 0x40,
    /// Chinese AVS3-P1 video (GB/T 40857.1)
    Avs3 = 0x41,
    /// Chinese AVS3-P10 video (GB/T 40857.10)
    Avs3P10 = 0x42,
    /// DiracI video (SMPTE 421M)
    DiracI = 0xA1,
    /// AC-3 audio stream (ATSC A/52B)
    Ac3 = 0x81,
    /// DTS audio stream
    Dts = 0x82,
    /// Dolby TrueHD audio stream
    TrueHd = 0x83,
    /// E-AC-3 audio stream (ATSC A/52B)
    EAc3 = 0x84,
    /// DTS-HD audio stream
    DtsHd = 0x85,
    /// DTS-HD Master Audio stream
    DtsHdMa = 0x86,
    /// Dolby E audio stream
    DolbyE = 0x87,
    /// Unknown stream type
    Unknown(u8),
}

impl From<u8> for StreamType {
    fn from(value: u8) -> Self {
        match value {
            0x01 => StreamType::Mpeg1Video,
            0x02 => StreamType::Mpeg2Video,
            0x03 => StreamType::Mpeg1Audio,
            0x04 => StreamType::Mpeg2Audio,
            0x05 => StreamType::Mpeg2PrivateSections,
            0x06 => StreamType::Mpeg2PrivatePes,
            0x07 => StreamType::Mheg,
            0x08 => StreamType::DsmCc,
            0x09 => StreamType::H2221,
            0x0A => StreamType::Iso13818_6TypeA,
            0x0B => StreamType::Iso13818_6TypeB,
            0x0C => StreamType::Iso13818_6TypeC,
            0x0D => StreamType::Iso13818_6TypeD,
            0x0E => StreamType::Mpeg2Auxiliary,
            0x0F => StreamType::AdtsAac,
            0x10 => StreamType::Mpeg4Visual,
            0x11 => StreamType::LatmAac,
            0x12 => StreamType::Mpeg4SlPes,
            0x13 => StreamType::Mpeg4SlSections,
            0x14 => StreamType::Iso13818_6Sdp,
            0x15 => StreamType::MetadataPes,
            0x16 => StreamType::MetadataSections,
            0x17 => StreamType::MetadataDataCarousel,
            0x18 => StreamType::MetadataObjectCarousel,
            0x19 => StreamType::MetadataSdp,
            0x1A => StreamType::Ipmp,
            0x1B => StreamType::H264,
            0x1C => StreamType::Mpeg4Audio,
            0x1D => StreamType::Mpeg4VisualPlain,
            0x1E => StreamType::Svc,
            0x1F => StreamType::Mvc,
            0x20 => StreamType::H264Additional,
            0x21 => StreamType::Jpeg2000,
            0x22 => StreamType::H262Additional,
            0x23 => StreamType::H264AdditionalView,
            0x24 => StreamType::H265,
            0x25 => StreamType::Mvcd,
            0x26 => StreamType::Timeline,
            0x27 => StreamType::H265Temporal,
            0x28 => StreamType::H265Enhancement,
            0x29 => StreamType::H265TemporalEnhancement,
            0x2A => StreamType::H265Tile,
            0x32 => StreamType::JpegXs,
            0x33 => StreamType::H266,
            0x34 => StreamType::Evc,
            0x35 => StreamType::Lcevc,
            0x40 => StreamType::Avs2,
            0x41 => StreamType::Avs3,
            0x42 => StreamType::Avs3P10,
            0x81 => StreamType::Ac3,
            0x82 => StreamType::Dts,
            0x83 => StreamType::TrueHd,
            0x84 => StreamType::EAc3,
            0x85 => StreamType::DtsHd,
            0x86 => StreamType::DtsHdMa,
            0x87 => StreamType::DolbyE,
            0xA1 => StreamType::DiracI,
            _ => StreamType::Unknown(value),
        }
    }
}

impl StreamType {
    /// Check if this stream type is video
    pub fn is_video(&self) -> bool {
        matches!(
            self,
            StreamType::Mpeg1Video
                | StreamType::Mpeg2Video
                | StreamType::Mpeg4Visual
                | StreamType::Mpeg4VisualPlain
                | StreamType::H264
                | StreamType::H264Additional
                | StreamType::H264AdditionalView
                | StreamType::H265
                | StreamType::H265Temporal
                | StreamType::H265Enhancement
                | StreamType::H265TemporalEnhancement
                | StreamType::H265Tile
                | StreamType::H266
                | StreamType::H262Additional
                | StreamType::Svc
                | StreamType::Mvc
                | StreamType::Mvcd
                | StreamType::Jpeg2000
                | StreamType::JpegXs
                | StreamType::Evc
                | StreamType::Lcevc
                | StreamType::Avs2
                | StreamType::Avs3
                | StreamType::Avs3P10
                | StreamType::DiracI
        )
    }

    /// Check if this stream type is audio
    pub fn is_audio(&self) -> bool {
        matches!(
            self,
            StreamType::Mpeg1Audio
                | StreamType::Mpeg2Audio
                | StreamType::AdtsAac
                | StreamType::LatmAac
                | StreamType::Mpeg4Audio
                | StreamType::Ac3
                | StreamType::EAc3
                | StreamType::Dts
                | StreamType::DtsHd
                | StreamType::DtsHdMa
                | StreamType::TrueHd
                | StreamType::DolbyE
        )
    }
}

/// Program Map Table (PMT) - Table ID 0x02
#[derive(Debug, Clone)]
pub struct Pmt {
    /// Table ID (should be 0x02 for PMT)
    pub table_id: u8,
    /// Program number
    pub program_number: u16,
    /// Version number
    pub version_number: u8,
    /// Current/next indicator
    pub current_next_indicator: bool,
    /// Section number
    pub section_number: u8,
    /// Last section number
    pub last_section_number: u8,
    /// PCR PID
    pub pcr_pid: u16,
    /// Program info descriptors
    pub program_info: Vec<u8>,
    /// Elementary streams
    pub streams: Vec<PmtStream>,
}

/// Elementary stream in PMT
#[derive(Debug, Clone)]
pub struct PmtStream {
    /// Stream type
    pub stream_type: StreamType,
    /// Elementary PID
    pub elementary_pid: u16,
    /// ES info descriptors
    pub es_info: Vec<u8>,
}

impl Pmt {
    /// Parse PMT from PSI section data
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 12 {
            return Err(TsError::InsufficientData {
                expected: 12,
                actual: data.len(),
            });
        }

        let table_id = data[0];
        if table_id != 0x02 {
            return Err(TsError::InvalidTableId {
                expected: 0x02,
                actual: table_id,
            });
        }

        // Parse section header
        let section_syntax_indicator = (data[1] & 0x80) != 0;
        if !section_syntax_indicator {
            return Err(TsError::ParseError(
                "PMT must have section syntax indicator set".to_string(),
            ));
        }

        let section_length = ((data[1] as u16 & 0x0F) << 8) | data[2] as u16;
        if section_length < 9 {
            return Err(TsError::InvalidSectionLength(section_length));
        }

        if data.len() < (3 + section_length as usize) {
            return Err(TsError::InsufficientData {
                expected: 3 + section_length as usize,
                actual: data.len(),
            });
        }

        let program_number = ((data[3] as u16) << 8) | data[4] as u16;
        let version_number = (data[5] >> 1) & 0x1F;
        let current_next_indicator = (data[5] & 0x01) != 0;
        let section_number = data[6];
        let last_section_number = data[7];
        let pcr_pid = ((data[8] as u16 & 0x1F) << 8) | data[9] as u16;

        let program_info_length = ((data[10] as u16 & 0x0F) << 8) | data[11] as u16;
        let mut offset = 12;

        // Parse program info descriptors
        let program_info = if program_info_length > 0 {
            if offset + program_info_length as usize > data.len() {
                return Err(TsError::InsufficientData {
                    expected: offset + program_info_length as usize,
                    actual: data.len(),
                });
            }
            let info = data[offset..offset + program_info_length as usize].to_vec();
            offset += program_info_length as usize;
            info
        } else {
            Vec::new()
        };

        // Parse elementary streams
        let mut streams = Vec::new();
        let streams_end = 3 + section_length as usize - 4; // Exclude CRC32

        while offset + 5 <= streams_end {
            let stream_type = StreamType::from(data[offset]);
            let elementary_pid = ((data[offset + 1] as u16 & 0x1F) << 8) | data[offset + 2] as u16;
            let es_info_length = ((data[offset + 3] as u16 & 0x0F) << 8) | data[offset + 4] as u16;
            offset += 5;

            // Parse ES info descriptors
            let es_info = if es_info_length > 0 {
                if offset + es_info_length as usize > streams_end {
                    return Err(TsError::InsufficientData {
                        expected: offset + es_info_length as usize,
                        actual: streams_end,
                    });
                }
                let info = data[offset..offset + es_info_length as usize].to_vec();
                offset += es_info_length as usize;
                info
            } else {
                Vec::new()
            };

            streams.push(PmtStream {
                stream_type,
                elementary_pid,
                es_info,
            });
        }

        // TODO: Verify CRC32 if needed

        Ok(Pmt {
            table_id,
            program_number,
            version_number,
            current_next_indicator,
            section_number,
            last_section_number,
            pcr_pid,
            program_info,
            streams,
        })
    }

    /// Get all video streams
    pub fn video_streams(&self) -> Vec<&PmtStream> {
        self.streams
            .iter()
            .filter(|s| s.stream_type.is_video())
            .collect()
    }

    /// Get all audio streams
    pub fn audio_streams(&self) -> Vec<&PmtStream> {
        self.streams
            .iter()
            .filter(|s| s.stream_type.is_audio())
            .collect()
    }

    /// Get stream by PID
    pub fn get_stream(&self, pid: u16) -> Option<&PmtStream> {
        self.streams.iter().find(|s| s.elementary_pid == pid)
    }

    /// Get all PIDs used by this program
    pub fn get_all_pids(&self) -> Vec<u16> {
        let mut pids = vec![self.pcr_pid];
        pids.extend(self.streams.iter().map(|s| s.elementary_pid));
        pids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_type_conversion() {
        assert_eq!(StreamType::from(0x1B), StreamType::H264);
        assert_eq!(StreamType::from(0x24), StreamType::H265);
        assert_eq!(StreamType::from(0x0F), StreamType::AdtsAac);
        assert_eq!(StreamType::from(0xFF), StreamType::Unknown(0xFF));
    }

    #[test]
    fn test_stream_type_classification() {
        assert!(StreamType::H264.is_video());
        assert!(!StreamType::H264.is_audio());
        assert!(StreamType::AdtsAac.is_audio());
        assert!(!StreamType::AdtsAac.is_video());
    }

    #[test]
    fn test_pmt_invalid_table_id() {
        let data = vec![
            0x01, 0x80, 0x0D, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert!(Pmt::parse(&data).is_err());
    }

    #[test]
    fn test_pmt_basic_parsing() {
        // Example PMT with one H.264 video stream
        let data = vec![
            0x02, // Table ID
            0x80, // Section syntax indicator + section length high
            0x12, // Section length low (18 bytes total)
            0x00, 0x01, // Program number
            0x01, // Version 0 + current/next = 1
            0x00, // Section number
            0x00, // Last section number
            0xE1, 0x00, // PCR PID (0x100)
            0x00, 0x00, // Program info length (0)
            // Elementary stream
            0x1B, // Stream type (H.264)
            0xE1, 0x00, // Elementary PID (0x100)
            0x00, 0x00, // ES info length (0)
            // CRC32 placeholder
            0x00, 0x00, 0x00, 0x00,
        ];

        let pmt = Pmt::parse(&data).unwrap();
        assert_eq!(pmt.table_id, 0x02);
        assert_eq!(pmt.program_number, 1);
        assert_eq!(pmt.pcr_pid, 0x100);
        assert_eq!(pmt.streams.len(), 1);
        assert_eq!(pmt.streams[0].stream_type, StreamType::H264);
        assert_eq!(pmt.streams[0].elementary_pid, 0x100);
        assert!(pmt.streams[0].stream_type.is_video());
    }
}
