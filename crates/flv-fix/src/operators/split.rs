//! # SplitOperator
//!
//! The `SplitOperator` processes FLV (Flash Video) streams and manages stream splitting
//! when video or audio parameters change.
//!
//! ## Purpose
//!
//! Media streams sometimes change encoding parameters mid-stream (resolution, bitrate,
//! codec settings). These changes require re-initialization of decoders, which many
//! players handle poorly. This operator detects such changes and helps maintain
//! proper playback by:
//!
//! 1. Monitoring audio and video sequence headers for parameter changes
//! 2. Re-injecting stream initialization data (headers, metadata) when changes occur
//! 3. Ensuring players can properly handle parameter transitions
//!
//! ## Operation
//!
//! The operator:
//! - Tracks FLV headers, metadata tags, and sequence headers
//! - Computes signatures of sequence headers to detect config changes
//! - When changes are detected, marks the stream for splitting
//! - At the next regular media tag, re-injects headers and sequence information
//!
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!
use std::sync::Arc;

use flv::audio::AudioCodec;
use flv::data::FlvData;
use flv::tag::{FlvTag, SequenceHeader};
use flv::video::{EnhancedPacketType, VideoCodec};
use pipeline_common::split_reason::{AudioCodecInfo, SplitReason, VideoCodecInfo};
use pipeline_common::{PipelineError, Processor, StreamerContext, crc32};
use tracing::{debug, info};

use crate::operators::segment_reinject::SegmentInitCache;

/// Controls how `SplitOperator` decides whether a sequence header "changed".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SequenceHeaderChangeMode {
    /// Legacy behavior: compute CRC32 over the full tag payload (`tag.data`).
    ///
    /// This triggers splits on any byte-level change, even if the decoder
    /// configuration is semantically identical.
    #[default]
    Crc32,
    /// Compare codec configuration by hashing only the relevant configuration
    /// portion of the sequence header.
    ///
    /// This reduces unnecessary splits caused by non-config fields changing
    /// (e.g. AVC composition-time bytes or legacy FLV audio header bits).
    /// Known timestamp ModEx records are ignored; unsupported enhanced layouts
    /// conservatively use the complete payload. Codec identity is compared
    /// directly in addition to the configuration signature.
    SemanticSignature,
}

// Codec identity is compared directly, independently of the configuration CRC.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SequenceKey {
    Raw(u32),
    Audio {
        codec: AudioCodec,
        signature: u32,
    },
    Video {
        codec: VideoCodec,
        packet_type: EnhancedPacketType,
        signature: u32,
    },
}

impl SequenceKey {
    fn signature(self) -> u32 {
        match self {
            Self::Raw(signature)
            | Self::Audio { signature, .. }
            | Self::Video { signature, .. } => signature,
        }
    }
}

struct StreamState {
    /// Cached header/metadata/sequence tags re-injected by `split_stream`.
    cache: SegmentInitCache,
    /// Key for detecting changes in the last seen video sequence header.
    ///
    /// The exact meaning depends on `SequenceHeaderChangeMode`.
    video_key: Option<SequenceKey>,
    /// Key for detecting changes in the last seen audio sequence header.
    ///
    /// The exact meaning depends on `SequenceHeaderChangeMode`.
    audio_key: Option<SequenceKey>,
    /// Parsed codec info from the video sequence header *before* the change.
    /// Populated eagerly when a codec change is detected, so we don't need
    /// to keep the full `FlvTag` around.
    prev_video_codec_info: Option<VideoCodecInfo>,
    /// Parsed codec info from the audio sequence header *before* the change.
    prev_audio_codec_info: Option<AudioCodecInfo>,
    /// Whether we've emitted any non-header/non-metadata/non-sequence *media* tag since the last
    /// header injection.
    ///
    /// This helps avoid creating an initial "empty segment" when upstream sends multiple sequence
    /// headers before the first media tag.
    has_emitted_media_tag: bool,
    changed: bool,
    buffered_metadata: bool,
    buffered_audio_sequence_tag: bool,
    buffered_video_sequence_tag: bool,
}

impl StreamState {
    fn new() -> Self {
        Self {
            cache: SegmentInitCache::new(),
            video_key: None,
            audio_key: None,
            prev_video_codec_info: None,
            prev_audio_codec_info: None,
            has_emitted_media_tag: false,
            changed: false,
            buffered_metadata: false,
            buffered_audio_sequence_tag: false,
            buffered_video_sequence_tag: false,
        }
    }

    fn reset(&mut self) {
        self.cache.clear();
        self.video_key = None;
        self.audio_key = None;
        self.prev_video_codec_info = None;
        self.prev_audio_codec_info = None;
        self.has_emitted_media_tag = false;
        self.changed = false;
        self.buffered_metadata = false;
        self.buffered_audio_sequence_tag = false;
        self.buffered_video_sequence_tag = false;
    }
}

pub struct SplitOperator {
    context: Arc<StreamerContext>,
    state: StreamState,
    drop_duplicate_sequence_headers: bool,
    sequence_header_change_mode: SequenceHeaderChangeMode,
}

impl SplitOperator {
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self::with_config(context, SequenceHeaderChangeMode::default(), false)
    }

    pub fn with_config(
        context: Arc<StreamerContext>,
        sequence_header_change_mode: SequenceHeaderChangeMode,
        drop_duplicate_sequence_headers: bool,
    ) -> Self {
        Self {
            context,
            state: StreamState::new(),
            drop_duplicate_sequence_headers,
            sequence_header_change_mode,
        }
    }

    fn sequence_change_key(&self, tag: &FlvTag) -> SequenceKey {
        if self.sequence_header_change_mode == SequenceHeaderChangeMode::SemanticSignature {
            match tag.sequence_header() {
                Some(SequenceHeader::Audio {
                    codec,
                    configuration,
                }) => {
                    let state = crc32::crc32(&codec.as_u32().to_be_bytes());
                    return SequenceKey::Audio {
                        codec,
                        signature: crc32::crc32_update(state, configuration),
                    };
                }
                Some(SequenceHeader::Video {
                    codec,
                    packet_type,
                    configuration,
                }) => {
                    let state = crc32::crc32(&codec.as_u32().to_be_bytes());
                    let state = crc32::crc32_update(state, &[packet_type.0]);
                    return SequenceKey::Video {
                        codec,
                        packet_type,
                        signature: crc32::crc32_update(state, configuration),
                    };
                }
                // Unknown modifiers, codecs and multitrack layouts retain all
                // their bytes; never guess which fields can be discarded.
                None => {}
            }
        }
        SequenceKey::Raw(crc32::crc32(tag.data()))
    }

    /// Codec info carrying only a name, for tags `VideoData::demux` cannot
    /// deep-parse (`EnhancedPacket::Unknown`, multitrack/ModEx wrappers) or
    /// cannot parse at all. `FlvTag::get_video_codec` covers both legacy codec
    /// IDs and enhanced FourCCs; enhanced names must match the strings the
    /// deep-parse arms in `extract_video_codec_info` produce ("AVC", "HEVC",
    /// "AV1") because `VideoCodecInfo::codec` pairs `from`/`to` values in
    /// `SplitReason::VideoCodecChange`.
    fn fallback_video_codec_info(tag: &FlvTag, signature: u32) -> VideoCodecInfo {
        use flv::video::{VideoCodec, VideoFourCC};

        let codec = match tag.get_video_codec() {
            Some(VideoCodec::Legacy(id)) => format!("{id:?}"),
            Some(VideoCodec::Enhanced(VideoFourCC::Avc1)) => "AVC".to_string(),
            Some(VideoCodec::Enhanced(VideoFourCC::Hvc1)) => "HEVC".to_string(),
            Some(VideoCodec::Enhanced(VideoFourCC::Av01)) => "AV1".to_string(),
            Some(VideoCodec::Enhanced(VideoFourCC::Vp08)) => "VP8".to_string(),
            Some(VideoCodec::Enhanced(VideoFourCC::Vp09)) => "VP9".to_string(),
            None => "unknown".to_string(),
        };
        VideoCodecInfo {
            codec,
            profile: None,
            level: None,
            width: None,
            height: None,
            signature,
        }
    }

    /// Extract video codec configuration info from a sequence header tag.
    ///
    /// Does best-effort deep parsing to extract codec name, profile, level,
    /// and resolution from the tag data.
    fn extract_video_codec_info(tag: &FlvTag, signature: u32) -> VideoCodecInfo {
        use flv::av1::Av1Packet;
        use flv::avc::AvcPacket;
        use flv::hevc::HevcPacket;
        use flv::video::{EnhancedPacket, VideoData, VideoTagBody};

        let data = tag.data().clone();
        let mut cursor = std::io::Cursor::new(data);

        // A multitrack tag reports its first track, matching
        // `FlvTag::get_video_codec`, which reads the first track's FourCC.
        let body = match VideoData::demux(&mut cursor) {
            Ok(video) => match video.body {
                VideoTagBody::Multitrack(mut tracks) if !tracks.is_empty() => {
                    VideoTagBody::Enhanced(tracks.remove(0).body)
                }
                body => body,
            },
            Err(_) => return Self::fallback_video_codec_info(tag, signature),
        };

        let resolution = body.get_video_resolution();
        match body {
            VideoTagBody::Avc(AvcPacket::SequenceHeader(config))
            | VideoTagBody::Enhanced(EnhancedPacket::Avc(AvcPacket::SequenceHeader(config))) => {
                VideoCodecInfo {
                    codec: "AVC".to_string(),
                    profile: Some(config.profile_indication),
                    level: Some(config.level_indication),
                    width: resolution.as_ref().map(|r| r.width as u32),
                    height: resolution.as_ref().map(|r| r.height as u32),
                    signature,
                }
            }
            VideoTagBody::Hevc(HevcPacket::SequenceStart(config))
            | VideoTagBody::Enhanced(EnhancedPacket::Hevc(HevcPacket::SequenceStart(config))) => {
                VideoCodecInfo {
                    codec: "HEVC".to_string(),
                    profile: Some(config.general_profile_idc),
                    level: Some(config.general_level_idc),
                    width: resolution.as_ref().map(|r| r.width as u32),
                    height: resolution.as_ref().map(|r| r.height as u32),
                    signature,
                }
            }
            VideoTagBody::Enhanced(EnhancedPacket::Av1(Av1Packet::SequenceStart(config))) => {
                VideoCodecInfo {
                    codec: "AV1".to_string(),
                    profile: Some(config.seq_profile),
                    level: Some(config.seq_level_idx_0),
                    width: resolution.as_ref().map(|r| r.width as u32),
                    height: resolution.as_ref().map(|r| r.height as u32),
                    signature,
                }
            }
            _ => Self::fallback_video_codec_info(tag, signature),
        }
    }

    /// Extract audio codec configuration info from a sequence header tag.
    ///
    /// For AAC, parses AudioSpecificConfig to extract sample rate and channels.
    /// For other codecs, returns the codec name only.
    fn extract_audio_codec_info(tag: &FlvTag, signature: u32) -> AudioCodecInfo {
        use flv::audio::AudioCodec;

        // `AudioFourCC`'s Debug names line up with `SoundFormat`'s (e.g.
        // "Aac", "Mp3"), so an enhanced tag reports the same codec string a
        // legacy tag with the equivalent format would, instead of "ExHeader".
        let codec_name = match tag.get_audio_codec() {
            Some(AudioCodec::Legacy(sound_format)) => format!("{sound_format:?}"),
            Some(AudioCodec::Enhanced(four_cc)) => format!("{four_cc:?}"),
            None => "unknown".to_string(),
        };

        let data = tag.data().as_ref();

        // For AAC: parse AudioSpecificConfig
        // Layout: [audio_header_byte][0x00=seq_header][AudioSpecificConfig...]
        let is_aac = data.first().is_some_and(|b| (b >> 4) & 0x0F == 10);
        if is_aac && data.len() >= 4 {
            // AudioSpecificConfig (ISO 14496-3):
            // First 5 bits: audioObjectType
            // Next 4 bits: samplingFrequencyIndex
            // Next 4 bits: channelConfiguration
            let asc = &data[2..];
            if asc.len() >= 2 {
                let byte0 = asc[0];
                let byte1 = asc[1];

                // Object type is bits [7..3] of byte0 (5 bits)
                let _object_type = byte0 >> 3;
                // Sample rate index is bits [2..0] of byte0 + bit [7] of byte1 (4 bits)
                let freq_index = ((byte0 & 0x07) << 1) | (byte1 >> 7);
                // Channel config is bits [6..3] of byte1 (4 bits)
                let channel_config = (byte1 >> 3) & 0x0F;

                static AAC_SAMPLE_RATES: [u32; 13] = [
                    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025,
                    8000, 7350,
                ];

                let sample_rate = AAC_SAMPLE_RATES.get(freq_index as usize).copied();
                let channels = if channel_config > 0 && channel_config <= 7 {
                    Some(channel_config)
                } else {
                    None
                };

                return AudioCodecInfo {
                    codec: "AAC".to_string(),
                    sample_rate,
                    channels,
                    signature,
                };
            }
        }

        AudioCodecInfo {
            codec: codec_name,
            sample_rate: None,
            channels: None,
            signature,
        }
    }

    // Split stream and re-inject header+sequence data
    fn split_stream(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // Emit Split markers before the header re-injection.
        if self.state.buffered_video_sequence_tag
            && let Some(from) = self.state.prev_video_codec_info.take()
        {
            let new_sig = self.state.video_key.map_or(0, SequenceKey::signature);
            let to = self
                .state
                .cache
                .video_sequence_tag
                .as_ref()
                .map(|t| Self::extract_video_codec_info(t, new_sig))
                .unwrap_or_else(|| VideoCodecInfo {
                    codec: "unknown".to_string(),
                    profile: None,
                    level: None,
                    width: None,
                    height: None,
                    signature: new_sig,
                });
            output(FlvData::Split(SplitReason::VideoCodecChange { from, to }))?;
        }
        if self.state.buffered_audio_sequence_tag
            && let Some(from) = self.state.prev_audio_codec_info.take()
        {
            let new_sig = self.state.audio_key.map_or(0, SequenceKey::signature);
            let to = self
                .state
                .cache
                .audio_sequence_tag
                .as_ref()
                .map(|t| Self::extract_audio_codec_info(t, new_sig))
                .unwrap_or_else(|| AudioCodecInfo {
                    codec: "unknown".to_string(),
                    sample_rate: None,
                    channels: None,
                    signature: new_sig,
                });
            output(FlvData::Split(SplitReason::AudioCodecChange { from, to }))?;
        }

        // Re-inject the cached header/metadata/sequence tags with the timestamps they
        // were stored with; the timeline is not reset, so downstream components may
        // need to handle a timestamp discontinuity at the split point.
        self.state.cache.reinject(output, None)?;
        self.state.changed = false;
        self.state.buffered_metadata = false;
        self.state.buffered_audio_sequence_tag = false;
        self.state.buffered_video_sequence_tag = false;
        self.state.has_emitted_media_tag = false;
        info!("{} Stream split", self.context.name);
        Ok(())
    }

    fn flush_buffered_tags_if_pending(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if !self.state.changed {
            return Ok(());
        }

        // We intentionally do NOT inject a new header here to avoid creating an empty segment (and
        // triggering writer rotation) when the stream ends before the next media tag arrives.
        if self.state.buffered_metadata
            && let Some(metadata) = self.state.cache.metadata.take()
        {
            output(FlvData::Tag(metadata))?;
        }
        if self.state.buffered_video_sequence_tag
            && let Some(video_seq) = self.state.cache.video_sequence_tag.take()
        {
            output(FlvData::Tag(video_seq))?;
        }
        if self.state.buffered_audio_sequence_tag
            && let Some(audio_seq) = self.state.cache.audio_sequence_tag.take()
        {
            output(FlvData::Tag(audio_seq))?;
        }

        self.state.changed = false;
        self.state.buffered_metadata = false;
        self.state.buffered_audio_sequence_tag = false;
        self.state.buffered_video_sequence_tag = false;
        Ok(())
    }
}

impl Processor<FlvData> for SplitOperator {
    fn process(
        &mut self,
        context: &Arc<StreamerContext>,
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if context.token.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        match input {
            FlvData::Header(header) => {
                // If we already have a header, this is a stream restart — emit a Split marker.
                let is_restart = self.state.cache.header.is_some();
                // Reset state when a new header is encountered
                self.state.reset();
                self.state.cache.header = Some(header.clone());
                if is_restart {
                    output(FlvData::Split(SplitReason::HeaderReceived))?;
                }
                output(FlvData::Header(header))
            }
            FlvData::EndOfSequence(_) => {
                // If we buffered tags for a pending split but never saw a regular media tag,
                // don't drop them on end-of-stream.
                self.flush_buffered_tags_if_pending(output)?;
                output(input)
            }
            FlvData::Tag(tag) => {
                // If we're waiting to split, buffer metadata/sequence headers and only emit once we
                // see the first regular media tag. This prevents duplicate sequence headers around
                // split points and ensures the injected header precedes new codec config.
                if self.state.changed {
                    if tag.is_script_tag() {
                        debug!(
                            "{} Metadata detected while split pending",
                            self.context.name
                        );
                        self.state.cache.store_metadata(tag);
                        self.state.buffered_metadata = true;
                        return Ok(());
                    }
                    if tag.is_video_sequence_header() {
                        debug!(
                            "{} Video sequence tag detected while split pending",
                            self.context.name
                        );
                        // If this is a new video config (different from what we had), save the old one.
                        let new_sig = self.sequence_change_key(&tag);
                        if let Some(old_sig) = self.state.video_key
                            && old_sig != new_sig
                            && let Some(old_tag) = self.state.cache.video_sequence_tag.as_ref()
                        {
                            self.state.prev_video_codec_info =
                                Some(Self::extract_video_codec_info(old_tag, old_sig.signature()));
                        }
                        self.state.cache.store_video_sequence_tag(tag, false);
                        self.state.buffered_video_sequence_tag = true;
                        self.state.video_key = Some(new_sig);
                        return Ok(());
                    }
                    if tag.is_audio_sequence_header() {
                        debug!(
                            "{} Audio sequence tag detected while split pending",
                            self.context.name
                        );
                        // If this is a new audio config (different from what we had), save the old one.
                        let new_sig = self.sequence_change_key(&tag);
                        if let Some(old_sig) = self.state.audio_key
                            && old_sig != new_sig
                            && let Some(old_tag) = self.state.cache.audio_sequence_tag.as_ref()
                        {
                            self.state.prev_audio_codec_info =
                                Some(Self::extract_audio_codec_info(old_tag, old_sig.signature()));
                        }
                        self.state.cache.store_audio_sequence_tag(tag, false);
                        self.state.buffered_audio_sequence_tag = true;
                        self.state.audio_key = Some(new_sig);
                        return Ok(());
                    }

                    // First regular tag after a pending change: split now, then emit the tag.
                    self.split_stream(output)?;
                    self.state.has_emitted_media_tag = true;
                    return output(FlvData::Tag(tag));
                }

                // Normal operation: track key tags and detect parameter changes.
                if tag.is_script_tag() {
                    debug!("{} Metadata detected", self.context.name);
                    self.state.cache.store_metadata(tag.clone());
                    return output(FlvData::Tag(tag));
                }

                if tag.is_video_sequence_header() {
                    debug!("{} Video sequence tag detected", self.context.name);
                    let sig = self.sequence_change_key(&tag);

                    if self.drop_duplicate_sequence_headers
                        && self.state.video_key.is_some_and(|prev| prev == sig)
                    {
                        debug!(
                            "{} Dropping duplicate video sequence header (sig: {:x})",
                            self.context.name,
                            sig.signature()
                        );
                        self.state.cache.store_video_sequence_tag(tag, false);
                        self.state.video_key = Some(sig);
                        return Ok(());
                    }

                    if let Some(prev_sig) = self.state.video_key
                        && prev_sig != sig
                    {
                        // If the stream hasn't produced any media tags yet, upstream may still be
                        // negotiating/settling the initial codec configuration (common right at
                        // stream start). Splitting here creates an "empty" first segment consisting
                        // only of headers/sequence tags.
                        if self.state.has_emitted_media_tag {
                            info!(
                                "{} Video sequence header changed (sig: {:x} -> {:x}), marking for split",
                                self.context.name,
                                prev_sig.signature(),
                                sig.signature()
                            );
                            // Eagerly extract codec info from the old tag before we overwrite it.
                            if let Some(old_tag) = self.state.cache.video_sequence_tag.as_ref() {
                                self.state.prev_video_codec_info = Some(
                                    Self::extract_video_codec_info(old_tag, prev_sig.signature()),
                                );
                            }
                            self.state.changed = true;
                            self.state.buffered_video_sequence_tag = true;
                        } else {
                            debug!(
                                "{} Video sequence header changed before first media tag (CRC: {:x} -> {:x}); treating as initial config update (no split)",
                                self.context.name,
                                prev_sig.signature(),
                                sig.signature()
                            );
                        }
                    }
                    self.state
                        .cache
                        .store_video_sequence_tag(tag.clone(), false);
                    self.state.video_key = Some(sig);

                    // If we just detected a change, buffer the new header and wait for the next
                    // regular tag to inject a fresh header+sequence set.
                    if self.state.changed {
                        return Ok(());
                    }

                    return output(FlvData::Tag(tag));
                }

                if tag.is_audio_sequence_header() {
                    debug!("{} Audio sequence tag detected", self.context.name);
                    let sig = self.sequence_change_key(&tag);

                    if self.drop_duplicate_sequence_headers
                        && self.state.audio_key.is_some_and(|prev| prev == sig)
                    {
                        debug!(
                            "{} Dropping duplicate audio sequence header (sig: {:x})",
                            self.context.name,
                            sig.signature()
                        );
                        self.state.cache.store_audio_sequence_tag(tag, false);
                        self.state.audio_key = Some(sig);
                        return Ok(());
                    }

                    if let Some(prev_sig) = self.state.audio_key
                        && prev_sig != sig
                    {
                        if self.state.has_emitted_media_tag {
                            info!(
                                "{} Audio parameters changed (sig: {:x} -> {:x})",
                                self.context.name,
                                prev_sig.signature(),
                                sig.signature()
                            );
                            // Eagerly extract codec info from the old tag before we overwrite it.
                            if let Some(old_tag) = self.state.cache.audio_sequence_tag.as_ref() {
                                self.state.prev_audio_codec_info = Some(
                                    Self::extract_audio_codec_info(old_tag, prev_sig.signature()),
                                );
                            }
                            self.state.changed = true;
                            self.state.buffered_audio_sequence_tag = true;
                        } else {
                            debug!(
                                "{} Audio sequence header changed before first media tag (CRC: {:x} -> {:x}); treating as initial config update (no split)",
                                self.context.name,
                                prev_sig.signature(),
                                sig.signature()
                            );
                        }
                    }
                    self.state
                        .cache
                        .store_audio_sequence_tag(tag.clone(), false);
                    self.state.audio_key = Some(sig);

                    if self.state.changed {
                        return Ok(());
                    }

                    return output(FlvData::Tag(tag));
                }

                // Regular media tag: if a change was detected earlier, split before emitting.
                if self.state.changed {
                    self.split_stream(output)?;
                }
                self.state.has_emitted_media_tag = true;
                output(FlvData::Tag(tag))
            }
            FlvData::Split(_) => output(input),
        }
    }

    fn finish(
        &mut self,
        _context: &Arc<StreamerContext>,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        debug!("{} completed.", self.context.name);
        self.flush_buffered_tags_if_pending(output)?;
        self.state.reset();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "SplitOperator"
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use pipeline_common::{CancellationToken, StreamerContext};

    use super::*;
    use crate::test_utils::{
        create_audio_sequence_header, create_audio_tag, create_test_header,
        create_video_sequence_header, create_video_tag,
    };

    fn run_split(
        mode: SequenceHeaderChangeMode,
        drop_repeats: bool,
        input: Vec<FlvData>,
    ) -> Vec<FlvData> {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::with_config(context.clone(), mode, drop_repeats);
        let mut output = Vec::new();
        let mut emit = |item| {
            output.push(item);
            Ok(())
        };
        for item in input {
            operator.process(&context, item, &mut emit).unwrap();
        }
        operator.finish(&context, &mut emit).unwrap();
        output
    }

    fn packet(kind: flv::FlvTagType, timestamp: u32, payload: &[u8]) -> FlvData {
        FlvData::Tag(FlvTag::new(
            timestamp,
            0,
            kind,
            false,
            Bytes::copy_from_slice(payload),
        ))
    }

    fn assert_configuration_split(
        kind: flv::FlvTagType,
        first: FlvData,
        second: FlvData,
        media: FlvData,
    ) {
        let header = create_test_header();
        let output = run_split(
            SequenceHeaderChangeMode::SemanticSignature,
            true,
            vec![
                header.clone(),
                first.clone(),
                media.clone(),
                second.clone(),
                media.clone(),
            ],
        );
        let [
            header_before,
            config_before,
            media_before,
            marker,
            header_after,
            config_after,
            media_after,
        ] = output.as_slice()
        else {
            panic!("unexpected split output: {output:?}");
        };
        assert_eq!(header_before, &header);
        assert_eq!(header_after, &header);
        assert_eq!(config_before, &first);
        assert_eq!(config_after, &second);
        assert_eq!(media_before, &media);
        assert_eq!(media_after, &media);
        match (kind, marker) {
            (flv::FlvTagType::Audio, FlvData::Split(SplitReason::AudioCodecChange { .. })) => {}
            (flv::FlvTagType::Video, FlvData::Split(SplitReason::VideoCodecChange { .. })) => {}
            _ => panic!("incorrect split reason: {marker:?}"),
        }
    }

    #[test]
    fn enhanced_audio_codec_change_preserves_the_full_fourcc() {
        use flv::FlvTagType::Audio;
        let ac3 = packet(Audio, 0, b"\x90ac-3");
        let eac3 = packet(Audio, 32, b"\x90ec-3");
        let mut first_payload = b"\x91ac-3".to_vec();
        first_payload.extend_from_slice(include_bytes!("../../tests/fixtures/ac3-silence.frame"));
        let mut second_payload = b"\x91ec-3".to_vec();
        second_payload.extend_from_slice(include_bytes!("../../tests/fixtures/eac3-silence.frame"));
        let first_media = packet(Audio, 0, &first_payload);
        let second_media = packet(Audio, 32, &second_payload);
        for mode in [
            SequenceHeaderChangeMode::Crc32,
            SequenceHeaderChangeMode::SemanticSignature,
        ] {
            let input = vec![
                create_test_header(),
                ac3.clone(),
                first_media.clone(),
                eac3.clone(),
                second_media.clone(),
            ];
            let output = run_split(mode, true, input);
            let [
                header,
                config0,
                media0,
                FlvData::Split(SplitReason::AudioCodecChange { from, to }),
                reinjected,
                config1,
                media1,
            ] = output.as_slice()
            else {
                panic!("missing codec change: {output:?}");
            };
            assert_eq!(header, &create_test_header());
            assert_eq!(reinjected, header);
            assert_eq!(config0, &ac3);
            assert_eq!(config1, &eac3);
            assert_eq!(media0, &first_media);
            assert_eq!(media1, &second_media);
            assert_eq!(from.codec, "Ac3");
            assert_eq!(to.codec, "Eac3");
        }
    }

    #[test]
    fn timestamp_modifiers_are_ignored_but_inner_configuration_changes_split() {
        use crate::test_utils::fixtures::{
            AAC_LC_SILENCE_ENHANCED, AV1_CODEC_CONFIG, AV1_KEYFRAME,
        };
        use flv::FlvTagType::{Audio, Video};
        for (kind, codec, config, frame) in [
            (
                Audio,
                &b"mp4a"[..],
                &b"\x12\x10"[..],
                AAC_LC_SILENCE_ENHANCED,
            ),
            (Video, &b"av01"[..], AV1_CODEC_CONFIG, AV1_KEYFRAME),
        ] {
            let mut plain = vec![0x90];
            plain.extend_from_slice(codec);
            plain.extend_from_slice(config);
            // TimestampOffsetNano=1 followed by a second TimestampOffsetNano=2;
            // both modifiers end at the same codec/configuration as the plain tag.
            let mut modified = vec![0x97, 2, 0, 0, 1, 7, 2, 0, 0, 2, 0];
            modified.extend_from_slice(codec);
            modified.extend_from_slice(config);
            let first = packet(kind, 0, &plain);
            let second = packet(kind, 0, &modified);
            let media = packet(kind, 100, frame);
            let input = vec![
                create_test_header(),
                first.clone(),
                media.clone(),
                second.clone(),
                media.clone(),
            ];
            assert_eq!(
                run_split(
                    SequenceHeaderChangeMode::SemanticSignature,
                    false,
                    input.clone()
                ),
                input
            );
            assert_eq!(
                run_split(SequenceHeaderChangeMode::SemanticSignature, true, input),
                vec![
                    create_test_header(),
                    first.clone(),
                    media.clone(),
                    media.clone()
                ]
            );
            // AAC sample rate changes; AV1 initial presentation delay changes.
            let config_start = modified.len() - config.len();
            if kind == Audio {
                modified[config_start] = 0x11;
                modified[config_start + 1] = 0x90;
            } else {
                modified[config_start + 3] = 0x10;
            }
            assert_configuration_split(kind, first, packet(kind, 0, &modified), media);
        }
    }

    #[test]
    fn unsupported_modifiers_and_multitrack_headers_use_full_payload_comparison() {
        use flv::FlvTagType::{Audio, Video};
        for (kind, mut first, changed_index) in [
            // Unknown ModEx type 1 must not discard its changing record.
            (Audio, b"\x97\x02\0\0\x01\x10mp4a\x12\x10".to_vec(), 4),
            (Video, b"\x97\x02\0\0\x01\x10av01\x81\0\x0c\0".to_vec(), 4),
            // OneTrack: a different track ID changes the initialization context.
            (Audio, b"\x95\0mp4a\0\x12\x10".to_vec(), 6),
            (Video, b"\x96\0av01\0\x81\0\x0c\0".to_vec(), 6),
        ] {
            let initial = packet(kind, 0, &first);
            first[changed_index] += 1;
            let changed = packet(kind, 0, &first);
            let media = if kind == Audio {
                create_audio_tag(100)
            } else {
                create_video_tag(100, true)
            };
            assert_configuration_split(kind, initial, changed, media);
        }
    }

    #[test]
    fn test_video_codec_change_detection() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Add a header and first video sequence header (version 1)
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();

        // Add some content tags
        for i in 1..5 {
            operator
                .process(
                    &context,
                    create_video_tag(i * 100, i % 3 == 0),
                    &mut output_fn,
                )
                .unwrap();
        }

        // Add a different video sequence header (version 2) - should trigger a split
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();

        // Add more content tags
        for i in 5..10 {
            operator
                .process(
                    &context,
                    create_video_tag(i * 100, i % 3 == 0),
                    &mut output_fn,
                )
                .unwrap();
        }

        // The header count indicates how many splits occurred
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        // Should have 2 headers: initial + 1 after codec change
        assert_eq!(
            header_count, 2,
            "Should detect video codec change and inject new header"
        );
    }

    #[test]
    fn test_extract_video_codec_info_fallback_enhanced_vp9() {
        // Enhanced keyframe + SEQUENCE_START with the VP9 FourCC: `VideoData::demux`
        // yields `EnhancedPacket::Unknown` (no VP9 config parser), so
        // `extract_video_codec_info` takes its `get_video_codec` fallback arm.
        let tag = FlvTag::new(
            0,
            0,
            flv::tag::FlvTagType::Video,
            false,
            Bytes::from(vec![0x90, b'v', b'p', b'0', b'9', 0x01, 0x02]),
        );

        let info = SplitOperator::extract_video_codec_info(&tag, 0);

        assert_eq!(info.codec, "VP9");
        assert_eq!(info.profile, None);
        assert_eq!(info.level, None);
        assert_eq!(info.width, None);
        assert_eq!(info.height, None);
    }

    #[test]
    fn test_extract_video_codec_info_deep_parses_multitrack_av1() {
        // OneTrack + SequenceStart multitrack wrapper: the first track's
        // `EnhancedPacket::Av1` sequence start supplies codec, profile, and
        // level, matching the single-track AV1 arm.
        let tag = FlvTag::new(
            0,
            0,
            flv::tag::FlvTagType::Video,
            false,
            Bytes::from(vec![
                0x96, 0x00, b'a', b'v', b'0', b'1', 0x00, 0x81, 0x0D, 0x0C, 0x00,
            ]),
        );

        let info = SplitOperator::extract_video_codec_info(&tag, 0);

        assert_eq!(info.codec, "AV1");
        assert_eq!(info.profile, Some(0));
        assert_eq!(info.level, Some(13));
    }

    #[test]
    fn test_audio_codec_change_detection() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Add a header and first audio sequence header
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();

        // Add some content tags
        for i in 1..5 {
            operator
                .process(&context, create_audio_tag(i * 100), &mut output_fn)
                .unwrap();
        }

        // Add a different audio sequence header - should trigger a split
        operator
            .process(&context, create_audio_sequence_header(0, 2), &mut output_fn)
            .unwrap();

        // Add more content tags
        for i in 5..10 {
            operator
                .process(&context, create_audio_tag(i * 100), &mut output_fn)
                .unwrap();
        }

        // The header count indicates how many splits occurred
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        // Should have 2 headers: initial + 1 after codec change
        assert_eq!(
            header_count, 2,
            "Should detect audio codec change and inject new header"
        );
    }

    #[test]
    fn test_no_codec_change() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Add a header and codec headers
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();

        // Add some content tags
        for i in 1..5 {
            operator
                .process(
                    &context,
                    create_video_tag(i * 100, i % 3 == 0),
                    &mut output_fn,
                )
                .unwrap();
            operator
                .process(&context, create_audio_tag(i * 100), &mut output_fn)
                .unwrap();
        }

        // Add identical codec headers again - should NOT trigger a split
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();

        // Add more content tags
        for i in 5..10 {
            operator
                .process(
                    &context,
                    create_video_tag(i * 100, i % 3 == 0),
                    &mut output_fn,
                )
                .unwrap();
            operator
                .process(&context, create_audio_tag(i * 100), &mut output_fn)
                .unwrap();
        }

        // The header count indicates how many splits occurred
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        // Should have only 1 header (the initial one)
        assert_eq!(
            header_count, 1,
            "Should not split when codec doesn't change"
        );
    }

    #[test]
    fn test_multiple_codec_changes() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // First segment
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Second segment (video codec change)
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(200, true), &mut output_fn)
            .unwrap();

        // Third segment (audio codec change)
        operator
            .process(&context, create_audio_sequence_header(0, 2), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(300), &mut output_fn)
            .unwrap();

        // Fourth segment (both codecs change)
        operator
            .process(&context, create_video_sequence_header(0, 3), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 3), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(400, true), &mut output_fn)
            .unwrap();

        // The header count indicates how many segments we have
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        // Should have 4 headers: initial + 3 after codec changes
        assert_eq!(
            header_count, 4,
            "Should detect all codec changes and inject new headers"
        );
    }

    #[test]
    fn test_pending_split_flushes_buffered_sequence_headers_on_finish() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Trigger a pending split by changing the video sequence header.
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();

        // No regular media tag arrives; finish must not drop the buffered sequence header.
        operator.finish(&context, &mut output_fn).unwrap();

        let last = output_items
            .iter()
            .rev()
            .find_map(|item| match item {
                FlvData::Tag(tag) => Some(tag),
                _ => None,
            })
            .expect("Expected at least one tag in output");

        assert!(
            last.is_video_sequence_header(),
            "Expected flushed video sequence header at end"
        );
        assert_eq!(last.data()[5], 2, "Expected version=2 sequence header");
    }

    #[test]
    fn test_pending_split_flushes_buffered_sequence_headers_on_end_of_sequence() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Trigger pending split.
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();

        // Emit EOS; buffered tags should be flushed before it.
        operator
            .process(
                &context,
                FlvData::EndOfSequence(Bytes::new()),
                &mut output_fn,
            )
            .unwrap();

        let last_tag_idx = output_items
            .iter()
            .rposition(|i| matches!(i, FlvData::Tag(_)))
            .unwrap();
        let eos_idx = output_items
            .iter()
            .rposition(|i| matches!(i, FlvData::EndOfSequence(_)))
            .unwrap();

        assert!(
            last_tag_idx < eos_idx,
            "Expected buffered tags to flush before EndOfSequence"
        );
    }

    #[test]
    fn test_no_split_when_sequence_header_changes_before_first_media_tag() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        // Upstream re-sends/changes sequence header before any media tags.
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();

        // First media tag arrives.
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        assert_eq!(
            header_count, 1,
            "Should not inject a new header before first media tag"
        );
    }

    #[test]
    fn test_no_split_when_video_sequence_header_differs_only_in_non_config_fields() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::with_config(
            context.clone(),
            SequenceHeaderChangeMode::SemanticSignature,
            false,
        );
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();

        // First config (version=1).
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Same codec-config bytes, but different frame-type + composition-time.
        // The operator should ignore these differences and avoid splitting.
        let same_config_different_prefix = FlvData::Tag(FlvTag::new(
            0,
            0,
            flv::tag::FlvTagType::Video,
            false,
            Bytes::from(vec![
                0x27, // Inter frame + AVC (same codec)
                0x00, // AVC sequence header
                0x12, 0x34, 0x56, // composition time (not part of config)
                1,    // AVC configurationVersion (same as before)
                0x64, 0x00, 0x28, // rest of AVCC bytes (same as before)
            ]),
        ));
        operator
            .process(
                &context,
                same_config_different_prefix.clone(),
                &mut output_fn,
            )
            .unwrap();
        operator
            .process(&context, create_video_tag(200, true), &mut output_fn)
            .unwrap();

        assert_eq!(
            output_items,
            vec![
                create_test_header(),
                create_video_sequence_header(0, 1),
                create_video_tag(100, true),
                same_config_different_prefix,
                create_video_tag(200, true),
            ]
        );
    }

    #[test]
    fn test_no_split_when_audio_sequence_header_differs_only_in_flv_audio_header_bits() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::with_config(
            context.clone(),
            SequenceHeaderChangeMode::SemanticSignature,
            false,
        );
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(
                &context,
                create_audio_sequence_header(0, 0x12),
                &mut output_fn,
            )
            .unwrap();
        operator
            .process(&context, create_audio_tag(100), &mut output_fn)
            .unwrap();

        // Same AudioSpecificConfig payload, but change legacy FLV audio header bits
        // (rate/size/type). The operator should ignore this and avoid splitting.
        let same_config_different_header_bits = FlvData::Tag(FlvTag::new(
            0,
            0,
            flv::tag::FlvTagType::Audio,
            false,
            Bytes::from(vec![
                0xA3, // AAC + different rate/size/type bits than 0xAF
                0x00, // AAC sequence header
                0x12, // same ASC payload
                0x10,
            ]),
        ));
        operator
            .process(
                &context,
                same_config_different_header_bits.clone(),
                &mut output_fn,
            )
            .unwrap();
        operator
            .process(&context, create_audio_tag(200), &mut output_fn)
            .unwrap();

        assert_eq!(
            output_items,
            vec![
                create_test_header(),
                create_audio_sequence_header(0, 0x12),
                create_audio_tag(100),
                same_config_different_header_bits,
                create_audio_tag(200),
            ]
        );
    }

    #[test]
    fn test_drop_duplicate_video_sequence_headers_when_enabled() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator =
            SplitOperator::with_config(context.clone(), SequenceHeaderChangeMode::Crc32, true);
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Same sequence header again: should be dropped.
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(200, true), &mut output_fn)
            .unwrap();

        let seq_hdr_count = output_items
            .iter()
            .filter_map(|item| match item {
                FlvData::Tag(tag) => Some(tag),
                _ => None,
            })
            .filter(|tag| tag.is_video_sequence_header())
            .count();

        assert_eq!(
            seq_hdr_count, 1,
            "Expected duplicate video sequence header to be dropped"
        );
    }

    #[test]
    fn test_drop_duplicate_audio_sequence_headers_when_enabled() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator =
            SplitOperator::with_config(context.clone(), SequenceHeaderChangeMode::Crc32, true);
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(100), &mut output_fn)
            .unwrap();

        // Same sequence header again: should be dropped.
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(200), &mut output_fn)
            .unwrap();

        let seq_hdr_count = output_items
            .iter()
            .filter_map(|item| match item {
                FlvData::Tag(tag) => Some(tag),
                _ => None,
            })
            .filter(|tag| tag.is_audio_sequence_header())
            .count();

        assert_eq!(
            seq_hdr_count, 1,
            "Expected duplicate audio sequence header to be dropped"
        );
    }

    #[test]
    fn test_split_marker_emitted_on_video_codec_change() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Change video codec config
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();
        // Trigger split with a regular tag
        operator
            .process(&context, create_video_tag(200, true), &mut output_fn)
            .unwrap();

        let split_items: Vec<_> = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Split(_)))
            .collect();

        assert_eq!(split_items.len(), 1, "Should emit exactly one Split marker");

        match &split_items[0] {
            FlvData::Split(SplitReason::VideoCodecChange { from, to }) => {
                assert_ne!(
                    from.signature, to.signature,
                    "from and to signatures should differ"
                );
            }
            other => panic!("Expected VideoCodecChange, got: {other:?}"),
        }

        // Verify Split comes before the re-injected Header
        let split_idx = output_items
            .iter()
            .position(|item| matches!(item, FlvData::Split(_)))
            .unwrap();
        let second_header_idx = output_items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, FlvData::Header(_)))
            .nth(1)
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            split_idx < second_header_idx,
            "Split marker should appear before the re-injected Header"
        );
    }

    #[test]
    fn test_split_marker_emitted_on_audio_codec_change() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(100), &mut output_fn)
            .unwrap();

        // Change audio codec config
        operator
            .process(&context, create_audio_sequence_header(0, 2), &mut output_fn)
            .unwrap();
        // Trigger split
        operator
            .process(&context, create_audio_tag(200), &mut output_fn)
            .unwrap();

        let split_items: Vec<_> = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Split(_)))
            .collect();

        assert_eq!(split_items.len(), 1, "Should emit exactly one Split marker");

        match &split_items[0] {
            FlvData::Split(SplitReason::AudioCodecChange { from, to }) => {
                assert_ne!(
                    from.signature, to.signature,
                    "from and to signatures should differ"
                );
            }
            other => panic!("Expected AudioCodecChange, got: {other:?}"),
        }
    }

    #[test]
    fn test_split_marker_header_received_on_second_upstream_header() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // First header — no Split
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Second header — should emit Split(HeaderReceived)
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();

        let split_items: Vec<_> = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Split(_)))
            .collect();

        assert_eq!(split_items.len(), 1, "Should emit exactly one Split marker");
        assert!(
            matches!(split_items[0], FlvData::Split(SplitReason::HeaderReceived)),
            "Expected HeaderReceived, got: {:?}",
            split_items[0]
        );
    }

    #[test]
    fn test_first_upstream_header_does_not_produce_split() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        let split_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Split(_)))
            .count();

        assert_eq!(
            split_count, 0,
            "First upstream header should NOT produce a Split marker"
        );
    }

    #[test]
    fn test_both_video_and_audio_change_emits_two_split_markers() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Change both video and audio
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 2), &mut output_fn)
            .unwrap();
        // Trigger split
        operator
            .process(&context, create_video_tag(200, true), &mut output_fn)
            .unwrap();

        let split_items: Vec<_> = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Split(_)))
            .collect();

        assert_eq!(
            split_items.len(),
            2,
            "Should emit two Split markers (one video, one audio)"
        );

        assert!(
            matches!(
                split_items[0],
                FlvData::Split(SplitReason::VideoCodecChange { .. })
            ),
            "First Split should be VideoCodecChange"
        );
        assert!(
            matches!(
                split_items[1],
                FlvData::Split(SplitReason::AudioCodecChange { .. })
            ),
            "Second Split should be AudioCodecChange"
        );
    }
}
