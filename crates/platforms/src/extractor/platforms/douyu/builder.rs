use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use boa_engine::{self, property::PropertyKey};
use regex::Regex;
use reqwest::Client;
use rustc_hash::FxHashMap;
use tokio::{join, task};
use tracing::debug;
use uuid::Uuid;

use crate::{
    extractor::{
        error::ExtractorError,
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::douyu::models::{DouyuH5PlayData, DouyuH5PlayResponse, DouyuRoomInfoResponse},
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

pub static URL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:https?://)?(?:www\.)?douyu\.com/(\d+)").unwrap());

const RID_REGEX_STR: &str = r#"\$ROOM\.room_id\s*=\s*(\d+)"#;
const ROOM_STATUS_REGEX_STR: &str = r#"\$ROOM\.show_status\s*=\s*(\d+)"#;
const VIDEO_LOOP_REGEX_STR: &str = r#"videoLoop":\s*(\d+)"#;
static RID_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(RID_REGEX_STR).unwrap());
static ROOM_STATUS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(ROOM_STATUS_REGEX_STR).unwrap());
static VIDEO_LOOP_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(VIDEO_LOOP_REGEX_STR).unwrap());

const ENCODED_SCRIPT_REGEX_STR: &str = r#"(var vdwdae325w_64we =[\s\S]+?)\s*</script>"#;
static ENCODED_SCRIPT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(ENCODED_SCRIPT_REGEX_STR).unwrap());
static SIGN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"v=(\d+)&did=(\w+)&tt=(\d+)&sign=(\w+)").unwrap());

struct DouyuTokenResult {
    v: String,
    did: String,
    tt: String,
    sign: String,
}

impl DouyuTokenResult {
    pub fn new(v: &str, did: &str, tt: &str, sign: &str) -> Self {
        Self {
            v: v.to_string(),
            did: did.to_string(),
            tt: tt.to_string(),
            sign: sign.to_string(),
        }
    }
}

pub struct DouyuExtractorConfig {
    pub extractor: Extractor,
    pub cdn: String,
}

impl DouyuExtractorConfig {
    pub(crate) fn extract_rid(&self, response: &str) -> Result<u64, ExtractorError> {
        let captures = RID_REGEX.captures(response);
        if let Some(captures) = captures {
            return Ok(captures.get(1).unwrap().as_str().parse::<u64>().unwrap());
        }
        Err(ExtractorError::ValidationError(
            "Failed to extract rid".to_string(),
        ))
    }

    pub(crate) fn extract_room_status(&self, response: &str) -> Result<u64, ExtractorError> {
        let captures = ROOM_STATUS_REGEX.captures(response);
        if let Some(captures) = captures {
            return Ok(captures.get(1).unwrap().as_str().parse::<u64>().unwrap());
        }
        Err(ExtractorError::ValidationError(
            "Failed to extract room status".to_string(),
        ))
    }

    pub(crate) fn extract_video_loop(&self, response: &str) -> Result<u32, ExtractorError> {
        let captures = VIDEO_LOOP_REGEX.captures(response);
        if let Some(captures) = captures {
            return Ok(captures.get(1).unwrap().as_str().parse::<u32>().unwrap());
        }
        Err(ExtractorError::ValidationError(
            "Failed to extract video loop".to_string(),
        ))
    }

    pub(crate) async fn get_web_response(&self) -> Result<String, ExtractorError> {
        let response = self
            .extractor
            .client
            .get(&self.extractor.url)
            .send()
            .await?;
        let body = response.text().await.map_err(ExtractorError::from)?;
        Ok(body)
    }

    pub(crate) async fn get_room_info(&self, rid: u64) -> Result<String, ExtractorError> {
        let response = self
            .extractor
            .client
            .get(format!("https://open.douyucdn.cn/api/RoomApi/room/{rid}"))
            .send()
            .await?;
        let body = response.text().await.map_err(ExtractorError::from)?;
        Ok(body)
    }

    pub(crate) fn parse_room_info(
        &self,
        response: &str,
    ) -> Result<DouyuRoomInfoResponse, ExtractorError> {
        let room_info: DouyuRoomInfoResponse = serde_json::from_str(response)?;
        if room_info.error != 0 {
            return Err(ExtractorError::ValidationError(
                "Failed to parse room info".to_string(),
            ));
        }
        Ok(room_info)
    }

    fn create_media_info(
        &self,
        title: &str,
        artist: &str,
        cover_url: Option<String>,
        avatar_url: Option<String>,
        is_live: bool,
        streams: Vec<StreamInfo>,
    ) -> MediaInfo {
        MediaInfo::new(
            self.extractor.url.clone(),
            title.to_string(),
            artist.to_string(),
            cover_url,
            avatar_url,
            is_live,
            streams,
            Some(self.extractor.get_platform_headers_map()),
        )
    }

    pub(crate) async fn parse_web_response(
        &self,
        response: Arc<str>,
    ) -> Result<MediaInfo, ExtractorError> {
        if response.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Empty response".to_string(),
            ));
        }
        if response.contains("该房间目前没有开放") || response.contains("房间不存在")
        {
            return Err(ExtractorError::StreamerNotFound);
        }

        if response.contains("房间已被关闭") {
            return Ok(self.create_media_info("Douyu", "", None, None, false, vec![]));
        }

        let rid = self.extract_rid(&response)?;
        let room_status = self.extract_room_status(&response)?;
        let video_loop = self.extract_video_loop(&response)?;

        let is_live = room_status == 1 && video_loop == 0;

        if !is_live {
            return Ok(self.create_media_info("Douyu", "", None, None, false, vec![]));
        }

        // streamer is live
        let response_for_thread = Arc::clone(&response);
        let js_token_handle =
            task::spawn_blocking(move || Self::get_js_token(&response_for_thread, rid));

        let room_info_fut = self.get_room_info(rid);

        let (room_info_res, js_token_res) = join!(room_info_fut, js_token_handle);

        let room_info = room_info_res?;
        let js_token = js_token_res.unwrap()?;

        let room_info = self.parse_room_info(&room_info)?;

        let title = room_info.data.room_name.to_string();
        let artist = room_info.data.owner_name.to_string();
        let cover_url = Some(room_info.data.room_thumb.clone());
        let avatar_url = Some(room_info.data.avatar.to_string());

        let streams = self.get_live_stream_info(&js_token, rid).await?;

        Ok(self.create_media_info(&title, &artist, cover_url, avatar_url, true, streams))
    }

    const JS_DOM: &str = "
        encripted = {decryptedCodes: []};
        if (!this.document) {document = {}}
    ";

    const JS_DEBUG: &str = "
        var encripted_fun = ub98484234;
        ub98484234 = function(p1, p2, p3) {
            try {
                encripted.sign = encripted_fun(p1, p2, p3);
            } catch(e) {
                encripted.sign = e.message;
            }
            return encripted;
        }
    ";

    fn get_js_token(response: &str, rid: u64) -> Result<DouyuTokenResult, ExtractorError> {
        let encoded_script = ENCODED_SCRIPT_REGEX
            .captures(response)
            .and_then(|c| c.get(1))
            .map_or("", |m| m.as_str());

        let did = Uuid::new_v4().to_string().replace("-", "");
        // epoch seconds
        let tt = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();
        // debug!("tt: {}", tt);

        let md5_encoded_script = include_str!("../../../resources/crypto-js-md5.min.js");

        let final_js = format!(
            "{}\n{}\n{}\n{}\nub98484234('{}', '{}', '{}')",
            md5_encoded_script,
            Self::JS_DOM,
            encoded_script,
            Self::JS_DEBUG,
            rid,
            did,
            tt
        );

        // debug!("final_js: {}", final_js);

        let mut context = boa_engine::Context::default();
        let eval_result = context
            .eval(boa_engine::Source::from_bytes(final_js.as_bytes()))
            .map_err(|e| ExtractorError::JsError(e.to_string()))?;
        let res = eval_result.as_object();

        if let Some(res) = res {
            // something like "v=220120250706&did=10000000000000000000000000003306&tt=1751804526&sign=5b1ce0e5888977265b4b378d1b3dcd98"
            let sign = res
                .get(PropertyKey::String("sign".into()), &mut context)
                .map_err(|e| ExtractorError::JsError(e.to_string()))?
                .as_string()
                .map_or("".to_string(), |m| m.to_std_string().unwrap());
            debug!("sign: {}", sign);

            let sign_captures = SIGN_REGEX.captures(&sign);
            if let Some(captures) = sign_captures {
                let v = captures.get(1).unwrap().as_str();
                let did = captures.get(2).unwrap().as_str();
                let tt = captures.get(3).unwrap().as_str();
                let sign = captures.get(4).unwrap().as_str();

                Ok(DouyuTokenResult::new(v, did, tt, sign))
            } else {
                Err(ExtractorError::JsError(
                    "Failed to get js token".to_string(),
                ))
            }
        } else {
            Err(ExtractorError::JsError(
                "Failed to get js token".to_string(),
            ))
        }
    }

    async fn call_get_h5_play(
        &self,
        token_result: &DouyuTokenResult,
        rid: u64,
        cdn: &str,
        rate: i64,
    ) -> Result<DouyuH5PlayData, ExtractorError> {
        let mut form_data: FxHashMap<&str, String> = FxHashMap::default();
        form_data.insert("v", token_result.v.to_string());
        form_data.insert("did", token_result.did.to_string());
        form_data.insert("tt", token_result.tt.to_string());
        form_data.insert("sign", token_result.sign.to_string());
        form_data.insert("cdn", cdn.to_string());
        form_data.insert("rate", rate.to_string());
        form_data.insert("iar", "0".to_string());
        form_data.insert("ive", "0".to_string());

        let resp = self
            .extractor
            .client
            .post(format!("https://www.douyu.com/lapi/live/getH5Play/{rid}"))
            .form(&form_data)
            .send()
            .await?
            .text()
            .await?;

        let resp: DouyuH5PlayResponse = serde_json::from_str(&resp)?;

        if resp.error != 0 {
            return Err(ExtractorError::ValidationError(format!(
                "Failed to get live stream info: {}",
                resp.msg
            )));
        }

        resp.data.ok_or_else(|| {
            ExtractorError::ValidationError("Failed to get live stream info: no data".to_string())
        })
    }

    async fn get_live_stream_info(
        &self,
        token_result: &DouyuTokenResult,
        rid: u64,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let mut stream_infos = vec![];

        let resp = self
            .call_get_h5_play(token_result, rid, &self.cdn, 0)
            .await?;

        let data = resp;
        for cdn in &data.cdns {
            debug!("cdn: {:?}", cdn);
            for rate in &data.multirates {
                debug!("rate: {:?}", rate);
                // compute only the first rate (original quality)
                let stream_url = if cdn.cdn == self.cdn && rate.rate == 0 {
                    format!("{}/{}", data.rtmp_url, data.rtmp_live)
                } else {
                    "".to_string()
                };

                let format = if data.rtmp_live.contains("flv") {
                    StreamFormat::Flv
                } else {
                    StreamFormat::Hls
                };
                let media_format = if data.rtmp_live.contains("flv") {
                    MediaFormat::Flv
                } else {
                    MediaFormat::Ts
                };

                let codec = if cdn.is_h265 { "hevc,aac" } else { "avc,aac" };

                let stream = StreamInfo {
                    url: stream_url,
                    stream_format: format,
                    media_format,
                    quality: rate.name.to_string(),
                    bitrate: rate.bit,
                    priority: 0,
                    extras: Some(serde_json::json!({
                        "cdn": cdn.cdn,
                        "rate": rate.rate,
                        "rid": rid,
                        "sign" : token_result.sign,
                        "v" : token_result.v,
                        "did" : token_result.did,
                        "tt" : token_result.tt,
                    })),
                    codec: codec.to_string(),
                    fps: 0.0,
                    is_headers_needed: false,
                };
                stream_infos.push(stream);
            }
        }
        Ok(stream_infos)
    }
}

pub struct DouyuExtractorBuilder {
    url: String,
    client: Client,
    cookies: Option<String>,
    _extras: Option<serde_json::Value>,
}

impl DouyuExtractorBuilder {
    const BASE_URL: &str = "https://www.douyu.com/";

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        Self {
            url,
            client,
            cookies,
            _extras: extras,
        }
    }

    pub fn build(self, cdn: Option<String>) -> DouyuExtractorConfig {
        let mut extractor = Extractor::new("Douyu".to_string(), self.url, self.client);

        extractor.add_header(
            reqwest::header::ORIGIN.to_string(),
            Self::BASE_URL.to_string(),
        );

        extractor.add_header(
            reqwest::header::REFERER.to_string(),
            Self::BASE_URL.to_string(),
        );

        if let Some(cookies) = self.cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        DouyuExtractorConfig {
            extractor,
            cdn: cdn.unwrap_or("tct-h5".to_string()),
        }
    }
}

#[async_trait]
impl PlatformExtractor for DouyuExtractorConfig {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let response = self.get_web_response().await?;
        let response_arc: Arc<str> = response.into();
        let media_info = self.parse_web_response(response_arc).await?;
        Ok(media_info)
    }

    async fn get_url(&self, mut stream_info: StreamInfo) -> Result<StreamInfo, ExtractorError> {
        if !stream_info.url.is_empty() {
            return Ok(stream_info);
        }

        let extras = stream_info.extras.as_ref().ok_or_else(|| {
            ExtractorError::ValidationError("Missing extras in stream info".to_string())
        })?;

        let rid = extras["rid"]
            .as_u64()
            .ok_or_else(|| ExtractorError::ValidationError("Missing rid in extras".to_string()))?;

        let tt = extras["tt"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing tt in extras".to_string()))?;
        let v = extras["v"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing v in extras".to_string()))?;
        let did = extras["did"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing did in extras".to_string()))?;
        let sign = extras["sign"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing sign in extras".to_string()))?;

        let token_result = DouyuTokenResult::new(v, did, tt, sign);

        let cdn = extras["cdn"]
            .as_str()
            .ok_or_else(|| ExtractorError::ValidationError("Missing cdn in extras".to_string()))?;

        let rate = extras["rate"]
            .as_i64()
            .ok_or_else(|| ExtractorError::ValidationError("Missing rate in extras".to_string()))?;

        let resp = self.call_get_h5_play(&token_result, rid, cdn, rate).await?;

        stream_info.url = format!("{}/{}", resp.rtmp_url, resp.rtmp_live);

        Ok(stream_info)
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use crate::extractor::{
        default::default_client, platform_extractor::PlatformExtractor,
        platforms::douyu::DouyuExtractorBuilder,
    };

    #[tokio::test]
    #[ignore]
    async fn test_douyu_extractor() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .try_init()
            .unwrap();

        let url = "https://www.douyu.com/8440385";

        let extractor =
            DouyuExtractorBuilder::new(url.to_string(), default_client(), None, None).build(None);
        let media_info = extractor.extract().await.unwrap();
        println!("{media_info:?}");
    }
}
