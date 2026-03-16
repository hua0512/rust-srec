use crate::extractor::error::ExtractorError;
use crate::media::media_info::MediaInfo;
use crate::media::stream_info::StreamInfo;
use rustc_hash::FxHashMap;

use super::builder::Huya;
use super::models::*;

impl Huya {
    pub(super) const MP_URL: &'static str = "https://mp.huya.com/cache.php";

    pub(super) async fn get_mp_page(&self, room_id: i64) -> Result<String, ExtractorError> {
        let url = format!(
            "{}?do=profileRoom&m=Live&roomid={}&showSecret=1",
            Self::MP_URL,
            room_id
        );
        let response = self.extractor.get(&url).send().await?;
        let response = Self::check_http_response(response).await?;
        let content = response.text().await?;
        Ok(content)
    }

    pub(super) fn parse_mp_live_status(
        &self,
        response: &MpApiResponse,
    ) -> Result<bool, ExtractorError> {
        if response.status != 200 {
            if response.status == 422
                && (response.message.contains("主播不存在")
                    || response.message.contains("该主播不存在"))
            {
                return Err(ExtractorError::StreamerNotFound);
            }
            return Err(ExtractorError::ValidationError(format!(
                "Failed to get live status: status {}, message: {}",
                response.status, response.message
            )));
        }

        let data = match &response.data {
            Some(data) => data,
            None => return Err(ExtractorError::StreamerNotFound),
        };

        if let Some(live_data) = &data.live_data
            && live_data.introduction.starts_with("【回放】")
        {
            return Ok(false);
        }

        let is_live = data.real_live_status == Some("ON") && data.live_status == Some("ON");

        Ok(is_live)
    }

    pub(crate) fn parse_mp_media_info<'a>(
        &self,
        response_text: &'a str,
    ) -> Result<MediaInfo, ExtractorError> {
        let response: MpApiResponse<'a> =
            serde_json::from_str(response_text).map_err(ExtractorError::JsonError)?;

        if response.status != 200 {
            if response.status == 422
                && (response.message.contains("主播不存在")
                    || response.message.contains("该主播不存在"))
            {
                return Err(ExtractorError::StreamerNotFound);
            }
            return Err(ExtractorError::ValidationError(format!(
                "API error: status {}, message: {}",
                response.status, response.message
            )));
        }

        let data = match &response.data {
            Some(data) => data,
            None => return Err(ExtractorError::StreamerNotFound),
        };

        let profile_info = match &data.profile_info {
            Some(info) => info,
            None => {
                return Err(ExtractorError::ValidationError(
                    "No profile info found".to_string(),
                ));
            }
        };

        let presenter_uid = profile_info.uid;
        let avatar_url = Some(profile_info.avatar180.to_string());
        let artist = profile_info.nick.to_string();

        let live_data = match &data.live_data {
            Some(data) => data,
            None => {
                return Ok(MediaInfo::new(
                    self.extractor.url.clone(),
                    String::new(),
                    artist,
                    None,
                    avatar_url,
                    false,
                    vec![],
                    None,
                    None,
                ));
            }
        };

        let title = live_data.introduction.to_string();
        let cover_url = Some(live_data.screenshot.to_string());

        let is_live = self.parse_mp_live_status(&response)?;

        if !is_live {
            return Ok(MediaInfo::new(
                self.extractor.url.clone(),
                title,
                artist,
                cover_url,
                avatar_url,
                false,
                vec![],
                None,
                None,
            ));
        }

        let stream_data = match &data.stream {
            Some(data) => data,
            None => {
                return Err(ExtractorError::ValidationError(
                    "No stream data found".into(),
                ));
            }
        };

        let stream_info_list = &stream_data.base_steam_info_list;
        let bitrate_info_list = &stream_data.bit_rate_info;
        let default_bitrate = live_data.bit_rate as u64;

        let streams = self.parse_streams(
            stream_info_list,
            bitrate_info_list,
            default_bitrate,
            presenter_uid,
        )?;

        let mut extras = FxHashMap::default();
        extras.insert("presenter_uid".to_string(), presenter_uid.to_string());

        Ok(MediaInfo::new(
            self.extractor.url.clone(),
            title,
            artist,
            cover_url,
            avatar_url,
            is_live,
            streams,
            Some(self.extractor.get_platform_headers_map()),
            Some(extras),
        ))
    }

    pub(super) fn parse_streams(
        &self,
        stream_info_list: &[StreamInfoItem],
        bitrate_info_list: &[BitrateInfo],
        default_bitrate: u64,
        presenter_uid: i64,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let mut streams = Vec::new();

        for stream_info in stream_info_list.iter() {
            if stream_info.s_stream_name.is_empty() {
                continue;
            }

            let stream_name = self.force_origin_quality(stream_info.s_stream_name);

            // Build URLs with anti-code
            let flv_url = format!(
                "{}/{}.{}?{}",
                stream_info.s_flv_url,
                stream_name,
                stream_info.s_flv_url_suffix,
                stream_info.s_flv_anti_code
            );
            let hls_url = format!(
                "{}/{}.{}?{}",
                stream_info.s_hls_url,
                stream_name,
                stream_info.s_hls_url_suffix,
                stream_info.s_hls_anti_code
            );

            let extras = serde_json::json!({
                "cdn": stream_info.s_cdn_type,
                "stream_name": stream_name,
                "presenter_uid": presenter_uid.to_string(),
                "default_bitrate": default_bitrate.to_string(),
            });

            // skip if priority is 0
            if stream_info.i_web_priority_rate <= 0 {
                continue;
            }

            let priority = stream_info.i_web_priority_rate as u32;

            // Add streams for each bitrate
            if bitrate_info_list.is_empty() {
                streams.extend(Self::create_stream_info(
                    &flv_url,
                    &hls_url,
                    "原画",
                    default_bitrate,
                    priority,
                    false,
                    &extras,
                ));
            } else {
                for bitrate_info in bitrate_info_list.iter() {
                    if bitrate_info.s_display_name.contains("HDR") {
                        continue;
                    }
                    let add_ratio = bitrate_info.i_bit_rate != 0;
                    streams.extend(Self::create_stream_info(
                        &flv_url,
                        &hls_url,
                        &bitrate_info.s_display_name,
                        if add_ratio {
                            bitrate_info.i_bit_rate.into()
                        } else {
                            default_bitrate
                        },
                        priority,
                        add_ratio,
                        &extras,
                    ));
                }
            }
        }

        Ok(streams)
    }
}

#[cfg(test)]
mod tests {
    use crate::extractor::default::default_client;
    use crate::extractor::error::ExtractorError;
    use crate::extractor::platforms::huya::Huya;

    fn read_test_file(file_name: &str) -> String {
        let mut d = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("src/extractor/tests/test_data/huya/");
        d.push(file_name);
        std::fs::read_to_string(d).unwrap()
    }

    #[test]
    #[ignore]
    fn test_parse_mp_live_status() {
        let extractor = Huya::new(
            "https://www.huya.com/".to_string(),
            default_client(),
            None,
            None,
        );

        let response_str = read_test_file("mp_api_response.json");
        let mut response: super::super::models::MpApiResponse =
            serde_json::from_str(&response_str).unwrap();

        // Test case 1: Live on
        response.data.as_mut().unwrap().real_live_status = Some("ON");
        response.data.as_mut().unwrap().live_status = Some("ON");
        assert!(extractor.parse_mp_live_status(&response).unwrap());

        // Test case 2: Live off
        response.data.as_mut().unwrap().real_live_status = Some("OFF");
        assert!(!extractor.parse_mp_live_status(&response).unwrap());

        // Test case 3: Replay
        response.data.as_mut().unwrap().real_live_status = Some("ON");
        response.data.as_mut().unwrap().live_status = Some("ON");
        response
            .data
            .as_mut()
            .unwrap()
            .live_data
            .as_mut()
            .unwrap()
            .introduction = "【回放】".to_string().into();
        assert!(!extractor.parse_mp_live_status(&response).unwrap());

        // Test case 4: Streamer not found
        response.status = 422;
        response.message = "主播不存在";
        let result = extractor.parse_mp_live_status(&response);
        assert!(matches!(result, Err(ExtractorError::StreamerNotFound)));
    }
}
