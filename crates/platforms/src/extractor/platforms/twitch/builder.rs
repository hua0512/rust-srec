use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::extractor::error::ExtractorError;
use crate::extractor::hls_extractor::HlsExtractor;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::twitch::models::TwitchResponse;
use crate::media::StreamInfo;
use crate::media::media_info::MediaInfo;
use async_trait::async_trait;
use rand::Rng;
use regex::Regex;
use reqwest::Client;
use rustc_hash::FxHashMap;
use tracing::debug;

pub static URL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^https?://(?:www\.)?twitch\.tv/([^/?#]+)").unwrap());

pub struct Twitch {
    extractor: Extractor,
    skip_live_extraction: bool,
}

impl Twitch {
    const BASE_URL: &str = "https://www.twitch.tv";

    pub fn new(
        platform_url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("Twitch".to_string(), platform_url, client);

        extractor.add_header(
            reqwest::header::ACCEPT_LANGUAGE.to_string(),
            "en-US,en;q=0.9",
        );
        extractor.add_header(
            reqwest::header::ACCEPT.to_string(),
            "application/vnd.twitchtv.v5+json",
        );
        extractor.add_header(reqwest::header::REFERER.to_string(), Self::BASE_URL);
        extractor.add_header("device-id", Self::get_device_id());
        extractor.add_header("Client-Id", "kimne78kx3ncx6brgo4mv6wki5h1ko");

        if let Some(extras) = extras {
            extractor.add_header(
                reqwest::header::AUTHORIZATION.to_string(),
                format!("OAuth {}", extras.get("oauth_token").unwrap()),
            );
        }

        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        }
        Self {
            extractor,
            skip_live_extraction: false,
        }
    }

    fn get_device_id() -> String {
        // random device id of 16 digits
        let device_id = format!(
            "{}",
            rand::rng().random_range(1000000000000000i64..9999999999999999i64)
        );
        device_id
    }

    pub fn extract_room_id(&self) -> Result<&str, ExtractorError> {
        let url =
            URL_REGEX
                .captures(&self.extractor.url)
                .ok_or(ExtractorError::ValidationError(
                    "Twitch URL is invalid".to_string(),
                ))?;
        let room_id = url.get(1).ok_or(ExtractorError::ValidationError(
            "Twitch URL is invalid".to_string(),
        ))?;
        Ok(room_id.as_str())
    }

    fn build_persisted_query_request(
        &self,
        operation_name: &str,
        sha256_hash: &str,
        variables: serde_json::Value,
    ) -> String {
        let query = format!(
            r#"
        {{  
         "operationName": "{operation_name}",
            "extensions": {{
                "persistedQuery": {{
                "version": 1,
                "sha256Hash": "{sha256_hash}"
            }}
        }},
            "variables": {variables}
        }}
        "#,
            operation_name = operation_name,
            sha256_hash = sha256_hash,
            variables = serde_json::to_string(&variables).unwrap()
        );
        query.trim().to_string()
    }

    const GPL_API_URL: &str = "https://gql.twitch.tv/gql";

    async fn post_gql<T: for<'de> serde::Deserialize<'de> + std::fmt::Debug>(
        &self,
        body: String,
    ) -> Result<Vec<T>, ExtractorError> {
        let response = self
            .extractor
            .post(Self::GPL_API_URL)
            .body(body)
            .send()
            .await?;
        let body = response.text().await?;
        debug!("body: {}", body);

        // Try to parse as array first, then as single object if that fails
        let responses: Vec<T> = match serde_json::from_str::<Vec<T>>(&body) {
            Ok(responses) => responses,
            Err(_) => {
                // If parsing as array fails, try parsing as single object
                let single_response: T = serde_json::from_str(&body)?;
                vec![single_response]
            }
        };

        debug!("responses: {:?}", responses);
        Ok(responses)
    }

    #[allow(clippy::too_many_arguments)]
    fn create_media_info(
        &self,
        title: String,
        artist: String,
        artist_url: Option<String>,
        cover_url: Option<String>,
        is_live: bool,
        streams: Vec<StreamInfo>,
        extras: Option<FxHashMap<String, String>>,
    ) -> MediaInfo {
        MediaInfo {
            site_url: Self::BASE_URL.to_string(),
            title,
            artist,
            artist_url,
            cover_url,
            is_live,
            streams,
            extras,
        }
    }

    pub async fn get_live_stream_info(&self) -> Result<MediaInfo, ExtractorError> {
        let room_id = self.extract_room_id()?;
        debug!("room_id: {}", room_id);
        let queries = [
            self.build_persisted_query_request(
                "ChannelShell",
                "c3ea5a669ec074a58df5c11ce3c27093fa38534c94286dc14b68a25d5adcbf55",
                serde_json::json!({
                    "login": room_id,
                    "lcpVideosEnabled": false,
                }),
            ),
            self.build_persisted_query_request(
                "StreamMetadata",
                "059c4653b788f5bdb2f5a2d2a24b0ddc3831a15079001a3d927556a96fb0517f",
                serde_json::json!({
                    "channelLogin": room_id,
                    "previewImageURL": "",
                }),
            ),
        ];
        let queries_string = format!(
            "[{}]",
            queries
                .iter()
                .map(|q| q.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        debug!("queries_string: {}", queries_string);

        let response = self.post_gql::<TwitchResponse>(queries_string).await?;
        debug!("response: {:?}", response);

        if response.len() < 2 {
            return Err(ExtractorError::ValidationError(
                "Invalid response from Twitch API".to_string(),
            ));
        }

        let channel_shell = response.first().unwrap();
        let stream_metadata = response.get(1).unwrap();

        let user_or_error = &channel_shell.data.user_or_error.as_ref().ok_or_else(|| {
            ExtractorError::ValidationError("Could not find user_or_error".to_string())
        })?;

        let user =
            &stream_metadata.data.user.as_ref().ok_or_else(|| {
                ExtractorError::ValidationError("Could not find user".to_string())
            })?;

        let is_live = user.stream.is_some()
            && user.stream.as_ref().unwrap().stream_type == Some("live".to_string());

        let artist = user_or_error.display_name.to_string();
        let last_broadcast = user.last_broadcast.as_ref();
        let title = last_broadcast
            .map(|l| l.title.to_string())
            .unwrap_or_default();
        let avatar_url = user.profile_image_url.to_string();

        if !is_live || self.skip_live_extraction {
            return Ok(self.create_media_info(
                title,
                artist,
                Some(avatar_url),
                None,
                is_live,
                vec![],
                None,
            ));
        }

        let streams = self.get_streams(room_id).await?;

        Ok(self.create_media_info(
            title,
            artist,
            Some(avatar_url),
            None,
            is_live,
            streams,
            Some(self.extractor.get_platform_headers_map()),
        ))
    }

    pub async fn get_streams(&self, rid: &str) -> Result<Vec<StreamInfo>, ExtractorError> {
        let live_gpl = self.build_persisted_query_request(
            "PlaybackAccessToken",
            "0828119ded1c13477966434e15800ff57ddacf13ba1911c129dc2200705b0712",
            serde_json::json!({
                "isLive": true,
                "login": rid,
                "isVod": false,
                "vodID": "",
                "playerType": "site",
                "isClip": false,
                "clipID": ""
            }),
        );

        let response = self.post_gql::<serde_json::Value>(live_gpl).await?;
        let stream_playback_access_token = response
            .first()
            .and_then(|data| {
                data.get("data")
                    .and_then(|data| data.get("streamPlaybackAccessToken"))
            })
            .ok_or_else(|| {
                ExtractorError::ValidationError(
                    "Could not find streamPlaybackAccessToken".to_string(),
                )
            })?;

        let playback_token = stream_playback_access_token.get("value").ok_or_else(|| {
            ExtractorError::ValidationError("Could not find token value".to_string())
        })?;
        let signature = stream_playback_access_token
            .get("signature")
            .ok_or_else(|| {
                ExtractorError::ValidationError("Could not find signature".to_string())
            })?;

        let m3u8_url = format!("https://usher.ttvnw.net/api/channel/hls/{rid}.m3u8");

        let epoch_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let epoch_seconds_str = epoch_seconds.to_string();

        let headers = self.extractor.get_platform_headers();
        let streams = self
            .extract_hls_stream(
                &self.extractor.client,
                Some(headers.clone()),
                Some(&[
                    ("player", "twitchweb"),
                    ("p", &epoch_seconds_str),
                    ("allow_source", "true"),
                    ("allow_audio_only", "true"),
                    ("allow_spectre", "true"),
                    ("fast_bread", "true"),
                    ("token", playback_token.as_str().unwrap_or("")),
                    ("sig", signature.as_str().unwrap_or("")),
                ]),
                &m3u8_url,
                None,
                None,
            )
            .await?;

        // debug!("response: {:?}", response);
        Ok(streams)
    }
}

impl HlsExtractor for Twitch {}

#[async_trait]
impl PlatformExtractor for Twitch {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let media_info = self.get_live_stream_info().await?;
        Ok(media_info)
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use crate::extractor::{default::default_client, platforms::twitch::builder::Twitch};

    #[tokio::test]
    async fn test_get_live_stream_info() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .init();
        let twitch = Twitch::new(
            "https://www.twitch.tv/abby_".to_string(),
            default_client(),
            None,
            None,
        );
        let media_info = twitch.get_live_stream_info().await.unwrap();
        println!("{media_info:?}");
    }
}
