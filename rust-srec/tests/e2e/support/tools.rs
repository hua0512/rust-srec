//! Helpers for the scenarios that need real media tools: ffmpeg runs,
//! ffprobe readings and a loopback HLS origin serving media ffmpeg made.
//!
//! Those scenarios are `#[ignore]`d so the plain suite runs on any machine;
//! CI's tool-gated job, which installs ffmpeg, rclone and Streamlink, runs
//! every ignored test but the `known_bug_*` ones.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde_json::Value;

use super::http::ReceivedRequest;
use super::{WAIT, eventually};

/// Run `program` with `args` and return its stdout, panicking with its
/// stderr unless it exits successfully within [`WAIT`].
async fn run(program: &str, args: &[&str]) -> Vec<u8> {
    let output = tokio::time::timeout(
        WAIT,
        tokio::process::Command::new(program)
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap_or_else(|_| panic!("{program} did not finish within {WAIT:?}"))
    .unwrap_or_else(|error| panic!("run {program}: {error}"));
    assert!(
        output.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

pub async fn ffmpeg(args: &[&str]) {
    run("ffmpeg", args).await;
}

/// ffprobe's description of the file at `path`: its `format` (with `tags`)
/// and `streams`.
pub async fn probe(path: &str) -> Value {
    let stdout = run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
            path,
        ],
    )
    .await;
    serde_json::from_slice(&stdout).expect("ffprobe prints JSON")
}

/// The `codec_name` of each stream in a [`probe`] result, in stream order.
pub fn codecs(probe: &Value) -> Vec<&str> {
    probe["streams"]
        .as_array()
        .expect("probed streams")
        .iter()
        .map(|stream| stream["codec_name"].as_str().expect("codec name"))
        .collect()
}

/// The container duration of a [`probe`] result, in seconds.
pub fn duration_secs(probe: &Value) -> f64 {
    probe["format"]["duration"]
        .as_str()
        .and_then(|duration| duration.parse().ok())
        .unwrap_or_else(|| panic!("no duration in {probe}"))
}

/// A live HLS stream on 127.0.0.1: a media playlist at `/live/index.m3u8`
/// (Streamlink claims a URL by its `.m3u8` path) listing one-second MPEG-TS
/// segments of 32x32 H.264 with AAC audio. The playlist stays live, without
/// `#EXT-X-ENDLIST`, until [`Self::end`].
pub struct HlsOrigin {
    origin: String,
    shared: Arc<HlsShared>,
    task: tokio::task::JoinHandle<()>,
}

struct HlsShared {
    segments: Vec<Bytes>,
    ended: AtomicBool,
    requests: Mutex<Vec<ReceivedRequest>>,
}

impl HlsOrigin {
    /// Make `segments` segments with ffmpeg and start serving them.
    pub async fn start(segments: usize) -> Self {
        let dir = tempfile::tempdir().expect("create the HLS work directory");
        let pattern = dir.path().join("seg%d.ts");
        let playlist = dir.path().join("made.m3u8");
        ffmpeg(&[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=32x32:rate=5",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=16000",
            "-t",
            &segments.to_string(),
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "5",
            "-c:a",
            "aac",
            "-b:a",
            "16k",
            "-ac",
            "1",
            "-f",
            "hls",
            "-hls_time",
            "1",
            "-hls_list_size",
            "0",
            "-hls_segment_filename",
            &pattern.to_string_lossy(),
            &playlist.to_string_lossy(),
        ])
        .await;
        let segments =
            (0..segments)
                .map(|index| {
                    let path = dir.path().join(format!("seg{index}.ts"));
                    Bytes::from(std::fs::read(&path).unwrap_or_else(|error| {
                        panic!("ffmpeg made no {}: {error}", path.display())
                    }))
                })
                .collect();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the HLS origin");
        let address = listener.local_addr().expect("HLS origin address");
        let shared = Arc::new(HlsShared {
            segments,
            ended: AtomicBool::new(false),
            requests: Mutex::new(Vec::new()),
        });
        let state = shared.clone();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let shared = state.clone();
            async move { shared.serve(request).await }
        });
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve the HLS origin");
        });
        Self {
            origin: format!("http://{address}"),
            shared,
            task,
        }
    }

    pub fn playlist_url(&self) -> String {
        format!("{}/live/index.m3u8", self.origin)
    }

    /// Mark the playlist complete, as a broadcast that has ended.
    pub fn end(&self) {
        self.shared.ended.store(true, Ordering::SeqCst);
    }

    /// Every request received so far, in arrival order.
    pub fn requests(&self) -> Vec<ReceivedRequest> {
        self.shared
            .requests
            .lock()
            .expect("HLS request log lock")
            .clone()
    }

    /// The requests for segment `index`.
    pub fn segment_requests(&self, index: usize) -> Vec<ReceivedRequest> {
        let path = format!("/live/seg{index}.ts");
        self.requests()
            .into_iter()
            .filter(|request| request.path.split('?').next() == Some(path.as_str()))
            .collect()
    }

    /// Wait until segment `index` has been requested.
    pub async fn wait_for_segment(&self, index: usize) {
        eventually(WAIT, &format!("a request for HLS segment {index}"), || {
            let requested = !self.segment_requests(index).is_empty();
            async move { requested.then_some(()) }
        })
        .await;
    }
}

impl HlsShared {
    fn playlist(&self) -> String {
        let mut playlist = String::from(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n",
        );
        for index in 0..self.segments.len() {
            playlist.push_str(&format!("#EXTINF:1.000,\nseg{index}.ts\n"));
        }
        if self.ended.load(Ordering::SeqCst) {
            playlist.push_str("#EXT-X-ENDLIST\n");
        }
        playlist
    }

    async fn serve(&self, request: axum::extract::Request) -> Response {
        let request = ReceivedRequest::from_axum(request).await;
        let path = request
            .path
            .split('?')
            .next()
            .unwrap_or_default()
            .to_owned();
        self.requests
            .lock()
            .expect("HLS request log lock")
            .push(request);
        if path == "/live/index.m3u8" {
            return (
                [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
                self.playlist(),
            )
                .into_response();
        }
        let segment = path
            .strip_prefix("/live/seg")
            .and_then(|rest| rest.strip_suffix(".ts"))
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| self.segments.get(index));
        match segment {
            Some(bytes) => ([(header::CONTENT_TYPE, "video/mp2t")], bytes.clone()).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }
}

impl Drop for HlsOrigin {
    fn drop(&mut self) {
        self.task.abort();
    }
}
