use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::extractor::error::ExtractorError;
use crate::extractor::hls_extractor::HlsExtractor;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::twitch::models::TwitchResponse;
use crate::extractor::utils::{capture_group_1_or_invalid_url, extras_get_str};
use crate::media::StreamInfo;
use crate::media::media_info::MediaInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rand::Rng;
use regex::Regex;
use reqwest::Client;
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
        let mut extractor = Extractor::new("Twitch", platform_url, client);

        extractor.add_header_typed(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9");
        extractor.add_header_typed(reqwest::header::ACCEPT, "application/vnd.twitchtv.v5+json");
        extractor.set_referer_static(Self::BASE_URL);
        extractor.add_header_str("device-id", Self::get_device_id());
        extractor.add_header_str("Client-Id", "kimne78kx3ncx6brgo4mv6wki5h1ko");

        if let Some(token) = extras_get_str(extras.as_ref(), "oauth_token") {
            extractor.add_header_typed(reqwest::header::AUTHORIZATION, format!("OAuth {token}"));
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
        capture_group_1_or_invalid_url(&URL_REGEX, &self.extractor.url)
    }

    fn build_persisted_query_request(
        &self,
        operation_name: &str,
        sha256_hash: &str,
        variables: serde_json::Value,
    ) -> String {
        serde_json::to_string(&serde_json::json!({
            "operationName": operation_name,
            "extensions": {
                "persistedQuery": {
                    "version": 1,
                    "sha256Hash": sha256_hash,
                }
            },
            "variables": variables,
        }))
        .unwrap_or_else(|_| "{}".to_string())
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
            Err(e) => {
                debug!("Failed to parse as array: {}", e);
                // If parsing as array fails, try parsing as single object
                let single_response: T = serde_json::from_str(&body).map_err(|e2| {
                    debug!("Failed to parse as single object: {}", e2);
                    e2
                })?;
                vec![single_response]
            }
        };

        debug!("responses: {:?}", responses);
        Ok(responses)
    }

    pub async fn get_live_stream_info(&self) -> Result<MediaInfo, ExtractorError> {
        let room_id = self.extract_room_id()?;
        debug!("room_id: {}", room_id);
        let channel_shell_query = self.build_persisted_query_request(
            "ChannelShell",
            "fea4573a7bf2644f5b3f2cbbdcbee0d17312e48d2e55f080589d053aad353f11",
            serde_json::json!({
                "login": room_id,
            }),
        );
        let stream_metadata_query = self.build_persisted_query_request(
            "StreamMetadata",
            "b57f9b910f8cd1a4659d894fe7550ccc81ec9052c01e438b290fd66a040b9b93",
            serde_json::json!({
                "channelLogin": room_id,
                "previewImageURL": "",
                "includeIsDJ": true,
            }),
        );
        let queries_string = format!("[{channel_shell_query},{stream_metadata_query}]");

        debug!("queries_string: {}", queries_string);

        let response = self.post_gql::<TwitchResponse>(queries_string).await?;
        debug!("response: {:?}", response);

        let mut valid_responses = response.iter().filter(|r| r.data.is_some());
        let Some(channel_shell) = valid_responses.next() else {
            return Err(ExtractorError::ValidationError(
                "No valid response from Twitch API".to_string(),
            ));
        };

        // Try to get stream_metadata, if not available use channel_shell data.
        let stream_metadata = valid_responses.next().unwrap_or(channel_shell);

        let user_or_error = &channel_shell
            .data
            .as_ref()
            .and_then(|d| d.user_or_error.as_ref())
            .ok_or_else(|| {
                ExtractorError::ValidationError("Could not find user_or_error".to_string())
            })?;

        // Try to get detailed user info from stream_metadata, fallback to user_or_error data
        let user_opt = stream_metadata.data.as_ref().and_then(|d| d.user.as_ref());

        // Determine if the stream is live
        let is_live = match user_opt {
            Some(user) => {
                user.stream.as_ref().and_then(|s| s.stream_type.as_deref()) == Some("live")
            }
            None => {
                // Fallback to user_or_error stream info
                user_or_error.stream.is_some()
            }
        };

        let artist = user_or_error.display_name.clone();

        // Get title from user's last_broadcast if available, otherwise use empty string
        let title = user_opt
            .and_then(|u| u.last_broadcast.as_ref())
            .and_then(|l| l.title.clone())
            .unwrap_or_default();

        // Get profile image URL, prefer from user_or_error
        let avatar_url = user_or_error.profile_image_url.clone();

        let stream = user_opt
            .and_then(|u| u.stream.as_ref())
            .or(user_or_error.stream.as_ref());

        let category = stream
            .and_then(|s| s.game.as_ref())
            .map(|g| vec![g.name.clone()]);

        let live_start_time = stream
            .and_then(|s| s.created_at.as_deref())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        if !is_live || self.skip_live_extraction {
            return Ok(MediaInfo::builder(Self::BASE_URL, title, artist)
                .category_opt(category)
                .live_start_time_opt(live_start_time)
                .artist_url(avatar_url)
                .is_live(is_live)
                .build());
        }

        let streams = self.get_streams(room_id).await?;

        Ok(MediaInfo::builder(Self::BASE_URL, title, artist)
            .category_opt(category)
            .live_start_time_opt(live_start_time)
            .artist_url(avatar_url)
            .is_live(is_live)
            .streams(streams)
            .headers(self.extractor.get_platform_headers_map())
            .build())
    }

    pub async fn get_streams(&self, rid: &str) -> Result<Vec<StreamInfo>, ExtractorError> {
        let live_gpl = self.build_persisted_query_request(
            "PlaybackAccessToken",
            "ed230aa1e33e07eebb8928504583da78a5173989fadfb1ac94be06a04f3cdbe9",
            serde_json::json!({
                "isLive": true,
                "login": rid,
                "isVod": false,
                "vodID": "",
                "playerType": "site",
                "isClip": false,
                "clipID": "",
                "platform" : "site",
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
            .extract_hls_stream_with_params(
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
    #[ignore]
    async fn test_get_live_stream_info() {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .with_test_writer()
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
