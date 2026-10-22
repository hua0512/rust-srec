//! An offline streaming platform: a control server on 127.0.0.1 that tests
//! script, and an extractor registered with the platform factory that reads
//! it.
//!
//! A room's page URL is `http://127.0.0.1:<port>/__srec_fake__/<room>`. The
//! extractor fetches `<page>/status` with the client the backend hands it, so
//! the request carries the platform's headers and cookies and follows the
//! platform's route; [`FakePlatform::requests`] shows what arrived. The status
//! answer becomes the `MediaInfo`, and an error answer becomes the
//! `ExtractorError` its `kind` names (the variant's
//! [`ExtractorError::category`]).
//!
//! Media is served at `<page>/live.flv` as a chunked body the test feeds with
//! [`FakeRoom::push`] and closes with [`FakeRoom::end`] or [`FakeRoom::reset`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Once};

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::channel::mpsc;
use platforms_parser::extractor::error::ExtractorError;
use platforms_parser::extractor::factory::register_platform;
use platforms_parser::extractor::platform_extractor::{Extractor, PlatformExtractor};
use platforms_parser::media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo};
use serde_json::{Map, Value, json};

use super::http::ReceivedRequest;
use super::media::FlvLoop;
use super::{WAIT, eventually};

/// The name the platform registers under; `platform_config.platform_name`
/// matches it ignoring case.
pub const FAKE_PLATFORM_NAME: &str = "Fake";
/// The `platform_config` row [`super::Seed`] adds for the fake platform.
pub const FAKE_PLATFORM_ID: &str = "platform-fake";
const PATH_PREFIX: &str = "/__srec_fake__/";
const URL_PATTERN: &str = r"^http://127\.0\.0\.1:\d{1,5}/__srec_fake__/[A-Za-z0-9_-]+$";

/// Every `kind` [`FakeRoom::set_error`] accepts: the categories of the
/// extractor errors, one per variant.
const ERROR_KINDS: [&str; 17] = [
    "authentication",
    "rate_limited",
    "invalid_url",
    "http",
    "unsupported_extractor",
    "json",
    "live_stream_not_supported",
    "age_restricted",
    "private",
    "region_locked",
    "not_found",
    "banned",
    "no_streams",
    "validation",
    "javascript",
    "hls_playlist",
    "other",
];

fn register() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let regex = regex::Regex::new(URL_PATTERN).expect("valid fake platform URL pattern");
        register_platform(
            FAKE_PLATFORM_NAME,
            regex,
            |url, client, cookies, _extras| {
                let mut extractor = Extractor::new(FAKE_PLATFORM_NAME, url, client);
                if let Some(cookies) = cookies {
                    extractor.set_cookies_from_string(&cookies);
                }
                Box::new(FakeExtractor { extractor })
            },
        )
        .expect("register the fake platform");
    });
}

struct FakeExtractor {
    extractor: Extractor,
}

#[async_trait::async_trait]
impl PlatformExtractor for FakeExtractor {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let response = self
            .extractor
            .get(&format!("{}/status", self.extractor.url))
            .send()
            .await?;
        let text = ExtractorError::check_response(response)?.text().await?;
        let status: Value = serde_json::from_str(&text)?;
        if let Some(error) = status.get("error") {
            return Err(extractor_error(error));
        }
        Ok(media_info(&self.extractor.url, &status))
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A JSON object's string values, as the map type the caller needs.
fn string_map<M: FromIterator<(String, String)>>(value: Option<&Value>) -> Option<M> {
    let object = value?.as_object()?;
    Some(
        object
            .iter()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect(),
    )
}

fn media_info(page_url: &str, status: &Value) -> MediaInfo {
    let live = status.get("live").and_then(Value::as_bool).unwrap_or(false);
    let streams = status
        .get("streams")
        .and_then(Value::as_array)
        .map(|streams| streams.iter().map(stream_info).collect())
        .unwrap_or_default();
    let extras = status
        .get("viewers")
        .and_then(Value::as_u64)
        .map(|viewers| {
            [("viewer_count".to_owned(), viewers.to_string())]
                .into_iter()
                .collect()
        });
    MediaInfo::builder(
        page_url,
        text(status, "title").unwrap_or_default(),
        page_url.rsplit('/').next().unwrap_or_default(),
    )
    .category_one_opt(text(status, "category"))
    .artist_url_opt(text(status, "avatar"))
    .is_live(live)
    .streams(streams)
    .headers_opt(string_map(status.get("headers")))
    .extras_opt(extras)
    .build()
}

fn stream_info(stream: &Value) -> StreamInfo {
    let format = text(stream, "format")
        .and_then(|format| format.parse().ok())
        .unwrap_or(StreamFormat::Flv);
    let media = text(stream, "media")
        .and_then(|media| media.parse().ok())
        .unwrap_or(MediaFormat::Flv);
    let mut builder = StreamInfo::builder(text(stream, "url").unwrap_or_default(), format, media)
        .quality(text(stream, "quality").unwrap_or_else(|| "origin".to_owned()))
        .bitrate(stream.get("bitrate").and_then(Value::as_u64).unwrap_or(0))
        .priority(
            stream
                .get("priority")
                .and_then(Value::as_u64)
                .and_then(|priority| u32::try_from(priority).ok())
                .unwrap_or(0),
        );
    if let Some(extras) = stream.get("extras") {
        builder = builder.extras(extras.clone());
    }
    builder.build()
}

fn extractor_error(error: &Value) -> ExtractorError {
    let message = text(error, "message").unwrap_or_default();
    match error
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "authentication" => ExtractorError::Authentication { code: message },
        "rate_limited" => ExtractorError::RateLimited {
            code: None,
            retry_after: None,
        },
        "invalid_url" => ExtractorError::InvalidUrl(message),
        "unsupported_extractor" => ExtractorError::UnsupportedExtractor,
        "live_stream_not_supported" => ExtractorError::LiveStreamNotSupported,
        "age_restricted" => ExtractorError::AgeRestrictedContent,
        "private" => ExtractorError::PrivateContent,
        "region_locked" => ExtractorError::RegionLockedContent,
        "not_found" => ExtractorError::StreamerNotFound,
        "banned" => ExtractorError::StreamerBanned,
        "no_streams" => ExtractorError::NoStreamsFound,
        "validation" => ExtractorError::ValidationError(message),
        "javascript" => ExtractorError::JsError(message),
        "hls_playlist" => ExtractorError::HlsPlaylistError(message),
        "other" => ExtractorError::Other(message),
        // `http` and `json` are served as a failing response instead, since
        // their variants wrap a client or parser error.
        kind => ExtractorError::Other(format!("the fake platform has no error kind {kind:?}")),
    }
}

/// The fake platform's control server.
pub struct FakePlatform {
    origin: String,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct Shared {
    rooms: Mutex<HashMap<String, Arc<Room>>>,
    requests: Mutex<Vec<ReceivedRequest>>,
}

impl Shared {
    fn room(&self, name: &str) -> Arc<Room> {
        self.rooms
            .lock()
            .expect("room table lock")
            .entry(name.to_owned())
            .or_default()
            .clone()
    }
}

#[derive(Default)]
struct Room {
    status: Mutex<Status>,
    media: Mutex<Media>,
}

#[derive(Default)]
enum Status {
    #[default]
    Offline,
    Live(Value),
    Error {
        kind: String,
        message: String,
    },
}

type MediaSender = mpsc::UnboundedSender<Result<Bytes, std::io::Error>>;

#[derive(Default)]
struct Media {
    /// The body of the newest media request, while it is open.
    body: Option<MediaSender>,
    connections: usize,
}

impl FakePlatform {
    /// Bind the control server on an ephemeral loopback port and make sure the
    /// platform is registered in this process.
    pub async fn start() -> Self {
        register();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the fake platform");
        let address = listener.local_addr().expect("fake platform address");
        let shared = Arc::new(Shared::default());
        let state = shared.clone();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let shared = state.clone();
            async move { serve(&shared, request).await }
        });
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve the fake platform");
        });
        Self {
            origin: format!("http://{address}"),
            shared,
            task,
        }
    }

    /// A room, created offline on first use.
    pub fn room(&self, name: &str) -> FakeRoom {
        let path = format!("{PATH_PREFIX}{name}");
        FakeRoom {
            page_url: format!("{}{path}", self.origin),
            path,
            room: self.shared.room(name),
        }
    }

    /// Every request received so far, in arrival order.
    pub fn requests(&self) -> Vec<ReceivedRequest> {
        self.shared
            .requests
            .lock()
            .expect("request log lock")
            .clone()
    }

    /// The requests for `room`'s `resource` (`status` or `live.flv`).
    pub fn requests_to(&self, room: &FakeRoom, resource: &str) -> Vec<ReceivedRequest> {
        let path = format!("{}/{resource}", room.path);
        self.requests()
            .into_iter()
            .filter(|request| request.path.split('?').next() == Some(path.as_str()))
            .collect()
    }
}

impl Drop for FakePlatform {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(shared: &Shared, request: axum::extract::Request) -> Response {
    let request = ReceivedRequest::from_axum(request).await;
    let resource = request
        .path
        .split('?')
        .next()
        .and_then(|path| path.strip_prefix(PATH_PREFIX))
        .and_then(|rest| rest.split_once('/'))
        .map(|(room, resource)| (shared.room(room), resource.to_owned()));
    shared
        .requests
        .lock()
        .expect("request log lock")
        .push(request);
    match resource {
        Some((room, resource)) if resource == "status" => room.status_response(),
        Some((room, resource)) if resource == "live.flv" => room.open_media(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

impl Room {
    fn status_response(&self) -> Response {
        let status = self.status.lock().expect("room status lock");
        let body = match &*status {
            Status::Offline => json!({ "live": false }),
            Status::Live(live) => live.clone(),
            // These two variants wrap a client or parser error, so the
            // extractor has to meet a failing response to produce them.
            Status::Error { kind, message } if kind == "http" => {
                return (StatusCode::SERVICE_UNAVAILABLE, message.clone()).into_response();
            }
            Status::Error { kind, .. } if kind == "json" => {
                return ([(header::CONTENT_TYPE, "application/json")], "{not json").into_response();
            }
            Status::Error { kind, message } => {
                json!({ "error": { "kind": kind, "message": message } })
            }
        };
        axum::Json(body).into_response()
    }

    fn open_media(&self) -> Response {
        let (sender, receiver) = mpsc::unbounded();
        {
            let mut media = self.media.lock().expect("room media lock");
            media.body = Some(sender);
            media.connections += 1;
        }
        (
            [(header::CONTENT_TYPE, "video/x-flv")],
            Body::from_stream(receiver),
        )
            .into_response()
    }
}

/// One room on the [`FakePlatform`]. Clones share the room.
#[derive(Clone)]
pub struct FakeRoom {
    page_url: String,
    /// `page_url` without the origin, as requests for the room arrive.
    path: String,
    room: Arc<Room>,
}

impl FakeRoom {
    /// The room page, the URL a streamer is created with.
    pub fn url(&self) -> &str {
        &self.page_url
    }

    /// The room's FLV endpoint.
    pub fn flv_url(&self) -> String {
        format!("{}/live.flv", self.page_url)
    }

    /// A live answer titled `title` offering the room's FLV endpoint.
    pub fn live(&self, title: &str) -> Live {
        Live::new(title).stream(json!({
            "url": self.flv_url(),
            "format": "flv",
            "media": "flv",
            "quality": "origin",
            "bitrate": 16_000,
        }))
    }

    pub fn set_live(&self, live: Live) {
        *self.room.status.lock().expect("room status lock") = Status::Live(Value::Object(live.0));
    }

    pub fn set_offline(&self) {
        *self.room.status.lock().expect("room status lock") = Status::Offline;
    }

    /// Answer status requests with the extractor error whose category is
    /// `kind` (one of [`ERROR_KINDS`]).
    pub fn set_error(&self, kind: &str, message: &str) {
        assert!(ERROR_KINDS.contains(&kind), "unknown error kind {kind:?}");
        *self.room.status.lock().expect("room status lock") = Status::Error {
            kind: kind.to_owned(),
            message: message.to_owned(),
        };
    }

    fn media_connections(&self) -> usize {
        self.room.media.lock().expect("room media lock").connections
    }

    /// Wait until the room has received `count` media requests.
    pub async fn wait_for_media_connection(&self, count: usize) {
        eventually(
            WAIT,
            &format!("media request {count} to {}", self.page_url),
            || async { (self.media_connections() >= count).then_some(()) },
        )
        .await;
    }

    /// Whether the backend has hung up the newest media request.
    pub fn media_closed(&self) -> bool {
        self.room
            .media
            .lock()
            .expect("room media lock")
            .body
            .as_ref()
            .is_none_or(MediaSender::is_closed)
    }

    /// Send `bytes` on the open media body.
    #[track_caller]
    pub fn push(&self, bytes: Bytes) {
        let media = self.room.media.lock().expect("room media lock");
        let body = media.body.as_ref().expect("no media request is open");
        body.unbounded_send(Ok(bytes))
            .expect("the backend closed the media request");
    }

    /// End the open media body cleanly.
    #[track_caller]
    pub fn end(&self) {
        let body = self.room.media.lock().expect("room media lock").body.take();
        assert!(body.is_some(), "no media request is open");
    }

    /// Send `loops` repetitions of [`FlvLoop::tiny`] on the open media body,
    /// then go offline and end the body.
    #[track_caller]
    pub fn broadcast_ends(&self, loops: u32) {
        self.push(FlvLoop::tiny().stream(loops));
        self.set_offline();
        self.end();
    }

    /// Abort the open media body, as a dropped connection would.
    #[track_caller]
    pub fn reset(&self) {
        let body = self.room.media.lock().expect("room media lock").body.take();
        let body = body.expect("no media request is open");
        // The backend may have hung up already; then there is nothing to abort.
        let _ = body.unbounded_send(Err(std::io::Error::other("fake platform reset")));
    }
}

/// A live status answer.
pub struct Live(Map<String, Value>);

impl Live {
    fn new(title: &str) -> Self {
        let mut answer = Map::new();
        answer.insert("live".to_owned(), json!(true));
        answer.insert("title".to_owned(), json!(title));
        answer.insert("streams".to_owned(), json!([]));
        Self(answer)
    }

    pub fn category(mut self, category: &str) -> Self {
        self.0.insert("category".to_owned(), json!(category));
        self
    }

    pub fn viewers(mut self, viewers: u64) -> Self {
        self.0.insert("viewers".to_owned(), json!(viewers));
        self
    }

    /// A header of `MediaInfo.headers`, which the media request carries too.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.0
            .entry("headers")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("headers object")
            .insert(name.to_owned(), json!(value));
        self
    }

    /// Add a stream candidate (`url`, `format`, `media`, `quality`, `bitrate`,
    /// `priority`, `extras`).
    pub fn stream(mut self, stream: Value) -> Self {
        self.0
            .get_mut("streams")
            .and_then(Value::as_array_mut)
            .expect("streams array")
            .push(stream);
        self
    }
}
