//! Typed media payloads.
//!
//! Payloads move through channels by value; cloning clones `Bytes` handles,
//! never byte buffers. The clear (unencrypted) path from fetcher to output is
//! zero-copy.

use bytes::Bytes;
use std::sync::Arc;

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
    /// An `EXT-X-MAP` section of an MPEG-TS stream (PAT/PMT packets).
    TsInit {
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
            | Self::TsInit { descriptor, .. }
            | Self::Mp4Media { descriptor, .. } => descriptor,
        }
    }

    pub fn msn(&self) -> u64 {
        self.descriptor().msn
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Ts { data, .. }
            | Self::Mp4Init { data, .. }
            | Self::TsInit { data, .. }
            | Self::Mp4Media { data, .. } => data.len(),
        }
    }

    pub fn is_init(&self) -> bool {
        matches!(self, Self::Mp4Init { .. } | Self::TsInit { .. })
    }

    pub fn is_fmp4(&self) -> bool {
        matches!(self, Self::Mp4Init { .. } | Self::Mp4Media { .. })
    }

    pub fn discontinuity(&self) -> bool {
        self.descriptor().discontinuity
    }
}
