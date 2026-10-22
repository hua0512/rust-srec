//! A scripted media server on 127.0.0.1 for streams the fake platform's rooms
//! point at: HLS playlists and segments, DASH presentations, and answers
//! that fail on purpose.
//!
//! A path serves what [`FakeCdn::put`] last stored there, after the failures
//! [`FakeCdn::fail`] queued for it; unknown paths answer 404. Every request is
//! recorded, so tests assert what the backend asked for and with which
//! headers.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;

use super::http::{LocalHttpServer, ReceivedRequest};
use super::{WAIT, eventually};

/// Two one-second MPEG-TS segments with continuous timestamps (see
/// `tests/fixtures/media/README.md`).
const TS_SEGMENTS: [&[u8]; 2] = [
    include_bytes!("../../fixtures/media/tiny0.ts"),
    include_bytes!("../../fixtures/media/tiny1.ts"),
];
/// A 32x32 fMP4 initialization section and two one-second fragments.
const FMP4_INIT: &[u8] = include_bytes!("../../../../crates/hls-fix/tests/fixtures/init.mp4");
const FMP4_SEGMENTS: [&[u8]; 2] = [
    include_bytes!("../../../../crates/hls-fix/tests/fixtures/media00.m4s"),
    include_bytes!("../../../../crates/hls-fix/tests/fixtures/media01.m4s"),
];

/// A DASH presentation with a video and an audio track, by path under the
/// presentation's directory.
const DASH_FILES: [(&str, &[u8]); 8] = [
    (
        "v/init.mp4",
        include_bytes!("../../../../crates/mp4/tests/fixtures/dash/video-init.mp4"),
    ),
    (
        "v/t0.m4s",
        include_bytes!("../../../../crates/mp4/tests/fixtures/dash/video-1.m4s"),
    ),
    (
        "v/t12288.m4s",
        include_bytes!("../../../../crates/mp4/tests/fixtures/dash/video-2.m4s"),
    ),
    (
        "a/init.mp4",
        include_bytes!("../../../../crates/mp4/tests/fixtures/dash/audio-init.mp4"),
    ),
    (
        "a/t0.m4s",
        include_bytes!("../../../../crates/mp4/tests/fixtures/dash/audio-1.m4s"),
    ),
    (
        "a/t45056.m4s",
        include_bytes!("../../../../crates/mp4/tests/fixtures/dash/audio-2.m4s"),
    ),
    (
        "a/t96256.m4s",
        include_bytes!("../../../../crates/mp4/tests/fixtures/dash/audio-3.m4s"),
    ),
    ("stream.mpd", STATIC_MPD.as_bytes()),
];

/// [`DASH_FILES`] as a static presentation: two video segments and three
/// audio segments addressed by `$Time$` timelines, 2.1 s in all.
const STATIC_MPD: &str = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2.1S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/t$Time$.m4s" initialization="v/init.mp4" timescale="12288">
        <SegmentTimeline><S t="0" d="12288" r="1"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v" bandwidth="40000" codecs="avc1.64000a"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="a/t$Time$.m4s" initialization="a/init.mp4" timescale="48000">
        <SegmentTimeline><S t="0" d="45056"/><S d="51200"/><S d="4096"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="a" bandwidth="24000" codecs="mp4a.40.2"/>
    </AdaptationSet>
  </Period>
</MPD>"#;

/// A presentation whose only video is encrypted (`ContentProtection`).
const DRM_MPD: &str = include_str!("../../../../crates/mesio/tests/fixtures/dash/drm.mpd");

pub(super) const M3U8: &str = "application/vnd.apple.mpegurl";
pub(super) const MPD: &str = "application/dash+xml";

/// The segment container of an HLS stream [`FakeCdn::publish_hls`] serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlsMedia {
    Ts,
    Fmp4,
}

impl HlsMedia {
    /// The `media` a fake-platform stream of this container names.
    pub fn media_format(self) -> &'static str {
        match self {
            Self::Ts => "ts",
            Self::Fmp4 => "fmp4",
        }
    }
}

struct File {
    content_type: &'static str,
    body: Bytes,
}

struct Fault {
    status: StatusCode,
    headers: Vec<(HeaderName, HeaderValue)>,
}

#[derive(Default)]
struct State {
    files: HashMap<String, File>,
    faults: HashMap<String, VecDeque<Fault>>,
}

impl State {
    fn reply(&mut self, path: &str) -> Response {
        if let Some(fault) = self.faults.get_mut(path).and_then(VecDeque::pop_front) {
            let mut response = fault.status.into_response();
            response.headers_mut().extend(fault.headers);
            return response;
        }
        match self.files.get(path) {
            Some(file) => (
                [(header::CONTENT_TYPE, file.content_type)],
                file.body.clone(),
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }
}

/// The media server. Paths are absolute (`/show/index.m3u8`).
pub struct FakeCdn {
    server: LocalHttpServer,
    state: Arc<Mutex<State>>,
}

impl FakeCdn {
    pub async fn start() -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let server = LocalHttpServer::start_with(move |_, request| {
            let path = request.path.split('?').next().unwrap_or_default();
            shared.lock().expect("CDN state lock").reply(path)
        })
        .await;
        Self { server, state }
    }

    pub fn url(&self, path: &str) -> String {
        self.server.url(path)
    }

    /// Serve `body` at `path` from now on.
    pub fn put(&self, path: &str, content_type: &'static str, body: impl Into<Bytes>) {
        self.state.lock().expect("CDN state lock").files.insert(
            path.to_owned(),
            File {
                content_type,
                body: body.into(),
            },
        );
    }

    /// Answer the next `times` requests for `path` with `status` and
    /// `headers`, before what [`Self::put`] stored there.
    pub fn fail(&self, path: &str, times: usize, status: u16, headers: &[(&str, &str)]) {
        let status = StatusCode::from_u16(status).expect("valid status");
        let headers: Vec<_> = headers
            .iter()
            .map(|(name, value)| {
                (
                    HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
                    HeaderValue::from_str(value).expect("valid header value"),
                )
            })
            .collect();
        let mut state = self.state.lock().expect("CDN state lock");
        let queue = state.faults.entry(path.to_owned()).or_default();
        for _ in 0..times {
            queue.push_back(Fault {
                status,
                headers: headers.clone(),
            });
        }
    }

    /// How many of the failures [`Self::fail`] queued for `path` are still
    /// to be served.
    pub fn queued_failures(&self, path: &str) -> usize {
        self.state
            .lock()
            .expect("CDN state lock")
            .faults
            .get(path)
            .map_or(0, VecDeque::len)
    }

    /// Every request received so far, in arrival order.
    pub fn requests(&self) -> Vec<ReceivedRequest> {
        self.server.received()
    }

    /// The requests for `path`, in arrival order.
    pub fn requests_to(&self, path: &str) -> Vec<ReceivedRequest> {
        self.requests()
            .into_iter()
            .filter(|request| request.path.split('?').next() == Some(path))
            .collect()
    }

    /// Wait until `path` has received `count` requests.
    pub async fn wait_for_requests(&self, path: &str, count: usize) {
        eventually(WAIT, &format!("request {count} for {path}"), || async {
            (self.requests_to(path).len() >= count).then_some(())
        })
        .await;
    }

    /// Serve the segments of an HLS stream under `/{name}/` and publish its
    /// media playlist at `/{name}/index.m3u8` listing the first `listed` of
    /// them (at most two), ended by `#EXT-X-ENDLIST` when `ended`. Returns the
    /// playlist's path; publish again to change it.
    pub fn publish_hls(&self, name: &str, media: HlsMedia, listed: usize, ended: bool) -> String {
        let (segments, extension, content_type) = match media {
            HlsMedia::Ts => (TS_SEGMENTS, "ts", "video/mp2t"),
            HlsMedia::Fmp4 => (FMP4_SEGMENTS, "m4s", "video/iso.segment"),
        };
        let mut playlist = String::from(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n",
        );
        if media == HlsMedia::Fmp4 {
            self.put(&format!("/{name}/init.mp4"), "video/mp4", FMP4_INIT);
            playlist.push_str("#EXT-X-MAP:URI=\"init.mp4\"\n");
        }
        for (index, segment) in segments.iter().enumerate() {
            let file = format!("segment{index}.{extension}");
            self.put(&format!("/{name}/{file}"), content_type, *segment);
            if index < listed {
                playlist.push_str(&format!("#EXTINF:1.000000,\n{file}\n"));
            }
        }
        if ended {
            playlist.push_str("#EXT-X-ENDLIST\n");
        }
        let path = format!("/{name}/index.m3u8");
        self.put(&path, M3U8, playlist);
        path
    }

    /// Serve a static two-track DASH presentation under `/{name}/` and return
    /// its MPD's path.
    pub fn publish_dash(&self, name: &str) -> String {
        for (file, body) in DASH_FILES {
            let content_type = if file.ends_with(".mpd") {
                MPD
            } else {
                "video/mp4"
            };
            self.put(&format!("/{name}/{file}"), content_type, body);
        }
        format!("/{name}/stream.mpd")
    }

    /// Serve a DASH presentation whose video is DRM-protected at
    /// `/{name}/stream.mpd` and return that path.
    pub fn publish_protected_dash(&self, name: &str) -> String {
        let path = format!("/{name}/stream.mpd");
        self.put(&path, MPD, DRM_MPD);
        path
    }
}
