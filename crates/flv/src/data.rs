use bytes::Bytes;

use crate::{header::FlvHeader, tag::FlvTag};

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
