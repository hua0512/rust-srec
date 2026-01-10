use crate::extractor::error::ExtractorError;
use crate::extractor::platform_extractor::{Extractor, PlatformExtractor};
use crate::media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo};
use async_trait::async_trait;
use reqwest::Client;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::LazyLock;
use tokio::process::Command;

static DEFAULT_STREAMLINK_PATH: &str = "streamlink";
static DEFAULT_STREAMLINK_QUALITY: &str = "best";

static STREAMLINK_AVAILABLE: LazyLock<bool> = LazyLock::new(|| {
    std::process::Command::new(DEFAULT_STREAMLINK_PATH)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
});

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct StreamlinkConfig {
    #[serde(default)]
    binary_path: Option<String>,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    extra_args: Vec<String>,
}

impl StreamlinkConfig {
    fn from_extras(extras: Option<&serde_json::Value>) -> Self {
        let Some(extras) = extras else {
            return Self::default();
        };
        let Some(v) = extras.get("streamlink") else {
            return Self::default();
        };
        serde_json::from_value(v.clone()).unwrap_or_default()
    }

    fn binary_path(&self) -> String {
        self.binary_path
            .clone()
            .or_else(|| std::env::var("STREAMLINK_PATH").ok())
            .unwrap_or_else(|| DEFAULT_STREAMLINK_PATH.to_string())
    }

    fn quality(&self) -> String {
        self.quality
            .clone()
            .unwrap_or_else(|| DEFAULT_STREAMLINK_QUALITY.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct StreamlinkExtractor {
    extractor: Extractor,
    config: StreamlinkConfig,
    cookie_string: Option<String>,
}

impl StreamlinkExtractor {
    pub fn is_available() -> bool {
        *STREAMLINK_AVAILABLE
    }

    pub fn new(
        url: String,
        client: Client,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Result<Self, ExtractorError> {
        let config = StreamlinkConfig::from_extras(extras.as_ref());
        let binary_path = config.binary_path();

        // If the user overrides the binary path, do a best-effort availability check.
        if binary_path != DEFAULT_STREAMLINK_PATH {
            let ok = std::process::Command::new(&binary_path)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                return Err(ExtractorError::UnsupportedExtractor);
            }
        } else if !Self::is_available() {
            return Err(ExtractorError::UnsupportedExtractor);
        }

        Ok(Self {
            extractor: Extractor::new("Streamlink", url, client),
            config,
            cookie_string: cookies,
        })
    }

    fn build_cookie_args(cookie_string: &str) -> Vec<String> {
        // Streamlink expects repeated `--http-cookie name=value`.
        cookie_string
            .split(&[';', '\n'][..])
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.trim(), v.trim())))
            .filter(|(k, v)| !k.is_empty() && !v.is_empty())
            .flat_map(|(k, v)| ["--http-cookie".to_string(), format!("{k}={v}")])
            .collect()
    }

    async fn run_streamlink_json(&self) -> Result<StreamlinkJson, ExtractorError> {
        let binary_path = self.config.binary_path();
        let mut cmd = Command::new(binary_path);
        cmd.arg("--json").arg("--url").arg(&self.extractor.url);

        if let Some(ref cookies) = self.cookie_string {
            cmd.args(Self::build_cookie_args(cookies));
        }

        cmd.args(self.config.extra_args.iter().cloned());

        let out = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ExtractorError::Other(format!("Failed to spawn streamlink: {e}")))?;

        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();

        // Streamlink prints JSON to stdout in `--json` mode (including errors).
        let parsed: Result<StreamlinkJson, _> = serde_json::from_str(&stdout);
        match parsed {
            Ok(v) => Ok(v),
            Err(e) => Err(ExtractorError::Other(format!(
                "Failed to parse streamlink JSON output: {e}; stderr: {stderr}"
            ))),
        }
    }

    async fn resolve_stream_url(&self, quality: &str) -> Result<String, ExtractorError> {
        let binary_path = self.config.binary_path();
        let mut cmd = Command::new(binary_path);
        cmd.arg("--stream-url")
            .arg("--url")
            .arg(&self.extractor.url)
            .arg(quality);

        if let Some(ref cookies) = self.cookie_string {
            cmd.args(Self::build_cookie_args(cookies));
        }

        cmd.args(self.config.extra_args.iter().cloned());

        let out = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ExtractorError::Other(format!("Failed to spawn streamlink: {e}")))?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(map_streamlink_error(&stderr));
        }

        let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if url.is_empty() {
            return Err(ExtractorError::Other(
                "Streamlink returned empty URL".to_string(),
            ));
        }
        Ok(url)
    }
}

#[async_trait]
impl PlatformExtractor for StreamlinkExtractor {
    fn get_extractor(&self) -> &Extractor {
        &self.extractor
    }

    async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
        let json = self.run_streamlink_json().await?;
        if let Some(err) = json.error.as_deref() {
            return Err(map_streamlink_error(err));
        }

        let plugin = json.plugin.unwrap_or_else(|| "unknown".to_string());
        let metadata = json.metadata.unwrap_or_default();
        let streams = json
            .streams
            .ok_or_else(|| ExtractorError::NoStreamsFound)?
            .into_iter()
            .filter_map(|(name, s)| s.url.clone().map(|url| (name, url, s)))
            .collect::<Vec<_>>();

        if streams.is_empty() {
            return Err(ExtractorError::NoStreamsFound);
        }

        let media_headers = streams
            .iter()
            .find(|(name, _, _)| name == "best")
            .or_else(|| streams.first())
            .and_then(|(_, _, s)| s.headers.clone())
            .map(|h| h.into_iter().collect::<FxHashMap<String, String>>());

        let mut media_extras = FxHashMap::default();
        media_extras.insert("streamlink_plugin".to_string(), plugin);
        if let Some(id) = metadata.id {
            media_extras.insert("id".to_string(), id);
        }
        if let Some(category) = metadata.category {
            media_extras.insert("category".to_string(), category);
        }

        let mut stream_infos = Vec::with_capacity(streams.len());
        for (idx, (name, url, s)) in streams.into_iter().enumerate() {
            let stream_format = infer_stream_format(s.stream_type.as_deref(), &url);
            let media_format = infer_media_format(stream_format, &url);
            let priority = match name.as_str() {
                "best" => 0,
                "worst" => 1000,
                _ => 10 + idx as u32,
            };

            stream_infos.push(StreamInfo {
                url,
                stream_format,
                media_format,
                quality: name,
                bitrate: 0,
                priority,
                extras: Some(
                    serde_json::json!({
                        "streamlink_type": s.stream_type.unwrap_or_default(),
                        "master": s.master,
                    })
                    .as_object()
                    .cloned()
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Null),
                )
                .filter(|v| !v.is_null()),
                codec: String::new(),
                fps: 0.0,
                is_headers_needed: false,
            });
        }

        Ok(MediaInfo {
            site_url: self.extractor.url.clone(),
            title: metadata.title.unwrap_or_default(),
            artist: metadata.author.unwrap_or_default(),
            cover_url: None,
            artist_url: None,
            is_live: true,
            streams: stream_infos,
            headers: media_headers,
            extras: Some(media_extras).filter(|m| !m.is_empty()),
        })
    }

    async fn get_url(&self, stream_info: &mut StreamInfo) -> Result<(), ExtractorError> {
        if !stream_info.url.is_empty() {
            return Ok(());
        }
        let quality = if stream_info.quality.is_empty() {
            self.config.quality()
        } else {
            stream_info.quality.clone()
        };
        let url = self.resolve_stream_url(&quality).await?;
        stream_info.url = url;
        Ok(())
    }
}

fn infer_stream_format(stream_type: Option<&str>, url: &str) -> StreamFormat {
    match stream_type.unwrap_or_default().to_lowercase().as_str() {
        "hls" => StreamFormat::Hls,
        "http" | "dash" => {
            if url.to_lowercase().contains(".m3u8") {
                StreamFormat::Hls
            } else if url.to_lowercase().contains(".flv") {
                StreamFormat::Flv
            } else {
                StreamFormat::Mp4
            }
        }
        "rtmp" => StreamFormat::Flv,
        _ => {
            let lower = url.to_lowercase();
            if lower.contains(".m3u8") {
                StreamFormat::Hls
            } else if lower.contains(".flv") {
                StreamFormat::Flv
            } else if lower.contains(".mp4") {
                StreamFormat::Mp4
            } else {
                StreamFormat::Flv
            }
        }
    }
}

fn infer_media_format(stream_format: StreamFormat, _url: &str) -> MediaFormat {
    match stream_format {
        StreamFormat::Hls => MediaFormat::Ts,
        StreamFormat::Flv => MediaFormat::Flv,
        StreamFormat::Mp4 => MediaFormat::Mp4,
        StreamFormat::Wss => MediaFormat::Flv,
    }
}

fn map_streamlink_error(msg: &str) -> ExtractorError {
    let lower = msg.to_lowercase();
    if lower.contains("no plugin can handle url") {
        return ExtractorError::UnsupportedExtractor;
    }
    if lower.contains("no streams found") || lower.contains("no playable streams") {
        return ExtractorError::NoStreamsFound;
    }
    ExtractorError::Other(msg.to_string())
}

#[derive(Debug, Clone, Deserialize)]
struct StreamlinkJson {
    #[serde(default)]
    plugin: Option<String>,
    #[serde(default)]
    metadata: Option<StreamlinkMetadata>,
    #[serde(default)]
    streams: Option<HashMap<String, StreamlinkStream>>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct StreamlinkMetadata {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct StreamlinkStream {
    #[serde(rename = "type", default)]
    stream_type: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default)]
    master: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_streamlink_json_hls() {
        let raw = r#"{
          "plugin": "hls",
          "metadata": { "id": null, "author": null, "category": null, "title": null },
          "streams": {
            "720p": {
              "type": "hls",
              "url": "https://example.com/playlist.m3u8",
              "headers": { "User-Agent": "UA" },
              "master": "https://example.com/master.m3u8"
            },
            "best": {
              "type": "hls",
              "url": "https://example.com/playlist-best.m3u8",
              "headers": { "User-Agent": "UA" },
              "master": "https://example.com/master.m3u8"
            }
          }
        }"#;

        let parsed: StreamlinkJson = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.plugin.as_deref(), Some("hls"));
        assert!(parsed.error.is_none());
        assert!(parsed.streams.as_ref().unwrap().contains_key("best"));
    }

    #[test]
    fn test_map_streamlink_error_unsupported() {
        let e = map_streamlink_error("No plugin can handle URL: https://x");
        assert!(matches!(e, ExtractorError::UnsupportedExtractor));
    }

    #[test]
    fn test_map_streamlink_error_offline() {
        let e = map_streamlink_error("No streams found on this URL");
        assert!(matches!(e, ExtractorError::NoStreamsFound));
    }
}
