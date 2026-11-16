//! Frames to FLV.
//!
//! H.264 goes out as legacy AVC tags. HEVC goes out as Enhanced RTMP `hvc1`
//! tags, because FFmpeg does not read the legacy HEVC codec id. AAC goes
//! out as raw frames after an AudioSpecificConfig. The stream carries no
//! `onMetaData`; the FLV processing that follows writes it.
//!
//! Tag times are the source's ms clock, counted from the first video
//! keyframe. The source clock is shared by both tracks and runs on across
//! media reconnects, so a reconnect continues the same timeline and the
//! frames it repeats are dropped.

use std::collections::VecDeque;

use ::flv::audio::SoundFormat;
use ::flv::avc::AvcPacketType;
use ::flv::video::{EnhancedPacketType, VideoCodecId, VideoFourCC, VideoFrameType};
use ::flv::{FlvData, FlvHeader, FlvTag, FlvTagType};
use bytes::{BufMut, Bytes};
use h264::AVCDecoderConfigurationRecord;
use h265::HEVCDecoderConfigurationRecord;

use super::NativeError;
use super::depacketize::{AudioFrame, VideoCodec, VideoFrame};

/// Audio kept while waiting for the first keyframe, which is at most a
/// keyframe interval older than the newest audio: several seconds of audio.
const MAX_HELD_AUDIO: usize = 256;
/// A frame this many ms behind its track's previous one means the source
/// clock was reset; anything closer is a repeat.
const CLOCK_RESET_MS: i32 = 10_000;

/// AAC, 44 kHz, 16-bit, stereo: the values FLV requires for AAC.
const AAC_TAG_HEADER: u8 = (SoundFormat::Aac as u8) << 4 | 0x0f;
const AAC_SEQUENCE_HEADER: u8 = 0;
const AAC_RAW: u8 = 1;

/// Turns frames into FLV, keeping one timeline across media reconnects.
#[derive(Default)]
pub(super) struct Muxer {
    header_written: bool,
    video_config: Option<VideoConfig>,
    audio_config: Option<[u8; 2]>,
    /// `None` until a keyframe starts the timeline, and again after a clock
    /// reset.
    timeline: Option<Timeline>,
    /// Source times of the last frames written, per track.
    last_video: Option<u32>,
    last_audio: Option<u32>,
    /// The latest tag time written.
    last_tag_time: Option<u32>,
    /// Audio that arrived before the timeline started.
    held_audio: VecDeque<AudioFrame>,
    keyframes: u64,
    audio_frames: u64,
}

/// The parameter sets of a video sequence header.
#[derive(Clone, PartialEq, Eq)]
enum VideoConfig {
    H264 { sps: Bytes, pps: Bytes },
    Hevc { vps: Bytes, sps: Bytes, pps: Bytes },
}

/// Maps source times to tag times.
#[derive(Clone, Copy)]
struct Timeline {
    /// The source time of tag time `offset`.
    base: u32,
    offset: u32,
}

impl Timeline {
    /// `None` for a time before the base.
    fn tag_time(self, source: u32) -> Option<u32> {
        let since = source.wrapping_sub(self.base) as i32;
        (since >= 0).then(|| self.offset.wrapping_add(since as u32))
    }
}

/// Whether `time` repeats or precedes `last`, and whether by so much that
/// the clock was reset.
enum Order {
    Next,
    Repeat,
    Reset,
}

fn order(time: u32, last: Option<u32>) -> Order {
    let Some(last) = last else {
        return Order::Next;
    };
    match time.wrapping_sub(last) as i32 {
        delta if delta < -CLOCK_RESET_MS => Order::Reset,
        delta if delta <= 0 => Order::Repeat,
        _ => Order::Next,
    }
}

impl Muxer {
    /// Video keyframes written so far.
    pub(super) fn keyframes(&self) -> u64 {
        self.keyframes
    }

    /// Audio frames written so far.
    pub(super) fn audio_frames(&self) -> u64 {
        self.audio_frames
    }

    /// Appends the tags for a video frame to `out`, after the file header
    /// and a sequence header when needed. Before the first keyframe, frames
    /// are dropped.
    pub(super) fn push_video(
        &mut self,
        frame: VideoFrame,
        out: &mut Vec<FlvData>,
    ) -> Result<(), NativeError> {
        match order(frame.dts, self.last_video) {
            Order::Next => {}
            Order::Repeat => return Ok(()),
            Order::Reset => self.reset(),
        }
        let nalus = annex_b_nalus(&frame.data);
        let config = frame
            .keyframe
            .then(|| VideoConfig::find(frame.codec, &nalus))
            .flatten();
        let config_changed = config.is_some() && config != self.video_config;
        if self.video_config.as_ref().map(VideoConfig::codec) != Some(frame.codec)
            && !config_changed
        {
            // Nothing describes this codec yet.
            return Ok(());
        }

        let timeline = match self.timeline {
            Some(timeline) => timeline,
            None if frame.keyframe => {
                let timeline = Timeline {
                    base: frame.dts,
                    offset: self.last_tag_time.map_or(0, |time| time.wrapping_add(1)),
                };
                self.timeline = Some(timeline);
                timeline
            }
            None => return Ok(()),
        };
        let Some(time) = timeline.tag_time(frame.dts) else {
            return Ok(());
        };

        self.write_header(out);
        if let Some(config) = config.filter(|_| config_changed) {
            out.push(video_tag(time, config.sequence_header()?));
            self.video_config = Some(config);
        }
        let composition_time = frame.pts.wrapping_sub(frame.dts) as i32;
        out.push(video_tag(
            time,
            coded_frame(frame.codec, frame.keyframe, composition_time, &nalus),
        ));
        self.last_video = Some(frame.dts);
        self.wrote(time);
        if frame.keyframe {
            self.keyframes += 1;
        }

        for audio in std::mem::take(&mut self.held_audio) {
            self.push_audio(audio, out)?;
        }
        Ok(())
    }

    /// Appends the tags for an audio frame to `out`, after a sequence header
    /// when needed. Audio from before the first keyframe is dropped.
    pub(super) fn push_audio(
        &mut self,
        frame: AudioFrame,
        out: &mut Vec<FlvData>,
    ) -> Result<(), NativeError> {
        match order(frame.pts, self.last_audio) {
            Order::Next => {}
            Order::Repeat => return Ok(()),
            Order::Reset => self.reset(),
        }
        let Some(timeline) = self.timeline else {
            if self.held_audio.len() == MAX_HELD_AUDIO {
                self.held_audio.pop_front();
            }
            self.held_audio.push_back(frame);
            return Ok(());
        };
        let Some(time) = timeline.tag_time(frame.pts) else {
            return Ok(());
        };
        if frame.header.raw_data_blocks != 1 {
            return Err(NativeError::Unsupported("AAC frames of several blocks"));
        }

        let config = frame.header.audio_specific_config();
        if self.audio_config != Some(config) {
            out.push(audio_tag(time, AAC_SEQUENCE_HEADER, &config));
            self.audio_config = Some(config);
        }
        let raw = frame
            .data
            .get(frame.header.header_length..)
            .unwrap_or_default();
        out.push(audio_tag(time, AAC_RAW, raw));
        self.last_audio = Some(frame.pts);
        self.wrote(time);
        self.audio_frames += 1;
        Ok(())
    }

    fn write_header(&mut self, out: &mut Vec<FlvData>) {
        if !self.header_written {
            out.push(FlvData::Header(FlvHeader::new(true, true)));
            self.header_written = true;
        }
    }

    fn wrote(&mut self, time: u32) {
        if self
            .last_tag_time
            .is_none_or(|last| time.wrapping_sub(last) as i32 > 0)
        {
            self.last_tag_time = Some(time);
        }
    }

    /// Starts over at the next keyframe, after the tags already written.
    fn reset(&mut self) {
        self.timeline = None;
        self.last_video = None;
        self.last_audio = None;
        self.held_audio.clear();
    }
}

impl VideoConfig {
    /// The parameter sets in an access unit, if it has a complete set.
    fn find(codec: VideoCodec, nalus: &[&[u8]]) -> Option<Self> {
        let find = |kind| {
            nalus
                .iter()
                .find(|nalu| nal_type(codec, nalu) == Some(kind))
                .map(|nalu| Bytes::copy_from_slice(nalu))
        };
        Some(match codec {
            VideoCodec::H264 => Self::H264 {
                sps: find(h264_nal::SPS)?,
                pps: find(h264_nal::PPS)?,
            },
            VideoCodec::Hevc => Self::Hevc {
                vps: find(hevc_nal::VPS)?,
                sps: find(hevc_nal::SPS)?,
                pps: find(hevc_nal::PPS)?,
            },
        })
    }

    fn codec(&self) -> VideoCodec {
        match self {
            Self::H264 { .. } => VideoCodec::H264,
            Self::Hevc { .. } => VideoCodec::Hevc,
        }
    }

    /// The body of the sequence header tag.
    fn sequence_header(&self) -> Result<Vec<u8>, NativeError> {
        let invalid =
            |error: std::io::Error| NativeError::Protocol(format!("video parameter sets: {error}"));
        let mut body = Vec::with_capacity(256);
        match self {
            Self::H264 { sps, pps } => {
                body.put_u8(video_tag_byte(
                    VideoFrameType::KeyFrame,
                    VideoCodecId::Avc as u8,
                ));
                body.put_u8(AvcPacketType::SeqHdr as u8);
                body.put_bytes(0, 3);
                AVCDecoderConfigurationRecord::from_parameter_sets(sps.clone(), pps.clone())
                    .and_then(|record| record.build(&mut body))
                    .map_err(invalid)?;
            }
            Self::Hevc { vps, sps, pps } => {
                body.put_u8(enhanced_tag_byte(
                    VideoFrameType::KeyFrame,
                    EnhancedPacketType::SEQUENCE_START,
                ));
                body.put_slice(&VideoFourCC::Hvc1.as_bytes());
                HEVCDecoderConfigurationRecord::from_parameter_sets(
                    vps.clone(),
                    sps.clone(),
                    pps.clone(),
                )
                .and_then(|record| record.mux(&mut body))
                .map_err(invalid)?;
            }
        }
        Ok(body)
    }
}

/// The body of a frame's tag: its NAL units, 4-byte length-prefixed, without
/// the parameter sets and delimiters that the sequence header replaces.
fn coded_frame(
    codec: VideoCodec,
    keyframe: bool,
    composition_time: i32,
    nalus: &[&[u8]],
) -> Vec<u8> {
    let frame_type = if keyframe {
        VideoFrameType::KeyFrame
    } else {
        VideoFrameType::InterFrame
    };
    let size = nalus.iter().map(|nalu| 4 + nalu.len()).sum::<usize>();
    let mut body = Vec::with_capacity(8 + size);
    match codec {
        VideoCodec::H264 => {
            body.put_u8(video_tag_byte(frame_type, VideoCodecId::Avc as u8));
            body.put_u8(AvcPacketType::Nalu as u8);
        }
        VideoCodec::Hevc => {
            body.put_u8(enhanced_tag_byte(
                frame_type,
                EnhancedPacketType::CODED_FRAMES,
            ));
            body.put_slice(&VideoFourCC::Hvc1.as_bytes());
        }
    }
    // SI24.
    let composition_time = composition_time.clamp(-(1 << 23), (1 << 23) - 1);
    body.put_slice(&composition_time.to_be_bytes()[1..]);
    let skipped = match codec {
        VideoCodec::H264 => &[h264_nal::SPS, h264_nal::PPS, h264_nal::AUD][..],
        VideoCodec::Hevc => &[hevc_nal::VPS, hevc_nal::SPS, hevc_nal::PPS, hevc_nal::AUD][..],
    };
    for nalu in nalus {
        if nal_type(codec, nalu).is_some_and(|kind| skipped.contains(&kind)) {
            continue;
        }
        body.put_u32(nalu.len() as u32);
        body.put_slice(nalu);
    }
    body
}

fn video_tag_byte(frame_type: VideoFrameType, codec_id: u8) -> u8 {
    (frame_type as u8) << 4 | codec_id
}

fn enhanced_tag_byte(frame_type: VideoFrameType, packet_type: EnhancedPacketType) -> u8 {
    0x80 | (frame_type as u8) << 4 | packet_type.0
}

fn video_tag(time: u32, body: Vec<u8>) -> FlvData {
    FlvData::Tag(FlvTag::new(time, 0, FlvTagType::Video, false, body.into()))
}

fn audio_tag(time: u32, packet_type: u8, payload: &[u8]) -> FlvData {
    let mut body = Vec::with_capacity(2 + payload.len());
    body.put_u8(AAC_TAG_HEADER);
    body.put_u8(packet_type);
    body.put_slice(payload);
    FlvData::Tag(FlvTag::new(time, 0, FlvTagType::Audio, false, body.into()))
}

mod h264_nal {
    pub(super) const SPS: u8 = 7;
    pub(super) const PPS: u8 = 8;
    pub(super) const AUD: u8 = 9;
}

mod hevc_nal {
    pub(super) const VPS: u8 = 32;
    pub(super) const SPS: u8 = 33;
    pub(super) const PPS: u8 = 34;
    pub(super) const AUD: u8 = 35;
}

fn nal_type(codec: VideoCodec, nalu: &[u8]) -> Option<u8> {
    let first = *nalu.first()?;
    Some(match codec {
        VideoCodec::H264 => first & 0x1f,
        VideoCodec::Hevc => first >> 1 & 0x3f,
    })
}

/// The NAL units of an Annex B access unit, between 3- or 4-byte start
/// codes, without trailing zero bytes.
fn annex_b_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let mut start = None;
    let mut at = 0;
    while at + 3 <= data.len() {
        if data[at..at + 3] == [0, 0, 1] {
            if let Some(start) = start {
                nalus.push(&data[start..at]);
            }
            at += 3;
            start = Some(at);
        } else {
            at += 1;
        }
    }
    if let Some(start) = start {
        nalus.push(&data[start..]);
    }
    for nalu in &mut nalus {
        while let [rest @ .., 0] = nalu {
            *nalu = rest;
        }
    }
    nalus.retain(|nalu| !nalu.is_empty());
    nalus
}

#[cfg(test)]
mod tests {
    use ::flv::avc::AvcPacket;
    use ::flv::hevc::HevcPacket;
    use ::flv::tag::SequenceHeader;
    use ::flv::video::{EnhancedPacket, VideoTagBody};
    use aac::AdtsHeader;

    use super::*;

    // Parameter sets of a live HEVC stream and of the H.264 substream the
    // server switched it to.
    const VPS: &[u8] = b"\x40\x01\x0c\x01\xff\xff\x01\x60\x00\x00\x03\x00\xb0\x00\x00\x03\x00\x00\x03\x00\x5a\x17\x02\x40";
    const SPS: &[u8] = b"\x42\x01\x01\x01\x60\x00\x00\x03\x00\xb0\x00\x00\x03\x00\x00\x03\x00\x5a\xa0\x04\x42\x00\xe4\x77\x3e\x20\x5e\xe4\x59\x14\xbf\xf2\xe7\xf1\x3f\xa9\xa8\x08\x08\x08\x04";
    const PPS: &[u8] = b"\x44\x01\xc0\x72\xf0\x53\x24";
    const AVC_SPS: &[u8] = b"\x67\x4d\x40\x1e\x95\x90\x17\x04\xde\x59\x70\x16\xa0\x20\x20\x28\x00\x00\x1f\x40\x00\x05\xdc\x04\x20";
    const AVC_PPS: &[u8] = b"\x68\xeb\x8f\x20";

    fn video(codec: VideoCodec, keyframe: bool, pts: u32, dts: u32, data: &[u8]) -> VideoFrame {
        VideoFrame {
            codec,
            keyframe,
            pts,
            dts,
            data: Bytes::copy_from_slice(data),
        }
    }

    /// A frame of a 22.05 kHz AAC stream, with the payload byte `fill`.
    fn audio(pts: u32, fill: u8) -> AudioFrame {
        let data = [&b"\xff\xf1\x5c\x40\x45\xa0\xd0"[..], &[fill; 550]].concat();
        AudioFrame {
            pts,
            header: AdtsHeader::parse(&data).unwrap(),
            data: data.into(),
        }
    }

    fn annex_b(nalus: &[&[u8]]) -> Vec<u8> {
        nalus
            .iter()
            .enumerate()
            // Both start code lengths.
            .flat_map(|(at, nalu)| {
                [
                    if at % 2 == 0 {
                        &b"\0\0\0\x01"[..]
                    } else {
                        b"\0\0\x01"
                    },
                    nalu,
                ]
            })
            .flatten()
            .copied()
            .collect()
    }

    fn length_prefixed(nalus: &[&[u8]]) -> Bytes {
        let mut data = Vec::new();
        for nalu in nalus {
            data.put_u32(nalu.len() as u32);
            data.put_slice(nalu);
        }
        data.into()
    }

    fn tag(data: &FlvData) -> &FlvTag {
        match data {
            FlvData::Tag(tag) => tag,
            other => panic!("not a tag: {other:?}"),
        }
    }

    fn video_body(data: &FlvData) -> VideoTagBody {
        tag(data).decode_video().unwrap().body
    }

    #[test]
    fn frames_become_flv_that_the_flv_and_codec_crates_read_back() {
        let sei: &[u8] = b"\x4e\x01\x05\x01\x80";
        let idr: &[u8] = b"\x26\x01\xaf\x38\x74";
        let trail: &[u8] = b"\x02\x01\xd0\x01";
        let avc_idr: &[u8] = b"\x65\xb8\x04\x13";
        let mut muxer = Muxer::default();
        let mut out = Vec::new();
        // From before the keyframe.
        muxer.push_audio(audio(900, 1), &mut out).unwrap();
        muxer
            .push_video(
                video(
                    VideoCodec::Hevc,
                    true,
                    1085,
                    1000,
                    &annex_b(&[VPS, SPS, PPS, sei, idr]),
                ),
                &mut out,
            )
            .unwrap();
        muxer.push_audio(audio(1010, 2), &mut out).unwrap();
        muxer
            .push_video(
                video(VideoCodec::Hevc, false, 1170, 1042, &annex_b(&[trail])),
                &mut out,
            )
            .unwrap();
        // A switch to H.264.
        muxer
            .push_video(
                video(
                    VideoCodec::H264,
                    true,
                    1084,
                    1084,
                    &annex_b(&[AVC_SPS, AVC_PPS, avc_idr]),
                ),
                &mut out,
            )
            .unwrap();

        assert_eq!(out.len(), 8);
        assert_eq!(out[0], FlvData::Header(FlvHeader::new(true, true)));
        let times: Vec<u32> = out[1..].iter().map(|data| tag(data).timestamp_ms).collect();
        assert_eq!(times, [0, 0, 10, 10, 42, 84, 84]);

        let VideoTagBody::Enhanced(EnhancedPacket::Hevc(HevcPacket::SequenceStart(record))) =
            video_body(&out[1])
        else {
            panic!("not an HEVC sequence start: {:?}", out[1]);
        };
        let arrays: Vec<&[u8]> = record
            .arrays
            .iter()
            .map(|array| &array.nalus[0][..])
            .collect();
        assert_eq!(arrays, [VPS, SPS, PPS]);
        assert!(tag(&out[2]).is_key_frame_nalu());
        assert_eq!(
            video_body(&out[2]),
            VideoTagBody::Enhanced(EnhancedPacket::Hevc(HevcPacket::Nalu {
                composition_time: Some(85),
                data: length_prefixed(&[sei, idr]),
            }))
        );

        let Some(SequenceHeader::Audio { configuration, .. }) = tag(&out[3]).sequence_header()
        else {
            panic!("not an audio sequence header: {:?}", out[3]);
        };
        assert_eq!(configuration, [0x13, 0x88]);
        assert_eq!(tag(&out[4]).data()[2..], [2; 550]);

        assert_eq!(
            video_body(&out[5]),
            VideoTagBody::Enhanced(EnhancedPacket::Hevc(HevcPacket::Nalu {
                composition_time: Some(128),
                data: length_prefixed(&[trail]),
            }))
        );
        let VideoTagBody::Avc(AvcPacket::SequenceHeader(record)) = video_body(&out[6]) else {
            panic!("not an AVC sequence header: {:?}", out[6]);
        };
        assert_eq!((&record.sps[0][..], &record.pps[0][..]), (AVC_SPS, AVC_PPS));
        assert_eq!(
            video_body(&out[7]),
            VideoTagBody::Avc(AvcPacket::Nalu {
                composition_time: 0,
                data: length_prefixed(&[avc_idr]),
            })
        );
    }

    #[test]
    fn the_timeline_continues_across_repeats_and_clock_resets() {
        enum Frame {
            Key(u32),
            Inter(u32),
            Audio(u32),
        }
        use Frame::{Audio, Inter, Key};

        let keyframe = annex_b(&[VPS, SPS, PPS, b"\x26\x01\xaf"]);
        let mut muxer = Muxer::default();
        // The source clock wraps around, then restarts an hour back.
        let t = |ms: u32| (u32::MAX - 20).wrapping_add(ms);
        let restart = |ms: u32| t(0).wrapping_sub(3_600_000).wrapping_add(ms);
        let steps: &[(Frame, &[u32])] = &[
            (Key(t(0)), &[0]),
            (Inter(t(40)), &[40]),
            (Audio(t(10)), &[10]),
            // A reconnect sends what was written again, then new frames.
            (Key(t(0)), &[]),
            (Inter(t(40)), &[]),
            (Audio(t(10)), &[]),
            (Inter(t(80)), &[80]),
            (Audio(t(56)), &[56]),
            // The source clock restarts: frames wait for a keyframe, which
            // continues after the last tag.
            (Inter(restart(100)), &[]),
            (Audio(restart(110)), &[]),
            (Key(restart(100)), &[81, 91]),
            (Inter(restart(140)), &[121]),
        ];
        for (at, (frame, expected)) in steps.iter().enumerate() {
            let mut out = Vec::new();
            match *frame {
                Key(dts) => {
                    muxer.push_video(video(VideoCodec::Hevc, true, dts, dts, &keyframe), &mut out)
                }
                Inter(dts) => muxer.push_video(
                    video(VideoCodec::Hevc, false, dts, dts, b"\0\0\x01\x02\x01"),
                    &mut out,
                ),
                Audio(pts) => muxer.push_audio(audio(pts, 0), &mut out),
            }
            .unwrap();
            let times: Vec<u32> = out
                .iter()
                .filter_map(|data| match data {
                    FlvData::Tag(tag) if !tag.classification().sequence_header => {
                        Some(tag.timestamp_ms)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(times, *expected, "step {at}");
        }
    }
}
