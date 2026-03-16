use crate::extractor::error::ExtractorError;
use crate::extractor::platforms::huya::{
    GetCdnTokenExRsp, GetLivingInfoRsp, HuyaUserId, get_anticode,
};
use crate::media::MediaInfo;
use crate::media::formats::StreamFormat;
use crate::media::stream_info::StreamInfo;
use rustc_hash::FxHashMap;
use tracing::debug;
use url::Url;

use super::builder::Huya;
use super::tars;

impl Huya {
    pub(super) const WUP_URL: &'static str = "https://wup.huya.com";

    // WUP User-Agent for Huya
    pub(super) fn get_wup_ua() -> String {
        let ua = crate::extractor::platforms::huya::sign::HuyaPlatform::HuyaPcExe.generate_ua();
        format!("HYSDK(Windows,30000002)_APP({})_SDK(trans&2.34.0.5795)", ua)
    }

    // /// Get living info via WUP using presenter UID
    // pub(super) async fn get_living_info_wup(
    //     &self,
    //     lp: i64,
    // ) -> Result<GetLivingInfoRsp, ExtractorError> {
    // use crate::extractor::platforms::huya::tars::decode_get_cdn_token_info_response;
    //
    //     let device = "chrome";
    //     let request_body = tars::build_get_living_info_request(lp, Self::PC_EXE_VER, device)
    //         .map_err(|e| {
    //             ExtractorError::ValidationError(format!(
    //                 "Failed to build getLivingInfo request: {:?}",
    //                 e
    //             ))
    //         })?;

    //     let response = self
    //         .extractor
    //         .post(Self::WUP_URL)
    //         .body(request_body)
    //         .send()
    //         .await?;
    //     let response = Self::check_http_response(response).await?;
    //     let response_bytes = response.bytes().await?;

    //     let living_info = tars::decode_get_living_info_response(response_bytes).map_err(|e| {
    //         ExtractorError::ValidationError(format!(
    //             "Failed to decode getLivingInfo response: {:?}",
    //             e
    //         ))
    //     })?;

    //     Ok(living_info)
    // }

    /// Get living info via WUP using room ID (numeric ID from URL)
    /// This avoids the need to parse the web page for presenter UID
    pub(super) async fn get_living_info_by_room_id_wup(
        &self,
        room_id: i64,
    ) -> Result<(GetLivingInfoRsp, String), ExtractorError> {
        let device = "";
        let ua = crate::extractor::platforms::huya::sign::HuyaPlatform::get_random().generate_ua();
        let request_body = tars::build_get_living_info_by_room_id_request(room_id, &ua, device)
            .map_err(|e| {
                ExtractorError::ValidationError(format!(
                    "Failed to build getLivingInfo request: {:?}",
                    e
                ))
            })?;

        let response = self
            .extractor
            .post(Self::WUP_URL)
            .body(request_body)
            .send()
            .await?;
        let response = Self::check_http_response(response).await?;
        let response_bytes = response.bytes().await?;

        let living_info = tars::decode_get_living_info_response(response_bytes).map_err(|e| {
            ExtractorError::ValidationError(format!(
                "Failed to decode getLivingInfo response: {:?}",
                e
            ))
        })?;

        Ok((living_info, ua))
    }

    /// Get CDN token info via WUP
    async fn get_cdn_token_info(
        &self,
        stream_name: &str,
        flv_url: &str,
        ua: &str,
    ) -> Result<GetCdnTokenExRsp, ExtractorError> {
        //  flv_url : http://al.flv.huya.com/src
        //  stream_name : 1199643886212-1199643886212-5789661561920421888-2399287895880-10057-A-0-1
        let user_id = HuyaUserId::new(0, String::new(), String::new(), ua.to_string());

        let request_body = tars::build_get_cdn_token_info_request(stream_name, flv_url, user_id)
            .map_err(|e| {
                ExtractorError::ValidationError(format!(
                    "Failed to build getCdnTokenInfo request: {:?}",
                    e
                ))
            })?;

        let response = self
            .extractor
            .post(Self::WUP_URL)
            .body(request_body)
            .send()
            .await?;
        let response = Self::check_http_response(response).await?;
        let response_bytes = response.bytes().await?;

        let cdn_token_info =
            tars::decode_get_cdn_token_info_response(response_bytes).map_err(|e| {
                ExtractorError::ValidationError(format!(
                    "Failed to decode getCdnTokenInfo response: {:?}",
                    e
                ))
            })?;

        Ok(cdn_token_info)
    }

    pub(super) async fn get_anticode_url(
        &self,
        stream_info: &mut StreamInfo,
        presenter_uid: i64,
        stream_name: &str,
        flv_url: &str,
        ua: &str,
    ) -> Result<(), ExtractorError> {
        let token_info = self.get_cdn_token_info(stream_name, flv_url, ua).await?;
        debug!("token_info: {:#?}", token_info);

        // wsSecret=787fbd35756215078b3343e1a1a2ca0b&wsTime=69b3281f&fm=RFdxOEJjSjNoNkRKdDZUWV8kMF8kMV8kMl8kMw%3D%3D&ctype=huya_webh5&fs=gctex
        if token_info.flv_token.is_empty() {
            return Err(ExtractorError::ValidationError(format!(
                "Failed to get flv_token from WUP response: {:?}",
                token_info
            )));
        }

        // parse and get anticode
        let anti_code = get_anticode(
            stream_name,
            &token_info.flv_token,
            Some(presenter_uid as u64),
            false,
        )?;

        // Parse URL components
        let url = Url::parse(&stream_info.url)
            .map_err(|e| ExtractorError::ValidationError(format!("Invalid URL: {}", e)))?;
        let host = url.host_str().unwrap_or("");
        let path = url.path().split('/').nth(1).unwrap_or("");
        let base_url = format!("{}://{}/{}", url.scheme(), host, path);

        // Determine file suffix
        let suffix = match stream_info.stream_format {
            StreamFormat::Flv => "flv",
            StreamFormat::Hls => "m3u8",
            _ => {
                return Err(ExtractorError::ValidationError(format!(
                    "Invalid stream format: {:?}",
                    stream_info.stream_format
                )));
            }
        };

        let bitrate = stream_info.bitrate;
        let default_bitrate = stream_info
            .extras
            .as_ref()
            .map(Self::extract_default_bitrate_from_extras)
            .unwrap_or(10000);

        // Build final URL
        let base_url = format!("{base_url}/{stream_name}.{suffix}?{anti_code}");
        stream_info.url = if bitrate != default_bitrate {
            format!("{base_url}&ratio={bitrate}")
        } else {
            base_url
        };

        debug!("Final URL: {}", stream_info.url);

        Ok(())
    }

    /// Parse media info from GetLivingInfoRsp
    /// Uses b_is_living to determine live status and extracts info from BeginLiveNotice
    pub(super) fn parse_living_info(
        &self,
        living_info: &GetLivingInfoRsp,
        presenter_uid: i64,
        ua: &str,
    ) -> Result<MediaInfo, ExtractorError> {
        // Check live status from GetLivingInfoRsp.b_is_living
        let is_live = living_info.b_is_living != 0;

        // Extract artist and avatar from BeginLiveNotice
        let notice = &living_info.t_notice;
        let artist = if notice.s_nick.is_empty() {
            "Unknown".to_string()
        } else {
            notice.s_nick.clone()
        };

        let avatar_url = if notice.s_avatar_url.is_empty() {
            None
        } else {
            Some(notice.s_avatar_url.clone())
        };

        // Extract title from BeginLiveNotice.s_live_desc
        let title = if notice.s_live_desc.is_empty() {
            "直播中".to_string()
        } else {
            notice.s_live_desc.clone()
        };

        // Extract cover from video capture URL if available
        let cover_url = if notice.s_video_capture_url.is_empty() {
            None
        } else {
            Some(notice.s_video_capture_url.clone())
        };

        if !is_live {
            return Ok(MediaInfo::new(
                self.extractor.url.clone(),
                "直播未开始".to_string(),
                artist,
                None,
                avatar_url,
                false,
                vec![],
                None,
                None,
            ));
        }

        // Parse streams from living_info
        let streams = self.parse_living_info_streams(living_info, ua)?;

        let mut extras = FxHashMap::default();
        extras.insert("presenter_uid".to_string(), presenter_uid.to_string());
        extras.insert("ua".to_string(), ua.to_string());

        Ok(MediaInfo::new(
            self.extractor.url.clone(),
            title,
            artist,
            cover_url,
            avatar_url,
            true,
            streams,
            Some(self.extractor.get_platform_headers_map()),
            Some(extras),
        ))
    }

    pub(super) fn parse_living_info_streams(
        &self,
        living_info: &GetLivingInfoRsp,
        ua: &str,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let mut streams = Vec::new();

        let bitrate_info_list = &living_info.t_notice.v_multi_stream_info;
        let default_bitrate = living_info.t_stream_setting_notice.i_bit_rate;
        let default_bitrate_u64 = u64::try_from(default_bitrate).unwrap_or(0);

        for stream_info in living_info.t_notice.v_stream_info.iter() {
            if stream_info.s_stream_name.is_empty() {
                continue;
            }

            let stream_name = self.force_origin_quality(&stream_info.s_stream_name);
            let presenter_uid = stream_info.l_presenter_uid;

            // Build queries with proper authentication
            let flv_query =
                self.build_stream_query(&stream_name, &stream_info.s_flv_anti_code, presenter_uid);
            let hls_query =
                self.build_stream_query(&stream_name, &stream_info.s_hls_anti_code, presenter_uid);

            // Build base URLs
            let flv_url = format!(
                "{}/{}.{}?{}",
                stream_info.s_flv_url, stream_name, stream_info.s_flv_url_suffix, flv_query
            );
            let hls_url = format!(
                "{}/{}.{}?{}",
                stream_info.s_hls_url, stream_name, stream_info.s_hls_url_suffix, hls_query
            );

            let extras = serde_json::json!({
                "cdn": stream_info.s_cdn_type,
                "stream_name": stream_name,
                "flv_url": stream_info.s_flv_url,
                "presenter_uid": presenter_uid.to_string(),
                "default_bitrate": default_bitrate.to_string(),
                "ua": ua.to_string(),
            });

            let priority = stream_info.i_web_priority_rate as u32;

            // Add streams for each bitrate
            if bitrate_info_list.is_empty() {
                streams.extend(Self::create_stream_info(
                    &flv_url,
                    &hls_url,
                    "原画",
                    default_bitrate_u64,
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
                            u64::try_from(bitrate_info.i_bit_rate).unwrap_or(default_bitrate_u64)
                        } else {
                            default_bitrate_u64
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
