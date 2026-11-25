use std::io;
use std::process::Stdio;
use std::time::Duration;

use rust_srec::backend::{
    EngineType, FfmpegEngineConfig, ServiceContainer, StreamlinkEngineConfig,
};
use serde::Serialize;
use tokio::process::Command;

use crate::DesktopBackendState;

const TOOL_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const MIN_RECOMMENDED_FFMPEG: (u64, u64) = (6, 1);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Ordered from healthy to broken; `probe_ffmpeg` relies on the ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAvailability {
    Available,
    Outdated,
    Missing,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopToolStatus {
    pub id: String,
    pub name: String,
    pub configured_path: String,
    pub status: ToolAvailability,
    pub version: Option<String>,
    pub detail: Option<String>,
}

/// The path the backend runs an external tool from: `configured` when set,
/// otherwise the `env_var` environment variable, otherwise `default`.
///
/// Mirrors `rust_srec::utils::tools::resolve`, which is crate-private; keep the
/// two in step so this check probes the binary the backend actually runs.
fn resolve_tool(configured: Option<&str>, env_var: &str, default: &str) -> String {
    configured
        .map(str::to_owned)
        .or_else(|| std::env::var(env_var).ok())
        .unwrap_or_else(|| default.to_owned())
}

/// The engine configuration recordings use when a streamer has no engine of its
/// own, resolved like `DownloadManager::resolve_engine`: a bare engine type
/// names the built-in engine with default settings, anything else is the id
/// of a stored engine configuration.
enum DefaultEngine {
    Ffmpeg(FfmpegEngineConfig),
    Streamlink(StreamlinkEngineConfig),
    Other,
}

async fn load_default_engine(container: &ServiceContainer) -> DefaultEngine {
    let config_service = container.config_service();
    let engine_id = match config_service.get_global_config().await {
        Ok(config) => config.default_download_engine,
        Err(error) => {
            log::warn!("Failed to read global config for desktop tool check: {error}");
            return DefaultEngine::Other;
        }
    };

    if let Ok(engine_type) = engine_id.parse::<EngineType>() {
        return match engine_type {
            EngineType::Ffmpeg => DefaultEngine::Ffmpeg(FfmpegEngineConfig::default()),
            EngineType::Streamlink => DefaultEngine::Streamlink(StreamlinkEngineConfig::default()),
            EngineType::Mesio => DefaultEngine::Other,
        };
    }

    let engine = match config_service.get_engine_config(&engine_id).await {
        Ok(engine) => engine,
        Err(error) => {
            log::warn!(
                "Failed to load default engine '{engine_id}' for desktop tool check: {error}"
            );
            return DefaultEngine::Other;
        }
    };
    let parsed = match engine.engine_type.parse::<EngineType>() {
        Ok(EngineType::Ffmpeg) => serde_json::from_str(&engine.config).map(DefaultEngine::Ffmpeg),
        Ok(EngineType::Streamlink) => {
            serde_json::from_str(&engine.config).map(DefaultEngine::Streamlink)
        }
        Ok(EngineType::Mesio) | Err(_) => Ok(DefaultEngine::Other),
    };
    parsed.unwrap_or_else(|error| {
        log::warn!("Invalid configuration for default engine '{engine_id}': {error}");
        DefaultEngine::Other
    })
}

/// Paths to probe, matching what the backend runs.
struct ToolPaths {
    /// FFmpeg used by post-processing (remux, thumbnails, metadata, audio
    /// extraction), which never reads the download engine's settings.
    pipeline_ffmpeg: String,
    /// FFmpeg run by the default download engine, when it runs one.
    engine_ffmpeg: Option<String>,
    ffprobe: String,
    streamlink: String,
}

async fn resolve_tool_paths(state: &DesktopBackendState) -> ToolPaths {
    let default_engine = match state.backend() {
        Some(container) => load_default_engine(&container).await,
        None => DefaultEngine::Other,
    };

    let (engine_ffmpeg, streamlink) = match default_engine {
        DefaultEngine::Ffmpeg(config) => (Some(config.binary_path), None),
        DefaultEngine::Streamlink(config) => (
            Some(resolve_tool(
                config.ffmpeg_path.as_deref(),
                "FFMPEG_PATH",
                "ffmpeg",
            )),
            Some(config.binary_path),
        ),
        DefaultEngine::Other => (None, None),
    };

    ToolPaths {
        pipeline_ffmpeg: resolve_tool(None, "FFMPEG_PATH", "ffmpeg"),
        engine_ffmpeg,
        ffprobe: resolve_tool(None, "FFPROBE_PATH", "ffprobe"),
        streamlink: streamlink.unwrap_or_else(|| StreamlinkEngineConfig::default().binary_path),
    }
}

/// Extract the version token reported by `ffmpeg -version`, `ffprobe -version`,
/// or `streamlink --version`.
pub fn parse_tool_version(tool_id: &str, output: &str) -> Option<String> {
    let first_line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;

    let prefix = match tool_id {
        "ffmpeg" => "ffmpeg version ",
        "ffprobe" => "ffprobe version ",
        "streamlink" => "streamlink ",
        _ => return None,
    };
    let head = first_line.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(prefix) {
        return None;
    }
    first_line[prefix.len()..]
        .split_whitespace()
        .next()
        .map(str::to_string)
}

/// Parse a leading `major.minor` pair from a tool version string, tolerating an
/// optional `n`/`v` prefix and trailing build suffixes.
pub fn parse_major_minor(version: &str) -> Option<(u64, u64)> {
    let trimmed = version.trim();
    let stripped = match trimmed.as_bytes() {
        [b'n' | b'N' | b'v' | b'V', next, ..] if next.is_ascii_digit() => &trimmed[1..],
        _ => trimmed,
    };

    let mut parts = stripped.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor_part = parts.next()?;
    let minor_digits: String = minor_part
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if minor_digits.is_empty() {
        return None;
    }
    let minor = minor_digits.parse::<u64>().ok()?;
    Some((major, minor))
}

/// Return `true` when an FFmpeg/FFprobe version parses to a release older than 6.1.
pub fn is_ffmpeg_outdated(version: &str) -> bool {
    parse_major_minor(version).is_some_and(|pair| pair < MIN_RECOMMENDED_FFMPEG)
}

async fn probe_tool(
    id: &'static str,
    name: &'static str,
    configured_path: String,
    version_arg: &'static str,
) -> DesktopToolStatus {
    let mut command = Command::new(&configured_path);
    command
        .arg(version_arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }

    match tokio::time::timeout(TOOL_CHECK_TIMEOUT, command.output()).await {
        Err(_) => DesktopToolStatus {
            id: id.to_string(),
            name: name.to_string(),
            configured_path,
            status: ToolAvailability::Error,
            version: None,
            detail: Some(format!(
                "Timed out after {}s while checking version",
                TOOL_CHECK_TIMEOUT.as_secs()
            )),
        },
        Ok(Err(error)) if error.kind() == io::ErrorKind::NotFound => DesktopToolStatus {
            id: id.to_string(),
            name: name.to_string(),
            configured_path: configured_path.clone(),
            status: ToolAvailability::Missing,
            version: None,
            detail: Some(format!("Executable '{configured_path}' was not found")),
        },
        Ok(Err(error)) => DesktopToolStatus {
            id: id.to_string(),
            name: name.to_string(),
            configured_path,
            status: ToolAvailability::Error,
            version: None,
            detail: Some(format!("Failed to launch process: {error}")),
        },
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = if stdout.trim().is_empty() {
                stderr.as_ref()
            } else {
                stdout.as_ref()
            };

            if !output.status.success() {
                let first_line = combined
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty());
                let detail = match (output.status.code(), first_line) {
                    (Some(code), Some(line)) => format!("Exited with status {code}: {line}"),
                    (Some(code), None) => format!("Exited with status {code}"),
                    (None, Some(line)) => format!("Process terminated abnormally: {line}"),
                    (None, None) => "Process terminated abnormally".to_string(),
                };
                return DesktopToolStatus {
                    id: id.to_string(),
                    name: name.to_string(),
                    configured_path,
                    status: ToolAvailability::Error,
                    version: None,
                    detail: Some(detail),
                };
            }

            let version = parse_tool_version(id, combined);
            let outdated = matches!(id, "ffmpeg" | "ffprobe")
                && version.as_deref().is_some_and(is_ffmpeg_outdated);

            let (status, detail) = if outdated {
                (
                    ToolAvailability::Outdated,
                    Some(format!(
                        "FFmpeg 6.1 or newer is recommended (detected {}).",
                        version.as_deref().unwrap_or("unknown")
                    )),
                )
            } else {
                (ToolAvailability::Available, None)
            };

            DesktopToolStatus {
                id: id.to_string(),
                name: name.to_string(),
                configured_path,
                status,
                version,
                detail,
            }
        }
    }
}

/// Probe FFmpeg for the single FFmpeg row. When the default download engine
/// runs a different FFmpeg than post-processing, both are probed and the row
/// reports the one in the worse state, so it never shows a working binary while
/// the other one is broken.
async fn probe_ffmpeg(pipeline_path: String, engine_path: Option<String>) -> DesktopToolStatus {
    match engine_path.filter(|path| *path != pipeline_path) {
        None => probe_tool("ffmpeg", "FFmpeg", pipeline_path, "-version").await,
        Some(engine_path) => {
            let (pipeline, engine) = tokio::join!(
                probe_tool("ffmpeg", "FFmpeg", pipeline_path, "-version"),
                probe_tool("ffmpeg", "FFmpeg", engine_path, "-version"),
            );
            if engine.status > pipeline.status {
                engine
            } else {
                pipeline
            }
        }
    }
}

#[tauri::command]
pub async fn check_desktop_tools(
    state: tauri::State<'_, DesktopBackendState>,
) -> Result<Vec<DesktopToolStatus>, String> {
    let paths = resolve_tool_paths(&state).await;

    let (ffmpeg, ffprobe, streamlink) = tokio::join!(
        probe_ffmpeg(paths.pipeline_ffmpeg, paths.engine_ffmpeg),
        probe_tool("ffprobe", "FFprobe", paths.ffprobe, "-version"),
        probe_tool("streamlink", "Streamlink", paths.streamlink, "--version"),
    );

    Ok(vec![ffmpeg, ffprobe, streamlink])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffmpeg_ffprobe_and_streamlink_versions() {
        assert_eq!(
            parse_tool_version(
                "ffmpeg",
                "ffmpeg version 7.1-essentials_build-www.gyan.dev Copyright (c) 2000-2024\nbuilt with gcc"
            ),
            Some("7.1-essentials_build-www.gyan.dev".to_string())
        );
        assert_eq!(
            parse_tool_version(
                "ffmpeg",
                "ffmpeg version N-117489-g9f8c7b6-20241015 Copyright (c) 2000-2024"
            ),
            Some("N-117489-g9f8c7b6-20241015".to_string())
        );
        assert_eq!(
            parse_tool_version("ffmpeg", "ffmpeg version n7.0 Copyright (c) 2000-2024"),
            Some("n7.0".to_string())
        );
        assert_eq!(
            parse_tool_version(
                "ffprobe",
                "ffprobe version 6.1.1-3ubuntu5 Copyright (c) 2007-2023"
            ),
            Some("6.1.1-3ubuntu5".to_string())
        );
        assert_eq!(
            parse_tool_version(
                "ffmpeg",
                "ffmpeg version 2024-10-15-git-4a7b2d5d7e-full_build-www.gyan.dev Copyright (c) 2000-2024"
            ),
            Some("2024-10-15-git-4a7b2d5d7e-full_build-www.gyan.dev".to_string())
        );
        assert_eq!(
            parse_tool_version("streamlink", "streamlink 6.11.0\n"),
            Some("6.11.0".to_string())
        );
        assert_eq!(parse_tool_version("ffmpeg", "unexpected output"), None);
    }

    #[test]
    fn flags_only_known_older_ffmpeg_releases_as_outdated() {
        assert!(is_ffmpeg_outdated("6.0"));
        assert!(is_ffmpeg_outdated("n6.0.1"));
        assert!(is_ffmpeg_outdated("5.1.4-essentials_build"));

        assert!(!is_ffmpeg_outdated("6.1.0"));
        assert!(!is_ffmpeg_outdated("6.1"));
        assert!(!is_ffmpeg_outdated("n7.0.2"));
        assert!(!is_ffmpeg_outdated("7.1-essentials_build"));

        // Git / nightly snapshot builds must not falsely trigger an outdated warning.
        assert!(!is_ffmpeg_outdated(
            "2024-10-15-git-4a7b2d5d7e-full_build-www.gyan.dev"
        ));
        assert!(!is_ffmpeg_outdated("N-117489-g9f8c7b6"));
    }
}
