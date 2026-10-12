//! HLS output: the assembler's items as consumer-facing `HlsStreamEvent`s.

use hls::{HlsData, M4sData, M4sInitSegmentData, M4sSegmentData};
use tokio::sync::mpsc;

use crate::DownloadError;
use crate::hls::events::HlsStreamEvent;
use crate::segment_engine::assembler::{AssembledItem, AssemblerSink, SinkClosed};
use crate::segment_engine::control::EngineNotice;
use crate::segment_engine::descriptor::{SegmentDescriptor, SegmentMeta};
use crate::segment_engine::payload::SegmentPayload;

/// Forwards assembled items to the HLS client event channel.
pub(crate) struct HlsEventSink {
    tx: mpsc::Sender<Result<HlsStreamEvent, DownloadError>>,
}

impl HlsEventSink {
    pub(crate) fn new(tx: mpsc::Sender<Result<HlsStreamEvent, DownloadError>>) -> Self {
        Self { tx }
    }
}

impl AssemblerSink for HlsEventSink {
    async fn send(&mut self, item: Result<AssembledItem, DownloadError>) -> Result<(), SinkClosed> {
        let event = match item {
            Ok(item) => match hls_event(item) {
                Some(event) => Ok(event),
                None => return Ok(()),
            },
            Err(error) => Err(error),
        };
        self.tx.send(event).await.map_err(|_| SinkClosed)
    }
}

/// The client event for an assembled item; `None` for notices HLS has no
/// event for (playlists have no track-level sections).
fn hls_event(item: AssembledItem) -> Option<HlsStreamEvent> {
    Some(match item {
        AssembledItem::Payload(payload) => HlsStreamEvent::Data(Box::new(payload.into_hls_data())),
        AssembledItem::Discontinuity => HlsStreamEvent::DiscontinuityTagEncountered {},
        AssembledItem::GapSkipped {
            from_sequence,
            to_sequence,
            reason,
        } => HlsStreamEvent::GapSkipped {
            from_sequence,
            to_sequence,
            reason,
        },
        AssembledItem::SegmentTimeout {
            sequence_number,
            waited_duration,
        } => HlsStreamEvent::SegmentTimeout {
            sequence_number,
            waited_duration,
        },
        AssembledItem::Notice(EngineNotice::ManifestRefreshed {
            sequence_base,
            target_duration,
        }) => HlsStreamEvent::PlaylistRefreshed {
            media_sequence_base: sequence_base,
            target_duration,
        },
        AssembledItem::Notice(EngineNotice::End) => HlsStreamEvent::EndlistEncountered,
        AssembledItem::Notice(EngineNotice::TrackAbsent { .. }) => return None,
        AssembledItem::StreamEnded => HlsStreamEvent::StreamEnded,
    })
}

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
        // DASH output is built by its muxer; this keeps a stray DASH payload
        // describable rather than unrepresentable.
        SegmentMeta::Dash(meta) => m3u8_rs::MediaSegment {
            uri: crate::redact::redact_url(&descriptor.parsed_url).to_string(),
            duration: meta.duration_secs() as f32,
            ..Default::default()
        },
    }
}
