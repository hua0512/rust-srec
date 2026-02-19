use bytes::Bytes;
pub use pipeline_common::split_reason::SplitReason;

use crate::{header::FlvHeader, tag::FlvTag};

#[derive(Debug, Clone, PartialEq)]
pub enum FlvData {
    Header(FlvHeader),
    Tag(FlvTag),
    /// Explicit split marker emitted before a re-injected header.
    Split(SplitReason),
    EndOfSequence(Bytes),
}

impl FlvData {
    pub fn size(&self) -> usize {
        match self {
            FlvData::Header(_) => 9 + 4,
            FlvData::Tag(tag) => tag.size() + 4,
            FlvData::Split(_) => 0,
            FlvData::EndOfSequence(data) => data.len() + 4,
        }
    }

    pub fn is_header(&self) -> bool {
        matches!(self, FlvData::Header(_))
    }

    pub fn is_tag(&self) -> bool {
        matches!(self, FlvData::Tag(_))
    }

    pub fn is_split(&self) -> bool {
        matches!(self, FlvData::Split(_))
    }

    pub fn is_end_of_sequence(&self) -> bool {
        matches!(self, FlvData::EndOfSequence(_))
    }

    // Helper for easier comparison in tests, ignoring data potentially
    pub fn description(&self) -> String {
        match self {
            FlvData::Header(_) => "Header".to_string(),
            FlvData::Tag(tag) => format!("{:?}@{}", tag.tag_type, tag.timestamp_ms),
            FlvData::Split(reason) => format!("Split({reason:?})"),
            FlvData::EndOfSequence(_) => "EndOfSequence".to_string(),
        }
    }
}
