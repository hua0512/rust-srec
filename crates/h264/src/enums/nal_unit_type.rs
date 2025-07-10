use std::io;
use std::fmt;

/// NAL (Network Abstraction Layer) unit types as defined by ISO/IEC 14496-10:2022 (Table 7-1).
///
/// ## Decoder Behavior:
/// - **Some NAL units may be ignored** depending on the decoder.
/// - Decoders using **Annex A** must ignore unit types **14, 15, and 20**.
/// - **Types 0 and 24-31** are application-specific and do not affect decoding.
/// - **Reserved values** should be ignored.
///
/// ## IDR (Instantaneous Decoder Refresh) Pictures:
/// - If `nal_unit_type` is **5**, the picture **must not contain** types **1-4**.
/// - `IdrPicFlag` is **1** if `nal_unit_type == 5`, otherwise **0**.
#[derive(Clone, Copy, PartialEq)]
pub enum NALUnitType {
    /// Unspecified (not used in decoding)
    Unspecified1 = 0,

    /// Regular video slice (non-IDR picture)
    NonIDRSliceLayerWithoutPartitioning = 1,

    /// Coded slice data partition A
    SliceDataPartitionALayer = 2,

    /// Coded slice data partition B
    SliceDataPartitionBLayer = 3,

    /// Coded slice data partition C
    SliceDataPartitionCLayer = 4,

    /// IDR picture (used to refresh the video stream)
    IDRSliceLayerWithoutPartitioning = 5,

    /// Extra metadata (Supplemental Enhancement Information)
    SEI = 6,

    /// Sequence Parameter Set (SPS) – contains video configuration details
    SPS = 7,

    /// Picture Parameter Set (PPS) – contains picture-specific settings
    PPS = 8,

    /// Marks the start of a new access unit (frame boundary)
    AccessUnitDelimiter = 9,

    /// End of video sequence
    EndOfSeq = 10,

    /// End of video stream
    EndOfStream = 11,

    /// Extra filler data (can be ignored)
    FillerData = 12,

    /// Extension to SPS (used for advanced encoding features)
    SPSExtension = 13,

    /// Prefix NAL unit (ignored by Annex A decoders)
    PrefixNalUnit = 14,

    /// Subset of SPS (ignored by Annex A decoders)
    SubsetSPS = 15,

    /// Depth parameter set (used for 3D video)
    DepthParameterSet = 16,

    /// Reserved (should be ignored)
    Reserved1 = 17,

    /// Reserved (should be ignored)
    Reserved2 = 18,

    /// Auxiliary coded slice (may be ignored by some decoders)
    AuxCodedPictureSliceLayerWithoutPartitioning = 19,

    /// Additional slice data for extended coding (ignored by Annex A decoders)
    SliceLayerExtension = 20,

    /// Slice extension for depth/3D-AVC video (ignored by some decoders)
    SliceLayerExtension2 = 21,

    /// Reserved (should be ignored)
    Reserved3 = 22,

    /// Reserved (should be ignored)
    Reserved4 = 23,

    /// Unspecified (application-defined use)
    Unspecified2 = 24,
}

// Implement Debug manually to ensure it includes the enum name in output
impl fmt::Debug for NALUnitType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NALUnitType::")?;
        match self {
            Self::Unspecified1 => write!(f, "Unspecified1"),
            Self::NonIDRSliceLayerWithoutPartitioning => write!(f, "NonIDRSliceLayerWithoutPartitioning"),
            Self::SliceDataPartitionALayer => write!(f, "SliceDataPartitionALayer"),
            Self::SliceDataPartitionBLayer => write!(f, "SliceDataPartitionBLayer"),
            Self::SliceDataPartitionCLayer => write!(f, "SliceDataPartitionCLayer"),
            Self::IDRSliceLayerWithoutPartitioning => write!(f, "IDRSliceLayerWithoutPartitioning"),
            Self::SEI => write!(f, "SEI"),
            Self::SPS => write!(f, "SPS"),
            Self::PPS => write!(f, "PPS"),
            Self::AccessUnitDelimiter => write!(f, "AccessUnitDelimiter"),
            Self::EndOfSeq => write!(f, "EndOfSeq"),
            Self::EndOfStream => write!(f, "EndOfStream"),
            Self::FillerData => write!(f, "FillerData"),
            Self::SPSExtension => write!(f, "SPSExtension"),
            Self::PrefixNalUnit => write!(f, "PrefixNalUnit"),
            Self::SubsetSPS => write!(f, "SubsetSPS"),
            Self::DepthParameterSet => write!(f, "DepthParameterSet"),
            Self::Reserved1 => write!(f, "Reserved1"),
            Self::Reserved2 => write!(f, "Reserved2"),
            Self::AuxCodedPictureSliceLayerWithoutPartitioning => write!(f, "AuxCodedPictureSliceLayerWithoutPartitioning"),
            Self::SliceLayerExtension => write!(f, "SliceLayerExtension"),
            Self::SliceLayerExtension2 => write!(f, "SliceLayerExtension2"),
            Self::Reserved3 => write!(f, "Reserved3"),
            Self::Reserved4 => write!(f, "Reserved4"),
            Self::Unspecified2 => write!(f, "Unspecified2"),
        }
    }
}

impl TryFrom<u8> for NALUnitType {
    type Error = io::Error;
    /// Converts a `u8` value to a `NALUnitType`.
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(NALUnitType::Unspecified1),
            1 => Ok(NALUnitType::NonIDRSliceLayerWithoutPartitioning),
            2 => Ok(NALUnitType::SliceDataPartitionALayer),
            3 => Ok(NALUnitType::SliceDataPartitionBLayer),
            4 => Ok(NALUnitType::SliceDataPartitionCLayer),
            5 => Ok(NALUnitType::IDRSliceLayerWithoutPartitioning),
            6 => Ok(NALUnitType::SEI),
            7 => Ok(NALUnitType::SPS),
            8 => Ok(NALUnitType::PPS),
            9 => Ok(NALUnitType::AccessUnitDelimiter),
            10 => Ok(NALUnitType::EndOfSeq),
            11 => Ok(NALUnitType::EndOfStream),
            12 => Ok(NALUnitType::FillerData),
            13 => Ok(NALUnitType::SPSExtension),
            14 => Ok(NALUnitType::PrefixNalUnit),
            15 => Ok(NALUnitType::SubsetSPS),
            16 => Ok(NALUnitType::DepthParameterSet),
            17 => Ok(NALUnitType::Reserved1),
            18 => Ok(NALUnitType::Reserved2),
            19 => Ok(NALUnitType::AuxCodedPictureSliceLayerWithoutPartitioning),
            20 => Ok(NALUnitType::SliceLayerExtension),
            21 => Ok(NALUnitType::SliceLayerExtension2),
            22 => Ok(NALUnitType::Reserved3),
            23 => Ok(NALUnitType::Reserved4),
            24..31 => Ok(NALUnitType::Unspecified2), // Application-specific
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid NAL unit type: {value}"),
            )),
        }
    }
}
