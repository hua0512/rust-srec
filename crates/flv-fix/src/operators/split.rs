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
use flv::data::FlvData;
use flv::header::FlvHeader;
use flv::tag::FlvTag;
use pipeline_common::split_reason::{AudioCodecInfo, SplitReason, VideoCodecInfo};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use tracing::{debug, info};

use crate::crc32;

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
    SemanticSignature,
}

// Store data wrapped in Arc for efficient cloning
struct StreamState {
    header: Option<FlvHeader>,
    metadata: Option<FlvTag>,
    audio_sequence_tag: Option<FlvTag>,
    video_sequence_tag: Option<FlvTag>,
    /// Key for detecting changes in the last seen video sequence header.
    ///
    /// The exact meaning depends on `SequenceHeaderChangeMode`.
    video_sig: Option<u32>,
    /// Key for detecting changes in the last seen audio sequence header.
    ///
    /// The exact meaning depends on `SequenceHeaderChangeMode`.
    audio_sig: Option<u32>,
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
            header: None,
            metadata: None,
            audio_sequence_tag: None,
            video_sequence_tag: None,
            video_sig: None,
            audio_sig: None,
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
        self.header = None;
        self.metadata = None;
        self.audio_sequence_tag = None;
        self.video_sequence_tag = None;
        self.video_sig = None;
        self.audio_sig = None;
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

    /// Calculate CRC32 for a byte slice.
    fn calculate_crc32(data: &[u8]) -> u32 {
        crc32::crc32(data)
    }

    fn video_change_key(&self, tag: &FlvTag) -> u32 {
        match self.sequence_header_change_mode {
            SequenceHeaderChangeMode::Crc32 => Self::calculate_crc32(tag.data.as_ref()),
            SequenceHeaderChangeMode::SemanticSignature => {
                Self::calculate_video_sequence_signature(tag)
            }
        }
    }

    fn audio_change_key(&self, tag: &FlvTag) -> u32 {
        match self.sequence_header_change_mode {
            SequenceHeaderChangeMode::Crc32 => Self::calculate_crc32(tag.data.as_ref()),
            SequenceHeaderChangeMode::SemanticSignature => {
                Self::calculate_audio_sequence_signature(tag)
            }
        }
    }

    /// Compute a "semantic signature" for video sequence headers.
    ///
    /// The old approach used a raw CRC32 of the entire tag payload (`tag.data`),
    /// which can false-positive on byte-level differences in fields that don't
    /// affect decoder initialization (e.g. AVC composition time, frame-type bits).
    ///
    /// This signature focuses on the codec-configuration portion of the payload:
    /// - legacy (AVC/legacy HEVC): `codec_id || payload[5..]`
    ///   - skips `[packet_type][composition_time(3)]`
    /// - enhanced: `fourcc || payload[5..]`
    ///   - skips the first byte (flags/packet type)
    fn calculate_video_sequence_signature(tag: &FlvTag) -> u32 {
        let data = tag.data.as_ref();
        if data.is_empty() {
            return 0;
        }

        let enhanced = (data[0] & 0b1000_0000) != 0;
        let mut state = 0u32;

        if enhanced {
            // Layout: [flags+packet_type][fourcc(4)][codec_config...]
            if data.len() >= 5 {
                state = crc32::crc32_update(state, &data[1..5]);
                state = crc32::crc32_update(state, &data[5..]);
            } else {
                state = crc32::crc32_update(state, data);
            }
        } else {
            // Layout: [frame_type+codec_id][packet_type][cts(3)][codec_config...]
            let codec_id = data[0] & 0x0F;
            state = crc32::crc32_update(state, &[codec_id]);

            if data.len() > 5 {
                state = crc32::crc32_update(state, &data[5..]);
            } else {
                state = crc32::crc32_update(state, data);
            }
        }

        state
    }

    /// Compute a "semantic signature" for AAC sequence headers.
    ///
    /// Layout: [AudioHeader][AACPacketType=0][AudioSpecificConfig...]
    /// We ignore the legacy audio header bits and only hash the AAC payload.
    fn calculate_audio_sequence_signature(tag: &FlvTag) -> u32 {
        let data = tag.data.as_ref();
        let mut state = 0u32;

        if data.len() >= 2 {
            // Keep the sound_format nibble to avoid accidentally equating future
            // non-AAC sequence headers if we extend detection.
            let sound_format = (data[0] >> 4) & 0x0F;
            state = crc32::crc32_update(state, &[sound_format]);

            if data.len() > 2 {
                state = crc32::crc32_update(state, &data[2..]);
            }
        } else {
            state = crc32::crc32_update(state, data);
        }

        state
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

        let data = tag.data.clone();
        let mut cursor = std::io::Cursor::new(data);

        match VideoData::demux(&mut cursor) {
            Ok(video) => match video.body {
                VideoTagBody::Avc(AvcPacket::SequenceHeader(config)) => {
                    let resolution =
                        AvcPacket::SequenceHeader(config.clone()).get_video_resolution();
                    VideoCodecInfo {
                        codec: "AVC".to_string(),
                        profile: Some(config.profile_indication),
                        level: Some(config.level_indication),
                        width: resolution.as_ref().map(|r| r.width as u32),
                        height: resolution.as_ref().map(|r| r.height as u32),
                        signature,
                    }
                }
                VideoTagBody::Hevc(HevcPacket::SequenceStart(config)) => {
                    let resolution =
                        HevcPacket::SequenceStart(config.clone()).get_video_resolution();
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
                    let resolution =
                        Av1Packet::SequenceStart(config.clone()).get_video_resolution();
                    VideoCodecInfo {
                        codec: "AV1".to_string(),
                        profile: Some(config.seq_profile),
                        level: Some(config.seq_level_idx_0),
                        width: resolution.as_ref().map(|r| r.width as u32),
                        height: resolution.as_ref().map(|r| r.height as u32),
                        signature,
                    }
                }
                VideoTagBody::Enhanced(EnhancedPacket::Avc(AvcPacket::SequenceHeader(config))) => {
                    let resolution =
                        AvcPacket::SequenceHeader(config.clone()).get_video_resolution();
                    VideoCodecInfo {
                        codec: "AVC".to_string(),
                        profile: Some(config.profile_indication),
                        level: Some(config.level_indication),
                        width: resolution.as_ref().map(|r| r.width as u32),
                        height: resolution.as_ref().map(|r| r.height as u32),
                        signature,
                    }
                }
                VideoTagBody::Enhanced(EnhancedPacket::Hevc(HevcPacket::SequenceStart(config))) => {
                    let resolution =
                        HevcPacket::SequenceStart(config.clone()).get_video_resolution();
                    VideoCodecInfo {
                        codec: "HEVC".to_string(),
                        profile: Some(config.general_profile_idc),
                        level: Some(config.general_level_idc),
                        width: resolution.as_ref().map(|r| r.width as u32),
                        height: resolution.as_ref().map(|r| r.height as u32),
                        signature,
                    }
                }
                _ => {
                    // Fallback: use codec ID from tag
                    let codec = tag
                        .get_video_codec_id()
                        .map(|id| format!("{id:?}"))
                        .unwrap_or_else(|| "unknown".to_string());
                    VideoCodecInfo {
                        codec,
                        profile: None,
                        level: None,
                        width: None,
                        height: None,
                        signature,
                    }
                }
            },
            Err(_) => VideoCodecInfo {
                codec: "unknown".to_string(),
                profile: None,
                level: None,
                width: None,
                height: None,
                signature,
            },
        }
    }

    /// Extract audio codec configuration info from a sequence header tag.
    ///
    /// For AAC, parses AudioSpecificConfig to extract sample rate and channels.
    /// For other codecs, returns the codec name only.
    fn extract_audio_codec_info(tag: &FlvTag, signature: u32) -> AudioCodecInfo {
        let codec_name = tag
            .get_audio_codec_id()
            .map(|sf| format!("{sf:?}"))
            .unwrap_or_else(|| "unknown".to_string());

        let data = tag.data.as_ref();

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
            let new_sig = self.state.video_sig.unwrap_or(0);
            let to = self
                .state
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
            let new_sig = self.state.audio_sig.unwrap_or(0);
            let to = self
                .state
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

        // Note on timestamp handling:
        // When we split the stream, we re-inject the header and sequence information
        // using the original timestamps from when they were first encountered.
        // This maintains timestamp consistency within the stream segments
        // but does not reset the timeline. Downstream components or players
        // may need to handle potential timestamp discontinuities at split points.
        if let Some(header) = &self.state.header {
            output(FlvData::Header(header.clone()))?;
        }
        if let Some(metadata) = &self.state.metadata {
            output(FlvData::Tag(metadata.clone()))?;
        }
        if let Some(video_seq) = &self.state.video_sequence_tag {
            output(FlvData::Tag(video_seq.clone()))?;
        }
        if let Some(audio_seq) = &self.state.audio_sequence_tag {
            output(FlvData::Tag(audio_seq.clone()))?;
        }
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
            && let Some(metadata) = self.state.metadata.take()
        {
            output(FlvData::Tag(metadata))?;
        }
        if self.state.buffered_video_sequence_tag
            && let Some(video_seq) = self.state.video_sequence_tag.take()
        {
            output(FlvData::Tag(video_seq))?;
        }
        if self.state.buffered_audio_sequence_tag
            && let Some(audio_seq) = self.state.audio_sequence_tag.take()
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
                let is_restart = self.state.header.is_some();
                // Reset state when a new header is encountered
                self.state.reset();
                self.state.header = Some(header.clone());
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
                        self.state.metadata = Some(tag);
                        self.state.buffered_metadata = true;
                        return Ok(());
                    }
                    if tag.is_video_sequence_header() {
                        debug!(
                            "{} Video sequence tag detected while split pending",
                            self.context.name
                        );
                        // If this is a new video config (different from what we had), save the old one.
                        let new_sig = self.video_change_key(&tag);
                        if let Some(old_sig) = self.state.video_sig
                            && old_sig != new_sig
                            && let Some(old_tag) = self.state.video_sequence_tag.as_ref()
                        {
                            self.state.prev_video_codec_info =
                                Some(Self::extract_video_codec_info(old_tag, old_sig));
                        }
                        self.state.video_sequence_tag = Some(tag);
                        self.state.buffered_video_sequence_tag = true;
                        self.state.video_sig = Some(new_sig);
                        return Ok(());
                    }
                    if tag.is_audio_sequence_header() {
                        debug!(
                            "{} Audio sequence tag detected while split pending",
                            self.context.name
                        );
                        // If this is a new audio config (different from what we had), save the old one.
                        let new_sig = self.audio_change_key(&tag);
                        if let Some(old_sig) = self.state.audio_sig
                            && old_sig != new_sig
                            && let Some(old_tag) = self.state.audio_sequence_tag.as_ref()
                        {
                            self.state.prev_audio_codec_info =
                                Some(Self::extract_audio_codec_info(old_tag, old_sig));
                        }
                        self.state.audio_sequence_tag = Some(tag);
                        self.state.buffered_audio_sequence_tag = true;
                        self.state.audio_sig = Some(new_sig);
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
                    self.state.metadata = Some(tag.clone());
                    return output(FlvData::Tag(tag));
                }

                if tag.is_video_sequence_header() {
                    debug!("{} Video sequence tag detected", self.context.name);
                    let sig = self.video_change_key(&tag);

                    if self.drop_duplicate_sequence_headers
                        && self.state.video_sig.is_some_and(|prev| prev == sig)
                    {
                        debug!(
                            "{} Dropping duplicate video sequence header (sig: {:x})",
                            self.context.name, sig
                        );
                        self.state.video_sequence_tag = Some(tag);
                        self.state.video_sig = Some(sig);
                        return Ok(());
                    }

                    if let Some(prev_sig) = self.state.video_sig
                        && prev_sig != sig
                    {
                        // If the stream hasn't produced any media tags yet, upstream may still be
                        // negotiating/settling the initial codec configuration (common right at
                        // stream start). Splitting here creates an "empty" first segment consisting
                        // only of headers/sequence tags.
                        if self.state.has_emitted_media_tag {
                            info!(
                                "{} Video sequence header changed (sig: {:x} -> {:x}), marking for split",
                                self.context.name, prev_sig, sig
                            );
                            // Eagerly extract codec info from the old tag before we overwrite it.
                            if let Some(old_tag) = self.state.video_sequence_tag.as_ref() {
                                self.state.prev_video_codec_info =
                                    Some(Self::extract_video_codec_info(old_tag, prev_sig));
                            }
                            self.state.changed = true;
                            self.state.buffered_video_sequence_tag = true;
                        } else {
                            debug!(
                                "{} Video sequence header changed before first media tag (CRC: {:x} -> {:x}); treating as initial config update (no split)",
                                self.context.name, prev_sig, sig
                            );
                        }
                    }
                    self.state.video_sequence_tag = Some(tag.clone());
                    self.state.video_sig = Some(sig);

                    // If we just detected a change, buffer the new header and wait for the next
                    // regular tag to inject a fresh header+sequence set.
                    if self.state.changed {
                        return Ok(());
                    }

                    return output(FlvData::Tag(tag));
                }

                if tag.is_audio_sequence_header() {
                    debug!("{} Audio sequence tag detected", self.context.name);
                    let sig = self.audio_change_key(&tag);

                    if self.drop_duplicate_sequence_headers
                        && self.state.audio_sig.is_some_and(|prev| prev == sig)
                    {
                        debug!(
                            "{} Dropping duplicate audio sequence header (sig: {:x})",
                            self.context.name, sig
                        );
                        self.state.audio_sequence_tag = Some(tag);
                        self.state.audio_sig = Some(sig);
                        return Ok(());
                    }

                    if let Some(prev_sig) = self.state.audio_sig
                        && prev_sig != sig
                    {
                        if self.state.has_emitted_media_tag {
                            info!(
                                "{} Audio parameters changed (sig: {:x} -> {:x})",
                                self.context.name, prev_sig, sig
                            );
                            // Eagerly extract codec info from the old tag before we overwrite it.
                            if let Some(old_tag) = self.state.audio_sequence_tag.as_ref() {
                                self.state.prev_audio_codec_info =
                                    Some(Self::extract_audio_codec_info(old_tag, prev_sig));
                            }
                            self.state.changed = true;
                            self.state.buffered_audio_sequence_tag = true;
                        } else {
                            debug!(
                                "{} Audio sequence header changed before first media tag (CRC: {:x} -> {:x}); treating as initial config update (no split)",
                                self.context.name, prev_sig, sig
                            );
                        }
                    }
                    self.state.audio_sequence_tag = Some(tag.clone());
                    self.state.audio_sig = Some(sig);

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
        assert_eq!(last.data[5], 2, "Expected version=2 sequence header");
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
        let same_config_different_prefix = FlvData::Tag(FlvTag {
            timestamp_ms: 0,
            stream_id: 0,
            tag_type: flv::tag::FlvTagType::Video,
            is_filtered: false,
            data: Bytes::from(vec![
                0x27, // Inter frame + AVC (same codec)
                0x00, // AVC sequence header
                0x12, 0x34, 0x56, // composition time (not part of config)
                1,    // AVC configurationVersion (same as before)
                0x64, 0x00, 0x28, // rest of AVCC bytes (same as before)
            ]),
        });
        operator
            .process(&context, same_config_different_prefix, &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(200, true), &mut output_fn)
            .unwrap();

        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        assert_eq!(
            header_count, 1,
            "Should not split on non-config differences"
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
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(100), &mut output_fn)
            .unwrap();

        // Same AudioSpecificConfig payload, but change legacy FLV audio header bits
        // (rate/size/type). The operator should ignore this and avoid splitting.
        let same_config_different_header_bits = FlvData::Tag(FlvTag {
            timestamp_ms: 0,
            stream_id: 0,
            tag_type: flv::tag::FlvTagType::Audio,
            is_filtered: false,
            data: Bytes::from(vec![
                0xA3, // AAC + different rate/size/type bits than 0xAF
                0x00, // AAC sequence header
                1,    // same ASC payload
                0x10,
            ]),
        });
        operator
            .process(&context, same_config_different_header_bits, &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(200), &mut output_fn)
            .unwrap();

        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        assert_eq!(header_count, 1, "Should not split on FLV audio header bits");
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
