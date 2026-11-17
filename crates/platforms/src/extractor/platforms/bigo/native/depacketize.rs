//! Media packets to frames.
//!
//! A video frame arrives in fragments, each packet carrying a header with the
//! frame number, its index and flags. The concatenated fragments are
//! `[u32 pts][u32 dts][u8][Annex B access unit]`, times in ms. The server
//! can send a room's video in several substreams ("modes", e.g. HEVC and a
//! lower H.264 rendition) and switches between them at keyframes; frame
//! numbers run on across a switch.
//!
//! An audio packet carries a header and a few ADTS frames.
//!
//! Headers are sequences of optional fields whose presence the flag bytes
//! before them announce. Only the fields read here are named; the others are
//! skipped by size.

use std::collections::{BTreeMap, HashMap};

use aac::AdtsHeader;
use bytes::{Buf, Bytes};
use tracing::{debug, info};

use super::NativeError;
use super::linkd::take;

/// Bound on the frames being reassembled.
const MAX_PENDING_FRAMES: usize = 32;
/// Bound on the memory of the frames being reassembled: their bytes, and
/// [`FRAGMENT_COST`] for each fragment, so that many tiny ones count too.
const MAX_PENDING_BYTES: usize = 8 << 20;
const FRAGMENT_COST: usize = 64;
/// Bytes before an assembled frame's access unit: pts, dts and one more.
const FRAME_PREFIX_LEN: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VideoCodec {
    H264,
    Hevc,
}

/// A complete video frame.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct VideoFrame {
    pub(super) codec: VideoCodec,
    pub(super) keyframe: bool,
    /// Presentation and decoding times, in ms.
    pub(super) pts: u32,
    pub(super) dts: u32,
    /// The access unit in Annex B form.
    pub(super) data: Bytes,
}

/// One ADTS frame.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct AudioFrame {
    /// Presentation time in ms, on the video's clock.
    pub(super) pts: u32,
    pub(super) header: AdtsHeader,
    /// The whole ADTS frame, header included.
    pub(super) data: Bytes,
}

/// One packet's share of a video frame.
#[derive(Debug)]
struct Fragment {
    frame: u32,
    keyframe: bool,
    codec: u8,
    /// The participant whose video this is, when the header says.
    owner: Option<u32>,
    mode: u8,
    index: u16,
    /// The frame's fragment count, when this fragment tells it.
    count: Option<u32>,
    /// Whether the frame combines several participants' video.
    combined: bool,
    payload: Bytes,
}

impl Fragment {
    fn parse(mut packet: Bytes) -> Result<Self, NativeError> {
        let mut header = &packet[..];

        // The packet word's top byte holds the frame kind; then two bytes
        // not used here.
        skip(&mut header, 3)?;
        let keyframe = header.try_get_u8()? & 7 == 1;
        skip(&mut header, 2)?;
        let frame = header.try_get_u32_le()?;
        let f = header.try_get_u8()?;
        let g = header.try_get_u8()?;
        let split = g & 16 != 0;
        let last = g & 32 != 0;

        let mut owner = None;
        let mut mode = 0;
        let mut index = 0u16;
        let mut count = None;
        let mut combined = false;
        // Counted from the packet's third byte.
        let mut header_len = None;
        if g & 1 != 0 {
            owner = Some(header.try_get_u32_le()?);
        }
        if g & 4 != 0 {
            skip(&mut header, 4)?;
        }
        if g & 8 != 0 {
            skip(&mut header, 1)?;
        }
        if split {
            index = u16::from(header.try_get_u8()?);
        }
        if g & 128 != 0 {
            skip(&mut header, 4)?;
        }
        if f & 64 != 0 {
            skip(&mut header, 5)?;
        }
        if f & 16 != 0 {
            skip(&mut header, 2)?;
        }
        if f & 32 != 0 {
            let e = header.try_get_u8()?;
            if e & 2 != 0 {
                mode = header.try_get_u8()?;
            }
            if e & 4 != 0 {
                header_len = Some(usize::from(header.try_get_u8()?));
            }
            if e & 8 != 0 {
                skip(&mut header, 4)?;
            }
            if e & 16 != 0 {
                count = Some(u32::from(header.try_get_u8()? & 127));
            }
            if e & 64 != 0 {
                index |= u16::from(header.try_get_u8()?) << 8;
            }
            if e & 128 != 0 {
                let e2 = header.try_get_u8()?;
                if e2 & 8 != 0 {
                    skip(&mut header, 1)?;
                }
                if e2 & 16 != 0 {
                    skip(&mut header, 33)?;
                }
                if e2 & 64 != 0 {
                    skip(&mut header, 2)?;
                }
                if e2 & 128 != 0 {
                    let e3 = header.try_get_u8()?;
                    if e3 & 1 != 0 {
                        // The low 32 bits of a 64-bit uid, then 16 bytes.
                        owner = Some(header.try_get_u64_le()? as u32);
                        skip(&mut header, 16)?;
                    }
                    if e3 & 2 != 0 {
                        skip(&mut header, 1)?;
                    }
                    if e3 & 4 != 0 {
                        skip(&mut header, 2)?;
                    }
                    if e3 & 32 != 0 {
                        skip(&mut header, 4)?;
                    }
                    if e3 & 64 != 0 {
                        skip(&mut header, 2)?;
                    }
                    if e3 & 128 != 0 {
                        let e4 = header.try_get_u8()?;
                        if e4 & 1 != 0 {
                            skip(&mut header, 1)?;
                        }
                        if e4 & 4 != 0 {
                            skip(&mut header, 1)?;
                        }
                        if e4 & 8 != 0 {
                            skip(&mut header, 4)?;
                        }
                        if e4 & 16 != 0 {
                            skip(&mut header, 2)?;
                        }
                        if e4 & 64 != 0 {
                            header_len = Some(usize::from(header.try_get_u16_le()?));
                        }
                        if e4 & 128 != 0 {
                            combined = header.try_get_u8()? & 1 != 0;
                        }
                    }
                }
            }
        }

        let mut payload_start = packet.len() - header.len();
        if let Some(header_len) = header_len {
            payload_start = payload_start.max(2 + header_len);
        }
        if payload_start > packet.len() {
            return Err(NativeError::Protocol(
                "video packet shorter than its header".to_owned(),
            ));
        }
        // A frame's last fragment, or its only one, tells the count.
        if !split || last {
            count = Some(u32::from(index) + 1);
        }
        packet.advance(payload_start);
        Ok(Self {
            frame,
            keyframe,
            codec: f & 7,
            owner,
            mode,
            index,
            count,
            combined,
            payload: packet,
        })
    }
}

/// A frame whose fragments are still arriving.
#[derive(Default)]
struct PartialFrame {
    keyframe: bool,
    codec: u8,
    combined: bool,
    count: Option<u32>,
    parts: BTreeMap<u16, Bytes>,
    /// The memory held, counted as for [`MAX_PENDING_BYTES`].
    cost: usize,
}

impl PartialFrame {
    /// Whether the fragments `0..count` are all there.
    fn is_complete(&self) -> bool {
        self.count.is_some_and(|count| {
            self.parts.len() as u32 == count
                && self
                    .parts
                    .last_key_value()
                    .is_some_and(|(&index, _)| u32::from(index) < count)
        })
    }

    fn assemble(self) -> Result<VideoFrame, NativeError> {
        if self.combined {
            return Err(NativeError::Unsupported("video of combined sources"));
        }
        let codec = match self.codec {
            0 | 2 | 3 => VideoCodec::H264,
            4 | 5 => VideoCodec::Hevc,
            _ => return Err(NativeError::Unsupported("video codec")),
        };
        let mut data = Vec::with_capacity(self.parts.values().map(Bytes::len).sum());
        for part in self.parts.into_values() {
            data.extend_from_slice(&part);
        }
        let mut data = Bytes::from(data);
        if data.len() < FRAME_PREFIX_LEN {
            return Err(NativeError::Protocol(
                "video frame without timing".to_owned(),
            ));
        }
        let pts = data.get_u32_le();
        let dts = data.get_u32_le();
        data.advance(1);
        Ok(VideoFrame {
            codec,
            keyframe: self.keyframe,
            pts,
            dts,
            data,
        })
    }
}

/// The frames of one substream, by frame number.
#[derive(Default)]
struct Substream {
    frames: BTreeMap<u32, PartialFrame>,
    /// A frame went missing; frames that may depend on it are dropped until
    /// the next keyframe.
    broken: bool,
    /// Frames numbered below this were output or given up. The server
    /// resends lost fragments a few frames late, and those of such a frame
    /// are dropped rather than holding up the frames after it.
    floor: u32,
}

/// Reassembles one streamer's video frames from their fragments.
///
/// Frames of a substream come out in frame-number order, starting at a
/// keyframe. Output follows one substream, and moves to another at that
/// one's first keyframe. Fragments of other participants are dropped, and so
/// are packets that cannot be read.
pub(super) struct VideoDepacketizer {
    owner_uid: u32,
    substreams: HashMap<u8, Substream>,
    /// The substream being output.
    current: Option<u8>,
    pending_frames: usize,
    pending_bytes: usize,
}

impl VideoDepacketizer {
    pub(super) fn new(owner_uid: u32) -> Self {
        Self {
            owner_uid,
            substreams: HashMap::new(),
            current: None,
            pending_frames: 0,
            pending_bytes: 0,
        }
    }

    /// Takes a video packet and appends the frames it completes to `out`.
    /// Fails only for a frame that would be output but cannot be.
    pub(super) fn push(
        &mut self,
        packet: Bytes,
        out: &mut Vec<VideoFrame>,
    ) -> Result<(), NativeError> {
        let fragment = match Fragment::parse(packet) {
            Ok(fragment) => fragment,
            Err(error) => {
                debug!(%error, "video packet unreadable; dropped");
                return Ok(());
            }
        };
        if fragment.owner.is_some_and(|owner| owner != self.owner_uid) {
            return Ok(());
        }
        let mode = fragment.mode;
        let substream = self.substreams.entry(mode).or_default();
        if fragment.frame < substream.floor {
            return Ok(());
        }
        let frame = substream.frames.entry(fragment.frame).or_insert_with(|| {
            self.pending_frames += 1;
            PartialFrame::default()
        });
        frame.keyframe |= fragment.keyframe;
        frame.codec = fragment.codec;
        frame.combined |= fragment.combined;
        frame.count = frame.count.or(fragment.count);
        let cost = fragment.payload.len() + FRAGMENT_COST;
        if let Some(replaced) = frame.parts.insert(fragment.index, fragment.payload) {
            frame.cost -= replaced.len() + FRAGMENT_COST;
            self.pending_bytes -= replaced.len() + FRAGMENT_COST;
        }
        frame.cost += cost;
        self.pending_bytes += cost;

        // Frames before a complete keyframe can no longer matter. Draining
        // the keyframe then raises the floor past them.
        if frame.keyframe && frame.is_complete() {
            let newer = substream.frames.split_off(&fragment.frame);
            for (_, stale) in std::mem::replace(&mut substream.frames, newer) {
                self.pending_frames -= 1;
                self.pending_bytes -= stale.cost;
            }
        }
        self.drain(mode, out)?;

        while self.pending_frames > MAX_PENDING_FRAMES || self.pending_bytes > MAX_PENDING_BYTES {
            let Some(mode) = self.oldest() else {
                break;
            };
            if let Some(substream) = self.substreams.get_mut(&mode)
                && let Some((number, lost)) = substream.frames.pop_first()
            {
                debug!(mode, frame = number, "video frame incomplete; dropped");
                substream.broken = true;
                substream.floor = number.saturating_add(1);
                self.pending_frames -= 1;
                self.pending_bytes -= lost.cost;
            }
            self.drain(mode, out)?;
        }
        Ok(())
    }

    /// Outputs the complete frames at the front of a substream.
    fn drain(&mut self, mode: u8, out: &mut Vec<VideoFrame>) -> Result<(), NativeError> {
        let Some(substream) = self.substreams.get_mut(&mode) else {
            return Ok(());
        };
        while let Some(entry) = substream.frames.first_entry() {
            if !entry.get().is_complete() {
                break;
            }
            let (number, frame) = entry.remove_entry();
            substream.floor = number.saturating_add(1);
            self.pending_frames -= 1;
            self.pending_bytes -= frame.cost;
            if frame.keyframe {
                substream.broken = false;
            } else if substream.broken {
                continue;
            }
            let switched = self.current != Some(mode);
            if switched && !frame.keyframe {
                continue;
            }
            let frame = match frame.assemble() {
                Ok(frame) => frame,
                // Output stays with a substream it can follow.
                Err(NativeError::Unsupported(what)) if switched && self.current.is_some() => {
                    debug!(mode, what, "video substream not followed");
                    continue;
                }
                Err(error @ NativeError::Unsupported(_)) => return Err(error),
                Err(error) => {
                    debug!(mode, frame = number, %error, "video frame unreadable; dropped");
                    substream.broken = true;
                    continue;
                }
            };
            if switched {
                match self.current {
                    Some(from) => info!(
                        owner = self.owner_uid,
                        from,
                        to = mode,
                        codec = ?frame.codec,
                        "video substream switched"
                    ),
                    None => {
                        debug!(owner = self.owner_uid, mode, codec = ?frame.codec, "video substream")
                    }
                }
                self.current = Some(mode);
            }
            out.push(frame);
        }
        Ok(())
    }

    /// The substream holding the oldest pending frame.
    fn oldest(&self) -> Option<u8> {
        self.substreams
            .iter()
            .filter_map(|(&mode, substream)| Some((*substream.frames.keys().next()?, mode)))
            .min()
            .map(|(_, mode)| mode)
    }
}

fn skip(body: &mut &[u8], len: usize) -> Result<(), NativeError> {
    take(body, len).map(|_| ())
}

/// The ADTS frames in an audio packet, with their times. Empty for another
/// participant's audio, a packet without a time and one that cannot be
/// read. Fails only for the streamer's audio in a format not recorded.
pub(super) fn audio_frames(packet: &Bytes, owner_uid: u32) -> Result<Vec<AudioFrame>, NativeError> {
    match read_audio(packet, owner_uid) {
        Err(error @ NativeError::Unsupported(_)) => Err(error),
        Err(error) => {
            debug!(%error, "audio packet unreadable; dropped");
            Ok(Vec::new())
        }
        frames => frames,
    }
}

fn read_audio(packet: &Bytes, owner_uid: u32) -> Result<Vec<AudioFrame>, NativeError> {
    let mut header = &packet[..];

    skip(&mut header, 2)?;
    let f = header.try_get_u8()?;
    let g = header.try_get_u8()?;
    if g & 128 == 0 {
        // Its owner cannot be told.
        return Err(NativeError::Protocol("legacy audio header".to_owned()));
    }
    // Sequence number.
    skip(&mut header, 4)?;
    if g & 2 != 0 {
        skip(&mut header, 4)?;
    }
    if g & 64 != 0 {
        skip(&mut header, 4)?;
    }
    let pts = if g & 32 != 0 {
        Some(header.try_get_u32_le()?)
    } else {
        None
    };
    let mut owner = None;
    if g & 16 != 0 {
        owner = Some(header.try_get_u32_le()?);
    }
    if g & 8 != 0 {
        skip(&mut header, 4)?;
    }
    skip(&mut header, 1)?;
    let codec = header.try_get_u8()?;
    skip(&mut header, 4)?;
    if f & 1 != 0 {
        let x = header.try_get_u8()?;
        if x & 1 != 0 {
            skip(&mut header, 4)?;
        }
        if x & 2 != 0 {
            skip(&mut header, 1)?;
        }
    }
    if g & 1 != 0 {
        let e = header.try_get_u8()?;
        let mut header_len = 0;
        if e & 2 != 0 {
            skip(&mut header, 1)?;
        }
        if e & 4 != 0 {
            skip(&mut header, 4)?;
        }
        if e & 16 != 0 {
            // Counted from the packet's first byte.
            header_len = usize::from(header.try_get_u8()?);
        }
        if e & 32 != 0 {
            skip(&mut header, 4)?;
        }
        if e & 64 != 0 {
            // The low 32 bits of a 64-bit uid, then 16 bytes.
            owner = Some(header.try_get_u64_le()? as u32);
            skip(&mut header, 16)?;
        }
        if e & 128 != 0 {
            let e2 = header.try_get_u8()?;
            if e2 & 4 != 0 {
                let len = usize::from(header.try_get_u16_le()?);
                skip(&mut header, len)?;
            }
            if e2 & 8 != 0 {
                skip(&mut header, 4)?;
            }
            if e2 & 16 != 0 {
                skip(&mut header, 4)?;
            }
            if e2 & 32 != 0 {
                let count = usize::from(header.try_get_u8()?);
                skip(&mut header, 8 * count)?;
            }
            if e2 & 128 != 0 {
                let e3 = header.try_get_u8()?;
                if e3 & 2 != 0 {
                    let count = usize::from(header.try_get_u8()?);
                    skip(&mut header, 13 * count)?;
                }
                if e3 & 4 != 0 {
                    let count = usize::from(header.try_get_u8()?);
                    skip(&mut header, 8 * count)?;
                }
            }
        }
        let read = packet.len() - header.len();
        skip(&mut header, header_len.saturating_sub(read))?;
    }

    if owner.is_some_and(|owner| owner != owner_uid) {
        return Ok(Vec::new());
    }
    match codec {
        6 => {}
        7 => return Err(NativeError::Unsupported("Opus audio")),
        _ => return Err(NativeError::Unsupported("audio codec")),
    }
    let Some(pts) = pts else {
        debug!("audio packet without a time; dropped");
        return Ok(Vec::new());
    };

    let mut frames = Vec::new();
    // Samples since the packet's first frame, at the frames' rate.
    let mut samples = 0u64;
    for _ in 0..f >> 5 {
        skip(&mut header, 1)?;
        let len = header.try_get_u16_le()?;
        if len & 0xfc00 != 0xfc00 {
            return Err(NativeError::Protocol(format!(
                "audio frame length {len:#x}"
            )));
        }
        let data = take(&mut header, usize::from(len & 0x3ff))?;
        let adts = AdtsHeader::parse(data)
            .map_err(|error| NativeError::Protocol(format!("audio frame: {error}")))?;
        if adts.frame_length != data.len() {
            return Err(NativeError::Protocol(
                "audio frame length differs from its ADTS header".to_owned(),
            ));
        }
        let since = (samples * 1000)
            .checked_div(u64::from(adts.sampling_frequency()))
            .ok_or_else(|| NativeError::Protocol("audio frame without a sample rate".to_owned()))?;
        frames.push(AudioFrame {
            pts: pts.wrapping_add(since as u32),
            header: adts,
            data: packet.slice_ref(data),
        });
        samples += 1024 * u64::from(adts.raw_data_blocks);
    }
    Ok(frames)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// The streamer in the captured headers below, whose ids are made up.
    pub(crate) const OWNER: u32 = 0x0102_0304;

    // Headers of fragments from a live room: the first and last of an HEVC
    // keyframe in the default substream, the three of the next frame but
    // one, and the first and last of the H.264 keyframe that the server
    // switched to in substream 160.
    const HEVC_KEY_FIRST: &str = "b546d0c1726ebb890200259d040302010100e80300000d0c0b0a84599d000000000000000000000000000000000000000000000000001800000030000000008104030201000000000d0c0b0a00000000000000000000000084ac00";
    const HEVC_KEY_LAST: &str = "6541d0c1ce6ebb890200253d040302012f00e803fe2e8430800104030201000000000d0c0b0a000000000000000000000000";
    const HEVC_P: [&str; 3] = [
        "e533d0c3dc6ebd890200259d040302013600e803fe000d0c0b0a8434850104030201000000000d0c0b0a000000000000000000000000",
        "a533d0c3de6ebd890200251d040302013700e803fe018430800104030201000000000d0c0b0a000000000000000000000000",
        "9533d0c3e06ebd890200253d040302013800e803fe028430800104030201000000000d0c0b0a000000000000000000000000",
    ];
    pub(crate) const H264_KEY_FIRST: &str = "b54bd0c162daeb890200229d040302013a01e803ff000d0c0b0a86a05a9d096801000058020000000000006701000000000000570200000000000030000000008104030201000000000d0c0b0a00000000000000008a008b0084ac00";
    pub(crate) const H264_KEY_LAST: &str = "e548d0c190daeb890200223d040302015101e803ff1786a033808104030201000000000d0c0b0a00000000000000000000000004ac";
    /// Where the frame number and the fragment index sit in these headers.
    const FRAME_AT: usize = 6;
    const INDEX_AT: usize = 21;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect()
    }

    pub(crate) fn packet(header: &str, payload: &[u8]) -> Bytes {
        [hex(header), payload.to_vec()].concat().into()
    }

    /// A last fragment renumbered as the second, so that the keyframe has
    /// two fragments.
    pub(crate) fn second_and_last(header: &str, payload: &[u8]) -> Bytes {
        let mut packet = [hex(header), payload.to_vec()].concat();
        packet[INDEX_AT] = 1;
        packet.into()
    }

    pub(crate) fn numbered(packet: Bytes, frame: u32) -> Bytes {
        let mut packet = packet.to_vec();
        packet[FRAME_AT..FRAME_AT + 4].copy_from_slice(&frame.to_le_bytes());
        packet.into()
    }

    /// The start of a frame's first fragment: pts, dts, one byte, then the
    /// access unit.
    pub(crate) fn frame_start(pts: u32, dts: u32, access_unit: &[u8]) -> Vec<u8> {
        [
            &pts.to_le_bytes()[..],
            &dts.to_le_bytes(),
            &[0],
            access_unit,
        ]
        .concat()
    }

    fn push(depacketizer: &mut VideoDepacketizer, packets: Vec<Bytes>) -> Vec<VideoFrame> {
        let mut frames = Vec::new();
        for packet in packets {
            depacketizer.push(packet, &mut frames).unwrap();
        }
        frames
    }

    fn hevc_keyframe() -> Vec<Bytes> {
        vec![
            packet(
                HEVC_KEY_FIRST,
                &frame_start(1085, 1000, b"\0\0\0\x01\x26\x01key"),
            ),
            second_and_last(HEVC_KEY_LAST, b"frame"),
        ]
    }

    #[test]
    fn frames_are_reassembled_in_fragment_order() {
        let mut depacketizer = VideoDepacketizer::new(OWNER);
        let mut packets = hevc_keyframe();
        // The next frame's fragments arrive last first.
        packets.extend([
            packet(HEVC_P[2], b" fragment"),
            packet(
                HEVC_P[0],
                &frame_start(1170, 1084, b"\0\0\x01\x02\x01first"),
            ),
            packet(HEVC_P[1], b", second"),
        ]);

        assert_eq!(
            push(&mut depacketizer, packets),
            [
                VideoFrame {
                    codec: VideoCodec::Hevc,
                    keyframe: true,
                    pts: 1085,
                    dts: 1000,
                    data: Bytes::from_static(b"\0\0\0\x01\x26\x01keyframe"),
                },
                VideoFrame {
                    codec: VideoCodec::Hevc,
                    keyframe: false,
                    pts: 1170,
                    dts: 1084,
                    data: Bytes::from_static(b"\0\0\x01\x02\x01first, second fragment"),
                },
            ]
        );
    }

    #[test]
    fn output_follows_the_substream_with_the_latest_keyframe_of_the_streamer() {
        let mut depacketizer = VideoDepacketizer::new(OWNER);
        let hevc_p = || {
            vec![
                packet(HEVC_P[0], &frame_start(1170, 1084, b"\0\0\x01\x02\x01")),
                packet(HEVC_P[1], b""),
                packet(HEVC_P[2], b""),
            ]
        };
        let output = |frames: Vec<VideoFrame>| -> Vec<(VideoCodec, bool, u32)> {
            frames
                .into_iter()
                .map(|frame| (frame.codec, frame.keyframe, frame.dts))
                .collect()
        };

        let mut packets = hevc_keyframe();
        packets.extend(hevc_p());
        assert_eq!(
            output(push(&mut depacketizer, packets)),
            [
                (VideoCodec::Hevc, true, 1000),
                (VideoCodec::Hevc, false, 1084)
            ]
        );

        // The server moves to the H.264 substream at its keyframe.
        let switch = vec![
            packet(H264_KEY_FIRST, &frame_start(2000, 2000, b"\0\0\0\x01\x65")),
            second_and_last(H264_KEY_LAST, b""),
        ];
        assert_eq!(
            output(push(&mut depacketizer, switch)),
            [(VideoCodec::H264, true, 2000)]
        );

        // A late frame of the old substream does not switch back, and
        // neither does another participant's keyframe.
        assert_eq!(push(&mut depacketizer, hevc_p()), []);
        let guest = hevc_keyframe()
            .into_iter()
            .map(|packet| {
                let mut packet = packet.to_vec();
                for at in 0..packet.len() - 4 {
                    if packet[at..at + 4] == OWNER.to_le_bytes() {
                        packet[at..at + 4].copy_from_slice(&0x0506_0708u32.to_le_bytes());
                    }
                }
                Bytes::from(packet)
            })
            .collect();
        assert_eq!(push(&mut depacketizer, guest), []);
    }

    #[test]
    fn late_fragments_neither_reorder_nor_hold_up_the_frames_after_them() {
        let mut depacketizer = VideoDepacketizer::new(OWNER);
        let keyframe = |number, dts| {
            [
                packet(HEVC_KEY_FIRST, &frame_start(dts, dts, b"\0\0\x01\x26\x01")),
                second_and_last(HEVC_KEY_LAST, b""),
            ]
            .map(|packet| numbered(packet, number))
        };
        let inter_frame = |number, dts| {
            [
                packet(HEVC_P[0], &frame_start(dts, dts, b"\0\0\x01\x02\x01")),
                packet(HEVC_P[1], b""),
                packet(HEVC_P[2], b""),
            ]
            .map(|packet| numbered(packet, number))
        };
        let mut output = |packets: &[&[Bytes]]| -> Vec<u32> {
            push(&mut depacketizer, packets.concat())
                .into_iter()
                .map(|frame| frame.dts)
                .collect()
        };

        // As seen live, the server resends a frame's lost middle fragments
        // after the next few frames.
        let [first, middle, last] = inter_frame(11, 1040);
        assert_eq!(
            output(&[
                &keyframe(10, 1000),
                &[first, last],
                &inter_frame(12, 1080),
                &inter_frame(13, 1120),
            ]),
            [1000]
        );
        assert_eq!(output(&[&[middle]]), [1040, 1080, 1120]);

        // When they come after a keyframe, the frames before it are lost.
        let [first, middle, last] = inter_frame(14, 1160);
        assert_eq!(
            output(&[&[first, last], &inter_frame(15, 1200), &keyframe(16, 1240)]),
            [1240]
        );
        assert_eq!(output(&[&[middle], &inter_frame(17, 1280)]), [1280]);
    }

    #[test]
    fn audio_packets_give_adts_frames_with_their_times() {
        // Captured headers of both layouts seen, with made-up ids, each
        // followed by two frames: a flags byte, the length with its marker
        // bits, and the ADTS frame. The frames have a captured 22.05 kHz
        // header and silent payloads.
        let adts = [&b"\xff\xf1\x5c\x40\x45\xa0\xd0"[..], &[0; 550]].concat();
        let sample = [&[0x40][..], &(0xfc00 | 557u16).to_le_bytes(), &adts].concat();
        let packets = [
            "c24740b90ad602006d85178c040302010d0c0b0a01062a6402008040",
            "124840b88af002001b541c8c040302010d0c0b0a0106aa7e0200",
        ]
        .map(|header| Bytes::from([hex(header), sample.clone(), sample.clone()].concat()));

        let times = |packet: &Bytes, owner| {
            audio_frames(packet, owner)
                .unwrap()
                .into_iter()
                .map(|frame| {
                    assert_eq!(frame.data, adts);
                    frame.pts
                })
                .collect::<Vec<_>>()
        };
        // 1024 samples at 22.05 kHz last 46.4 ms.
        assert_eq!(times(&packets[0], OWNER), [2_350_351_725, 2_350_351_771]);
        assert_eq!(times(&packets[1], OWNER), [2_350_666_779, 2_350_666_825]);
        assert!(times(&packets[0], 0x0506_0708).is_empty());
    }
}
