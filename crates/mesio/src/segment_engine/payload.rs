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

    /// A copy of an init to deliver again ahead of the media at `msn`. It
    /// carries no discontinuity: that belongs to the delivery the init was
    /// planned with, not to a later use of the same map.
    pub fn replayed_at(&self, msn: u64) -> Self {
        if self.msn() == msn && !self.discontinuity() {
            return self.clone();
        }
        self.with_descriptor(|descriptor| {
            descriptor.msn = msn;
            descriptor.discontinuity = false;
        })
    }

    /// A copy that carries a discontinuity.
    pub fn with_discontinuity(&self) -> Self {
        if self.discontinuity() {
            return self.clone();
        }
        self.with_descriptor(|descriptor| descriptor.discontinuity = true)
    }

    fn with_descriptor(&self, edit: impl FnOnce(&mut SegmentDescriptor)) -> Self {
        let mut updated = (**self.descriptor()).clone();
        edit(&mut updated);
        let descriptor = Arc::new(updated);
        match self {
            Self::Ts { data, .. } => Self::Ts {
                data: data.clone(),
                descriptor,
            },
            Self::Mp4Init { data, .. } => Self::Mp4Init {
                data: data.clone(),
                descriptor,
            },
            Self::TsInit { data, .. } => Self::TsInit {
                data: data.clone(),
                descriptor,
            },
            Self::Mp4Media { data, .. } => Self::Mp4Media {
                data: data.clone(),
                descriptor,
            },
        }
    }
}
