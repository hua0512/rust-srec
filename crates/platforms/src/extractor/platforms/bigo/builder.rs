//! Bigo Live stream extractor.
//!
//! Resolves `siteId` via `getInternalStudioInfo` to a single HLS media playlist.
//! Mints an integrity token by default (website parity) and supports password rooms.

use std::sync::LazyLock;

use async_trait::async_trait;
use md5::{Digest, Md5};
use regex::Regex;
use reqwest::Client;
use rustc_hash::FxHashMap;
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

pub struct Bigo {
    extractor: Extractor,
    stream_password: Option<String>,
    mint_token: bool,
}

impl Bigo {
    const BASE_URL: &str = "https://www.bigo.tv";
    const STUDIO_API: &str = "https://ta.bigo.tv/official_website/studio/getInternalStudioInfo";

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("bigo", url, client);
        extractor.set_origin_and_referer_static(Self::BASE_URL);
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

    fn media_from_data(
        &self,
        data: StudioData,
        site_id: &str,
    ) -> Result<MediaInfo, ExtractorError> {
        let title = data.display_title();
        let artist = data.artist();
        let cover = data.snapshot.clone().filter(|s| !s.is_empty());
        let artist_url = data.avatar.clone().filter(|s| !s.is_empty());

        if !data.is_online() {
            // Protected offline-looking response without pwd is still PrivateContent
            // when the API marks passRoom and we have no password.
            if data.pass_room.unwrap_or(false) && self.resolve_password().is_none() {
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

        if data.pass_room.unwrap_or(false) && self.resolve_password().is_none() {
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
        if let Some(viewers) = data.reserver {
            extras.insert("viewer_count".to_string(), viewers.to_string());
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
}

#[async_trait]
impl PlatformExtractor for Bigo {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let site_id = self.extract_site_id()?;
        debug!(%site_id, "bigo extract");

        let password = self.resolve_password();

        let integrity_token = if self.mint_token {
            match token::mint_token(&self.extractor.client).await {
                Ok(t) => {
                    debug!("bigo integrity token minted");
                    Some(t)
                }
                Err(e) => {
                    warn!(error = %e, "bigo integrity token mint failed; falling back to bare studio POST");
                    None
                }
            }
        } else {
            None
        };

        let response = self
            .fetch_studio(
                site_id.as_str(),
                password.as_deref(),
                integrity_token.as_deref(),
            )
            .await?;

        if !response.is_success() {
            return Err(ExtractorError::ValidationError(format!(
                "bigo studio API error: code={:?} msg={:?}",
                response.code, response.msg
            )));
        }

        let data = response.data.unwrap_or_default();
        self.media_from_data(data, &site_id)
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
    fn mint_token_default_true() {
        let client = crate::extractor::default::default_client();
        let b = Bigo::new("https://www.bigo.tv/1".into(), client.clone(), None, None);
        assert!(b.mint_token);

        let b = Bigo::new(
            "https://www.bigo.tv/1".into(),
            client,
            None,
            Some(serde_json::json!({"mint_token": false})),
        );
        assert!(!b.mint_token);
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
