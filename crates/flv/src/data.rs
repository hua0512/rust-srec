use bytes::Bytes;

use crate::{
    header::FlvHeader,
    tag::{FlvTag, FlvTagOwned, FlvUtil},
};

#[derive(Debug, Clone, PartialEq)]
pub enum FlvDataOwned {
    Header(FlvHeader),
    Tag(FlvTagOwned),
    EndOfSequence(Bytes),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FlvData {
    Header(FlvHeader),
    Tag(FlvTag),
    EndOfSequence(Bytes),
}

impl FlvData {
    pub fn size(&self) -> usize {
        match self {
            FlvData::Header(_) => 9 + 4,
            FlvData::Tag(tag) => tag.size() + 4,
            FlvData::EndOfSequence(data) => data.len() + 4,
        }
    }

    pub fn is_header(&self) -> bool {
        matches!(self, FlvData::Header(_))
    }

    pub fn is_tag(&self) -> bool {
        matches!(self, FlvData::Tag(_))
    }

    pub fn is_end_of_sequence(&self) -> bool {
        matches!(self, FlvData::EndOfSequence(_))
    }

    // Helper for easier comparison in tests, ignoring data potentially
    pub fn description(&self) -> String {
        match self {
            FlvData::Header(_) => "Header".to_string(),
            FlvData::Tag(tag) => format!("{:?}@{}", tag.tag_type, tag.timestamp_ms),
            FlvData::EndOfSequence(_) => "EndOfSequence".to_string(),
        }
    }
}

impl FlvDataOwned {
    pub fn timestamp(&self) -> u32 {
        match self {
            FlvDataOwned::Header(_) => 0,
            FlvDataOwned::Tag(tag) => tag.timestamp_ms,
            FlvDataOwned::EndOfSequence(_) => 0,
        }
    }

    pub fn is_header(&self) -> bool {
        matches!(self, FlvDataOwned::Header(_))
    }

    pub fn is_end_of_sequence(&self) -> bool {
        matches!(self, FlvDataOwned::EndOfSequence(_))
    }

    pub fn is_key_frame(&self) -> bool {
        match self {
            FlvDataOwned::Header(_) => false,
            FlvDataOwned::Tag(tag) => tag.is_key_frame(),
            FlvDataOwned::EndOfSequence(_) => false,
        }
    }
}
