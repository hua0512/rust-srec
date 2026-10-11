//! HLS output: assembled segment payloads as consumer-facing `HlsData`.

use hls::{HlsData, M4sData, M4sInitSegmentData, M4sSegmentData};

use crate::segment_engine::descriptor::{SegmentDescriptor, SegmentMeta};
use crate::segment_engine::payload::SegmentPayload;

impl SegmentPayload {
    /// Convert into the consumer-facing `HlsData`. The `MediaSegment` clone is
    /// metadata-only; the media `Bytes` moves as a handle.
    pub fn into_hls_data(self) -> HlsData {
        match self {
            // A TS init section is TS packets; consumers take it as TS data.
            Self::Ts { data, descriptor } | Self::TsInit { data, descriptor } => {
                HlsData::ts(media_segment(&descriptor), data)
            }
            Self::Mp4Init { data, descriptor } => {
                HlsData::M4sData(M4sData::InitSegment(M4sInitSegmentData {
                    segment: media_segment(&descriptor),
                    data,
                }))
            }
            Self::Mp4Media { data, descriptor } => {
                HlsData::M4sData(M4sData::Segment(M4sSegmentData {
                    segment: media_segment(&descriptor),
                    data,
                }))
            }
        }
    }
}

fn media_segment(descriptor: &SegmentDescriptor) -> m3u8_rs::MediaSegment {
    match &descriptor.meta {
        SegmentMeta::Hls(segment) => segment.as_ref().clone(),
    }
}
