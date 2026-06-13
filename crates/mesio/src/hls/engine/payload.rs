//! Typed media payloads.
//!
//! Payloads move through channels by value; cloning clones `Bytes` handles,
//! never byte buffers. The clear (unencrypted) path from fetcher to output is
//! zero-copy.

use bytes::Bytes;
use std::sync::Arc;

use hls::{HlsData, M4sData, M4sInitSegmentData, M4sSegmentData};

use super::descriptor::SegmentDescriptor;

#[derive(Debug, Clone)]
pub enum SegmentPayload {
    Ts {
        data: Bytes,
        descriptor: Arc<SegmentDescriptor>,
    },
    Mp4Init {
        data: Bytes,
        descriptor: Arc<SegmentDescriptor>,
    },
    Mp4Media {
        data: Bytes,
        descriptor: Arc<SegmentDescriptor>,
    },
}

impl SegmentPayload {
    pub fn descriptor(&self) -> &Arc<SegmentDescriptor> {
        match self {
            Self::Ts { descriptor, .. }
            | Self::Mp4Init { descriptor, .. }
            | Self::Mp4Media { descriptor, .. } => descriptor,
        }
    }

    pub fn msn(&self) -> u64 {
        self.descriptor().msn
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Ts { data, .. } | Self::Mp4Init { data, .. } | Self::Mp4Media { data, .. } => {
                data.len()
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_init(&self) -> bool {
        matches!(self, Self::Mp4Init { .. })
    }

    pub fn is_fmp4(&self) -> bool {
        matches!(self, Self::Mp4Init { .. } | Self::Mp4Media { .. })
    }

    pub fn discontinuity(&self) -> bool {
        self.descriptor().discontinuity
    }

    /// Convert into the consumer-facing `HlsData`. The `MediaSegment` clone is
    /// metadata-only; the media `Bytes` moves as a handle.
    pub fn into_hls_data(self) -> HlsData {
        match self {
            Self::Ts { data, descriptor } => {
                HlsData::ts(descriptor.media_segment.as_ref().clone(), data)
            }
            Self::Mp4Init { data, descriptor } => {
                HlsData::M4sData(M4sData::InitSegment(M4sInitSegmentData {
                    segment: descriptor.media_segment.as_ref().clone(),
                    data,
                }))
            }
            Self::Mp4Media { data, descriptor } => {
                HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                    segment: descriptor.media_segment.as_ref().clone(),
                    data,
                }))
            }
        }
    }
}
