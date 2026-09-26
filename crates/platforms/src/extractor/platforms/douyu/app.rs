use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::{RequestBuilder, header};
use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::{
    extractor::{error::ExtractorError, platform_configs::DouyuCodec, utils::extras_get_u64},
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

use super::{
    app_sign::{self, Params},
    builder::Douyu,
};

#[derive(Debug, Deserialize)]
struct AppResponse {
    error: i32,
    // Parse data only after checking the error: offline responses use "".
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AppPlayData {
    #[serde(default)]
    rtmp_cdn: String,
    #[serde(default)]
    rtmp_url: String,
    #[serde(default)]
    rtmp_live: String,
    #[serde(default)]
    player_1: Option<String>,
    #[serde(default)]
    rate: Option<u64>,
    #[serde(default, rename = "cdnsWithName")]
    cdns: Vec<AppCdn>,
    #[serde(default, rename = "rateSetting", alias = "multirates")]
    rates: Vec<AppRate>,
}

#[derive(Debug, Deserialize)]
struct AppCdn {
    cdn: String,
}

#[derive(Debug, Deserialize)]
struct AppRate {
    name: String,
    rate: u64,
    #[serde(default)]
    bit: u64,
}

impl AppResponse {
    fn into_data(self) -> Result<AppPlayData, ExtractorError> {
        match self.error {
            0 => Ok(serde_json::from_value(self.data)?),
            -5..=-3 => Err(ExtractorError::NoStreamsFound),
            126 => Err(ExtractorError::RegionLockedContent),
            -9 => Err(ExtractorError::ValidationError(
                "Douyu app clock skew (error -9)".into(),
            )),
            code => Err(ExtractorError::ValidationError(format!(
                "Douyu app playback error {code}"
            ))),
        }
    }
}

fn app_cdn(cdn: &str) -> &str {
    cdn.strip_suffix("-h5").unwrap_or(cdn)
}

impl Douyu {
    async fn app_request(
        &self,
        rid: u64,
        cdn: &str,
        rate: u64,
        timestamp: u64,
    ) -> Result<RequestBuilder, ExtractorError> {
        let device = self
            .app_device
            .as_ref()
            .map_err(|error| ExtractorError::ValidationError(error.clone()))?;
        let identity = device.identity(&self.extractor.client).await?;
        let mut params: Params = [
            ("txdw", "0"),
            ("cdn", app_cdn(cdn)),
            ("token", ""),
            (
                "hevc",
                if self.codec == DouyuCodec::Hevc {
                    "1"
                } else {
                    "0"
                },
            ),
            ("ilow", "0"),
            ("iar", "0"),
            ("net", "WIFI"),
            ("device", identity.query_device.as_str()),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
        params.insert("rate".into(), rate.to_string());
        let auth = app_sign::sign(rid, &identity.did, timestamp, &mut params);
        // Do not forward browser login cookies to the app CDN host. Only the
        // device cookie participates in this anonymous Android protocol.
        Ok(self
            .extractor
            .client
            .get(format!(
                "https://playclient.douyucdn.cn/lapi/live/appGetPlayer/stream/{rid}"
            ))
            .query(&params)
            .header("User-Device", identity.user_device.clone())
            .header("aid", "android1")
            .header("channel", "447")
            .header(header::USER_AGENT, identity.user_agent.clone())
            .header("time", timestamp.to_string())
            .header("auth", auth)
            .header(header::COOKIE, identity.cookie.clone()))
    }

    async fn get_app_play_info(
        &self,
        rid: u64,
        cdn: &str,
        rate: u64,
    ) -> Result<AppPlayData, ExtractorError> {
        let mut server_time = None;
        for attempt in 0..self.request_retries {
            let timestamp = server_time.unwrap_or(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| {
                        ExtractorError::ValidationError(format!("Invalid system time: {e}"))
                    })?
                    .as_secs(),
            );
            let result = async {
                let response = self
                    .app_request(rid, cdn, rate, timestamp)
                    .await?
                    .send()
                    .await?
                    .error_for_status()?;
                let date = response
                    .headers()
                    .get(header::DATE)
                    .and_then(|date| date.to_str().ok())
                    .and_then(|date| chrono::DateTime::parse_from_rfc2822(date).ok())
                    .and_then(|date| u64::try_from(date.timestamp()).ok());
                let response: AppResponse = response.json().await?;
                if response.error == -9 {
                    server_time = date;
                }
                response.into_data()
            }
            .await;
            match result {
                Ok(data) => return Ok(data),
                Err(e @ (ExtractorError::NoStreamsFound | ExtractorError::RegionLockedContent)) => {
                    return Err(e);
                }
                Err(e) if attempt + 1 == self.request_retries => return Err(e),
                Err(_) => debug!(
                    rid,
                    attempt = attempt + 1,
                    "Retrying Douyu app playback request"
                ),
            }
        }
        Err(ExtractorError::ValidationError(
            "Douyu app request retries must be positive".into(),
        ))
    }

    pub(super) async fn extract_app(&self) -> Result<MediaInfo, ExtractorError> {
        let rid = self.get_real_room_id(&self.extractor.url).await?;
        let room = self.get_betard_room_info(rid).await?.room;
        let mut is_live = room.show_status == 1 && room.video_loop == 0;
        if is_live && self.disable_interactive_game {
            match self.has_interactive_game(room.room_id).await {
                Ok(true) => is_live = false,
                Ok(false) => {}
                Err(e) => debug!(rid, error = %e, "Could not check Douyu interactive game status"),
            }
        }
        let streams = if is_live {
            match self
                .get_app_play_info(room.room_id, &self.cdn, self.rate as u64)
                .await
            {
                Ok(data) => self.app_streams(room.room_id, &data)?,
                Err(ExtractorError::NoStreamsFound) => {
                    is_live = false;
                    vec![]
                }
                Err(e) => return Err(e),
            }
        } else {
            vec![]
        };
        Ok(self.create_media_info(
            &room.room_name,
            &room.owner_name,
            (!room.room_thumb.is_empty()).then_some(room.room_thumb),
            room.avatar.get_best(),
            is_live,
            streams,
            Some(room.room_id),
        ))
    }

    fn app_streams(&self, rid: u64, data: &AppPlayData) -> Result<Vec<StreamInfo>, ExtractorError> {
        let preferred_cdn = app_cdn(&self.cdn);
        let mut cdns: Vec<&str> = data
            .cdns
            .iter()
            .map(|cdn| cdn.cdn.as_str())
            .filter(|cdn| !cdn.is_empty())
            .collect();
        if !cdns.contains(&preferred_cdn) {
            cdns.insert(0, preferred_cdn);
        }
        let fallback_rate = AppRate {
            name: format!("Rate {}", self.rate),
            rate: self.rate as u64,
            bit: 0,
        };
        let mut rates: Vec<&AppRate> = data.rates.iter().collect();
        if !rates.iter().any(|rate| rate.rate == self.rate as u64) {
            rates.insert(0, &fallback_rate);
        }
        let mut streams = Vec::new();
        for cdn in cdns {
            for rate in &rates {
                // Resolve only the selected candidate; dispatch can downgrade
                // rate/CDN/codec, so resolve_app_stream replaces its metadata.
                streams.push(StreamInfo::builder("", StreamFormat::Flv, MediaFormat::Flv)
                    .quality(&rate.name).bitrate(rate.bit)
                    .priority(if cdn == preferred_cdn && rate.rate == self.rate as u64 { 0 } else { 10 })
                    .codec(if self.codec == DouyuCodec::Hevc { "hevc,aac" } else { "avc,aac" })
                    .extras(json!({"api_mode":"app", "rid":rid.to_string(), "cdn":cdn, "rate":rate.rate.to_string()}))
                    .is_headers_needed(true).build());
            }
        }
        // This response already resolves the requested CDN/rate. Reuse it so
        // the normal extraction path needs only one signed playback request.
        // Other candidates remain deferred and are fetched only if selected.
        if let Some(preferred) = streams.iter_mut().find(|stream| stream.priority == 0) {
            self.apply_app_play_info(preferred, data)?;
        }
        Ok(streams)
    }

    pub(super) async fn resolve_app_stream(
        &self,
        stream: &mut StreamInfo,
    ) -> Result<(), ExtractorError> {
        let extras = stream.extras.as_ref();
        let rid = extras_get_u64(extras, "rid")
            .ok_or_else(|| ExtractorError::ValidationError("Missing Douyu room ID".into()))?;
        let rate = extras_get_u64(extras, "rate")
            .ok_or_else(|| ExtractorError::ValidationError("Missing Douyu rate".into()))?;
        let cdn = extras
            .and_then(|e| e["cdn"].as_str())
            .ok_or_else(|| ExtractorError::ValidationError("Missing Douyu CDN".into()))?;
        let data = self.get_app_play_info(rid, cdn, rate).await?;
        self.apply_app_play_info(stream, &data)
    }

    fn apply_app_play_info(
        &self,
        stream: &mut StreamInfo,
        data: &AppPlayData,
    ) -> Result<(), ExtractorError> {
        let hevc_url = data
            .player_1
            .as_deref()
            .filter(|url| !url.trim().is_empty());
        let is_hevc = self.codec == DouyuCodec::Hevc && hevc_url.is_some();
        let stream_url = if is_hevc {
            hevc_url.unwrap_or_default().to_owned()
        } else {
            if data.rtmp_url.is_empty() || data.rtmp_live.is_empty() {
                return Err(ExtractorError::ValidationError(
                    "Douyu app response has no playable URL".into(),
                ));
            }
            format!(
                "{}/{}",
                data.rtmp_url.trim_end_matches('/'),
                data.rtmp_live.trim_start_matches('/')
            )
        };
        let url = url::Url::parse(&stream_url)
            .map_err(|_| ExtractorError::ValidationError("Invalid Douyu app stream URL".into()))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(ExtractorError::ValidationError(
                "Unsupported Douyu app stream URL".into(),
            ));
        }
        // Preserve signed query parameters, including the server's expiry.
        stream.url = stream_url;
        stream.codec = if is_hevc { "hevc,aac" } else { "avc,aac" }.into();
        stream.is_audio_only = false;
        if url.path().ends_with(".m3u8") {
            stream.stream_format = StreamFormat::Hls;
            stream.media_format = MediaFormat::Ts;
        } else {
            stream.stream_format = StreamFormat::Flv;
            stream.media_format = MediaFormat::Flv;
        }
        if let Some(rate) = data.rate {
            if let Some(quality) = data.rates.iter().find(|quality| quality.rate == rate) {
                stream.quality.clone_from(&quality.name);
                stream.bitrate = quality.bit;
            } else {
                stream.quality = format!("Rate {rate}");
                stream.bitrate = 0;
            }
        }
        if let Some(extras) = stream
            .extras
            .as_mut()
            .and_then(|extras| extras.as_object_mut())
        {
            if !data.rtmp_cdn.is_empty() {
                extras.insert("cdn".into(), json!(data.rtmp_cdn));
            }
            if let Some(rate) = data.rate {
                extras.insert("rate".into(), json!(rate.to_string()));
            }
            extras.insert("is_h265".into(), json!(is_hevc));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::platform_extractor::PlatformExtractor;
    use base64::{Engine, engine::general_purpose::STANDARD};

    fn extractor(options: serde_json::Value) -> Douyu {
        Douyu::new(
            "https://www.douyu.com/100".into(),
            crate::extractor::default::default_client(),
            None,
            Some(options),
        )
    }

    fn data(extra: serde_json::Value) -> AppPlayData {
        let mut data = json!({
            "rtmp_url":"https://example.com/live/", "rtmp_live":"/avc.flv?token=abc&expire=0",
            "rtmp_cdn":"ws", "rate":3,
            "rateSetting":[{"name":"HD","rate":3,"bit":2000}],
            "cdnsWithName":[{"cdn":"ws"}]
        });
        data.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(data).unwrap()
    }

    #[tokio::test]
    async fn signed_request_uses_preferences_and_consistent_device() {
        let douyu = Douyu::new(
            "https://www.douyu.com/100".into(),
            crate::extractor::default::default_client(),
            Some("acf_did=0123456789abcdef0123456789abcdef; secret=private".into()),
            Some(json!({"codec":"hevc", "device_name":"OnePlus 12", "os_version":"15"})),
        );
        let request = douyu
            .app_request(100, "tct-h5", 2, 1790312470)
            .await
            .unwrap()
            .build()
            .unwrap();
        let mut params: Params = request
            .url()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(params["cdn"], "tct");
        assert_eq!(params["rate"], "2");
        assert_eq!(params["hevc"], "1");
        assert_eq!(params["device"], "OnePlus-12");
        assert_eq!(
            request.headers()[header::USER_AGENT],
            "android/8.2.2.0 (android 15; ; OnePlus+12)"
        );
        assert_eq!(request.headers()["time"], "1790312470");
        assert_eq!(
            request.headers()["Cookie"],
            "acf_did=0123456789abcdef0123456789abcdef"
        );
        assert_eq!(
            STANDARD
                .decode(request.headers()["User-Device"].as_bytes())
                .unwrap(),
            b"0123456789abcdef0123456789abcdef|v8.2.2.0"
        );
        assert_eq!(
            request.headers()["auth"],
            app_sign::sign(
                100,
                "0123456789abcdef0123456789abcdef",
                1790312470,
                &mut params
            )
        );
    }

    #[test]
    fn selects_hevc_and_falls_back_with_correct_metadata() {
        let douyu = extractor(json!({"codec":"hevc", "cdn":"hw-h5", "rate":0}));
        let data = data(json!({"player_1":"https://example.com/hevc.flv?token=x&expire=0"}));
        let mut stream = douyu.app_streams(100, &data).unwrap().remove(0);
        assert_eq!(stream.priority, 0);
        assert!(!stream.url.is_empty());
        assert_eq!(stream.codec, "hevc,aac");
        assert_eq!(stream.url, data.player_1.clone().unwrap());
        assert_eq!(stream.quality, "HD");
        assert_eq!(stream.extras.as_ref().unwrap()["cdn"], "ws");
        assert_eq!(stream.extras.as_ref().unwrap()["rate"], "3");

        let data = AppPlayData {
            player_1: Some(String::new()),
            ..data
        };
        douyu.apply_app_play_info(&mut stream, &data).unwrap();
        assert_eq!(stream.codec, "avc,aac");
        assert_eq!(
            stream.url,
            "https://example.com/live/avc.flv?token=abc&expire=0"
        );
        assert_eq!(stream.extras.as_ref().unwrap()["is_h265"], false);
    }

    #[test]
    fn avc_preference_ignores_hevc_and_rejects_empty_urls() {
        let douyu = extractor(json!({}));
        let mut data = data(json!({"player_1":"https://example.com/hevc.flv"}));
        let mut stream = douyu.app_streams(100, &data).unwrap().remove(0);
        assert_eq!(stream.codec, "avc,aac");
        data.rtmp_live.clear();
        assert!(douyu.apply_app_play_info(&mut stream, &data).is_err());
    }

    #[test]
    fn playback_errors_allow_empty_data() {
        for code in [-5, -4, -3, -9, 126, 2005, 2006] {
            let response: AppResponse =
                serde_json::from_value(json!({"error":code,"data":""})).unwrap();
            let error = response.into_data().unwrap_err();
            match code {
                -5..=-3 => assert!(matches!(error, ExtractorError::NoStreamsFound)),
                126 => assert!(matches!(error, ExtractorError::RegionLockedContent)),
                _ => assert!(matches!(error, ExtractorError::ValidationError(_))),
            }
        }
    }

    #[tokio::test]
    #[ignore = "live Douyu integration; requires room 100 to be broadcasting"]
    async fn live_app_playback() {
        tokio::time::timeout(std::time::Duration::from_secs(45), async {
            for (codec, device_id_mode) in [("avc", "local"), ("hevc", "server")] {
                let client = crate::extractor::default::create_client_builder(None)
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                    .unwrap();
                let douyu = Douyu::new(
                    "https://www.douyu.com/100".into(),
                    client,
                    None,
                    Some(json!({"codec":codec, "device_id_mode":device_id_mode})),
                );
                let info = douyu.extract().await.unwrap();
                assert!(info.is_live);
                let mut stream = info
                    .streams
                    .into_iter()
                    .min_by_key(|stream| stream.priority)
                    .unwrap();
                douyu.get_url(&mut stream).await.unwrap();
                assert!(!stream.url.is_empty());
                assert!(matches!(stream.codec.as_str(), "avc,aac" | "hevc,aac"));
                let mut response = douyu
                    .extractor
                    .get(&stream.url)
                    .send()
                    .await
                    .unwrap()
                    .error_for_status()
                    .unwrap();
                let mut magic = Vec::new();
                while magic.len() < 4 {
                    let chunk = response.chunk().await.unwrap().unwrap();
                    magic.extend_from_slice(&chunk[..chunk.len().min(4 - magic.len())]);
                }
                assert_eq!(&magic[..3], b"FLV");
            }
        })
        .await
        .unwrap();
    }
}
