use std::{fmt::Display, sync::LazyLock};

use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use tracing::debug;

use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::bilibili::{
            models::{RoomInfo, RoomInfoAnchorInfo, RoomInfoDetails, RoomPlayInfo},
            wbi::{encode_wbi, get_wbi_keys},
        },
        utils::capture_group_1_or_invalid_url,
        utils::extras_get_u64,
    },
    media::{MediaInfo, StreamFormat, StreamInfo, formats::MediaFormat},
};
use rustc_hash::FxHashMap;

pub static URL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?:\/\/(?:www\.)?(?:live\.)?bilibili\.com\/(\d+)").unwrap());

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
#[repr(u32)]
pub enum BilibiliQuality {
    // 最低画质
    Lowest = 0,
    // 流畅
    Low = 80,
    // 高清
    Medium = 150,
    // 超清
    Ultra = 250,
    // 蓝光
    Blue = 400,
    // 蓝光-杜比
    BlueDolby = 401,
    // 原画
    Original = 10000,
    // 4K
    FourK = 20000,
    // 杜比视界
    DolbyVision = 30000,
}

impl TryFrom<u32> for BilibiliQuality {
    type Error = ExtractorError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(BilibiliQuality::Lowest),
            80 => Ok(BilibiliQuality::Low),
            150 => Ok(BilibiliQuality::Medium),
            250 => Ok(BilibiliQuality::Ultra),
            400 => Ok(BilibiliQuality::Blue),
            401 => Ok(BilibiliQuality::BlueDolby),
            10000 => Ok(BilibiliQuality::Original),
            20000 => Ok(BilibiliQuality::FourK),
            30000 => Ok(BilibiliQuality::DolbyVision),
            _ => Err(ExtractorError::ValidationError(format!(
                "Invalid quality: {}",
                value
            ))),
        }
    }
}

impl Display for BilibiliQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u32)
    }
}

pub struct Bilibili {
    pub extractor: Extractor,
    pub quality: BilibiliQuality,
}

impl Bilibili {
    pub(in crate::extractor::platforms::bilibili) const BASE_URL: &str = "https://www.bilibili.com";

    const ROOM_INFO_URL: &str =
        "https://api.live.bilibili.com/xlive/web-room/v1/index/getInfoByRoom";

    const ROOM_PLAY_INFO_URL: &str =
        "https://api.live.bilibili.com/xlive/web-room/v2/index/getRoomPlayInfo";

    const WBI_WEB_LOCATION: &str = "444.8";

    fn extract_cdn_from_host(host: &str) -> &str {
        let host = host.split_once("//").map_or(host, |(_, rest)| rest);
        host.split_once('.').map_or("", |(cdn, _)| cdn)
    }

    fn concat_url(host: &str, base_url: &str, extra: &str) -> String {
        let mut url = String::with_capacity(host.len() + base_url.len() + extra.len());
        url.push_str(host);
        url.push_str(base_url);
        url.push_str(extra);
        url
    }

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("Bilibili", url, client);

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }
        extractor.set_referer_static(Self::BASE_URL);

        let quality = extras_get_u64(extras.as_ref(), "quality")
            .and_then(|num| BilibiliQuality::try_from(num as u32).ok())
            .unwrap_or(BilibiliQuality::DolbyVision);

        Self { extractor, quality }
    }

    pub fn extract_room_id(&self) -> Result<&str, ExtractorError> {
        capture_group_1_or_invalid_url(&URL_REGEX, &self.extractor.url)
    }

    async fn get_bilibili_api<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        params: Vec<(&str, String)>,
    ) -> Result<T, ExtractorError> {
        let keys = get_wbi_keys(&self.extractor.client).await?;

        let params = encode_wbi(params, keys)?;
        debug!("params: {:?}", params);

        let api_url = format!("{url}?{params}");

        let response = self.extractor.get(&api_url).send().await?;

        let json = response.json::<T>().await?;

        Ok(json)
    }

    async fn fetch_room_info(
        &self,
        room_id: &str,
    ) -> Result<(RoomInfoDetails, RoomInfoAnchorInfo), ExtractorError> {
        let params = vec![
            ("room_id", room_id.to_string()),
            ("web_location", Self::WBI_WEB_LOCATION.to_string()),
        ];

        let json: RoomInfo = self.get_bilibili_api(Self::ROOM_INFO_URL, params).await?;

        debug!("json: {:?}", json);

        if json.code != 0 {
            return Err(ExtractorError::ValidationError(json.message));
        }

        let data = json
            .data
            .ok_or_else(|| ExtractorError::ValidationError("No room data found".to_string()))?;

        let room_info = data
            .room_info
            .ok_or_else(|| ExtractorError::ValidationError("No room info found".to_string()))?;

        let anchor_info = data
            .anchor_info
            .ok_or_else(|| ExtractorError::ValidationError("No anchor info found".to_string()))?;

        Ok((room_info, anchor_info))
    }

    async fn process_streams(
        &self,
        room_id: u64,
        quality: BilibiliQuality,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let params = vec![
            ("room_id", room_id.to_string()),
            ("qn", quality.to_string()),
            ("platform", "html5".to_string()),
            ("protocol", "0,1".to_string()),
            ("format", "0,1,2".to_string()),
            ("codec", "0,1".to_string()),
            ("dolby", "5".to_string()),
            ("web_location", Self::WBI_WEB_LOCATION.to_string()),
        ];

        let json: RoomPlayInfo = self
            .get_bilibili_api(Self::ROOM_PLAY_INFO_URL, params)
            .await?;

        if json.code != 0 {
            return Err(ExtractorError::ValidationError(json.message));
        }

        let data = json.data;
        let playurl_info = data.playurl_info;

        let mut quality_map = FxHashMap::default();
        quality_map.reserve(playurl_info.playurl.g_qn_desc.len());
        for q in &playurl_info.playurl.g_qn_desc {
            quality_map.insert(q.qn, q.desc.as_str());
        }

        let estimated_streams = playurl_info.playurl.stream.iter().fold(0usize, |acc, s| {
            s.format.iter().fold(acc, |acc, f| {
                f.codec.iter().fold(acc, |acc, c| {
                    acc.saturating_add(c.url_info.len().saturating_mul(c.accept_qn.len()))
                })
            })
        });
        let mut streams = Vec::with_capacity(estimated_streams);
        for s in &playurl_info.playurl.stream {
            debug!("protocol_name: {:?}", s.protocol_name);
            let protocol_name = if s.protocol_name == "http_stream" {
                StreamFormat::Flv
            } else {
                StreamFormat::Hls
            };

            for f in &s.format {
                debug!("format_name: {:?}", f.format_name);
                let media_format = MediaFormat::from_extension(&f.format_name);
                for c in &f.codec {
                    let current_qn = c.current_qn;
                    let accept_qn = c
                        .accept_qn
                        .iter()
                        .map(|&qn| {
                            let quality_desc = quality_map.get(&qn).copied().unwrap_or("Unknown");
                            let qn_u64 = qn.max(0) as u64;
                            let bitrate = if qn_u64 < 1000 { qn_u64 * 10 } else { qn_u64 };
                            (qn, quality_desc, bitrate)
                        })
                        .collect::<Vec<_>>();

                    for u in &c.url_info {
                        let cdn = Self::extract_cdn_from_host(&u.host);

                        for &(qn, quality_desc, bitrate) in &accept_qn {
                            let url = if qn == current_qn {
                                Self::concat_url(&u.host, &c.base_url, &u.extra)
                            } else {
                                String::new()
                            };

                            streams.push(
                                StreamInfo::builder(url, protocol_name, media_format)
                                    .quality(quality_desc.to_string())
                                    .bitrate(bitrate)
                                    .extras(serde_json::json!({
                                        "qn": qn,
                                        "rid": room_id,
                                        "cdn": cdn,
                                    }))
                                    .codec(c.codec_name.to_string())
                                    .is_headers_needed(true)
                                    .build(),
                            );
                        }
                    }
                }
            }
        }
        Ok(streams)
    }

    pub async fn get_live_info(&self, room_id: &str) -> Result<MediaInfo, ExtractorError> {
        let (room_info, anchor_info) = self.fetch_room_info(room_id).await?;

        let RoomInfoDetails {
            room_id,
            title,
            cover,
            tags,
            live_status,
            live_start_time,
            ..
        } = room_info;

        let is_live = live_status == 1;
        let artist = anchor_info.base_info.uname;
        let categories = tags
            .split(&[',', '|', '/'][..])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let category = (!categories.is_empty()).then_some(categories);

        let streams = if is_live {
            self.process_streams(room_id, self.quality).await?
        } else {
            Vec::new()
        };

        let headers = Some(self.extractor.get_platform_headers_map());

        let builder = MediaInfo::builder(self.extractor.url.clone(), title, artist)
            .category_opt(category)
            .cover_url(cover)
            .artist_url(anchor_info.base_info.face)
            .is_live(is_live)
            .streams(streams)
            .headers_opt(headers);

        let builder = if live_start_time > 0 {
            builder.live_start_time_unix_seconds(live_start_time as i64)
        } else {
            builder
        };

        Ok(builder.build())
    }
}

#[async_trait]
impl PlatformExtractor for Bilibili {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let room_id = self.extract_room_id()?;
        self.get_live_info(room_id).await
    }

    async fn get_url(&self, stream_info: &mut StreamInfo) -> Result<(), ExtractorError> {
        let extras = stream_info.extras.as_ref().ok_or_else(|| {
            ExtractorError::ValidationError("Stream extras not found".to_string())
        })?;

        let qn = extras["qn"]
            .as_u64()
            .ok_or_else(|| ExtractorError::ValidationError("QN not found in extras".to_string()))?;

        let rid = extras["rid"].as_u64().ok_or_else(|| {
            ExtractorError::ValidationError("Room ID not found in extras".to_string())
        })?;

        let cdn = extras.get("cdn").and_then(|c| c.as_str());

        // skip extraction if url is already present
        if !stream_info.url.is_empty() {
            return Ok(());
        }

        // 协议格式，0: http_stream(flv), 1: http_hls
        let protocol = match stream_info.stream_format {
            StreamFormat::Flv => "0",
            StreamFormat::Hls => "1",
            _ => "0,1",
        };

        // 编码格式，0: flv, 1: ts, 2: fmp4
        let format = match stream_info.media_format {
            MediaFormat::Flv => "0",
            MediaFormat::Ts => "1",
            MediaFormat::Fmp4 => "2",
            _ => "0,1,2",
        };

        let params = vec![
            ("room_id", rid.to_string()),
            ("qn", qn.to_string()),
            ("platform", "html5".to_string()),
            ("protocol", protocol.to_string()),
            ("format", format.to_string()),
            ("codec", "0,1".to_string()),
            ("dolby", "5".to_string()),
            ("web_location", Self::WBI_WEB_LOCATION.to_string()),
        ];

        let json: RoomPlayInfo = self
            .get_bilibili_api(Self::ROOM_PLAY_INFO_URL, params)
            .await?;

        if json.code != 0 {
            return Err(ExtractorError::ValidationError(json.message));
        }

        let playurl_info = json.data.playurl_info;
        let stream = playurl_info
            .playurl
            .stream
            .first()
            .ok_or_else(|| ExtractorError::ValidationError("No stream found".to_string()))?;

        let format = stream
            .format
            .first()
            .ok_or_else(|| ExtractorError::ValidationError("No format found".to_string()))?;

        let codec = format
            .codec
            .iter()
            .find(|c| c.codec_name == stream_info.codec)
            .ok_or_else(|| {
                ExtractorError::ValidationError("No matching codec found".to_string())
            })?;

        let current_qn: u64 = codec
            .current_qn
            .try_into()
            .map_err(|_| ExtractorError::ValidationError("Invalid current_qn".to_string()))?;
        if current_qn != qn {
            return Err(ExtractorError::ValidationError(
                "Failed to get the stream for the requested quality.".to_string(),
            ));
        }

        if let Some(cdn) = cdn {
            if let Some(url_info) = codec
                .url_info
                .iter()
                .find(|&u| Self::extract_cdn_from_host(&u.host) == cdn)
            {
                let url = Self::concat_url(&url_info.host, &codec.base_url, &url_info.extra);
                if reqwest::Url::parse(&url).is_ok() {
                    stream_info.url = url;
                    return Ok(());
                }
            }
            return Err(ExtractorError::ValidationError(format!(
                "Requested CDN '{cdn}' not found."
            )));
        }

        // If no CDN is specified, just pick the first valid URL.
        for url_info in &codec.url_info {
            let url = Self::concat_url(&url_info.host, &codec.base_url, &url_info.extra);
            if reqwest::Url::parse(&url).is_ok() {
                stream_info.url = url;
                return Ok(());
            }
        }

        Err(ExtractorError::ValidationError(
            "No valid stream URL found".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use crate::extractor::{
        default::default_client, platform_extractor::PlatformExtractor,
        platforms::bilibili::Bilibili,
    };

    #[tokio::test]
    #[ignore]
    async fn test_extract() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .init();
        let bilibili = Bilibili::new(
            "https://live.bilibili.com/6".to_string(),
            default_client(),
            None,
            None,
        );
        let media_info = bilibili.extract().await.unwrap();
        println!("{media_info:?}");
    }
}
