//! Media for in-process recording tests.

use std::path::{Path, PathBuf};
use std::pin::Pin;

use bytes::Bytes;
use flv::data::FlvData;
use flv::header::FlvHeader;
use flv::parser::FlvParser;
use flv::tag::{FlvTag, FlvTagType};
use futures::{Stream, stream};
use pipeline_common::PipelineError;

/// Interval between video frames.
pub const FRAME_MS: u32 = 40;

pub type ItemStream<T> = Pin<Box<dyn Stream<Item = Result<T, PipelineError>> + Send>>;

pub fn item_stream<T: Send + 'static>(items: Vec<T>) -> ItemStream<T> {
    Box::pin(stream::iter(items.into_iter().map(Ok)))
}

fn tag(tag_type: FlvTagType, timestamp_ms: u32, data: &[u8]) -> FlvData {
    FlvData::Tag(FlvTag::new(
        timestamp_ms,
        0,
        tag_type,
        false,
        Bytes::copy_from_slice(data),
    ))
}

/// `onMetaData` with an empty ECMA array.
fn metadata(timestamp_ms: u32) -> FlvData {
    let mut data = vec![0x02, 0x00, 0x0a];
    data.extend_from_slice(b"onMetaData");
    data.extend_from_slice(&[0x08, 0, 0, 0, 0, 0, 0, 0x09]);
    tag(FlvTagType::ScriptData, timestamp_ms, &data)
}

/// An AVC decoder configuration for 1280x720 High profile.
fn avc_sequence_header(timestamp_ms: u32) -> FlvData {
    const SPS: &[u8] = &[
        0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40, 0x50, 0x05, 0xbb, 0x01, 0x10, 0x00, 0x00, 0x03,
        0x00, 0x10, 0x00, 0x00, 0x03, 0x03, 0xc0, 0xf1, 0x83, 0x19, 0x60,
    ];
    const PPS: &[u8] = &[0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0];
    let mut data = vec![0x17, 0x00, 0, 0, 0, 0x01, 0x64, 0x00, 0x1f, 0xff, 0xe1];
    data.extend_from_slice(&(SPS.len() as u16).to_be_bytes());
    data.extend_from_slice(SPS);
    data.push(0x01);
    data.extend_from_slice(&(PPS.len() as u16).to_be_bytes());
    data.extend_from_slice(PPS);
    tag(FlvTagType::Video, timestamp_ms, &data)
}

/// AAC-LC, 44.1 kHz, stereo.
fn aac_sequence_header(timestamp_ms: u32) -> FlvData {
    tag(FlvTagType::Audio, timestamp_ms, &[0xaf, 0x00, 0x12, 0x10])
}

fn video(timestamp_ms: u32, keyframe: bool) -> FlvData {
    let (frame_type, nal_header) = if keyframe { (0x17, 0x65) } else { (0x27, 0x41) };
    tag(
        FlvTagType::Video,
        timestamp_ms,
        &[
            frame_type, 0x01, 0, 0, 0, 0, 0, 0, 5, nal_header, 0x88, 0x84, 0x00, 0x33,
        ],
    )
}

fn audio(timestamp_ms: u32) -> FlvData {
    tag(
        FlvTagType::Audio,
        timestamp_ms,
        &[0xaf, 0x01, 0x21, 0x10, 0x04, 0x60, 0x8c, 0x1c],
    )
}

/// A recording from `start_ms`: the stream's initialization, then `gops`
/// groups of `frames` video frames, each followed by an audio frame.
pub fn flv_recording(start_ms: u32, gops: u32, frames: u32) -> Vec<FlvData> {
    let mut items = vec![
        FlvData::Header(FlvHeader::new(true, true)),
        metadata(start_ms),
        avc_sequence_header(start_ms),
        aac_sequence_header(start_ms),
    ];
    for frame in 0..gops * frames {
        let timestamp_ms = start_ms + frame * FRAME_MS;
        items.push(video(timestamp_ms, frame % frames == 0));
        items.push(audio(timestamp_ms));
    }
    items
}

/// The files in `dir` with `extension`, in name order.
pub fn files(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("output directory")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .collect();
    files.sort();
    files
}

pub fn read_flv(path: &Path) -> Vec<FlvTag> {
    let mut reader = std::io::BufReader::new(std::fs::File::open(path).expect("FLV file"));
    let header = FlvParser::parse_header(&mut reader).expect("FLV header");
    let mut tags = Vec::new();
    FlvParser::parse_tags(
        &mut reader,
        |tag, _, _| tags.push(tag.clone()),
        u64::from(header.data_offset),
    )
    .expect("FLV tags");
    tags
}

/// Video frames, excluding sequence headers.
pub fn frames(tags: &[FlvTag]) -> Vec<&FlvTag> {
    tags.iter()
        .filter(|tag| tag.is_video_tag() && !tag.is_video_sequence_header())
        .collect()
}
