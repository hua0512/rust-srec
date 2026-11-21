use std::borrow::Cow;
use std::sync::{Arc, LazyLock, Once};

use async_trait::async_trait;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, REFERER, RETRY_AFTER};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use tracing::{debug, info, warn};

use crate::{
    extractor::{
        error::{ExtractorError, parse_retry_after},
        platform_extractor::{Extractor, PlatformExtractor},
        platforms::weibo::{
            guest::{GLOBAL_GUEST, GUEST_SESSION_UNAVAILABLE, GuestSession, VisitorApi},
            models::{ContainerResponse, ContainerUser, LiveData, LiveResponse, Pull},
        },
    },
    media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo},
};

/// Room links (`weibo.com/l/wblive/p/show/<live id>`, group 1) and user links
/// (`weibo.com/u/<uid>`, `weibo.com/<uid>`, `m.weibo.cn/u/<uid>` and
/// `m.weibo.cn/profile/<uid>`, groups 2–4). A bare `weibo.com/<uid>` must end
/// at the digits, so custom names and post links are not taken for a uid.
pub static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:https?://)?(?:(?:www\.)?weibo\.com/(?:l/wblive/p/show/(\d+:\d+)|u/(\d+)|(\d+)/?(?:[?#]|$))|m\.weibo\.cn/(?:u|profile)/(\d+))",
    )
    .unwrap()
});
static GUEST_ONLY_NOTICE: Once = Once::new();

const BASE_URL: &str = "https://weibo.com";
const M_WEIBO_REFERER: &str = "https://m.weibo.cn/";
const CONTAINER_API_URL: &str = "https://m.weibo.cn/api/container/getIndex";
const STATUS_API_URL: &str = "https://weibo.com/ajax/statuses/mymblog";
const LIVE_API_URL: &str = "https://weibo.com/l/pc/anchor/live";

/// Every live id starts with this prefix.
const LIVE_ID_PREFIX: &str = "1022:";
/// The container's answer when the guest session is not accepted.
const CONTAINER_SESSION_REJECTED: i64 = -100;
/// `anchor/live` error codes.
const LIVE_ROOM_NOT_FOUND: i64 = 27401;
const LIVE_ID_MISSING: i64 = 10016;
/// `SinaSec check failure`, Weibo's gateway check, answered with HTTP 200.
/// It usually means a missing `Referer`, never a throttle or a bad account.
const GATEWAY_CHECK: i64 = 100_004;

/// Account failure codes for [`ExtractorError::Authentication`].
const LOGIN_EXPIRED: &str = "weibo_login_expired";
const LOGIN_REQUIRED: &str = "weibo_login_required";

/// The room a link leads to, as far as the user lookup can tell.
#[derive(Debug, PartialEq)]
enum WeiboRoom {
    /// A room to confirm with `anchor/live`, which can still find it ended.
    Live {
        live_id: String,
        profile: Profile,
    },
    Offline {
        profile: Profile,
    },
}

/// The streamer's name and avatar from the user lookup, used while offline
/// and when the room does not name the streamer.
#[derive(Debug, Default, PartialEq)]
struct Profile {
    name: String,
    avatar: Option<String>,
}

enum Target {
    Room(String),
    User(String),
}

/// The container's answer to a user lookup.
enum Container {
    Room(WeiboRoom),
    /// The guest session was rejected, by a redirect or `ok: -100`.
    SessionRejected,
}

pub struct Weibo {
    extractor: Extractor,
    /// Whether the caller supplied account cookies, which are then the
    /// fallback when the guest session is unavailable.
    has_user_cookies: bool,
    guest: Arc<GuestSession>,
    visitor_api: VisitorApi,
    container_api: Cow<'static, str>,
    status_api: Cow<'static, str>,
    live_api: Cow<'static, str>,
    _extras: Option<serde_json::Value>,
}

impl Weibo {
    pub fn new(
        platform_url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Self {
        let mut extractor = Extractor::new("weibo", platform_url, client);
        extractor.set_referer_static(BASE_URL);
        let cookies = cookies.filter(|cookies| !cookies.trim().is_empty());
        let has_user_cookies = cookies.is_some();
        if let Some(cookies) = cookies {
            extractor.set_cookies_from_string(&cookies);
        } else {
            GUEST_ONLY_NOTICE.call_once(|| {
                info!("weibo: no account configured; user links are resolved with a guest session");
            });
        }
        Self {
            extractor,
            has_user_cookies,
            guest: Arc::clone(&GLOBAL_GUEST),
            visitor_api: VisitorApi::default(),
            container_api: Cow::Borrowed(CONTAINER_API_URL),
            status_api: Cow::Borrowed(STATUS_API_URL),
            live_api: Cow::Borrowed(LIVE_API_URL),
            _extras: extras,
        }
    }

    fn target(&self) -> Result<Target, ExtractorError> {
        let invalid = || ExtractorError::InvalidUrl(self.extractor.url.clone());
        let captures = URL_REGEX
            .captures(&self.extractor.url)
            .ok_or_else(invalid)?;
        if let Some(room) = captures.get(1) {
            return Ok(Target::Room(room.as_str().to_owned()));
        }
        (2..=4)
            .find_map(|group| captures.get(group))
            .map(|uid| Target::User(uid.as_str().to_owned()))
            .ok_or_else(invalid)
    }

    async fn resolve_room(&self) -> Result<WeiboRoom, ExtractorError> {
        let uid = match self.target()? {
            Target::Room(live_id) => {
                return Ok(WeiboRoom::Live {
                    live_id,
                    profile: Profile::default(),
                });
            }
            Target::User(uid) => uid,
        };
        debug!(uid, "weibo: looking up the user's room");
        match self.lookup_with_guest(&uid).await {
            Ok(room) => Ok(room),
            // While Weibo throttles us, back off instead of adding account
            // requests on top.
            Err(error @ ExtractorError::RateLimited { .. }) => Err(error),
            Err(error) if self.has_user_cookies => {
                debug!(
                    error = error.category(),
                    "weibo: guest lookup failed; using the account"
                );
                self.lookup_with_account(&uid).await
            }
            Err(error) => Err(error),
        }
    }

    /// Looks the user up on m.weibo.cn with the guest session, refreshing a
    /// rejected session once.
    async fn lookup_with_guest(&self, uid: &str) -> Result<WeiboRoom, ExtractorError> {
        let sub = self.guest_sub().await?;
        if let Container::Room(room) = self.lookup_container(uid, &sub).await? {
            return Ok(room);
        }
        debug!("weibo: guest session rejected; refreshing it");
        self.guest.invalidate(&sub);
        let sub = self.guest_sub().await?;
        match self.lookup_container(uid, &sub).await? {
            Container::Room(room) => Ok(room),
            Container::SessionRejected => {
                warn!("weibo: a fresh guest session was rejected too");
                Err(ExtractorError::Other(GUEST_SESSION_UNAVAILABLE.to_owned()))
            }
        }
    }

    async fn guest_sub(&self) -> Result<String, ExtractorError> {
        let headers = self.extractor.get_platform_headers();
        self.guest
            .sub(&self.extractor.client, headers, &self.visitor_api)
            .await
    }

    async fn lookup_container(&self, uid: &str, sub: &str) -> Result<Container, ExtractorError> {
        let response = with_referer(
            self.extractor.request_with_cookie(
                Method::GET,
                &self.container_api,
                &format!("SUB={sub}"),
            ),
            M_WEIBO_REFERER,
        )
        .query(&[("type", "uid"), ("value", uid)])
        .send()
        .await?;
        check_throttle(&response)?;
        if redirected(&response, &self.container_api) {
            return Ok(Container::SessionRejected);
        }
        let body: ContainerResponse = response.error_for_status()?.json().await?;
        match body.ok {
            1 => {}
            CONTAINER_SESSION_REJECTED => return Ok(Container::SessionRejected),
            ok => {
                return Err(ExtractorError::ValidationError(format!(
                    "weibo user lookup failed (ok: {ok})"
                )));
            }
        }
        let user = body.data.and_then(|data| data.user_info);
        let live_id = user.as_ref().and_then(live_id_of);
        let profile = user.map(profile_of).unwrap_or_default();
        Ok(Container::Room(match live_id {
            Some(live_id) => WeiboRoom::Live { live_id, profile },
            None => WeiboRoom::Offline { profile },
        }))
    }

    /// Looks the user's latest posts up for a live room, which needs a
    /// signed-in account.
    async fn lookup_with_account(&self, uid: &str) -> Result<WeiboRoom, ExtractorError> {
        let response = with_referer(
            self.extractor.get(&self.status_api),
            &format!("{BASE_URL}/u/{uid}"),
        )
        .query(&[("uid", uid), ("page", "1"), ("feature", "0")])
        .send()
        .await?;
        check_throttle(&response)?;
        if let Some(code) = login_failure(&response, &self.status_api) {
            return Err(ExtractorError::Authentication {
                code: code.to_owned(),
            });
        }
        // A 403 or an HTML page is a block, not proof of a signed-out account.
        let body: serde_json::Value = response.error_for_status()?.json().await?;
        check_gateway(body.get("code").and_then(serde_json::Value::as_i64))?;
        let list = body
            .pointer("/data/list")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                ExtractorError::ValidationError("Unexpected response format".to_owned())
            })?;
        let live_id = list.iter().find_map(|item| {
            let page_info = item.get("page_info")?;
            (page_info.get("object_type")? == "live")
                .then(|| page_info.get("object_id")?.as_str())
                .flatten()
        });
        let profile = Profile::default();
        Ok(match live_id {
            Some(live_id) => WeiboRoom::Live {
                live_id: live_id.to_owned(),
                profile,
            },
            None => WeiboRoom::Offline { profile },
        })
    }

    fn offline(&self, title: String, profile: Profile) -> MediaInfo {
        MediaInfo::builder(self.extractor.url.clone(), title, profile.name)
            .artist_url_opt(profile.avatar)
            .is_live(false)
            .build()
    }

    async fn get_live_info(
        &self,
        live_id: &str,
        profile: Profile,
    ) -> Result<MediaInfo, ExtractorError> {
        let response = self
            .extractor
            .get(&self.live_api)
            .query(&[("live_id", live_id)])
            .send()
            .await?;
        check_throttle(&response)?;
        // Errors such as an unknown room come as HTTP 400 with a JSON body.
        let response = if response.status() == StatusCode::BAD_REQUEST {
            response
        } else {
            response.error_for_status()?
        };
        let body: LiveResponse = response.json().await?;
        check_gateway(body.code)?;
        match body.error_code {
            0 => {}
            LIVE_ROOM_NOT_FOUND => return Ok(self.offline(String::new(), profile)),
            LIVE_ID_MISSING => {
                return Err(ExtractorError::ValidationError(
                    "weibo live id missing".to_owned(),
                ));
            }
            code => {
                return Err(ExtractorError::ValidationError(format!(
                    "weibo live API error {code}"
                )));
            }
        }
        let Some(data) = body.data else {
            return Ok(self.offline(String::new(), profile));
        };
        let data: LiveData = serde_json::from_value(data)?;
        let user = data.user_info;
        let profile = Profile {
            name: if user.screen_name.is_empty() {
                profile.name
            } else {
                user.screen_name
            },
            avatar: Some(user.profile_image_url)
                .filter(|avatar| !avatar.is_empty())
                .or(profile.avatar),
        };
        let item = data.item;

        if item.status != 1 {
            return Ok(
                MediaInfo::builder(self.extractor.url.clone(), item.desc, profile.name)
                    .artist_url_opt(profile.avatar)
                    .cover_url(item.cover)
                    .is_live(false)
                    .build(),
            );
        }

        let streams = item
            .stream_info
            .and_then(|info| origin_prefix(&info.pull))
            .map(|origin| {
                let stream = |url, stream_format, media_format| {
                    StreamInfo::builder(url, stream_format, media_format)
                        .quality("Source")
                        .codec("hvc1")
                        .is_headers_needed(true)
                        .build()
                };
                vec![
                    stream(format!("{origin}.flv"), StreamFormat::Flv, MediaFormat::Flv),
                    stream(format!("{origin}.m3u8"), StreamFormat::Hls, MediaFormat::Ts),
                ]
            })
            .unwrap_or_default();

        Ok(
            MediaInfo::builder(self.extractor.url.clone(), item.desc, profile.name)
                .artist_url_opt(profile.avatar)
                .cover_url(item.cover)
                .is_live(true)
                .streams(streams)
                .headers(self.extractor.get_platform_headers_map())
                .build(),
        )
    }
}

/// Replaces the request's `Referer`.
fn with_referer(request: RequestBuilder, referer: &str) -> RequestBuilder {
    let mut headers = HeaderMap::new();
    if let Ok(referer) = HeaderValue::from_str(referer) {
        headers.insert(REFERER, referer);
    }
    request.headers(headers)
}

/// Throttling: HTTP 429 or 418, or any response asking to retry later.
pub(super) fn check_throttle(response: &Response) -> Result<(), ExtractorError> {
    let status = response.status();
    let retry_after = response.headers().get(RETRY_AFTER);
    if status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::IM_A_TEAPOT
        || retry_after.is_some()
    {
        return Err(ExtractorError::RateLimited {
            code: Some(status.as_u16().to_string()),
            retry_after: retry_after
                .and_then(|value| value.to_str().ok())
                .and_then(|value| parse_retry_after(value, jiff::Timestamp::now())),
        });
    }
    Ok(())
}

pub(super) fn check_gateway(code: Option<i64>) -> Result<(), ExtractorError> {
    if code == Some(GATEWAY_CHECK) {
        return Err(ExtractorError::ValidationError(
            "weibo_gateway_check".to_owned(),
        ));
    }
    Ok(())
}

/// Whether the request to `api` was redirected, judged by where the response
/// came from: the shared client follows redirects.
fn redirected(response: &Response, api: &str) -> bool {
    !response.url().as_str().starts_with(api)
}

/// The account failure a `mymblog` response shows, if any. An expired login
/// is answered with `X-Login-failed` and a redirect through `login.php` to
/// `passport.weibo.com/sso/signin`; cookies without a login at all are
/// redirected to the visitor page. Only these count: a 403, an HTML page or a
/// gateway check is a block, and taking it for a failed login would mark a
/// working account invalid.
///
/// The extractor's shared client follows redirects, so there only the final
/// URL shows where the request went. The account check uses a client that
/// does not, and sees the first response's header and `Location` instead.
/// Paths are matched alone: every hop of these redirects stays on Weibo's
/// hosts.
pub(super) fn login_failure(response: &Response, api: &str) -> Option<&'static str> {
    let login_failed = response
        .headers()
        .get("x-login-failed")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    if login_failed {
        return Some(LOGIN_EXPIRED);
    }
    let target = if response.status().is_redirection() {
        let location = response.headers().get(reqwest::header::LOCATION)?;
        response.url().join(location.to_str().ok()?).ok()?
    } else if redirected(response, api) {
        response.url().clone()
    } else {
        return None;
    };
    let path = target.path();
    if path == "/login.php" || path.starts_with("/sso/signin") {
        Some(LOGIN_EXPIRED)
    } else if path.starts_with("/visitor/visitor") {
        Some(LOGIN_REQUIRED)
    } else {
        None
    }
}

/// The live id of the live avatar among the profile buttons.
fn live_id_of(user: &ContainerUser) -> Option<String> {
    user.toolbar_menus.iter().find_map(|menu| {
        menu.pointer("/userInfo/avatar/oid")?
            .as_str()
            .filter(|oid| oid.starts_with(LIVE_ID_PREFIX))
            .map(str::to_owned)
    })
}

fn profile_of(user: ContainerUser) -> Profile {
    Profile {
        name: user.screen_name.unwrap_or_default(),
        avatar: [user.avatar_hd, user.profile_image_url]
            .into_iter()
            .flatten()
            .find(|avatar| !avatar.is_empty()),
    }
}

static TRANSCODE_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_wb\d+avc$").unwrap());

/// The original-quality stream's URL without its extension, from which the
/// `.flv` and `.m3u8` streams are built.
///
/// Weibo returns a transcode (`<id>_wb720avc.flv`); dropping the suffix from
/// the file name gives the original, which is always recorded instead. A URL
/// without the suffix is kept as returned. The HLS field is only a fallback
/// for the prefix, and its extension is ignored because it has been seen
/// returning `.flv`.
fn origin_prefix(pull: &Pull) -> Option<String> {
    let url = [&pull.live_origin_flv_url, &pull.live_origin_hls_url]
        .into_iter()
        .find(|url| !url.is_empty())?;
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let name_start = url.rfind('/').map_or(0, |slash| slash + 1);
    let (dir, name) = url.split_at(name_start);
    let stem = name.rfind('.').map_or(name, |dot| &name[..dot]);
    let stem = TRANSCODE_SUFFIX.replace(stem, "");
    (!stem.is_empty()).then(|| format!("{dir}{stem}"))
}

#[async_trait]
impl PlatformExtractor for Weibo {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        match self.resolve_room().await? {
            WeiboRoom::Live { live_id, profile } => {
                debug!(live_id, "weibo: checking the room");
                self.get_live_info(&live_id, profile).await
            }
            WeiboRoom::Offline { profile } => Ok(self.offline(String::new(), profile)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::extractor::default::default_client;
    use crate::extractor::test_support::{ReceivedRequests, Reply, serve_in_order};

    const USER_URL: &str = "https://weibo.com/u/1000000001";
    const ROOM_URL: &str = "https://weibo.com/l/wblive/p/show/1022:2321320000000000000001";
    const ACCOUNT: &str = "SUB=account-sub; SUBP=account-subp";
    const GUEST_1: &str = r#"window.visitor_gray_callback && visitor_gray_callback({"retcode":20000000,"msg":"succ","data":{"sub":"_2AkGuestOne","subp":"p"}});"#;
    const GUEST_2: &str = r#"window.visitor_gray_callback && visitor_gray_callback({"retcode":20000000,"msg":"succ","data":{"sub":"_2AkGuestTwo","subp":"p"}});"#;
    const GUEST_REFUSED: &str = r#"window.visitor_gray_callback && visitor_gray_callback({"retcode":50111299,"msg":"fail","data":[]});"#;
    const CONTAINER_LIVE: &str = r#"{"ok":1,"data":{"userInfo":{"screen_name":"Streamer",
        "avatar_hd":"https://tvax1.sinaimg.cn/large/avatar.jpg","toolbar_menus":[{"type":"profile_follow"},
        {"type":"link","userInfo":{"avatar":{"oid":"1022:2321320000000000000001","state":2}}}]}}}"#;
    const CONTAINER_OFFLINE: &str = r#"{"ok":1,"data":{"userInfo":{"screen_name":"Streamer",
        "avatar_hd":"https://tvax1.sinaimg.cn/large/avatar.jpg","toolbar_menus":[{"type":"profile_follow"}]}}}"#;
    /// Live, with the transcode in both fields and no streamer name.
    const ROOM_LIVE: &str = r#"{"error_code":0,"msg":"success","code":100000,"data":{"item":{"status":1,
        "desc":"Title","cover":"https://wx1.sinaimg.cn/large/cover.jpg","stream_info":{"pull":{
        "live_origin_flv_url":"https://plwb00.live.weibo.com/alicdn/1000000000000001_wb720avc.flv",
        "live_origin_hls_url":"https://plwb00.live.weibo.com/alicdn/1000000000000001_wb720avc.flv"}}},
        "user_info":{"screen_name":"","profile_image_url":""}}}"#;
    const ROOM_NOT_FOUND: &str = r#"{"error":"LiveRoom does not exists! (1022:2321320000000000000001).","error_code":27401}"#;

    fn enter_page() -> Reply {
        Reply::raw(
            "200 OK",
            "text/html",
            r#"<script>var request_id = "0123456789abcdef"; var return_url = "https://m.weibo.cn/";</script>"#,
        )
    }

    fn guest(body: &'static str) -> Reply {
        Reply::raw("200 OK", "application/javascript", body)
    }

    fn visitor_api(base: &str) -> VisitorApi {
        VisitorApi {
            enter: Cow::Owned(format!("{base}visitor/visitor")),
            generate: Cow::Owned(format!("{base}visitor/genvisitor2")),
        }
    }

    /// A Weibo extractor for `url` with its own guest session, whose requests
    /// are all answered by one local server with `replies`, in order.
    async fn local_weibo(
        url: &str,
        cookies: Option<&str>,
        replies: Vec<Reply>,
    ) -> (Weibo, ReceivedRequests) {
        let (base, received) = serve_in_order(replies).await;
        let mut weibo = Weibo::new(
            url.to_owned(),
            default_client(),
            cookies.map(str::to_owned),
            None,
        );
        weibo.guest = Arc::default();
        weibo.visitor_api = visitor_api(&base);
        weibo.container_api = Cow::Owned(format!("{base}api/container/getIndex"));
        weibo.status_api = Cow::Owned(format!("{base}ajax/statuses/mymblog"));
        weibo.live_api = Cow::Owned(format!("{base}l/pc/anchor/live"));
        (weibo, received)
    }

    async fn extract(weibo: &Weibo) -> Result<MediaInfo, ExtractorError> {
        tokio::time::timeout(Duration::from_secs(10), weibo.extract())
            .await
            .expect("local Weibo requests timed out")
    }

    /// The path of each request received, without its query.
    fn paths(received: &ReceivedRequests) -> Vec<String> {
        received
            .lock()
            .unwrap()
            .iter()
            .map(|request| {
                let target = request.split(' ').nth(1).unwrap_or_default();
                target.split('?').next().unwrap_or_default().to_owned()
            })
            .collect()
    }

    fn request(received: &ReceivedRequests, index: usize) -> String {
        received.lock().unwrap()[index].clone()
    }

    #[tokio::test]
    async fn user_link_resolves_the_original_stream_with_a_guest_session() {
        let (weibo, received) = local_weibo(
            USER_URL,
            Some(ACCOUNT),
            vec![
                enter_page(),
                guest(GUEST_1),
                Reply::json("200 OK", CONTAINER_LIVE),
                Reply::json("200 OK", ROOM_LIVE),
            ],
        )
        .await;

        let media = extract(&weibo).await.unwrap();

        assert!(media.is_live);
        assert_eq!(media.artist, "Streamer");
        assert_eq!(
            media.artist_url.as_deref(),
            Some("https://tvax1.sinaimg.cn/large/avatar.jpg")
        );
        let streams: Vec<_> = media
            .streams
            .iter()
            .map(|stream| (stream.url.as_str(), stream.quality.as_str()))
            .collect();
        assert_eq!(
            streams,
            [
                (
                    "https://plwb00.live.weibo.com/alicdn/1000000000000001.flv",
                    "Source"
                ),
                (
                    "https://plwb00.live.weibo.com/alicdn/1000000000000001.m3u8",
                    "Source"
                ),
            ]
        );
        assert_eq!(
            paths(&received),
            [
                "/visitor/visitor",
                "/visitor/genvisitor2",
                "/api/container/getIndex",
                "/l/pc/anchor/live"
            ]
        );
        // Only weibo.com gets the account's cookies; m.weibo.cn gets the
        // guest session.
        let lookup = request(&received, 2);
        assert!(lookup.contains("cookie: SUB=_2AkGuestOne\r\n"), "{lookup}");
        for index in 0..3 {
            assert!(!request(&received, index).contains("account-sub"));
        }
    }

    #[tokio::test]
    async fn offline_user_is_answered_by_the_lookup_alone() {
        let (weibo, received) = local_weibo(
            "https://m.weibo.cn/profile/1000000001",
            None,
            vec![
                enter_page(),
                guest(GUEST_1),
                Reply::json("200 OK", CONTAINER_OFFLINE),
            ],
        )
        .await;

        let media = extract(&weibo).await.unwrap();

        assert!(!media.is_live);
        assert_eq!(media.artist, "Streamer");
        assert_eq!(
            media.artist_url.as_deref(),
            Some("https://tvax1.sinaimg.cn/large/avatar.jpg")
        );
        assert_eq!(received.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn rejected_guest_session_is_refreshed_once() {
        let (weibo, received) = local_weibo(
            USER_URL,
            None,
            vec![
                enter_page(),
                guest(GUEST_1),
                Reply::raw("302 Found", "text/html", "")
                    .header("location", "/sso/signin?entry=wapsso"),
                Reply::raw("200 OK", "text/html", "<html>sign in</html>"),
                enter_page(),
                guest(GUEST_2),
                Reply::json("200 OK", CONTAINER_OFFLINE),
            ],
        )
        .await;

        let media = extract(&weibo).await.unwrap();

        assert!(!media.is_live);
        let lookup = request(&received, 6);
        assert!(lookup.contains("cookie: SUB=_2AkGuestTwo\r\n"), "{lookup}");
    }

    #[tokio::test]
    async fn unavailable_guest_session_without_an_account_is_not_an_account_failure() {
        let (weibo, _) =
            local_weibo(USER_URL, None, vec![enter_page(), guest(GUEST_REFUSED)]).await;

        let error = extract(&weibo).await.unwrap_err();

        assert!(
            matches!(&error, ExtractorError::Other(code) if code == GUEST_SESSION_UNAVAILABLE),
            "{error:?}"
        );
    }

    /// When the guest lookup fails, the account's answer decides: only a
    /// sign-in redirect is an account failure.
    #[tokio::test]
    async fn account_fallback_reports_only_sign_in_redirects_as_account_failures() {
        // The visitor page names no request id, so there is no guest session.
        let no_guest = || vec![Reply::raw("200 OK", "text/html", "<html></html>")];
        let cases = [
            (
                no_guest(),
                vec![
                    Reply::raw("302 Found", "text/html", "").header("location", "/login.php?url=x"),
                    Reply::raw("302 Found", "text/html", "")
                        .header("location", "/sso/signin?entry=miniblog"),
                    Reply::raw("200 OK", "text/html", "<html>sign in</html>"),
                ],
                Some(LOGIN_EXPIRED),
            ),
            (
                // m.weibo.cn refuses the guest lookup.
                vec![
                    enter_page(),
                    guest(GUEST_1),
                    Reply::raw("403 Forbidden", "text/html", "<h2>403 Forbidden</h2>"),
                ],
                vec![
                    Reply::raw("302 Found", "text/html", "")
                        .header("location", "/visitor/visitor?entry=miniblog&a=enter"),
                    Reply::raw("200 OK", "text/html", "<html>visitor</html>"),
                ],
                Some(LOGIN_REQUIRED),
            ),
            (
                no_guest(),
                vec![Reply::raw(
                    "403 Forbidden",
                    "text/html",
                    "<h2>403 Forbidden</h2>",
                )],
                None,
            ),
        ];
        for (mut replies, account_replies, expected) in cases {
            let account_request = replies.len();
            replies.extend(account_replies);
            let (weibo, received) = local_weibo(USER_URL, Some(ACCOUNT), replies).await;

            let error = extract(&weibo).await.unwrap_err();

            match (&error, expected) {
                (ExtractorError::Authentication { code }, Some(expected)) => {
                    assert_eq!(code, expected);
                }
                (ExtractorError::Authentication { .. }, None) | (_, Some(_)) => {
                    panic!("{error:?} instead of {expected:?}");
                }
                (_, None) => {}
            }
            assert!(request(&received, account_request).contains("account-sub"));
        }
    }

    #[tokio::test]
    async fn ended_room_is_offline() {
        let (weibo, _) = local_weibo(
            ROOM_URL,
            None,
            vec![Reply::json("400 Bad Request", ROOM_NOT_FOUND)],
        )
        .await;

        let media = extract(&weibo).await.unwrap();

        assert!(!media.is_live);
    }

    #[tokio::test]
    async fn throttles_and_gateway_checks_are_typed() {
        let (weibo, _) = local_weibo(
            USER_URL,
            None,
            vec![
                enter_page(),
                guest(GUEST_1),
                Reply::json("429 Too Many Requests", "{}").header("retry-after", "120"),
            ],
        )
        .await;
        let error = extract(&weibo).await.unwrap_err();
        assert!(
            matches!(
                error,
                ExtractorError::RateLimited { retry_after: Some(delay), .. }
                    if delay == Duration::from_secs(120)
            ),
            "{error:?}"
        );

        let (weibo, _) = local_weibo(
            ROOM_URL,
            None,
            vec![Reply::json(
                "200 OK",
                r#"{"code":100004,"msg":"SinaSec check failure","error_code":-1,"data":[]}"#,
            )],
        )
        .await;
        let error = extract(&weibo).await.unwrap_err();
        assert!(
            matches!(&error, ExtractorError::ValidationError(code) if code == "weibo_gateway_check"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn concurrent_checks_share_one_guest_session_fetch() {
        let (base, received) = serve_in_order(vec![enter_page(), guest(GUEST_1)]).await;
        let api = visitor_api(&base);
        let session = GuestSession::default();
        let client = default_client();
        let headers = HeaderMap::new();

        let fetches = (0..4).map(|_| session.sub(&client, &headers, &api));
        let subs =
            tokio::time::timeout(Duration::from_secs(10), futures::future::join_all(fetches))
                .await
                .expect("local visitor requests timed out");

        for sub in subs {
            assert_eq!(sub.unwrap(), "_2AkGuestOne");
        }
        assert_eq!(received.lock().unwrap().len(), 2);
    }

    #[test]
    fn user_and_room_links_are_recognised() {
        let cases = [
            ("https://weibo.com/u/1000000001", Some("1000000001")),
            ("https://www.weibo.com/1000000001", Some("1000000001")),
            ("weibo.com/1000000001/", Some("1000000001")),
            (
                "https://weibo.com/1000000001?refer_flag=1",
                Some("1000000001"),
            ),
            ("https://m.weibo.cn/u/1000000001", Some("1000000001")),
            ("https://m.weibo.cn/profile/1000000001", Some("1000000001")),
            (ROOM_URL, Some("1022:2321320000000000000001")),
            ("https://weibo.com/somename", None),
            ("https://weibo.com/1000000001/PostId123", None),
            ("https://weibo.com/1000000001abc", None),
        ];
        for (url, expected) in cases {
            let weibo = Weibo::new(url.to_owned(), default_client(), None, None);
            let found = match weibo.target() {
                Ok(Target::User(id) | Target::Room(id)) => Some(id),
                Err(_) => None,
            };
            assert_eq!(found.as_deref(), expected, "{url}");
        }
    }

    #[test]
    fn only_the_transcode_suffix_is_dropped() {
        let pull = |flv: &str, hls: &str| Pull {
            live_origin_flv_url: flv.to_owned(),
            live_origin_hls_url: hls.to_owned(),
        };
        let cases = [
            (
                pull("https://cdn_a.example/live_x/123_wb1080avc.flv?k=v", ""),
                "https://cdn_a.example/live_x/123",
            ),
            (
                pull("", "https://cdn.example/live/123_wb720avc.m3u8"),
                "https://cdn.example/live/123",
            ),
            (
                pull("https://cdn.example/live/123_456.flv", ""),
                "https://cdn.example/live/123_456",
            ),
        ];
        for (pull, expected) in cases {
            assert_eq!(origin_prefix(&pull).as_deref(), Some(expected));
        }
    }

    /// Real-network smoke run, by hand before merging: a guest session and
    /// user links in two forms. Set `WEIBO_LIVE_URL` to the link of a user
    /// who is live to check the streams too.
    #[tokio::test]
    #[ignore]
    async fn live_smoke() {
        let mut urls = vec![
            "https://weibo.com/u/6034381748".to_owned(),
            "https://m.weibo.cn/u/6124785491".to_owned(),
        ];
        urls.extend(std::env::var("WEIBO_LIVE_URL").ok());
        for url in urls {
            let weibo = Weibo::new(url.clone(), default_client(), None, None);
            let media = weibo.extract().await.unwrap();
            let streams: Vec<_> = media
                .streams
                .iter()
                .map(|stream| (&stream.url, &stream.quality))
                .collect();
            println!(
                "{url}: live={} artist={:?} streams={streams:?}",
                media.is_live, media.artist
            );
        }
    }
}
