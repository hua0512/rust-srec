//! Bigo Live stream extractor.
//!
//! Live status comes from the public profile API, `getUserInfoStudio`. It
//! needs no login and is sent no cookies, so Bigo's login wall cannot refuse
//! it, and an offline check is that one request.
//!
//! A live room lists up to two streams, in this order:
//!
//! - `native`: the broadcaster's own stream from BIGO's app servers, as a
//!   [`NativeRoom`] URL that only mesio records. Its [`get_url`] looks the
//!   room up as a guest and succeeds only for a single host with video and
//!   audio servers. It is listed only when the caller gave the route native
//!   connections take, `native_stream` is on and no room password is set.
//! - `web`: the website's HLS playlist. Its URL is filled in by [`get_url`]
//!   from `getInternalStudioInfo`, with the account's cookies when one is
//!   selected and the room password when one is set, so the studio, and any
//!   account, is asked only when the web stream is tried.
//!
//! Bigo restricts some accounts without signing them out: the studio then
//! answers every room as offline. When the studio reads a room as offline that
//! the profile reports live, a room the guest lookup finds ended is offline.
//! Otherwise the profile and the studio are asked again a few seconds later,
//! as they can disagree for a moment around a stream's start and end, and the
//! lookup keeps finding a room for a while after it ends. If they still
//! disagree about a room that exists (the lookup found it, or without a
//! lookup the profile still links it), a signed-in check fails with
//! `account_restricted`, so the account is set aside. A room the profile marks
//! as broadcast for the app only is the exception: the website has no stream
//! for it whatever the account, so that check fails without blaming the
//! account.
//!
//! [`get_url`]: PlatformExtractor::get_url

use std::borrow::Cow;
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use md5::{Digest, Md5};
use parking_lot::Mutex;
use regex::Regex;
use reqwest::Client;
use rustc_hash::FxHashMap;
use tokio::sync::OnceCell;
use tracing::debug;
use url::Url;

use crate::danmaku::DanmuProxy;
use crate::digest_to_hex;
use crate::extractor::error::ExtractorError;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::extractor::platforms::bigo::metadata::metadata;
use crate::extractor::platforms::bigo::models::{
    Presence, PresenceResponse, StudioData, StudioResponse,
};
use crate::extractor::platforms::bigo::native::stream::{self, NativeRoom, RoomKind};
use crate::extractor::platforms::bigo::native::{NativeError, Route};
use crate::extractor::utils::{capture_group_1_owned, extras_get_bool, extras_get_str};
use crate::media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo};

pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?bigo\.tv/(?:[a-z]{2}/)?([^/?#]+)").unwrap()
});

const PRESENCE_API: &str = "https://ta.bigo.tv/official_website/OUserCenter/getUserInfoStudio";
const STUDIO_API: &str = "https://ta.bigo.tv/official_website/studio/getInternalStudioInfo";
/// The profile API's code for an id that names no account.
const UNKNOWN_ID: i64 = 1;
/// How long to wait before asking the profile and the studio again when the
/// studio reads a live room as offline. Around a stream's start and end the
/// two can disagree for a moment.
const RECHECK_DELAY: Duration = Duration::from_secs(5);
/// How long room lookups skip a route on which BIGO could not be reached or
/// turned a lookup away. A network that drops them would otherwise hold every
/// live check for the lookup's timeouts, about 30 s, before the web stream,
/// and one that BIGO refuses would only be refused again.
const UNREACHABLE_FOR: Duration = Duration::from_secs(5 * 60);

/// Routes, by their tunnel (`None`: direct), on which BIGO could not be
/// reached or turned a room lookup away, with when.
static UNREACHABLE: Mutex<Vec<(Option<DanmuProxy>, std::time::Instant)>> = Mutex::new(Vec::new());

/// Whether BIGO could not be reached or turned a room lookup away on `route`
/// in the last 5 minutes.
fn unreachable_recently(route: &Route) -> bool {
    UNREACHABLE
        .lock()
        .iter()
        .any(|(proxy, at)| *proxy == route.proxy && at.elapsed() < UNREACHABLE_FOR)
}

/// The room lookup `probe` on `route`. One that cannot reach BIGO's app
/// servers, that they turn away for its address, or whose ticket request the
/// website refuses makes later lookups skip the route for a while.
async fn lookup_on(
    route: &Route,
    probe: impl Future<Output = Result<RoomKind, NativeError>>,
) -> Lookup {
    if unreachable_recently(route) {
        return Lookup::Failed(
            "BIGO could not be reached or refused a lookup a moment ago".to_owned(),
        );
    }
    match probe.await {
        Ok(kind) => Lookup::Room(kind),
        Err(NativeError::RoomEnded) => Lookup::Ended,
        Err(error) => {
            if matches!(
                error,
                NativeError::Io(_)
                    | NativeError::Timeout(_)
                    | NativeError::IpRestricted
                    | NativeError::TicketRefused(_)
            ) {
                let mut routes = UNREACHABLE.lock();
                routes
                    .retain(|(proxy, at)| *proxy != route.proxy && at.elapsed() < UNREACHABLE_FOR);
                routes.push((route.proxy.clone(), std::time::Instant::now()));
            }
            Lookup::Failed(error.to_string())
        }
    }
}

/// Both streams carry the same quality label, so a quality preference never
/// outranks a format preference between them: `preferred_formats` alone
/// chooses, and the native stream comes first otherwise.
const QUALITY: &str = "live";
/// The web stream's extra naming its room, as a decimal string: room ids
/// exceed the integers JavaScript holds exactly, and the player sends the
/// stream back to have its URL resolved.
const ROOM_ID_EXTRA: &str = "room_id";

/// What the guest room lookup of a check said.
#[derive(Debug)]
enum Lookup {
    Room(RoomKind),
    Ended,
    /// The lookup failed, for this reason.
    Failed(String),
}

pub struct Bigo {
    extractor: Extractor,
    stream_password: Option<String>,
    /// Cookies were supplied, so studio requests carry a login.
    signed_in: bool,
    /// `native_stream`: whether to offer the native stream (default on).
    native_stream: bool,
    /// How native connections reach BIGO. Without it the native stream is
    /// not offered and rooms cannot be looked up.
    native_route: Option<Route>,
    /// The room lookup of this check, shared by its streams, so that a check
    /// signs in to Linkd once.
    lookup: OnceCell<Lookup>,
    presence_api: Cow<'static, str>,
    studio_api: Cow<'static, str>,
    recheck_delay: Duration,
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
            native_stream: extras_get_bool(extras.as_ref(), "native_stream").unwrap_or(true),
            native_route: None,
            lookup: OnceCell::new(),
            presence_api: Cow::Borrowed(PRESENCE_API),
            studio_api: Cow::Borrowed(STUDIO_API),
            recheck_delay: RECHECK_DELAY,
        }
    }

    /// Lets the extractor offer the native stream and look rooms up, with
    /// native connections going through `route`.
    pub fn with_native_route(mut self, route: Route) -> Self {
        self.native_route = Some(route);
        self
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

    /// Asks the profile API about `site_id`.
    ///
    /// The request goes through the client directly, never through
    /// [`Extractor::request`], which would add the account's cookies: whether
    /// a room is live must not depend on the state of the account's login.
    async fn fetch_presence(&self, site_id: &str) -> Result<PresenceResponse, ExtractorError> {
        let response = self
            .extractor
            .client
            .get(self.presence_api.as_ref())
            .headers(self.extractor.get_platform_headers().clone())
            .query(&[("bigoId", site_id)])
            .send()
            .await?;
        Ok(ExtractorError::check_response(response)?
            .json::<PresenceResponse>()
            .await?)
    }

    async fn presence(&self, site_id: &str) -> Result<Presence, ExtractorError> {
        let mut response = self.fetch_presence(site_id).await?;
        // A not-found result stops monitoring the streamer, and Bigo answers
        // this code with a generic "paramters invalid", so one answer is not
        // enough.
        if response.code == UNKNOWN_ID {
            response = self.fetch_presence(site_id).await?;
        }
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

    /// Looks `room_id` up as a guest, once per check. `None` without a route.
    async fn lookup(&self, room_id: u64) -> Option<&Lookup> {
        let route = self.native_route.as_ref()?;
        Some(
            self.lookup
                .get_or_init(|| lookup_on(route, stream::probe(room_id, route)))
                .await,
        )
    }

    /// Title, artist, avatar and identity, all from the profile.
    fn media_info(&self, presence: &Presence, site_id: &str) -> MediaInfo {
        let nickname = presence.nickname.as_deref().filter(|s| !s.is_empty());
        let title = presence
            .room_topic
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(nickname)
            .unwrap_or(site_id);
        let mut media = MediaInfo::builder(Self::BASE_URL, title, nickname.unwrap_or(site_id))
            .artist_url_opt(presence.og_image.clone().filter(|s| !s.is_empty()))
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

    /// The live result for a room the profile reports live.
    fn live_media(&self, presence: &Presence, site_id: &str) -> Result<MediaInfo, ExtractorError> {
        let room_id = presence.room_id().ok_or_else(|| {
            ExtractorError::ValidationError(format!(
                "bigo profile reports room {site_id} live without naming the room"
            ))
        })?;
        let password = self.resolve_password();

        let mut streams = Vec::with_capacity(2);
        let owner_uid = u32::try_from(presence.uid).ok().filter(|&uid| uid != 0);
        if let Some(owner_uid) = owner_uid
            && self.native_stream
            && self.native_route.is_some()
            && password.is_none()
        {
            let room = NativeRoom { room_id, owner_uid };
            streams.push(
                StreamInfo::builder(room.url(), StreamFormat::Flv, MediaFormat::Flv)
                    .quality(QUALITY)
                    .priority(0)
                    .build(),
            );
        }
        streams.push(
            StreamInfo::builder("", StreamFormat::Hls, MediaFormat::Ts)
                .quality(QUALITY)
                .priority(1)
                .is_headers_needed(true)
                .extras(serde_json::json!({ ROOM_ID_EXTRA: room_id.to_string() }))
                .build(),
        );

        let mut extras = FxHashMap::default();
        // The danmu provider enters the chat room by this id.
        extras.insert("room_id".to_string(), room_id.to_string());
        extras.insert("site_id".to_string(), site_id.to_string());
        extras.insert("uid".to_string(), presence.uid.to_string());
        if let Some(pwd) = password {
            // The danmu provider sends it as the room's secret key.
            extras.insert("stream_password".to_string(), pwd);
        }

        let mut media = self.media_info(presence, site_id);
        media.is_live = true;
        media.streams = streams;
        media.extras = Some(extras);
        Ok(media)
    }

    /// Whether the native stream of `room` can be recorded.
    async fn check_native(&self, room: NativeRoom) -> Result<(), ExtractorError> {
        let unavailable = |reason: String| {
            debug!(room_id = room.room_id, %reason, "bigo native stream unavailable");
            Err(ExtractorError::Other(format!(
                "bigo native stream unavailable: {reason}"
            )))
        };
        if stream::recently_failed(room.owner_uid) {
            return unavailable("it failed to record recently".to_owned());
        }
        match self.lookup(room.room_id).await {
            Some(Lookup::Room(RoomKind::Host)) => Ok(()),
            Some(Lookup::Ended) => Err(ExtractorError::NoStreamsFound),
            Some(Lookup::Room(kind)) => unavailable(format!("the room is {kind:?}")),
            Some(Lookup::Failed(reason)) => unavailable(format!("room lookup failed: {reason}")),
            None => unavailable("no route for native connections".to_owned()),
        }
    }

    /// The playlist URL in a studio answer for a room the profile reports
    /// live. `None` means the studio reads the room as offline.
    fn web_url(data: StudioData, password: Option<&str>) -> Result<Option<String>, ExtractorError> {
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
                return Err(ExtractorError::ValidationError(
                    "bigo studio response for a live room omitted hls_src".to_string(),
                ));
            }
            return Ok(None);
        }
        Ok(data.hls_src)
    }

    /// Fills in the web stream's URL from the studio.
    async fn resolve_web(&self, stream: &mut StreamInfo) -> Result<(), ExtractorError> {
        let site_id = self.extract_site_id()?;
        let password = self.resolve_password();
        let password = password.as_deref();
        if let Some(url) = Self::web_url(self.studio(&site_id, password).await?, password)? {
            stream.url = url;
            return Ok(());
        }

        // The studio reads a room the profile reports live as offline. A room
        // the guest lookup finds ended is offline.
        let room_id = extras_get_str(stream.extras.as_ref(), ROOM_ID_EXTRA)
            .and_then(|room_id| room_id.parse::<u64>().ok());
        let lookup = match room_id {
            Some(room_id) => self.lookup(room_id).await,
            None => None,
        };
        if let Some(Lookup::Ended) = lookup {
            return Err(ExtractorError::NoStreamsFound);
        }

        // Otherwise both are asked again: around a stream's start or end the
        // profile and the studio can disagree for a while, and the lookup
        // finds a room for a while after it ends.
        tokio::time::sleep(self.recheck_delay).await;
        let presence = self.presence(&site_id).await?;
        if presence.alive != 1 {
            return Err(ExtractorError::NoStreamsFound);
        }
        if let Some(url) = Self::web_url(self.studio(&site_id, password).await?, password)? {
            stream.url = url;
            return Ok(());
        }
        // The website reads a room broadcast for the app only as offline,
        // whatever the account.
        if presence.is_app_only() {
            return Err(ExtractorError::Other(format!(
                "bigo room {site_id} is broadcast for the app only and has no web stream"
            )));
        }
        // Without a lookup, a live profile without a link to its room is not
        // definite enough to set an account aside.
        let room_exists = match lookup {
            Some(Lookup::Room(_)) => true,
            _ => presence.room_id().is_some(),
        };
        if room_exists {
            return Err(self.studio_disagrees(&site_id));
        }
        Err(ExtractorError::ValidationError(format!(
            "bigo profile reports room {site_id} live, but the studio reads it as offline"
        )))
    }

    /// The error for a room that is live while the studio reads it as
    /// offline: a restricted account when signed in.
    fn studio_disagrees(&self, site_id: &str) -> ExtractorError {
        if self.signed_in {
            ExtractorError::Authentication {
                code: "account_restricted".to_string(),
            }
        } else {
            ExtractorError::ValidationError(format!(
                "bigo room {site_id} is live, but the studio reads it as offline"
            ))
        }
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
            // The broadcast that failed to record natively is over.
            if let Ok(owner_uid) = u32::try_from(presence.uid) {
                stream::forget_failure(owner_uid);
            }
            return Ok(self.offline_media(&presence, &site_id));
        }
        self.live_media(&presence, &site_id)
    }

    async fn get_url(&self, stream: &mut StreamInfo) -> Result<(), ExtractorError> {
        if let Some(room) = NativeRoom::parse(&stream.url) {
            return self.check_native(room).await;
        }
        if !stream.url.is_empty() {
            return Ok(());
        }
        self.resolve_web(stream).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::default::default_client;
    use crate::extractor::test_support::{ReceivedRequests, serve_in_order};

    const ROOM_ID: u64 = 7000000000000000002;
    const LIVE: &str = r#"{"code":0,"msg":"success","data":{"uid":300000003,
        "nickname":"Streamer","roomTopic":"","alive":1,"ogImage":"https://esx.bigo.sg/profile_avatar.jpg",
        "aliOSUrl":"bigolive://livevideoshow?roomid=7000000000000000002&uid=300000003&visitor=1"}}"#;
    /// Live, broadcast for the app only.
    const LIVE_APP_ONLY: &str = r#"{"code":0,"msg":"success","data":{"uid":300000003,
        "nickname":"Streamer","alive":1,"roomStatus":4,
        "aliOSUrl":"bigolive://livevideoshow?roomid=7000000000000000002&uid=300000003&visitor=1"}}"#;
    /// Live, but without a link to the room.
    const LIVE_WITHOUT_ROOM: &str = r#"{"code":0,"msg":"success","data":{"uid":300000003,
        "nickname":"Streamer","alive":1,"aliOSUrl":"bigolive://livevideoshow?roomid=0&uid=300000003"}}"#;
    /// Just ended: the link still names the last room.
    const OFFLINE: &str = r#"{"code":0,"msg":"success","data":{"uid":300000003,
        "nickname":"Streamer","roomTopic":"Back soon","alive":0,"ogImage":"https://esx.bigo.sg/profile_avatar.jpg",
        "aliOSUrl":"bigolive://livevideoshow?roomid=7000000000000000002&uid=300000003&visitor=1"}}"#;
    const STUDIO_LIVE: &str = r#"{"code":0,"msg":"success","data":{"sid":2478571522,
        "uid":300000003,"alive":1,"roomId":"7000000000000000002","needLogin":false,"passRoom":false,
        "hls_src":"https://cdn.example.com/list_x.m3u8","avatar":"https://esx.bigo.sg/avatar.jpg"}}"#;
    const STUDIO_OFFLINE: &str = r#"{"code":0,"msg":"success","data":{"sid":2478571522,
        "uid":300000003,"alive":0,"roomId":"0","hls_src":"","needLogin":false,"passRoom":false}}"#;
    const STUDIO_NEED_LOGIN: &str = r#"{"code":0,"msg":"success","data":{"sid":2478571522,
        "uid":300000003,"alive":0,"roomId":"0","hls_src":"","needLogin":true,"passRoom":false}}"#;
    const STUDIO_LOCKED: &str = r#"{"code":0,"msg":"success","data":{"sid":2478571522,
        "uid":300000003,"alive":0,"roomId":"0","hls_src":"","needLogin":false,"passRoom":true}}"#;
    /// Locked, yet carrying a playlist.
    const STUDIO_LOCKED_WITH_HLS: &str = r#"{"code":0,"msg":"success","data":{"alive":1,
        "roomId":"7000000000000000002","hls_src":"https://cdn.example.com/list_x.m3u8",
        "needLogin":false,"passRoom":true}}"#;
    /// Live by `alive`/`roomId`, but without a playlist.
    const STUDIO_LIVE_WITHOUT_HLS: &str = r#"{"code":0,"msg":"success","data":{"alive":1,
        "roomId":"7000000000000000002","hls_src":"","needLogin":false,"passRoom":false}}"#;
    const ACCOUNT: &str = "uniqid=u; tid=t; yyuid=1";
    const PLAYLIST: &str = "https://cdn.example.com/list_x.m3u8";

    /// An extractor whose profile requests are answered by `presence` and
    /// studio requests by `studio`, in order, without the recheck delay. A
    /// request beyond those fails. With `lookup`, it has a native route and
    /// that lookup answer, so it never reaches BIGO. Also returns the profile
    /// requests received.
    async fn bigo_received(
        presence: &[&'static str],
        studio: &[&'static str],
        cookies: Option<&str>,
        extras: Option<serde_json::Value>,
        lookup: Option<Lookup>,
    ) -> (Bigo, ReceivedRequests) {
        let ok = |bodies: &[&'static str]| -> Vec<_> {
            bodies.iter().map(|body| ("200 OK", *body)).collect()
        };
        let mut bigo = Bigo::new(
            "https://www.bigo.tv/400500600".to_string(),
            default_client(),
            cookies.map(str::to_owned),
            extras,
        );
        let (presence_api, received) = serve_in_order(ok(presence)).await;
        bigo.presence_api = Cow::Owned(presence_api);
        bigo.studio_api = Cow::Owned(serve_in_order(ok(studio)).await.0);
        bigo.recheck_delay = Duration::ZERO;
        if let Some(lookup) = lookup {
            bigo.native_route = Some(Route {
                client: default_client(),
                proxy: None,
            });
            bigo.lookup = OnceCell::new_with(Some(lookup));
        }
        (bigo, received)
    }

    async fn bigo(
        presence: &[&'static str],
        studio: &[&'static str],
        cookies: Option<&str>,
        extras: Option<serde_json::Value>,
        lookup: Option<Lookup>,
    ) -> Bigo {
        bigo_received(presence, studio, cookies, extras, lookup)
            .await
            .0
    }

    async fn extract(bigo: &Bigo) -> Result<MediaInfo, ExtractorError> {
        tokio::time::timeout(Duration::from_secs(10), bigo.extract())
            .await
            .expect("local Bigo requests timed out")
    }

    /// Resolves the web stream of a live check, as the monitor does when the
    /// native stream is not used, returning its URL.
    async fn web_url(bigo: &Bigo) -> Result<String, ExtractorError> {
        let media = extract(bigo).await?;
        let mut web = media
            .streams
            .into_iter()
            .find(|stream| stream.stream_format == StreamFormat::Hls)
            .expect("a web stream");
        tokio::time::timeout(Duration::from_secs(10), bigo.get_url(&mut web))
            .await
            .expect("local Bigo requests timed out")?;
        Ok(web.url)
    }

    async fn check_native(bigo: &Bigo, room: NativeRoom) -> Result<(), ExtractorError> {
        let mut native =
            StreamInfo::builder(room.url(), StreamFormat::Flv, MediaFormat::Flv).build();
        bigo.get_url(&mut native).await
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

    #[tokio::test]
    async fn profile_request_carries_no_account_cookies() {
        let (bigo, received) = bigo_received(&[OFFLINE], &[], Some(ACCOUNT), None, None).await;
        extract(&bigo).await.unwrap();

        let request = received.lock().unwrap()[0].to_ascii_lowercase();
        assert!(request.starts_with("get /?bigoid=400500600 "), "{request}");
        assert!(!request.contains("\r\ncookie:"), "{request}");
    }

    #[tokio::test]
    async fn offline_profile_answers_without_the_studio() {
        // The studio has no answers, so asking it would fail the check.
        let bigo = bigo(&[OFFLINE], &[], Some(ACCOUNT), None, None).await;
        let media = extract(&bigo).await.unwrap();

        assert!(!media.is_live);
        assert_eq!(media.title, "Back soon");
        assert_eq!(media.artist, "Streamer");
        assert_eq!(
            media.artist_url.as_deref(),
            Some("https://esx.bigo.sg/profile_avatar.jpg")
        );
        assert_eq!(extra(&media, "room_id"), None);
        assert_eq!(metadata_uid(&media), "300000003");
    }

    #[tokio::test]
    async fn profile_codes_map_to_outcomes() {
        let unknown = r#"{"code":1,"data":null,"msg":"paramters invalid"}"#;
        let refused = r#"{"code":80009,"data":null,"msg":"empty bigoId or sid"}"#;

        let bigo1 = bigo(&[unknown, unknown], &[], None, None, None).await;
        let error = extract(&bigo1).await.unwrap_err();
        assert!(
            matches!(error, ExtractorError::StreamerNotFound),
            "{error:?}"
        );
        // One unknown-id answer is asked again before the streamer is given up.
        let bigo2 = bigo(&[unknown, OFFLINE], &[], None, None, None).await;
        assert!(!extract(&bigo2).await.unwrap().is_live);
        let bigo3 = bigo(&[refused], &[], None, None, None).await;
        let error = extract(&bigo3).await.unwrap_err();
        assert!(
            matches!(error, ExtractorError::ValidationError(_)),
            "{error:?}"
        );
    }

    /// A live check lists the native stream first and the web stream after
    /// it, without asking the studio; the native stream is left out without
    /// a native route, with `native_stream` off and for a password.
    #[tokio::test]
    async fn live_room_lists_native_then_web() {
        let host = || Some(Lookup::Room(RoomKind::Host));
        // No studio answers: listing the streams must not ask it.
        let bigo1 = bigo(&[LIVE], &[], None, None, host()).await;
        let media = extract(&bigo1).await.unwrap();
        assert!(media.is_live);
        let urls: Vec<_> = media.streams.iter().map(|s| s.url.as_str()).collect();
        assert_eq!(urls, ["bigo://room/7000000000000000002?uid=300000003", ""]);
        assert_eq!(
            NativeRoom::parse(urls[0]),
            Some(NativeRoom {
                room_id: ROOM_ID,
                owner_uid: 300000003
            })
        );
        assert_eq!(extra(&media, "room_id"), Some("7000000000000000002"));
        assert_eq!(extra(&media, "uid"), Some("300000003"));
        assert_eq!(metadata_uid(&media), "300000003");

        for (extras, lookup) in [
            (None, None),
            (Some(serde_json::json!({"native_stream": false})), host()),
            (Some(serde_json::json!({"stream_password": "pw"})), host()),
        ] {
            let bigo = bigo(&[LIVE], &[], None, extras.clone(), lookup).await;
            let media = extract(&bigo).await.unwrap();
            let formats: Vec<_> = media.streams.iter().map(|s| s.stream_format).collect();
            assert_eq!(formats, [StreamFormat::Hls], "{extras:?}");
            if extras.is_some_and(|extras| extras.get("stream_password").is_some()) {
                assert_eq!(extra(&media, "stream_password"), Some("pw"));
            }
        }

        let bigo4 = bigo(&[LIVE_WITHOUT_ROOM], &[], None, None, host()).await;
        let error = extract(&bigo4).await.unwrap_err();
        assert!(
            matches!(error, ExtractorError::ValidationError(_)),
            "{error:?}"
        );
    }

    /// The native stream records only a single host's room. A room the lookup
    /// finds ended is offline; any other failure leaves the next stream to be
    /// tried, never an account error.
    #[tokio::test]
    async fn native_stream_needs_a_host_room() {
        let room = NativeRoom {
            room_id: ROOM_ID,
            owner_uid: 300000003,
        };
        let check = async |lookup| {
            let bigo = bigo(&[], &[], Some(ACCOUNT), None, Some(lookup)).await;
            check_native(&bigo, room).await
        };
        check(Lookup::Room(RoomKind::Host)).await.unwrap();
        let ended = check(Lookup::Ended).await.unwrap_err();
        assert!(matches!(ended, ExtractorError::NoStreamsFound), "{ended:?}");
        for lookup in [
            Lookup::Room(RoomKind::MultiGuest),
            Lookup::Room(RoomKind::NoMedia),
            Lookup::Failed("IP restricted".to_owned()),
        ] {
            let error = check(lookup).await.unwrap_err();
            assert_eq!(error.category(), "other", "{error:?}");
        }
    }

    /// A broadcaster whose native recording just failed is left to the web
    /// stream, whatever the lookup says, until the profile reads offline.
    #[tokio::test]
    async fn native_stream_skips_a_broadcaster_that_failed_to_record() {
        // A broadcaster of its own: the memory is shared by the whole process.
        const OWNER_OFFLINE: &str = r#"{"code":0,"msg":"success","data":{"uid":17,"alive":0}}"#;
        let room = NativeRoom {
            room_id: ROOM_ID,
            owner_uid: 17,
        };
        let host = || Some(Lookup::Room(RoomKind::Host));
        let live = bigo(&[], &[], None, None, host()).await;
        check_native(&live, room).await.unwrap();

        stream::remember_failure(room.owner_uid);
        let error = check_native(&live, room).await.unwrap_err();
        assert_eq!(error.category(), "other", "{error:?}");

        let ended = bigo(&[OWNER_OFFLINE], &[], None, None, host()).await;
        assert!(!extract(&ended).await.unwrap().is_live);
        check_native(&ended, room).await.unwrap();
    }

    /// A room lookup that could not reach BIGO's app servers, or whose ticket
    /// request the website refused, makes the lookups on its route, and with
    /// them the native stream, wait a while. Other failures and other routes
    /// still look rooms up.
    #[tokio::test]
    async fn lookups_skip_a_route_that_could_not_reach_bigo() {
        // Routes of their own: the memory is shared by the whole process.
        let route = |tunnel_port: u16| Route {
            client: default_client(),
            proxy: Some(
                DanmuProxy::new(crate::proxy::ProxyEndpoint::from_url(&format!(
                    "http://127.0.0.1:{tunnel_port}"
                )))
                .unwrap(),
            ),
        };
        let (unreachable, refused, other) = (route(2), route(3), route(4));
        let ticket_refused = route(5);
        let reset = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
        lookup_on(&unreachable, async { Err(NativeError::Io(reset)) }).await;
        let too_many = NativeError::TicketRefused(reqwest::StatusCode::TOO_MANY_REQUESTS);
        lookup_on(&ticket_refused, async { Err(too_many) }).await;
        let rejected = NativeError::Rejected {
            request: "room lookup",
            result: 1,
        };
        lookup_on(&refused, async { Err(rejected) }).await;

        let host = || async { Ok(RoomKind::Host) };
        for route in [unreachable, ticket_refused] {
            let skipped = lookup_on(&route, host()).await;
            assert!(matches!(skipped, Lookup::Failed(_)), "{skipped:?}");
        }
        for route in [refused, other] {
            let found = lookup_on(&route, host()).await;
            assert!(matches!(found, Lookup::Room(RoomKind::Host)), "{found:?}");
        }
    }

    #[tokio::test]
    async fn web_stream_resolves_through_the_studio() {
        let extras = serde_json::json!({"stream_password": "pw"});
        let bigo = bigo(&[LIVE], &[STUDIO_LIVE], None, Some(extras), None).await;
        assert_eq!(web_url(&bigo).await.unwrap(), PLAYLIST);
    }

    #[tokio::test]
    async fn studio_refusals_are_not_offline() {
        let password = || Some(serde_json::json!({"stream_password": "wrong"}));
        for (studio, cookies, extras, expected) in [
            (STUDIO_NEED_LOGIN, None, None, "authentication"),
            (STUDIO_LOCKED_WITH_HLS, Some(ACCOUNT), None, "private"),
            // A locked room is private, never evidence of a restricted account.
            (STUDIO_LOCKED, Some(ACCOUNT), password(), "private"),
            (STUDIO_LIVE_WITHOUT_HLS, Some(ACCOUNT), None, "validation"),
        ] {
            let bigo = bigo(&[LIVE], &[studio], cookies, extras, None).await;
            let error = web_url(&bigo).await.unwrap_err();
            assert_eq!(error.category(), expected, "{studio}: {error:?}");
        }
    }

    /// When the studio reads a live room as offline, a room the guest lookup
    /// finds ended is offline at once. A room it finds is asked about again,
    /// as the studio can lag behind the profile, and only then means a
    /// restricted account when signed in, also for a profile that no longer
    /// links the room, unless the profile marks the room as app-only.
    #[tokio::test]
    async fn studio_offline_is_settled_with_the_room_lookup() {
        let exists = || Some(Lookup::Room(RoomKind::Host));
        let both = [STUDIO_OFFLINE, STUDIO_OFFLINE];
        for presence in [[LIVE, LIVE], [LIVE, LIVE_WITHOUT_ROOM]] {
            let restricted = bigo(&presence, &both, Some(ACCOUNT), None, exists()).await;
            let error = web_url(&restricted).await.unwrap_err();
            assert!(
                matches!(&error, ExtractorError::Authentication { code } if code == "account_restricted"),
                "{error:?}"
            );
        }

        let caught_up = [STUDIO_OFFLINE, STUDIO_LIVE];
        let started = bigo(&[LIVE, LIVE], &caught_up, Some(ACCOUNT), None, exists()).await;
        assert_eq!(web_url(&started).await.unwrap(), PLAYLIST);

        // A room broadcast for the app only has no web stream for any account.
        let app_only = [LIVE_APP_ONLY, LIVE_APP_ONLY];
        let app_only = bigo(&app_only, &both, Some(ACCOUNT), None, exists()).await;
        let error = web_url(&app_only).await.unwrap_err();
        assert_eq!(error.category(), "other", "{error:?}");

        let anonymous = bigo(&[LIVE, LIVE], &both, None, None, exists()).await;
        let error = web_url(&anonymous).await.unwrap_err();
        assert!(
            matches!(error, ExtractorError::ValidationError(_)),
            "{error:?}"
        );

        // No more answers: an ended room is not asked about again.
        let ended = Some(Lookup::Ended);
        let ended = bigo(&[LIVE], &[STUDIO_OFFLINE], Some(ACCOUNT), None, ended).await;
        let error = web_url(&ended).await.unwrap_err();
        assert!(matches!(error, ExtractorError::NoStreamsFound), "{error:?}");
    }

    /// Without a room lookup, a studio answer that reads a live room as
    /// offline is asked again, together with the profile. Only a signed-in
    /// check whose answers stay that way, for a profile that links its room,
    /// means a restricted account.
    #[tokio::test]
    async fn studio_offline_without_a_lookup_is_rechecked() {
        let failed = || Some(Lookup::Failed("unreachable".to_owned()));
        let ended = bigo(
            &[LIVE, OFFLINE],
            &[STUDIO_OFFLINE],
            Some(ACCOUNT),
            None,
            failed(),
        )
        .await;
        let error = web_url(&ended).await.unwrap_err();
        assert!(matches!(error, ExtractorError::NoStreamsFound), "{error:?}");

        let started = bigo(
            &[LIVE, LIVE],
            &[STUDIO_OFFLINE, STUDIO_LIVE],
            Some(ACCOUNT),
            None,
            None,
        )
        .await;
        assert_eq!(web_url(&started).await.unwrap(), PLAYLIST);

        let both = [STUDIO_OFFLINE, STUDIO_OFFLINE];
        let restricted = bigo(&[LIVE, LIVE], &both, Some(ACCOUNT), None, None).await;
        let error = web_url(&restricted).await.unwrap_err();
        assert!(
            matches!(&error, ExtractorError::Authentication { code } if code == "account_restricted"),
            "{error:?}"
        );

        for (presence, cookies) in [
            ([LIVE, LIVE], None),
            ([LIVE, LIVE_WITHOUT_ROOM], Some(ACCOUNT)),
        ] {
            let bigo = bigo(&presence, &both, cookies, None, None).await;
            let error = web_url(&bigo).await.unwrap_err();
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
        let route = Route {
            client: default_client(),
            proxy: None,
        };
        let bigo = Bigo::new(url, default_client(), cookies, None).with_native_route(route);
        let mut media = bigo.extract().await.expect("extract failed");

        println!(
            "live={} title={:?} artist={:?} room_id={:?} metadata={:?}",
            media.is_live,
            media.title,
            media.artist,
            extra(&media, "room_id"),
            media
                .platform_metadata
                .as_ref()
                .map(|metadata| &metadata.data),
        );
        assert!(media.platform_metadata.is_some());
        for stream in &mut media.streams {
            let result = bigo.get_url(stream).await;
            println!("{:?} {}: {result:?}", stream.stream_format, stream.url);
        }
    }
}
