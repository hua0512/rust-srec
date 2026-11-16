//! Bigo Live stream extractor.
//!
//! Live status comes from the public profile API, `getUserInfoStudio`. It
//! needs no login and is sent no cookies, so Bigo's login wall cannot refuse
//! it, and an offline check is that one request. A live room then goes to
//! `getInternalStudioInfo` for its single HLS media playlist, with the
//! account's cookies when one is selected and the room password when one is
//! set.
//!
//! Bigo restricts some accounts without signing them out: the studio then
//! answers every room as offline. When the studio reads a room as offline that
//! the profile reports live, the profile is asked again, and if the room is
//! still live a signed-in check fails with `account_restricted`, so the
//! account is set aside instead of the room reading offline.

use std::borrow::Cow;
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use md5::{Digest, Md5};
use regex::Regex;
use reqwest::{Client, RequestBuilder};
use rustc_hash::FxHashMap;
use tracing::debug;
use url::Url;

use crate::digest_to_hex;
use crate::extractor::error::ExtractorError;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::bigo::metadata::metadata;
use crate::extractor::platforms::bigo::models::{
    Presence, PresenceResponse, StudioData, StudioResponse,
};
use crate::extractor::utils::{capture_group_1_owned, extras_get_str};
use crate::media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo};

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?bigo\.tv/(?:[a-z]{2}/)?([^/?#]+)").unwrap()
});

const PRESENCE_API: &str = "https://ta.bigo.tv/official_website/OUserCenter/getUserInfoStudio";
const STUDIO_API: &str = "https://ta.bigo.tv/official_website/studio/getInternalStudioInfo";
/// The profile API's code for an id that names no account.
const UNKNOWN_ID: i64 = 1;
/// How long to wait before asking the profile again when the studio reads a
/// live room as offline, so a stream that has just ended reads as ended.
const RECHECK_DELAY: Duration = Duration::from_secs(1);

pub struct Bigo {
    extractor: Extractor,
    stream_password: Option<String>,
    /// Cookies were supplied, so studio requests carry a login.
    signed_in: bool,
    presence_api: Cow<'static, str>,
    studio_api: Cow<'static, str>,
}

impl Bigo {
    const BASE_URL: &str = "https://www.bigo.tv";

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("bigo", url, client);
        extractor.set_origin_and_referer_static(Self::BASE_URL);
        let signed_in = cookies
            .as_deref()
            .is_some_and(|cookies| !cookies.trim().is_empty());
        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        let stream_password = extras_get_str(extras.as_ref(), "stream_password")
            .or_else(|| extras_get_str(extras.as_ref(), "password"))
            .map(str::to_string);

        Self {
            extractor,
            stream_password,
            signed_in,
            presence_api: Cow::Borrowed(PRESENCE_API),
            studio_api: Cow::Borrowed(STUDIO_API),
        }
    }

    fn extract_site_id(&self) -> Result<String, ExtractorError> {
        capture_group_1_owned(&URL_REGEX, &self.extractor.url)
            .ok_or_else(|| ExtractorError::InvalidUrl(self.extractor.url.clone()))
    }

    fn resolve_password(&self) -> Option<String> {
        let url = Url::parse(&self.extractor.url).ok();
        let from_query = url.as_ref().and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "pwd")
                .map(|(_, v)| v.to_string())
                .filter(|s| !s.is_empty())
        });
        from_query.or_else(|| self.stream_password.clone())
    }

    fn md5_hex(input: &str) -> String {
        let mut hasher = Md5::new();
        hasher.update(input.as_bytes());
        digest_to_hex(&hasher.finalize())
    }

    /// The profile request. It goes through the client directly because
    /// [`Extractor::request`] would add the account's cookies, which a public
    /// lookup has no use for.
    fn presence_request(&self, site_id: &str) -> RequestBuilder {
        self.extractor
            .client
            .get(self.presence_api.as_ref())
            .headers(self.extractor.get_platform_headers().clone())
            .query(&[("bigoId", site_id)])
    }

    async fn presence(&self, site_id: &str) -> Result<Presence, ExtractorError> {
        let response = self.presence_request(site_id).send().await?;
        let response = ExtractorError::check_response(response)?
            .json::<PresenceResponse>()
            .await?;
        match (response.code, response.data) {
            (0, Some(presence)) => Ok(presence),
            (UNKNOWN_ID, _) => Err(ExtractorError::StreamerNotFound),
            (code, _) => Err(ExtractorError::ValidationError(format!(
                "bigo profile API answered code {code} ({:?})",
                response.msg
            ))),
        }
    }

    async fn studio(
        &self,
        site_id: &str,
        password: Option<&str>,
    ) -> Result<StudioData, ExtractorError> {
        let mut form = vec![("siteId", site_id.to_string())];
        if let Some(pwd) = password.filter(|p| !p.is_empty()) {
            form.push(("verify", Self::md5_hex(pwd)));
            form.push(("isEncrypt", "1".to_string()));
        } else {
            form.push(("verify", String::new()));
        }

        let response = self
            .extractor
            .post(self.studio_api.as_ref())
            .form(&form)
            .send()
            .await?;
        let response = ExtractorError::check_response(response)?
            .json::<StudioResponse>()
            .await?;
        if !response.is_success() {
            return Err(ExtractorError::ValidationError(format!(
                "bigo studio API error: code={:?} msg={:?}",
                response.code, response.msg
            )));
        }
        Ok(response.data.unwrap_or_default())
    }

    /// Title, artist, cover and identity, all from the profile.
    fn media_info(&self, presence: &Presence, site_id: &str) -> MediaInfo {
        let nickname = presence.nickname.as_deref().filter(|s| !s.is_empty());
        let title = presence
            .room_topic
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(nickname)
            .unwrap_or(site_id);
        let mut media = MediaInfo::builder(Self::BASE_URL, title, nickname.unwrap_or(site_id))
            .cover_url_opt(presence.og_image.clone().filter(|s| !s.is_empty()))
            .headers(self.extractor.get_platform_headers_map())
            .build();
        media.platform_metadata = metadata(presence.uid);
        media
    }

    fn offline_media(&self, presence: &Presence, site_id: &str) -> MediaInfo {
        let mut media = self.media_info(presence, site_id);
        media.extras = Some(FxHashMap::from_iter([(
            "site_id".to_string(),
            site_id.to_string(),
        )]));
        media
    }

    /// The live result for a room the profile reports live, from the
    /// studio's answer. `None` means the studio reads the room as offline.
    fn live_media(
        &self,
        presence: &Presence,
        data: StudioData,
        site_id: &str,
        password: Option<String>,
    ) -> Result<Option<MediaInfo>, ExtractorError> {
        // The website stops playback on this flag before reading `alive`.
        if data.need_login == Some(true) {
            return Err(ExtractorError::Authentication {
                code: "need_login".to_string(),
            });
        }
        // A locked room reads as offline until `verify` unlocks it, so this
        // comes before any conclusion drawn from an offline answer.
        if data.pass_room == Some(true) && (password.is_none() || !data.has_playable_source()) {
            return Err(ExtractorError::PrivateContent);
        }
        if !data.has_playable_source() {
            if data.is_room_live() {
                return Err(ExtractorError::ValidationError(format!(
                    "bigo studio response for live room {site_id} omitted hls_src"
                )));
            }
            return Ok(None);
        }

        let room_id = presence
            .room_id()
            .map(|room_id| room_id.to_string())
            .or_else(|| data.room_id.filter(|s| !s.is_empty() && s != "0"))
            .ok_or_else(|| {
                ExtractorError::ValidationError("bigo online room missing roomId".to_string())
            })?;
        let hls_src = data.hls_src.unwrap_or_default();
        let stream = StreamInfo::builder(hls_src, StreamFormat::Hls, MediaFormat::Ts)
            .quality("live")
            .priority(0)
            .is_headers_needed(true)
            .build();

        let mut extras = FxHashMap::default();
        // The danmu provider enters the chat room by this id.
        extras.insert("room_id".to_string(), room_id);
        extras.insert("site_id".to_string(), site_id.to_string());
        extras.insert("uid".to_string(), presence.uid.to_string());
        if let Some(sid) = data.sid {
            extras.insert("sid".to_string(), sid);
        }
        if let Some(pwd) = password {
            // The danmu provider sends it as the room's secret key.
            extras.insert("stream_password".to_string(), pwd);
        }

        let mut media = self.media_info(presence, site_id);
        media.is_live = true;
        media.artist_url = data.avatar.filter(|s| !s.is_empty());
        media.streams = vec![stream];
        media.extras = Some(extras);
        Ok(Some(media))
    }
}

#[async_trait]
impl PlatformExtractor for Bigo {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let site_id = self.extract_site_id()?;
        debug!(%site_id, "bigo extract");

        let presence = self.presence(&site_id).await?;
        if presence.alive != 1 {
            return Ok(self.offline_media(&presence, &site_id));
        }

        let password = self.resolve_password();
        let data = self.studio(&site_id, password.as_deref()).await?;
        if let Some(media) = self.live_media(&presence, data, &site_id, password)? {
            return Ok(media);
        }

        // The studio reads a room the profile reports live as offline. Either
        // the stream ended in between, or Bigo answers this session that way
        // for every room.
        tokio::time::sleep(RECHECK_DELAY).await;
        let again = self.presence(&site_id).await?;
        if again.alive != 1 || again.room_id() != presence.room_id() {
            return Ok(self.offline_media(&again, &site_id));
        }
        if self.signed_in && presence.room_id().is_some() {
            return Err(ExtractorError::Authentication {
                code: "account_restricted".to_string(),
            });
        }
        Err(ExtractorError::ValidationError(format!(
            "bigo studio reads live room {site_id} as offline"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::default::default_client;
    use crate::extractor::test_support::serve_in_order;

    const ROOM_ID: &str = "6935768021135771674";
    const LIVE: &str = r#"{"code":0,"msg":"success","data":{"uid":509239070,
        "nickname":"Passi","roomTopic":"","alive":1,"ogImage":"https://esx.bigo.sg/cover.jpg",
        "aliOSUrl":"bigolive://livevideoshow?roomid=6935768021135771674&uid=509239070&visitor=1"}}"#;
    /// Live, but without a link to the room.
    const LIVE_WITHOUT_ROOM: &str = r#"{"code":0,"msg":"success","data":{"uid":509239070,
        "nickname":"Passi","alive":1,"aliOSUrl":"bigolive://livevideoshow?roomid=0&uid=509239070"}}"#;
    /// Just ended: the link still names the last room.
    const OFFLINE: &str = r#"{"code":0,"msg":"success","data":{"uid":509239070,
        "nickname":"Passi","roomTopic":"Back soon","alive":0,"ogImage":"https://esx.bigo.sg/cover.jpg",
        "aliOSUrl":"bigolive://livevideoshow?roomid=6935768021135771674&uid=509239070&visitor=1"}}"#;
    const STUDIO_LIVE: &str = r#"{"code":0,"msg":"success","data":{"sid":2555365402,
        "uid":509239070,"alive":1,"roomId":"6935768021135771674","needLogin":false,"passRoom":false,
        "hls_src":"https://cdn.example.com/list_x.m3u8","avatar":"https://esx.bigo.sg/avatar.jpg"}}"#;
    const STUDIO_OFFLINE: &str = r#"{"code":0,"msg":"success","data":{"sid":2555365402,
        "uid":509239070,"alive":0,"roomId":"0","hls_src":"","needLogin":false,"passRoom":false}}"#;
    const STUDIO_NEED_LOGIN: &str = r#"{"code":0,"msg":"success","data":{"sid":2555365402,
        "uid":509239070,"alive":0,"roomId":"0","hls_src":"","needLogin":true,"passRoom":false}}"#;
    const STUDIO_LOCKED: &str = r#"{"code":0,"msg":"success","data":{"sid":2555365402,
        "uid":509239070,"alive":0,"roomId":"0","hls_src":"","needLogin":false,"passRoom":true}}"#;
    /// Live by `alive`/`roomId`, but without a playlist.
    const STUDIO_LIVE_WITHOUT_HLS: &str = r#"{"code":0,"msg":"success","data":{"alive":1,
        "roomId":"6935768021135771674","hls_src":"","needLogin":false,"passRoom":false}}"#;
    const ACCOUNT: &str = "uniqid=u; tid=t; yyuid=1";

    /// Runs a check whose profile requests are answered by `presence` and
    /// studio requests by `studio`, in order. A request beyond those fails.
    async fn check(
        presence: &[&'static str],
        studio: &[&'static str],
        cookies: Option<&str>,
        extras: Option<serde_json::Value>,
    ) -> Result<MediaInfo, ExtractorError> {
        let ok = |bodies: &[&'static str]| bodies.iter().map(|body| ("200 OK", *body)).collect();
        let mut bigo = Bigo::new(
            "https://www.bigo.tv/725943857".to_string(),
            default_client(),
            cookies.map(str::to_owned),
            extras,
        );
        bigo.presence_api = Cow::Owned(serve_in_order(ok(presence)).await);
        bigo.studio_api = Cow::Owned(serve_in_order(ok(studio)).await);
        tokio::time::timeout(Duration::from_secs(10), bigo.extract())
            .await
            .expect("local Bigo requests timed out")
    }

    fn extra<'a>(media: &'a MediaInfo, key: &str) -> Option<&'a str> {
        media.extras.as_ref()?.get(key).map(String::as_str)
    }

    fn metadata_uid(media: &MediaInfo) -> &serde_json::Value {
        &media
            .platform_metadata
            .as_ref()
            .expect("platform metadata")
            .data["uid"]
    }

    #[test]
    fn url_regex_matches() {
        assert!(URL_REGEX.is_match("https://www.bigo.tv/221338632"));
        assert!(URL_REGEX.is_match("https://bigo.tv/221338632"));
        assert!(URL_REGEX.is_match("https://www.bigo.tv/ja/221338632"));
        assert!(URL_REGEX.is_match("www.bigo.tv/username"));
        assert!(URL_REGEX.is_match("https://www.bigo.tv/ba.brendac"));
        assert!(!URL_REGEX.is_match("https://www.twitch.tv/foo"));
        assert!(!URL_REGEX.is_match("https://bigo.tv/"));
    }

    #[test]
    fn url_regex_captures_site_id() {
        let m = URL_REGEX
            .captures("https://www.bigo.tv/ja/221338632")
            .unwrap();
        assert_eq!(m.get(1).unwrap().as_str(), "221338632");
        let m = URL_REGEX
            .captures("https://www.bigo.tv/ba.brendac")
            .unwrap();
        assert_eq!(m.get(1).unwrap().as_str(), "ba.brendac");
    }

    #[test]
    fn password_md5() {
        assert_eq!(Bigo::md5_hex("secret"), "5ebe2294ecd0e0f08eab7690d2a6ee69");
    }

    #[test]
    fn profile_request_carries_no_account_cookies() {
        let bigo = Bigo::new(
            "https://www.bigo.tv/1".to_string(),
            default_client(),
            Some(ACCOUNT.to_string()),
            None,
        );
        let request = bigo.presence_request("1").build().unwrap();
        assert!(!request.headers().contains_key(reqwest::header::COOKIE));
    }

    #[tokio::test]
    async fn offline_profile_answers_without_the_studio() {
        // The studio has no answers, so asking it would fail the check.
        let media = check(&[OFFLINE], &[], Some(ACCOUNT), None).await.unwrap();

        assert!(!media.is_live);
        assert_eq!(media.title, "Back soon");
        assert_eq!(media.artist, "Passi");
        assert_eq!(extra(&media, "room_id"), None);
        assert_eq!(metadata_uid(&media), "509239070");
    }

    #[tokio::test]
    async fn profile_codes_map_to_outcomes() {
        let unknown = r#"{"code":1,"data":null,"msg":"paramters invalid"}"#;
        let refused = r#"{"code":80009,"data":null,"msg":"empty bigoId or sid"}"#;

        let error = check(&[unknown], &[], None, None).await.unwrap_err();
        assert!(
            matches!(error, ExtractorError::StreamerNotFound),
            "{error:?}"
        );
        let error = check(&[refused], &[], None, None).await.unwrap_err();
        assert!(
            matches!(error, ExtractorError::ValidationError(_)),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn live_room_resolves_its_stream_through_the_studio() {
        let extras = serde_json::json!({"stream_password": "pw"});
        let media = check(&[LIVE], &[STUDIO_LIVE], None, Some(extras))
            .await
            .unwrap();

        assert!(media.is_live);
        assert_eq!(media.streams.len(), 1);
        assert_eq!(media.streams[0].url, "https://cdn.example.com/list_x.m3u8");
        assert_eq!(
            media.artist_url.as_deref(),
            Some("https://esx.bigo.sg/avatar.jpg")
        );
        assert_eq!(extra(&media, "room_id"), Some(ROOM_ID));
        assert_eq!(extra(&media, "stream_password"), Some("pw"));
        assert_eq!(metadata_uid(&media), "509239070");
    }

    #[tokio::test]
    async fn studio_refusals_are_not_offline() {
        let password = || Some(serde_json::json!({"stream_password": "wrong"}));
        for (studio, cookies, extras, expected) in [
            (STUDIO_NEED_LOGIN, None, None, "authentication"),
            // A locked room is private, never evidence of a restricted account.
            (STUDIO_LOCKED, Some(ACCOUNT), None, "private"),
            (STUDIO_LOCKED, Some(ACCOUNT), password(), "private"),
            (STUDIO_LIVE_WITHOUT_HLS, Some(ACCOUNT), None, "validation"),
        ] {
            let error = check(&[LIVE], &[studio], cookies, extras)
                .await
                .unwrap_err();
            assert_eq!(error.category(), expected, "{studio}: {error:?}");
        }
    }

    /// A signed-in studio answer that reads a live room as offline means a
    /// restricted account only while the profile still reports the same room.
    #[tokio::test]
    async fn studio_offline_for_a_live_room_is_rechecked() {
        let restricted = check(&[LIVE, LIVE], &[STUDIO_OFFLINE], Some(ACCOUNT), None)
            .await
            .unwrap_err();
        assert!(
            matches!(&restricted, ExtractorError::Authentication { code } if code == "account_restricted"),
            "{restricted:?}"
        );

        let ended = check(&[LIVE, OFFLINE], &[STUDIO_OFFLINE], Some(ACCOUNT), None)
            .await
            .unwrap();
        assert!(!ended.is_live);

        for (presence, cookies) in [
            ([LIVE, LIVE], None),
            ([LIVE_WITHOUT_ROOM, LIVE_WITHOUT_ROOM], Some(ACCOUNT)),
        ] {
            let error = check(&presence, &[STUDIO_OFFLINE], cookies, None)
                .await
                .unwrap_err();
            assert!(
                matches!(error, ExtractorError::ValidationError(_)),
                "{error:?}"
            );
        }
    }

    /// Checks a real room. Run with
    /// `BIGO_URL=https://www.bigo.tv/<id> cargo test -p platforms-parser bigo::builder::tests::live_extract -- --ignored --nocapture`;
    /// set `BIGO_COOKIE_FILE` to a file holding a signed-in cookie header to
    /// check with an account.
    #[tokio::test]
    #[ignore]
    async fn live_extract() {
        let url = std::env::var("BIGO_URL").expect("BIGO_URL");
        let cookies = std::env::var("BIGO_COOKIE_FILE").ok().map(|path| {
            std::fs::read_to_string(path)
                .expect("BIGO_COOKIE_FILE")
                .trim()
                .to_owned()
        });
        let bigo = Bigo::new(url, default_client(), cookies, None);
        let media = bigo.extract().await.expect("extract failed");

        println!(
            "live={} title={:?} artist={:?} streams={} room_id={:?} metadata={:?}",
            media.is_live,
            media.title,
            media.artist,
            media.streams.len(),
            extra(&media, "room_id"),
            media
                .platform_metadata
                .as_ref()
                .map(|metadata| &metadata.data),
        );
        assert!(media.platform_metadata.is_some());
        if media.is_live {
            assert!(media.streams[0].url.contains(".m3u8"));
            assert!(extra(&media, "room_id").is_some());
        }
    }
}
