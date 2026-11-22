//! Unified Douyin stream extraction across borrowed (`DouyinQuality`) and owned
//! (`DouyinQualityOwned`) SDK quality descriptors and legacy pull URLs.

use rustc_hash::FxHashMap;
use tracing::debug;

use crate::extractor::error::ExtractorError;
use crate::media::formats::{MediaFormat, StreamFormat};
use crate::media::stream_info::StreamInfo;

use super::builder::Douyin;
use super::models::{
    DouyinQuality, DouyinQualityOwned, DouyinStreamDataParsed, DouyinStreamUrl, normalize_bitrate,
    normalize_codec,
};

pub(super) trait QualitySpec {
    fn sdk_key(&self) -> &str;
    fn name(&self) -> &str;
    fn v_bit_rate(&self) -> i32;
    fn v_codec(&self) -> &str;
    fn fps(&self) -> i32;
    fn resolution(&self) -> &str;
}

impl QualitySpec for DouyinQuality<'_> {
    fn sdk_key(&self) -> &str {
        self.sdk_key
    }
    fn name(&self) -> &str {
        self.name
    }
    fn v_bit_rate(&self) -> i32 {
        self.v_bit_rate
    }
    fn v_codec(&self) -> &str {
        self.v_codec
    }
    fn fps(&self) -> i32 {
        self.fps
    }
    fn resolution(&self) -> &str {
        self.resolution
    }
}

impl QualitySpec for DouyinQualityOwned {
    fn sdk_key(&self) -> &str {
        &self.sdk_key
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn v_bit_rate(&self) -> i32 {
        self.v_bit_rate
    }
    fn v_codec(&self) -> &str {
        &self.v_codec
    }
    fn fps(&self) -> i32 {
        self.fps
    }
    fn resolution(&self) -> &str {
        &self.resolution
    }
}

/// Orchestrates extraction of all available streams for `stream_url`.
pub(super) fn extract_streams(
    config: &Douyin,
    stream_url: &DouyinStreamUrl,
) -> Result<Vec<StreamInfo>, ExtractorError> {
    if config.double_screen
        && !stream_url.pull_datas.data.is_empty()
        && let Some(pull_data) = stream_url.pull_datas.data.values().next()
    {
        debug!("Using double screen data");
        return Ok(extract_from_qualities(
            config.force_origin_quality,
            &pull_data.stream_data,
            &pull_data.options.qualities,
            stream_url,
        ));
    }

    let sdk_pull_data = &stream_url.live_core_sdk_data.pull_data;
    Ok(extract_from_qualities(
        config.force_origin_quality,
        &sdk_pull_data.stream_data,
        &sdk_pull_data.options.qualities,
        stream_url,
    ))
}

fn extract_from_qualities<Q: QualitySpec>(
    force_origin_quality: bool,
    stream_data: &DouyinStreamDataParsed,
    qualities: &[Q],
    stream_url: &DouyinStreamUrl,
) -> Vec<StreamInfo> {
    let mut streams = Vec::new();

    let mut origin_quality_filled = false;
    if force_origin_quality
        && let Some(origin_stream) = extract_valid_origin_stream(stream_data, qualities)
    {
        streams.push(origin_stream);
        origin_quality_filled = true;
    }

    if !stream_data.data.is_empty() {
        streams.extend(extract_sdk_streams(
            stream_data,
            qualities,
            origin_quality_filled,
        ));
    }

    if streams.is_empty() {
        streams.extend(extract_legacy_streams(stream_url));
    }

    streams
}

fn is_or4_stream_url(url: &str) -> bool {
    url.contains("_or4.")
}

fn safe_bitrate_from_i32(v_bit_rate: i32) -> u32 {
    u32::try_from(v_bit_rate)
        .map(normalize_bitrate)
        .unwrap_or(0)
}

fn extract_valid_origin_stream<Q: QualitySpec>(
    stream_data: &DouyinStreamDataParsed,
    qualities: &[Q],
) -> Option<StreamInfo> {
    let origin_details = qualities.iter().find(|q| q.sdk_key() == "origin")?;
    let origin_data = stream_data.data.get(origin_details.sdk_key())?;
    if origin_data.main.flv.is_empty() || !is_or4_stream_url(&origin_data.main.flv) {
        return None;
    }

    let ao_data = stream_data.data.get("ao")?;
    if ao_data.main.flv.is_empty() {
        return None;
    }
    let origin_url = ao_data.main.flv.replace("&only_audio=1", "");

    let bitrate = safe_bitrate_from_i32(origin_details.v_bit_rate());
    let codec = normalize_codec(origin_details.v_codec());
    let fps = origin_details.fps();
    let extras = Some(serde_json::json!({
        "resolution": origin_details.resolution(),
        "sdk_key": origin_details.sdk_key(),
    }));

    Some(
        StreamInfo::builder(origin_url, StreamFormat::Flv, MediaFormat::Flv)
            .quality(origin_details.name().to_string())
            .bitrate(bitrate as u64)
            .priority(0)
            .extras_opt(extras)
            .codec(codec)
            .fps(fps as f64)
            .is_headers_needed(true)
            .build(),
    )
}

fn extract_sdk_streams<Q: QualitySpec>(
    stream_data: &DouyinStreamDataParsed,
    qualities: &[Q],
    origin_quality_filled: bool,
) -> Vec<StreamInfo> {
    let mut streams = Vec::new();
    let quality_map: FxHashMap<&str, &Q> = qualities.iter().map(|q| (q.sdk_key(), q)).collect();

    for (sdk_key, quality_data) in &stream_data.data {
        let quality_details = quality_map.get(sdk_key.as_str()).copied();
        let (quality_name, bitrate, fps, codec) = match quality_details {
            Some(details) => (
                details.name(),
                safe_bitrate_from_i32(details.v_bit_rate()),
                details.fps(),
                normalize_codec(details.v_codec()),
            ),
            None => (sdk_key.as_str(), 0, 0, String::new()),
        };

        let extras = quality_details.map(|details| {
            serde_json::json!({
                "resolution": details.resolution(),
                "sdk_key": details.sdk_key(),
            })
        });

        if !(origin_quality_filled && sdk_key == "origin") {
            add_stream_if_url_present(
                &quality_data.main.flv,
                StreamFormat::Flv,
                MediaFormat::Flv,
                quality_name,
                bitrate as u64,
                &codec,
                fps,
                extras.as_ref(),
                sdk_key,
                &mut streams,
            );
        }

        add_stream_if_url_present(
            &quality_data.main.hls,
            StreamFormat::Hls,
            MediaFormat::Ts,
            quality_name,
            bitrate as u64,
            &codec,
            fps,
            extras.as_ref(),
            sdk_key,
            &mut streams,
        );
    }
    streams
}

#[expect(
    clippy::too_many_arguments,
    reason = "arguments map directly to one extracted stream representation"
)]
pub(super) fn add_stream_if_url_present(
    url: &str,
    format: StreamFormat,
    media_format: MediaFormat,
    quality_name: &str,
    bitrate: u64,
    codec: &str,
    fps: i32,
    extras: Option<&serde_json::Value>,
    sdk_key: &str,
    streams: &mut Vec<StreamInfo>,
) {
    if !url.is_empty() {
        let priority = if sdk_key == "origin" { 0 } else { 10 };
        let is_audio_only = sdk_key == "ao";

        streams.push(
            StreamInfo::builder(url.to_string(), format, media_format)
                .quality(quality_name.to_string())
                .bitrate(bitrate)
                .priority(priority)
                .extras_opt(extras.cloned())
                .codec(codec.to_string())
                .fps(fps as f64)
                .is_headers_needed(true)
                .is_audio_only(is_audio_only)
                .build(),
        );
    }
}

fn extract_legacy_streams(stream_url: &DouyinStreamUrl) -> Vec<StreamInfo> {
    let mut streams =
        Vec::with_capacity(stream_url.flv_pull_url.len() + stream_url.hls_pull_url_map.len());

    for (quality, url) in &stream_url.flv_pull_url {
        streams.push(
            StreamInfo::builder(url.to_string(), StreamFormat::Flv, MediaFormat::Flv)
                .quality(quality.to_string())
                .is_headers_needed(true)
                .build(),
        );
    }

    for (quality, url) in &stream_url.hls_pull_url_map {
        streams.push(
            StreamInfo::builder(url.to_string(), StreamFormat::Hls, MediaFormat::Ts)
                .quality(quality.to_string())
                .is_headers_needed(true)
                .build(),
        );
    }
    streams
}
