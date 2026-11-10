//! Bigo Live stream extractor.
//!
//! Resolves `siteId` via `getInternalStudioInfo` to a single HLS media playlist.
//! A signed-in check is answered in full without an integrity token, so it
//! mints one only when a live room comes back without its playlist; an
//! anonymous check mints one first by default (website parity). Supports
//! password rooms.
//!
//! Bigo restricts some accounts without signing them out: `getUserInfo` still
//! succeeds, but the studio API reports every room offline, `needLogin`
//! unset, the same as for a room that really is offline. A signed-in check
//! that reads a room as offline therefore confirms, at most every
//! [`ACCOUNT_TRUSTED_FOR`], that the account still sees a room from Bigo's
//! public list of live rooms.

use std::hash::BuildHasher;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use md5::{Digest, Md5};
use parking_lot::Mutex;
use regex::Regex;
use reqwest::Client;
use rustc_hash::{FxBuildHasher, FxHashMap};
use serde::Deserialize;
use tracing::{debug, warn};
use url::Url;

use crate::digest_to_hex;
use crate::extractor::error::ExtractorError;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::bigo::models::{StudioData, StudioResponse};
use crate::extractor::platforms::bigo::token;
use crate::extractor::utils::{capture_group_1_owned, extras_get_bool, extras_get_str};
use crate::media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo};

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?bigo\.tv/(?:[a-z]{2}/)?([^/?#]+)").unwrap()
});

/// How long an account that saw a live room is trusted to report offline
/// rooms truthfully.
const ACCOUNT_TRUSTED_FOR: Duration = Duration::from_secs(5 * 60);
/// Listed rooms an account is asked about. A room can end between the list
/// and the request, so one offline answer does not condemn the account.
const PROBE_ROOMS: usize = 2;

/// When each account, keyed by [`Bigo::account`], last saw a live room.
static ACCOUNTS_SEEING_LIVE: LazyLock<Mutex<FxHashMap<u64, Instant>>> =
    LazyLock::new(|| Mutex::new(FxHashMap::default()));

fn saw_live_room(account: u64) {
    let mut seen = ACCOUNTS_SEEING_LIVE.lock();
    seen.retain(|_, at| at.elapsed() < ACCOUNT_TRUSTED_FOR);
    seen.insert(account, Instant::now());
}

#[derive(Deserialize)]
struct LiveListResponse {
    #[serde(default)]
    data: Option<LiveListData>,
}

#[derive(Deserialize)]
struct LiveListData {
    #[serde(default)]
    data: Vec<ListedRoom>,
}

#[derive(Deserialize)]
struct ListedRoom {
    #[serde(default)]
    bigo_id: Option<String>,
    #[serde(default)]
    is_locked: Option<serde_json::Value>,
}

impl ListedRoom {
    /// The room's site id, unless it needs a password.
    fn public_site_id(self) -> Option<String> {
        let locked = self.is_locked.is_some_and(|value| {
            value.as_i64().is_some_and(|n| n != 0) || value.as_bool() == Some(true)
        });
        self.bigo_id.filter(|id| !locked && !id.is_empty())
    }
}

pub struct Bigo {
    extractor: Extractor,
    stream_password: Option<String>,
    mint_token: bool,
    /// Identifies the login when cookies were supplied: a hash of them.
    account: Option<u64>,
}

impl Bigo {
    const BASE_URL: &str = "https://www.bigo.tv";
    const STUDIO_API: &str = "https://ta.bigo.tv/official_website/studio/getInternalStudioInfo";
    /// Public rooms broadcasting now, as listed on the website's home page.
    /// Answered without a login.
    const LIVE_LIST_API: &str = "https://ta.bigo.tv/official_website/OInterfaceWeb/vedioList/72";

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("bigo", url, client);
        extractor.set_origin_and_referer_static(Self::BASE_URL);
        let account = cookies
            .as_deref()
            .map(str::trim)
            .filter(|cookies| !cookies.is_empty())
            .map(|cookies| FxBuildHasher.hash_one(cookies));
        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }

        let stream_password = extras_get_str(extras.as_ref(), "stream_password")
            .or_else(|| extras_get_str(extras.as_ref(), "password"))
            .map(str::to_string);
        // Default true when unset.
        let mint_token = extras_get_bool(extras.as_ref(), "mint_token").unwrap_or(true);

        Self {
            extractor,
            stream_password,
            mint_token,
            account,
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

    async fn fetch_studio(
        &self,
        site_id: &str,
        password: Option<&str>,
        integrity_token: Option<&str>,
    ) -> Result<StudioResponse, ExtractorError> {
        let mut form: Vec<(String, String)> = Vec::with_capacity(5);
        form.push(("siteId".to_string(), site_id.to_string()));

        if let Some(pwd) = password.filter(|p| !p.is_empty()) {
            form.push(("verify".to_string(), Self::md5_hex(pwd)));
            form.push(("isEncrypt".to_string(), "1".to_string()));
        } else {
            form.push(("verify".to_string(), String::new()));
        }

        if let Some(token) = integrity_token.filter(|t| !t.is_empty()) {
            form.push(("token".to_string(), token.to_string()));
            form.push(("supportHevc".to_string(), "1".to_string()));
        }

        let response = self
            .extractor
            .post(Self::STUDIO_API)
            .form(&form)
            .send()
            .await?
            .error_for_status()?
            .json::<StudioResponse>()
            .await?;

        Ok(response)
    }

    async fn studio(
        &self,
        site_id: &str,
        password: Option<&str>,
        integrity_token: Option<&str>,
    ) -> Result<StudioData, ExtractorError> {
        let response = self
            .fetch_studio(site_id, password, integrity_token)
            .await?;
        if !response.is_success() {
            return Err(ExtractorError::ValidationError(format!(
                "bigo studio API error: code={:?} msg={:?}",
                response.code, response.msg
            )));
        }
        Ok(response.data.unwrap_or_default())
    }

    fn media_from_data(
        &self,
        data: StudioData,
        site_id: &str,
    ) -> Result<MediaInfo, ExtractorError> {
        let title = data.display_title();
        let artist = data.artist();
        let cover = data.snapshot.clone().filter(|s| !s.is_empty());
        let artist_url = data.avatar.clone().filter(|s| !s.is_empty());
        let pass_room = data.pass_room.unwrap_or(false);

        // The website stops playback on this flag before reading `alive`, so a
        // missing or expired login must not be reported as an offline room.
        if data.need_login == Some(true) {
            return Err(ExtractorError::Authentication {
                code: "need_login".to_string(),
            });
        }

        if !data.is_room_live() {
            if pass_room {
                return Err(ExtractorError::PrivateContent);
            }

            let mut extras = FxHashMap::default();
            if let Some(rid) = data.room_id.clone().filter(|s| s != "0" && !s.is_empty()) {
                extras.insert("room_id".to_string(), rid);
            }
            extras.insert("site_id".to_string(), site_id.to_string());

            return Ok(MediaInfo::builder(Self::BASE_URL, title, artist)
                .is_live(false)
                .artist_url_opt(artist_url)
                .cover_url_opt(cover)
                .headers(self.extractor.get_platform_headers_map())
                .extras(extras)
                .build());
        }

        // The room is broadcasting but the response carried no playlist. Either
        // `verify` did not unlock it, or `getInternalStudioInfo` withheld the
        // source because `token` had already been spent — a minted token is
        // accepted once. Neither is an offline room, so surface an error the
        // caller can retry instead of a `is_live(false)` MediaInfo.
        if !data.has_playable_source() {
            if pass_room {
                return Err(ExtractorError::PrivateContent);
            }
            return Err(ExtractorError::ValidationError(format!(
                "bigo studio response for live room {site_id} omitted hls_src"
            )));
        }

        let hls_src = data
            .hls_src
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or(ExtractorError::NoStreamsFound)?;

        let room_id = data
            .room_id
            .clone()
            .filter(|s| !s.is_empty() && s != "0")
            .ok_or_else(|| {
                ExtractorError::ValidationError("bigo online room missing roomId".to_string())
            })?;

        if pass_room && self.resolve_password().is_none() {
            return Err(ExtractorError::PrivateContent);
        }

        let stream = StreamInfo::builder(hls_src, StreamFormat::Hls, MediaFormat::Ts)
            .quality("live")
            .priority(0)
            .is_headers_needed(true)
            .build();

        let mut extras = FxHashMap::default();
        extras.insert("room_id".to_string(), room_id);
        extras.insert("site_id".to_string(), site_id.to_string());
        if let Some(sid) = data.sid.clone() {
            extras.insert("sid".to_string(), sid);
        }
        if let Some(uid) = data.uid.clone() {
            extras.insert("uid".to_string(), uid);
        }
        if let Some(pwd) = self.resolve_password() {
            // Forward to danmu enter secretKey via ConnectionConfig extras.
            extras.insert("stream_password".to_string(), pwd);
        }

        Ok(MediaInfo::builder(Self::BASE_URL, title, artist)
            .is_live(true)
            .artist_url_opt(artist_url)
            .cover_url_opt(cover)
            .streams(vec![stream])
            .headers(self.extractor.get_platform_headers_map())
            .extras(extras)
            .build())
    }

    async fn live_site_ids(&self) -> Result<Vec<String>, ExtractorError> {
        let response = self
            .extractor
            .get(Self::LIVE_LIST_API)
            .send()
            .await?
            .error_for_status()?
            .json::<LiveListResponse>()
            .await?;
        Ok(response
            .data
            .map(|list| {
                list.data
                    .into_iter()
                    .filter_map(ListedRoom::public_site_id)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Fails with [`ExtractorError::Authentication`] when the signed-in
    /// account reads rooms from Bigo's live list as offline, which Bigo does
    /// to accounts it restricts. Anything that prevents the comparison, such
    /// as an empty list or a failed request, lets the offline result stand.
    async fn confirm_account_sees_live_rooms(
        &self,
        account: u64,
        site_id: &str,
    ) -> Result<(), ExtractorError> {
        if ACCOUNTS_SEEING_LIVE
            .lock()
            .get(&account)
            .is_some_and(|seen| seen.elapsed() < ACCOUNT_TRUSTED_FOR)
        {
            return Ok(());
        }
        let rooms = match self.live_site_ids().await {
            Ok(rooms) => rooms,
            Err(error) => {
                debug!(%error, "bigo live list unavailable; offline result unconfirmed");
                return Ok(());
            }
        };
        let mut offline = 0;
        for room in rooms
            .iter()
            .filter(|room| room.as_str() != site_id)
            .take(PROBE_ROOMS)
        {
            match self.studio(room, None, None).await {
                Ok(data) if data.is_room_live() => {
                    saw_live_room(account);
                    return Ok(());
                }
                Ok(_) => offline += 1,
                Err(error) => {
                    debug!(%error, "bigo listed-room probe failed; offline result unconfirmed");
                    return Ok(());
                }
            }
        }
        if offline == 0 {
            return Ok(());
        }
        warn!(
            rooms = offline,
            "bigo account reads rooms from the live list as offline; Bigo has restricted it"
        );
        Err(ExtractorError::Authentication {
            code: "account_restricted".to_string(),
        })
    }

    async fn extract_media(&self) -> Result<MediaInfo, ExtractorError> {
        let site_id = self.extract_site_id()?;
        debug!(%site_id, "bigo extract");

        let password = self.resolve_password();

        if self.account.is_some() || !self.mint_token {
            match self
                .studio(site_id.as_str(), password.as_deref(), None)
                .await
            {
                // Without a token Bigo may still withhold the playlist of a
                // live room, or refuse the request; only then is a token
                // worth its two extra requests.
                Ok(data)
                    if !(self.mint_token && data.is_room_live() && !data.has_playable_source()) =>
                {
                    return self.media_from_data(data, &site_id);
                }
                Ok(_) => {}
                Err(error @ ExtractorError::ValidationError(_)) if self.mint_token => {
                    debug!(%error, "bigo studio refused the request without a token");
                }
                Err(error) => return Err(error),
            }
        }

        let integrity_token = match token::mint_token(&self.extractor.client).await {
            Ok(t) => {
                debug!("bigo integrity token minted");
                Some(t)
            }
            Err(e) => {
                warn!(error = %e, "bigo integrity token mint failed; falling back to bare studio POST");
                None
            }
        };
        let data = self
            .studio(
                site_id.as_str(),
                password.as_deref(),
                integrity_token.as_deref(),
            )
            .await?;
        self.media_from_data(data, &site_id)
    }
}

#[async_trait]
impl PlatformExtractor for Bigo {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let media = self.extract_media().await?;
        if let Some(account) = self.account {
            if media.is_live {
                saw_live_room(account);
            } else {
                let site_id = self.extract_site_id()?;
                self.confirm_account_sees_live_rooms(account, &site_id)
                    .await?;
            }
        }
        Ok(media)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn locked_offline_response_is_private_with_or_without_password() {
        let client = crate::extractor::default::default_client();
        let locked_data = || StudioData {
            alive: Some(0),
            pass_room: Some(true),
            room_id: Some("0".to_string()),
            hls_src: Some(String::new()),
            ..StudioData::default()
        };

        let without_password = Bigo::new(
            "https://www.bigo.tv/1".to_string(),
            client.clone(),
            None,
            None,
        );
        assert!(matches!(
            without_password.media_from_data(locked_data(), "1"),
            Err(ExtractorError::PrivateContent)
        ));

        let with_password = Bigo::new(
            "https://www.bigo.tv/1".to_string(),
            client,
            None,
            Some(serde_json::json!({"stream_password": "wrong"})),
        );
        assert!(matches!(
            with_password.media_from_data(locked_data(), "1"),
            Err(ExtractorError::PrivateContent)
        ));
    }

    #[test]
    fn public_offline_response_remains_offline() {
        let bigo = Bigo::new(
            "https://www.bigo.tv/1".to_string(),
            crate::extractor::default::default_client(),
            None,
            None,
        );
        let media = bigo
            .media_from_data(
                StudioData {
                    alive: Some(0),
                    pass_room: Some(false),
                    room_id: Some("0".to_string()),
                    hls_src: Some(String::new()),
                    ..StudioData::default()
                },
                "1",
            )
            .expect("public offline room should remain a normal offline result");

        assert!(!media.is_live);
    }

    /// A spent integrity token leaves `alive`/`roomId` live and blanks
    /// `hls_src`; that must not read back as an offline room.
    #[test]
    fn live_room_without_hls_is_an_error() {
        let bigo = Bigo::new(
            "https://www.bigo.tv/1".to_string(),
            crate::extractor::default::default_client(),
            None,
            None,
        );
        assert!(matches!(
            bigo.media_from_data(
                StudioData {
                    alive: Some(1),
                    pass_room: Some(false),
                    room_id: Some("7631598258675132407".to_string()),
                    hls_src: Some(String::new()),
                    ..StudioData::default()
                },
                "1",
            ),
            Err(ExtractorError::ValidationError(_))
        ));
    }

    #[test]
    fn live_locked_room_without_hls_is_private() {
        let bigo = Bigo::new(
            "https://www.bigo.tv/1".to_string(),
            crate::extractor::default::default_client(),
            None,
            None,
        );
        assert!(matches!(
            bigo.media_from_data(
                StudioData {
                    alive: Some(1),
                    pass_room: Some(true),
                    room_id: Some("7631598258675132407".to_string()),
                    hls_src: Some(String::new()),
                    ..StudioData::default()
                },
                "1",
            ),
            Err(ExtractorError::PrivateContent)
        ));
    }

    /// Live integration: mint token + studio API + HLS URL.
    /// Run with:
    ///   cargo test -p platforms-parser bigo::builder::tests::test_live_extract -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_live_extract() {
        use crate::extractor::default::default_client;
        use tracing::Level;

        let _ = tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .try_init();

        let url = "https://www.bigo.tv/221338632";
        let extractor = Bigo::new(url.to_string(), default_client(), None, None);
        let media = extractor.extract().await.expect("extract failed");

        println!(
            "live={} title={:?} artist={:?} streams={}",
            media.is_live,
            media.title,
            media.artist,
            media.streams.len()
        );
        assert!(
            media.is_live,
            "room 221338632 should be live (retry if flaky)"
        );
        assert_eq!(media.streams.len(), 1);
        let stream = &media.streams[0];
        assert_eq!(stream.quality, "live");
        assert!(
            stream.url.contains(".m3u8"),
            "expected HLS url, got {}",
            stream.url
        );
        let extras = media.extras.as_ref().expect("extras");
        let room_id = extras.get("room_id").expect("room_id in extras");
        assert!(!room_id.is_empty() && room_id != "0");
        println!("room_id={room_id} hls={}", stream.url);
    }

    /// Live extract without integrity token (Streamlink-style bare POST).
    #[tokio::test]
    #[ignore]
    async fn test_live_extract_no_token() {
        use crate::extractor::default::default_client;

        let extractor = Bigo::new(
            "https://www.bigo.tv/221338632".to_string(),
            default_client(),
            None,
            Some(serde_json::json!({"mint_token": false})),
        );
        let media = extractor.extract().await.expect("extract failed");
        assert!(media.is_live);
        assert!(!media.streams.is_empty());
        println!(
            "no-token live ok hls={}",
            media.streams.first().map(|s| s.url.as_str()).unwrap_or("")
        );
    }
}
