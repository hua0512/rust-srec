use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use tracing::debug;

use crate::extractor::error::ExtractorError;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::douyin::apis::LIVE_DOUYIN_URL;
use crate::extractor::platforms::douyin::utils::extract_rid;
use crate::extractor::utils::{extras_get_bool, extras_get_str};
use crate::media::media_info::MediaInfo;

use super::request::DouyinRequest;

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?live.douyin\.com/([a-zA-Z0-9_\-\.]+)").unwrap()
});

/// Manages the ttwid cookie strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtwidManagementMode {
    /// Use a single, globally shared ttwid across all extractors.
    Global,
    /// Use a separate ttwid for each extractor instance.
    PerExtractor,
}

/// Configuration for the Douyin extractor.
///
/// This struct holds immutable configuration settings.
/// Use `DouyinExtractorBuilder` to construct an instance.
pub struct DouyinExtractorConfig {
    /// The base extractor, holding common configuration like URL, client, headers, and params.
    pub extractor: Extractor,
    /// Force the use of origin quality stream if available.
    pub force_origin_quality: bool,
    /// The management mode for the `ttwid` cookie.
    pub ttwid_management_mode: TtwidManagementMode,
    /// A specific `ttwid` to use when in `PerExtractor` mode.
    pub ttwid: Option<String>,
    /// Whether to use double screen stream data if available.
    pub double_screen: bool,
    /// Force the use of mobile API for stream extraction.
    pub force_mobile_api: bool,
    /// Skip interactive game streams (互动玩法), treat as offline.
    pub skip_interactive_games: bool,
}

/// Builder for `DouyinExtractorConfig`.
pub struct Douyin {
    pub(super) extractor: Extractor,
    pub(super) force_origin_quality: bool,
    pub(super) ttwid_management_mode: TtwidManagementMode,
    pub(super) ttwid: Option<String>,
    pub(super) double_screen: bool,
    pub(super) force_mobile_api: bool,
    pub(super) skip_interactive_games: bool,
}

impl Douyin {
    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("Douyin", url, client);

        extractor.set_referer_static(LIVE_DOUYIN_URL);

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        let force_origin_quality =
            extras_get_bool(extras.as_ref(), "force_origin_quality").unwrap_or(false);

        let double_screen = extras_get_bool(extras.as_ref(), "double_screen").unwrap_or(true);

        let ttwid_management_mode_str =
            extras_get_str(extras.as_ref(), "ttwid_management_mode").unwrap_or("global");

        let ttwid_management_mode = match ttwid_management_mode_str {
            "global" => TtwidManagementMode::Global,
            _ => TtwidManagementMode::PerExtractor,
        };

        let ttwid = extras_get_str(extras.as_ref(), "ttwid").map(ToOwned::to_owned);

        let force_mobile_api =
            extras_get_bool(extras.as_ref(), "force_mobile_api").unwrap_or(false);
        let skip_interactive_games =
            extras_get_bool(extras.as_ref(), "skip_interactive_games").unwrap_or(true);

        Self {
            extractor,
            force_origin_quality,
            ttwid_management_mode,
            ttwid,
            double_screen,
            force_mobile_api,
            skip_interactive_games,
        }
    }

    pub fn force_origin_quality(mut self, force: bool) -> Self {
        self.force_origin_quality = force;
        self
    }

    pub fn double_screen(mut self, enable: bool) -> Self {
        self.double_screen = enable;
        self
    }

    pub fn ttwid_mode(mut self, mode: TtwidManagementMode) -> Self {
        self.ttwid_management_mode = mode;
        self
    }

    /// Set a specific ttwid to use.
    /// This automatically switches to `PerExtractor` mode.
    pub fn ttwid(mut self, ttwid: String) -> Self {
        self.ttwid = Some(ttwid);
        self.ttwid_management_mode = TtwidManagementMode::PerExtractor;
        self
    }

    /// Force the use of mobile API for stream extraction.
    pub fn force_mobile_api(mut self, enable: bool) -> Self {
        self.force_mobile_api = enable;
        self
    }

    /// Skip interactive game streams (互动玩法), treating them as offline.
    pub fn skip_interactive_games(mut self, enable: bool) -> Self {
        self.skip_interactive_games = enable;
        self
    }
}

#[async_trait]
impl PlatformExtractor for Douyin {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let web_rid = extract_rid(&self.extractor.url)?;
        debug!("extract web_rid: {}", web_rid);

        let mut request = DouyinRequest::new(self.extractor.cookies.clone(), self, web_rid);
        request.extract().await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::extractor::default::default_client;
    use crate::extractor::platform_extractor::PlatformExtractor;
    use crate::extractor::platforms::douyin::builder::{Douyin, TtwidManagementMode};
    use crate::extractor::platforms::douyin::models::{DouyinAvatarThumb, DouyinUserInfo};
    use crate::extractor::platforms::douyin::request::DouyinRequest;
    use crate::extractor::platforms::douyin::streams::add_stream_if_url_present;
    use crate::extractor::platforms::douyin::utils::GlobalTtwidManager;
    use crate::media::formats::{MediaFormat, StreamFormat};
    use crate::media::media_info::MediaInfo;

    const TEST_URL: &str = "https://live.douyin.com/PenguinVal";

    const LIVE_FLV: &str = "https://example.com/stream_or4.flv?token=test";
    const LIVE_HLS: &str = "https://example.com/stream.m3u8?token=test";
    const REFLOW_FLV: &str = "https://example.com/stream_or4.flv?rtm_expr_tag=reflow_room_info";
    const REFLOW_HLS: &str = "https://example.com/stream.m3u8?rtm_expr_tag=reflow_room_info";

    #[derive(Clone, Copy, Debug)]
    enum StreamSource {
        Legacy,
        Sdk,
        DoubleScreen,
        Origin,
        DoubleScreenOrigin,
    }

    const STREAM_SOURCES: [StreamSource; 5] = [
        StreamSource::Legacy,
        StreamSource::Sdk,
        StreamSource::DoubleScreen,
        StreamSource::Origin,
        StreamSource::DoubleScreenOrigin,
    ];

    fn room_fixture(source: StreamSource, flv: &str, hls: &str) -> Value {
        let quality = json!({
            "name": "Origin", "sdk_key": "origin", "v_codec": "h264",
            "resolution": "1920x1080", "level": 0, "v_bit_rate": 8000000,
            "additional_content": "", "fps": 30, "disable": 0
        });
        let main = json!({
            "flv": flv, "hls": hls, "cmaf": "", "dash": "", "lls": "",
            "tsl": "", "tile": "", "http_ts": "", "ll_hls": "",
            "sdk_params": "", "enableEncryption": false
        });
        let mut stream_data = json!({"data": {"origin": {"main": main}}});
        if matches!(
            source,
            StreamSource::Origin | StreamSource::DoubleScreenOrigin
        ) {
            let mut audio_main = main.clone();
            audio_main["flv"] = json!(format!("{flv}&only_audio=1"));
            audio_main["hls"] = json!("");
            stream_data["data"]["ao"] = json!({"main": audio_main});
        }
        let empty_pull_data = json!({
            "options": {"default_quality": quality, "qualities": [quality]},
            "stream_data": "{}"
        });
        let mut pull_data = empty_pull_data.clone();
        pull_data["stream_data"] = json!(stream_data.to_string());
        let mut room = json!({
            "id_str": "room_123", "status": 2, "title": "Test Room",
            "cover": {"url_list": ["https://example.com/cover.jpg"]},
            "stream_url": {
                "flv_pull_url": {}, "hls_pull_url_map": {}, "hls_pull_url": "",
                "default_resolution": "origin", "stream_orientation": 0,
                "live_core_sdk_data": {"pull_data": empty_pull_data}, "pull_datas": {},
                "extra": {
                    "height": 1080, "width": 1920, "fps": 30, "max_bitrate": 8000000,
                    "min_bitrate": 0, "default_bitrate": 8000000, "bitrate_adapt_strategy": 0,
                    "anchor_interact_profile": 0, "audience_interact_profile": 0,
                    "hardware_encode": false, "video_profile": 0, "h265_enable": false,
                    "gop_sec": 2, "bframe_enable": false, "roi": false,
                    "sw_roi": false, "bytevc1_enable": false
                }
            }
        });
        match source {
            StreamSource::Legacy => {
                room["stream_url"]["flv_pull_url"] = json!({"origin": flv});
                room["stream_url"]["hls_pull_url_map"] = json!({"origin": hls});
            }
            StreamSource::Sdk | StreamSource::Origin => {
                room["stream_url"]["live_core_sdk_data"]["pull_data"] = pull_data;
            }
            StreamSource::DoubleScreen | StreamSource::DoubleScreenOrigin => {
                room["stream_url"]["pull_datas"] = json!({"main": pull_data});
            }
        }
        room
    }

    fn parse_room_fixture(room: Value, mobile: bool, source: StreamSource) -> MediaInfo {
        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None)
            .force_origin_quality(matches!(
                source,
                StreamSource::Origin | StreamSource::DoubleScreenOrigin
            ));
        let mut request =
            DouyinRequest::new(config.extractor.cookies.clone(), &config, "123".to_string());
        let mut response = json!({"data": {"user": {
            "id_str": "123", "sec_uid": "sec_123", "nickname": "Test User",
            "avatar_thumb": {"url_list": ["https://example.com/avatar.jpg"]}
        }}});
        if mobile {
            response["data"]["room"] = room;
            request.parse_app_response(&response.to_string()).unwrap()
        } else {
            response["data"]["data"] = json!([room]);
            request.parse_pc_response(&response.to_string()).unwrap()
        }
    }

    const REFLOW_LIVE_ROOM: &str = include_str!("fixtures/reflow_live_room.json");
    const REFLOW_ENDED_ROOM: &str = include_str!("fixtures/reflow_ended_room.json");

    fn pc_identity_body(user_id: &str, room: Value) -> Value {
        json!({
            "status_code": 0,
            "data": {
                "user": {"id_str": user_id, "sec_uid": "sec_uid", "nickname": "Broadcaster"},
                "enter_room_id": "7654321",
                "data": [room]
            }
        })
    }

    #[test]
    fn platform_identity_survives_live_offline_and_filtered_results() {
        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None);
        for case in ["live", "ended", "prompt", "interactive"] {
            let mut room = room_fixture(StreamSource::Sdk, LIVE_FLV, LIVE_HLS);
            if case == "ended" {
                room["status"] = json!(4);
            }
            if case == "interactive" {
                room["scene_type_info"] = json!({"is_union_live_room": false});
                room["toolbar_data"] = json!({"entrance_list": [{"text": "互动玩法"}]});
            }
            let mut body = pc_identity_body("9007199254740993", room);
            if case == "prompt" {
                body["data"]["prompts"] = json!("直播已结束");
            }
            let mut request = DouyinRequest::new(
                config.extractor.cookies.clone(),
                &config,
                "page-id".to_string(),
            );
            let info = request.parse_pc_response(&body.to_string()).unwrap();
            assert_eq!(info.is_live, case == "live", "{case}");
            let metadata = info.platform_metadata.unwrap();
            assert_eq!(
                metadata.data,
                json!({"web_rid": "page-id", "user_id": "9007199254740993"}),
                "{case}"
            );
        }
    }

    #[test]
    fn forced_mobile_result_keeps_the_pc_broadcaster_identity() {
        let config =
            Douyin::new(TEST_URL.to_string(), default_client(), None, None).force_mobile_api(true);
        let pc = pc_identity_body(
            "9007199254740993",
            room_fixture(StreamSource::Sdk, LIVE_FLV, LIVE_HLS),
        );
        for app_body in [REFLOW_LIVE_ROOM, REFLOW_ENDED_ROOM] {
            let mut request = DouyinRequest::new(
                config.extractor.cookies.clone(),
                &config,
                "page-id".to_string(),
            );
            request
                .extract_user_info_from_pc_response(&pc.to_string())
                .unwrap();
            let info = request.parse_app_response(app_body).unwrap();
            assert_eq!(
                info.platform_metadata.unwrap().data["user_id"],
                "9007199254740993"
            );
        }
    }

    #[test]
    fn unverified_response_cannot_establish_platform_identity() {
        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None);
        for id in ["", "0", "-1", "not-a-uid"] {
            let mut request = DouyinRequest::new(
                config.extractor.cookies.clone(),
                &config,
                "page-id".to_string(),
            );
            let body = pc_identity_body(id, room_fixture(StreamSource::Sdk, LIVE_FLV, LIVE_HLS));
            assert!(
                request
                    .parse_pc_response(&body.to_string())
                    .unwrap()
                    .platform_metadata
                    .is_none()
            );
        }
        let mut body = pc_identity_body("123", room_fixture(StreamSource::Sdk, LIVE_FLV, LIVE_HLS));
        // The platform's error envelope: the prompt still means offline, but the reply
        // cannot verify who the broadcaster is.
        body["status_code"] = json!(30003);
        body["data"]["prompts"] = json!("直播已结束");
        for forced_mobile in [false, true] {
            let mut request = DouyinRequest::new(
                config.extractor.cookies.clone(),
                &config,
                "page-id".to_string(),
            );
            if forced_mobile {
                request
                    .extract_user_info_from_pc_response(&body.to_string())
                    .unwrap();
            } else {
                let info = request.parse_pc_response(&body.to_string()).unwrap();
                assert!(!info.is_live);
            }
            assert!(request.platform_metadata.is_none());
        }
        body["status_code"] = json!(0);
        body["data"]["user"] = Value::Null;
        body["data"]["prompts"] = json!("直播已结束");
        let mut request = DouyinRequest::new(
            config.extractor.cookies.clone(),
            &config,
            "page-id".to_string(),
        );
        assert!(
            request
                .parse_pc_response(&body.to_string())
                .unwrap()
                .platform_metadata
                .is_none()
        );
    }

    fn parse_app_body(body: &str) -> MediaInfo {
        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None);
        let mut request =
            DouyinRequest::new(config.extractor.cookies.clone(), &config, "123".to_string());
        request.parse_app_response(body).unwrap()
    }

    fn has_reflow_tag(url: &str) -> bool {
        url::Url::parse(url).is_ok_and(|url| {
            url.query_pairs()
                .any(|(key, value)| key == "rtm_expr_tag" && value == "reflow_room_info")
        })
    }

    #[test]
    fn test_finished_room_is_offline_despite_live_status_and_streams() {
        for mobile in [false, true] {
            let mut room = room_fixture(StreamSource::Sdk, LIVE_FLV, LIVE_HLS);
            room["finish_time"] = json!(1787737453_i64);
            let info = parse_room_fixture(room, mobile, StreamSource::Sdk);
            assert!(!info.is_live);
            assert!(info.streams.is_empty());
            assert_eq!(info.title, "Test Room");
            assert_eq!(info.artist, "Test User");
            assert_eq!(
                info.cover_url.as_deref(),
                Some("https://example.com/cover.jpg")
            );
        }
    }

    #[test]
    fn test_live_room_with_missing_null_or_zero_finish_time() {
        for mobile in [false, true] {
            for source in STREAM_SOURCES {
                for finish_time in [None, Some(Value::Null), Some(json!(0))] {
                    let mut room = room_fixture(source, LIVE_FLV, LIVE_HLS);
                    if let Some(finish_time) = finish_time {
                        room["finish_time"] = finish_time;
                    }
                    let info = parse_room_fixture(room, mobile, source);
                    assert!(info.is_live, "mobile={mobile}, source={source:?}");
                    assert!(info.streams.iter().any(|stream| stream.url == LIVE_FLV));
                    assert!(info.streams.iter().any(|stream| stream.url == LIVE_HLS));
                    if matches!(
                        source,
                        StreamSource::Origin | StreamSource::DoubleScreenOrigin
                    ) {
                        assert_eq!(info.streams[0].url, LIVE_FLV);
                        assert_eq!(info.streams[0].priority, 0);
                    }
                }
            }
        }
    }

    #[test]
    fn test_live_room_with_heartbeat_finish_time_is_live() {
        for mobile in [false, true] {
            for source in STREAM_SOURCES {
                for (finish_time, last_ping_time) in [
                    (1787737453_i64, 1787737453_i64),
                    (1787737400_i64, 1787737453_i64),
                ] {
                    let mut room = room_fixture(source, LIVE_FLV, LIVE_HLS);
                    room["finish_time"] = json!(finish_time);
                    room["last_ping_time"] = json!(last_ping_time);
                    room["finish_reason"] = json!(0);
                    let info = parse_room_fixture(room, mobile, source);
                    assert!(info.is_live, "mobile={mobile}, source={source:?}");
                    assert!(info.streams.iter().any(|stream| stream.url == LIVE_FLV));
                }
            }
        }
    }

    #[test]
    fn test_end_markers_make_live_status_room_offline() {
        for mobile in [false, true] {
            for (finish_time, last_ping_time, finish_reason) in [
                (json!(1787737453_i64), json!(1787737453_i64), json!(1)),
                (json!(0), json!(0), json!(5)),
                (json!(1787737453_i64), json!(0), json!(0)),
                (json!(1787737453_i64), Value::Null, json!(0)),
                (json!(1787737453_i64), json!(1787737400_i64), json!(0)),
            ] {
                let mut room = room_fixture(StreamSource::Sdk, LIVE_FLV, LIVE_HLS);
                room["finish_time"] = finish_time.clone();
                room["last_ping_time"] = last_ping_time.clone();
                room["finish_reason"] = finish_reason.clone();
                let info = parse_room_fixture(room, mobile, StreamSource::Sdk);
                assert!(
                    !info.is_live,
                    "mobile={mobile}, finish_time={finish_time}, last_ping_time={last_ping_time}, finish_reason={finish_reason}"
                );
                assert!(info.streams.is_empty());
                assert_eq!(info.title, "Test Room");
            }
        }
    }

    #[test]
    fn test_reflow_tagged_streams_stay_live_across_extraction_paths() {
        for mobile in [false, true] {
            for source in STREAM_SOURCES {
                let info = parse_room_fixture(
                    room_fixture(source, REFLOW_FLV, REFLOW_HLS),
                    mobile,
                    source,
                );
                assert!(info.is_live, "mobile={mobile}, source={source:?}");
                assert!(info.streams.iter().any(|stream| stream.url == REFLOW_FLV));
                assert!(info.streams.iter().any(|stream| stream.url == REFLOW_HLS));
            }
        }
    }

    #[test]
    fn test_empty_legacy_urls_are_dropped() {
        for mobile in [false, true] {
            for (flv, hls, expected) in [(LIVE_FLV, "", LIVE_FLV), ("", LIVE_HLS, LIVE_HLS)] {
                let info = parse_room_fixture(
                    room_fixture(StreamSource::Legacy, flv, hls),
                    mobile,
                    StreamSource::Legacy,
                );
                assert!(info.is_live);
                assert!(!info.streams.is_empty());
                assert!(info.streams.iter().all(|stream| stream.url == expected));
            }
        }
    }

    #[test]
    fn test_reflow_live_room_is_live() {
        let info = parse_app_body(REFLOW_LIVE_ROOM);
        assert!(info.is_live);
        assert!(!info.streams.is_empty());
        assert!(
            info.streams
                .iter()
                .all(|stream| has_reflow_tag(&stream.url))
        );
        assert_eq!(info.title, "Test Room");
    }

    #[test]
    fn test_reflow_ended_room_is_offline() {
        let info = parse_app_body(REFLOW_ENDED_ROOM);
        assert!(!info.is_live);
        assert!(info.streams.is_empty());
        assert_eq!(info.title, "Test Room");
    }

    #[test]
    fn test_reflow_ended_room_with_stale_live_status_is_offline() {
        // A response that still says status=2 next to end markers is an ended room;
        // each end marker on its own must keep it offline.
        let ended: Value = serde_json::from_str(REFLOW_ENDED_ROOM).unwrap();
        for (finish_reason, last_ping_time) in [(json!(1), json!(0)), (json!(0), json!(0))] {
            let mut body = ended.clone();
            let room = &mut body["data"]["room"];
            room["status"] = json!(2);
            room["finish_reason"] = finish_reason;
            room["last_ping_time"] = last_ping_time;
            let info = parse_app_body(&body.to_string());
            assert!(!info.is_live);
            assert!(info.streams.is_empty());
        }
    }

    #[test]
    fn test_reflow_streamer_identity_comes_from_room_owner() {
        for body in [REFLOW_LIVE_ROOM, REFLOW_ENDED_ROOM] {
            let info = parse_app_body(body);
            assert_eq!(info.artist, "Test User");
            assert_eq!(
                info.artist_url.as_deref(),
                Some("https://p3-webcast.example.com/avatar.image")
            );
        }

        // An ended room answered with a prompt also names the owner.
        let mut body: Value = serde_json::from_str(REFLOW_ENDED_ROOM).unwrap();
        body["data"]["prompts"] = json!("直播已结束");
        let info = parse_app_body(&body.to_string());
        assert!(!info.is_live);
        assert_eq!(info.artist, "Test User");
    }

    #[test]
    fn test_reflow_streamer_identity_without_owner_stays_empty() {
        for owner in [
            Value::Null,
            json!({}),
            json!({"nickname": "", "avatar_thumb": null}),
        ] {
            let mut body: Value = serde_json::from_str(REFLOW_LIVE_ROOM).unwrap();
            body["data"]["room"]["owner"] = owner.clone();
            let info = parse_app_body(&body.to_string());
            assert!(info.is_live, "owner={owner}");
            assert_eq!(info.artist, "", "owner={owner}");
            assert_eq!(info.artist_url, None, "owner={owner}");
        }
    }

    #[test]
    fn test_replay_room_with_live_status_is_offline_across_extraction_paths() {
        for mobile in [false, true] {
            for source in STREAM_SOURCES {
                for (flv, hls) in [(LIVE_FLV, LIVE_HLS), (REFLOW_FLV, REFLOW_HLS)] {
                    let mut room = room_fixture(source, flv, hls);
                    room["is_replay"] = json!(true);
                    room["finish_time"] = json!(1787737453_i64);
                    room["last_ping_time"] = json!(1787737453_i64);
                    room["finish_reason"] = json!(0);
                    let info = parse_room_fixture(room, mobile, source);
                    assert!(!info.is_live, "mobile={mobile}, source={source:?}");
                    assert!(info.streams.is_empty());
                    assert_eq!(info.title, "Test Room");
                }
            }
        }
    }

    #[test]
    fn test_reflow_replay_room_is_offline() {
        for is_replay in [json!(true), json!(false), Value::Null] {
            let mut body: Value = serde_json::from_str(REFLOW_LIVE_ROOM).unwrap();
            body["data"]["room"]["is_replay"] = is_replay.clone();
            let info = parse_app_body(&body.to_string());
            let expect_live = is_replay != json!(true);
            assert_eq!(info.is_live, expect_live, "is_replay={is_replay}");
            assert_eq!(
                info.streams.is_empty(),
                !expect_live,
                "is_replay={is_replay}"
            );
        }
    }

    #[test]
    fn test_user_identity_takes_precedence_over_room_owner() {
        for mobile in [false, true] {
            let mut room = room_fixture(StreamSource::Sdk, LIVE_FLV, LIVE_HLS);
            room["owner"] = json!({
                "nickname": "Owner Name",
                "avatar_thumb": {"url_list": ["https://example.com/owner.jpg"]}
            });
            let info = parse_room_fixture(room, mobile, StreamSource::Sdk);
            assert_eq!(info.artist, "Test User");
            assert_eq!(
                info.artist_url.as_deref(),
                Some("https://example.com/avatar.jpg")
            );
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_extract_live() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();

        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None);
        let media_info = config.extract().await;

        if let Ok(media_info) = media_info {
            println!("{}", media_info.pretty_print());
        } else {
            println!("{:?}", media_info);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_request_cookie_logic() {
        GlobalTtwidManager::set_global_ttwid("global_ttwid_for_test");
        let config_global = Douyin::new(TEST_URL.to_string(), default_client(), None, None)
            .ttwid_mode(TtwidManagementMode::Global);

        let mut request_global = DouyinRequest::new(
            config_global.extractor.cookies.clone(),
            &config_global,
            "123".to_string(),
        );
        request_global.ensure_ttwid().await.unwrap();
        assert_eq!(
            request_global.cookies.get("ttwid"),
            Some(&"global_ttwid_for_test".to_string())
        );

        let config_per_extractor_set =
            Douyin::new(TEST_URL.to_string(), default_client(), None, None)
                .ttwid("preset_ttwid".to_string());

        let mut request_per_extractor_set = DouyinRequest::new(
            config_per_extractor_set.extractor.cookies.clone(),
            &config_per_extractor_set,
            "123".to_string(),
        );
        request_per_extractor_set.ensure_ttwid().await.unwrap();
        assert_eq!(
            request_per_extractor_set.cookies.get("ttwid"),
            Some(&"preset_ttwid".to_string())
        );
    }

    #[test]
    fn test_is_account_banned() {
        let banned_user = DouyinUserInfo {
            id_str: "1",
            sec_uid: "1",
            nickname: "账号已注销".to_string(),
            avatar_thumb: Some(DouyinAvatarThumb {
                url_list: vec!["http://example.com/aweme_default_avatar.png".into()],
            }),
        };
        assert!(banned_user.is_cancelled());

        let active_user = DouyinUserInfo {
            id_str: "2",
            sec_uid: "2",
            nickname: "ActiveUser".to_string(),
            avatar_thumb: Some(DouyinAvatarThumb {
                url_list: vec!["http://example.com/real_avatar.png".into()],
            }),
        };
        assert!(!active_user.is_cancelled());
    }

    #[test]
    fn test_parse_pc_response_stream_ended() {
        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None);
        let mut request =
            DouyinRequest::new(config.extractor.cookies.clone(), &config, "123".to_string());

        let json_response = r#"{
            "data": {
                "prompts": "直播已结束",
                "user": {
                    "id_str": "12345",
                    "sec_uid": "sec_uid_123",
                    "nickname": "Test User",
                    "avatar_thumb": {
                        "url_list": ["http://example.com/avatar.jpg"]
                    }
                },
                "data": [{
                    "id_str": "room_123",
                    "status": 4,
                    "title": "Test Room Title",
                    "cover": {
                        "url_list": ["http://example.com/cover.jpg"]
                    },
                    "stream_url": null
                }],
                "enter_room_id": "room_123"
            }
        }"#;

        let result = request.parse_pc_response(json_response);
        assert!(
            result.is_ok(),
            "Should return Ok for stream ended with data"
        );
        let media_info = result.unwrap();
        assert!(!media_info.is_live, "Should be offline");
        assert_eq!(media_info.title, "Test Room Title");
        assert_eq!(media_info.artist, "Test User");
    }

    #[tokio::test]
    async fn test_parse_pc_response_ended_no_user() {
        let json_response = r#"{
            "data": {
                "prompts": "直播已结束",
                "user": null,
                "data": [],
                "partition_road_map": {},
                "enter_room_id": "123"
            }
        }"#;

        let config = Douyin::new(TEST_URL.to_string(), default_client(), None, None);
        let mut request =
            DouyinRequest::new(config.extractor.cookies.clone(), &config, "123".to_string());

        let result = request.parse_pc_response(json_response);
        assert!(
            result.is_ok(),
            "Should successfully parse ended stream with no user data as offline"
        );
        let media_info = result.unwrap();
        assert!(!media_info.is_live);
    }

    #[test]
    fn test_priority_assignment() {
        let mut streams = Vec::new();

        add_stream_if_url_present(
            "http://example.com/origin.flv",
            StreamFormat::Flv,
            MediaFormat::Flv,
            "原画",
            8000,
            "h264",
            30,
            None,
            "origin",
            &mut streams,
        );

        add_stream_if_url_present(
            "http://example.com/uhd.flv",
            StreamFormat::Flv,
            MediaFormat::Flv,
            "蓝光",
            10000,
            "h264",
            30,
            None,
            "uhd",
            &mut streams,
        );

        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].quality, "原画");
        assert_eq!(streams[0].priority, 0);

        assert_eq!(streams[1].quality, "蓝光");
        assert_eq!(streams[1].priority, 10);
    }

    #[test]
    fn negative_v_bit_rate_maps_to_zero_without_wrapping() {
        let mut room = room_fixture(StreamSource::Sdk, LIVE_FLV, LIVE_HLS);
        room["stream_url"]["live_core_sdk_data"]["pull_data"]["options"]["qualities"][0]["v_bit_rate"] =
            json!(-1);
        let info = parse_room_fixture(room, false, StreamSource::Sdk);
        assert!(!info.streams.is_empty());
        for s in &info.streams {
            assert_eq!(s.bitrate, 0);
        }
    }
}
