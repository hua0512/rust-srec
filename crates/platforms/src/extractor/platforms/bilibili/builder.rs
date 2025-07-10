use std::{fmt::Display, sync::LazyLock};

use async_trait::async_trait;
use num_enum::TryFromPrimitive;
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
    },
    media::{MediaInfo, StreamFormat, StreamInfo, formats::MediaFormat},
};
use rustc_hash::FxHashMap;

pub static URL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?:\/\/(?:www\.)?(?:live\.)?bilibili\.com\/(\d+)").unwrap());

#[derive(Debug, PartialEq, Eq, Copy, Clone, TryFromPrimitive)]
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

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        _extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("Bilibili", url, client);

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        extractor.add_header(reqwest::header::REFERER.to_string(), Self::BASE_URL);
        let mut quality = BilibiliQuality::Original;

        if let Some(extras) = _extras {
            let quality_value = extras.get("quality").unwrap_or(&serde_json::Value::Null);
            if quality_value.is_number() {
                let quality_num = quality_value.as_u64().unwrap_or(10000);
                quality = BilibiliQuality::try_from_primitive(quality_num as u32)
                    .unwrap_or(BilibiliQuality::Original);
            }
        }

        Self { extractor, quality }
    }

    pub fn extract_room_id(&self) -> Result<String, ExtractorError> {
        let caps = URL_REGEX.captures(&self.extractor.url);
        caps.and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or(ExtractorError::InvalidUrl(self.extractor.url.clone()))
    }

    async fn get_bilibili_api<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        params: Vec<(&str, String)>,
    ) -> Result<T, ExtractorError> {
        let (img_key, sub_key) = get_wbi_keys(&self.extractor.client)
            .await
            .map_err(ExtractorError::from)?;

        let params = encode_wbi(params, (img_key.as_str(), sub_key.as_str()));
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

        let quality_map = playurl_info
            .playurl
            .g_qn_desc
            .iter()
            .map(|q| (q.qn, q.desc.clone()))
            .collect::<FxHashMap<_, _>>();

        let mut streams = Vec::new();
        for s in &playurl_info.playurl.stream {
            debug!("protocol_name: {:?}", s.protocol_name);
            let protocol_name = if s.protocol_name == "http_stream" {
                StreamFormat::Flv
            } else {
                StreamFormat::Hls
            };

            for f in &s.format {
                let format_name = f.format_name.clone();
                debug!("format_name: {:?}", format_name);
                for c in &f.codec {
                    let current_qn = c.current_qn;
                    for u in &c.url_info {
                        for &qn in &c.accept_qn {
                            let url = if qn == current_qn {
                                format!("{}{}{}", u.host, c.base_url, u.extra)
                            } else {
                                String::new()
                            };

                            let cdn = u
                                .host
                                .split("//")
                                .nth(1)
                                .unwrap_or("")
                                .split('.')
                                .next()
                                .unwrap_or("");

                            let quality_desc = quality_map
                                .get(&qn)
                                .cloned()
                                .unwrap_or_else(|| "Unknown".to_string());
                            let quality = format!("{} - {} - {}", quality_desc, c.codec_name, cdn);

                            let bitrate = if qn < 1000 { qn as u64 * 10 } else { qn as u64 };
                            streams.push(StreamInfo {
                                url,
                                stream_format: protocol_name,
                                media_format: MediaFormat::from_extension(&format_name),
                                quality,
                                bitrate,
                                priority: 0,
                                extras: Some(serde_json::json!({
                                    "qn": qn,
                                    "rid": room_id,
                                })),
                                codec: c.codec_name.clone(),
                                fps: 0.0,
                                is_headers_needed: false,
                            });
                        }
                    }
                }
            }
        }
        Ok(streams)
    }

    pub async fn get_live_info(&self, room_id: String) -> Result<MediaInfo, ExtractorError> {
        let (room_info, anchor_info) = self.fetch_room_info(&room_id).await?;

        let is_live = room_info.live_status == 1;
        let title = room_info.title.clone();
        let artist = anchor_info.base_info.uname.clone();
        let cover_url = Some(room_info.cover.clone());
        let artist_url = Some(anchor_info.base_info.face.clone());

        let streams = if is_live {
            self.process_streams(room_info.room_id, self.quality)
                .await?
        } else {
            Vec::new()
        };

        Ok(MediaInfo::new(
            self.extractor.url.clone(),
            title,
            artist,
            cover_url,
            artist_url,
            is_live,
            streams,
            None,
        ))
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

    async fn get_url(&self, stream_info: StreamInfo) -> Result<StreamInfo, ExtractorError> {
        let extras = stream_info.extras.as_ref().ok_or_else(|| {
            ExtractorError::ValidationError("Stream extras not found".to_string())
        })?;

        let qn = extras["qn"]
            .as_u64()
            .ok_or_else(|| ExtractorError::ValidationError("QN not found in extras".to_string()))?;

        let rid = extras["rid"].as_u64().ok_or_else(|| {
            ExtractorError::ValidationError("Room ID not found in extras".to_string())
        })?;

        let current_qn = extras["qn"].as_u64().ok_or_else(|| {
            ExtractorError::ValidationError("Current QN not found in extras".to_string())
        })?;

        // skip extraction if the requested quality is the same as the current quality
        if qn == current_qn && !stream_info.url.is_empty() {
            // return the original stream info
            return Ok(stream_info);
        }

        let params = vec![
            ("room_id", rid.to_string()),
            ("qn", qn.to_string()),
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

        let playurl_info = json.data.playurl_info;
        let stream = playurl_info
            .playurl
            .stream
            .first()
            .ok_or_else(|| ExtractorError::ValidationError("No stream found".to_string()))?;

        let codec = stream
            .format
            .first()
            .and_then(|f| f.codec.first())
            .ok_or_else(|| ExtractorError::ValidationError("No codec found".to_string()))?;

        let current_qn: u64 = codec.current_qn.try_into().unwrap();
        if current_qn != qn {
            return Err(ExtractorError::ValidationError(
                "Failed to get the stream for the requested quality.".to_string(),
            ));
        }

        for url_info in &codec.url_info {
            let url = format!("{}{}{}", url_info.host, codec.base_url, url_info.extra);
            // check if url is valid
            if reqwest::Url::parse(&url).is_ok() {
                let mut new_stream_info = stream_info.clone();
                new_stream_info.url = url;
                return Ok(new_stream_info);
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
            "https://live.bilibili.com/10".to_string(),
            default_client(),
            None,
            None,
        );
        let media_info = bilibili.extract().await.unwrap();
        println!("{media_info:?}");
    }
}
