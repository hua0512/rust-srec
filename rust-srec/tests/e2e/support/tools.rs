//! Helpers for the scenarios that need real media tools: ffmpeg runs,
//! ffprobe readings and a loopback HLS origin serving media ffmpeg made.
//!
//! Those scenarios are `#[ignore]`d so the plain suite runs on any machine;
//! CI's tool-gated job, which installs ffmpeg, rclone and Streamlink, runs
//! every ignored test but the `known_bug_*` ones.

use serde_json::Value;

use super::WAIT;
use super::cdn::{FakeCdn, M3U8};
use super::http::ReceivedRequest;

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
    cdn: FakeCdn,
    segments: usize,
}

const PLAYLIST: &str = "/live/index.m3u8";

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

        let cdn = FakeCdn::start().await;
        for index in 0..segments {
            let path = dir.path().join(format!("seg{index}.ts"));
            let segment = std::fs::read(&path)
                .unwrap_or_else(|error| panic!("ffmpeg made no {}: {error}", path.display()));
            cdn.put(&segment_path(index), "video/mp2t", segment);
        }
        let origin = Self { cdn, segments };
        origin.publish(false);
        origin
    }

    fn publish(&self, ended: bool) {
        let mut playlist = String::from(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n",
        );
        for index in 0..self.segments {
            playlist.push_str(&format!("#EXTINF:1.000,\nseg{index}.ts\n"));
        }
        if ended {
            playlist.push_str("#EXT-X-ENDLIST\n");
        }
        self.cdn.put(PLAYLIST, M3U8, playlist);
    }

    pub fn playlist_url(&self) -> String {
        self.cdn.url(PLAYLIST)
    }

    /// Mark the playlist complete, as a broadcast that has ended.
    pub fn end(&self) {
        self.publish(true);
    }

    /// The requests for segment `index`.
    pub fn segment_requests(&self, index: usize) -> Vec<ReceivedRequest> {
        self.cdn.requests_to(&segment_path(index))
    }

    /// Wait until segment `index` has been requested.
    pub async fn wait_for_segment(&self, index: usize) {
        self.cdn.wait_for_requests(&segment_path(index), 1).await;
    }
}

fn segment_path(index: usize) -> String {
    format!("/live/seg{index}.ts")
}
