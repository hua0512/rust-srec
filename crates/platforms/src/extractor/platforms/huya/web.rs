use std::sync::LazyLock;

use crate::extractor::error::ExtractorError;
use crate::extractor::platforms::huya::Huya;
use crate::extractor::platforms::huya::models::{RoomData, WebProfileInfo, WebStreamResponse};
use crate::media::media_info::MediaInfo;
use regex::Regex;
use rustc_hash::FxHashMap;

pub(super) static ROOM_DATA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"var TT_ROOM_DATA = (.*?);"#).unwrap());

pub(super) static PROFILE_INFO_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"var TT_PROFILE_INFO = (.*?);"#).unwrap());
pub(super) static STREAM_DATA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"stream: (\{.+)\n.*?};"#).unwrap());

impl Huya {
    pub(super) async fn parse_web_media_info(
        &self,
        page_content: &str,
    ) -> Result<MediaInfo, ExtractorError> {
        // Extract profile info first (needed for presenter_uid)
        let profile_info_str = PROFILE_INFO_REGEX
            .captures(page_content)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| {
                ExtractorError::ValidationError("Could not find profile info".to_string())
            })?;

        let profile_info: WebProfileInfo =
            serde_json::from_str(profile_info_str).map_err(ExtractorError::JsonError)?;

        if profile_info.lp <= 0 {
            return Err(ExtractorError::StreamerNotFound);
        }

        let presenter_uid = profile_info.lp;
        let live_status = self.parse_live_status(page_content)?;
        let artist = profile_info.nick;

        let avatar_url = if profile_info.avatar.is_empty() {
            None
        } else {
            Some(profile_info.avatar.into_owned())
        };

        if !live_status {
            return Ok(MediaInfo::new(
                self.extractor.url.clone(),
                "直播未开始".to_string(),
                artist.to_string(),
                None,
                avatar_url,
                false,
                vec![],
                None,
                None,
            ));
        }

        let stream_data_str = STREAM_DATA_REGEX
            .captures(page_content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| {
                ExtractorError::ValidationError("Could not find stream object".to_string())
            })?;

        let stream_response: WebStreamResponse =
            serde_json::from_str(stream_data_str).map_err(ExtractorError::JsonError)?;

        let stream_container = stream_response.data.first().ok_or_else(|| {
            ExtractorError::ValidationError("No stream data container found".to_string())
        })?;

        let game_live_info = &stream_container.game_live_info;

        let title = &game_live_info.introduction;
        let cover_url = if game_live_info.screenshot.is_empty() {
            None
        } else {
            Some(game_live_info.screenshot.to_string())
        };

        let stream_info_list = &stream_container.game_stream_info_list;
        let default_bitrate = game_live_info.bit_rate as u64;
        let bitrate_info_list = &stream_response.v_multi_stream_info;

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
            title.to_string(),
            artist.to_string(),
            cover_url,
            avatar_url,
            true,
            streams,
            Some(self.extractor.get_platform_headers_map()),
            Some(extras),
        ))
    }

    pub(super) fn parse_live_status(&self, response_text: &str) -> Result<bool, ExtractorError> {
        if response_text.contains("找不到这个主播") {
            return Err(ExtractorError::StreamerNotFound);
        }

        if response_text.contains("该主播涉嫌违规，正在整改中") {
            return Err(ExtractorError::StreamerBanned);
        }

        if response_text.is_empty() {
            return Err(ExtractorError::ValidationError(
                "Failed to extract room data".to_string(),
            ));
        }

        let room_data_str = ROOM_DATA_REGEX
            .captures(response_text)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str())
            .ok_or(ExtractorError::ValidationError(
                "Failed to extract room data".to_string(),
            ))?;

        let room_data: RoomData =
            serde_json::from_str(room_data_str).map_err(ExtractorError::JsonError)?;

        if room_data.introduction.contains("【回放】") {
            return Ok(false);
        }

        if room_data.state != "ON" {
            return Ok(false);
        }

        Ok(true)
    }
}
