use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::DesktopBackendState;
use crate::desktop_settings::DesktopSettings;

pub const EVENT_DESKTOP_UPDATE_AVAILABLE: &str = "rust-srec://desktop-update-available";

const GITHUB_RELEASES_API_URL: &str =
    "https://api.github.com/repos/hua0512/rust-srec/releases?per_page=50";
const RELEASE_TAG_PREFIX: &str = "rust-srec-v";
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_NOTES_EXCERPT_CHARS: usize = 400;
/// Upper bound on the release-list body. Fifty releases with their asset lists
/// come to about 1.5 MB, so this leaves room to grow while refusing an
/// unbounded response.
const MAX_RELEASES_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_url: String,
    pub published_at: Option<String>,
    pub notes_excerpt: Option<String>,
}

/// Parse a `major.minor.patch` version, ignoring build metadata. Returns the
/// core triple and whether a pre-release identifier (`-rc.1`) follows it.
fn parse_version(raw: &str) -> Option<((u64, u64, u64), bool)> {
    let trimmed = raw.trim();
    let without_build = trimmed.split_once('+').map_or(trimmed, |(core, _)| core);
    let (core, prerelease) = match without_build.split_once('-') {
        Some((core, _)) => (core, true),
        None => (without_build, false),
    };

    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }

    Some(((major, minor, patch), prerelease))
}

/// Parse a stable `major.minor.patch` version, rejecting pre-releases.
pub fn parse_semver(raw: &str) -> Option<(u64, u64, u64)> {
    match parse_version(raw)? {
        (version, false) => Some(version),
        (_, true) => None,
    }
}

/// Return `true` when the stable `candidate` is newer than `current`. A
/// pre-release `current` ranks below the stable release with the same core, so
/// `0.9.0-rc.1` is offered `0.9.0`.
pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    match (parse_version(current), parse_semver(candidate)) {
        (Some((current_ver, current_pre)), Some(candidate_ver)) => {
            candidate_ver > current_ver || (current_pre && candidate_ver == current_ver)
        }
        _ => false,
    }
}

fn truncate_notes_excerpt(body: Option<&str>) -> Option<String> {
    let trimmed = body?.trim();
    if trimmed.is_empty() {
        return None;
    }

    let total_chars = trimmed.chars().count();
    if total_chars <= MAX_NOTES_EXCERPT_CHARS {
        return Some(trimmed.to_string());
    }

    let prefix: String = trimmed
        .chars()
        .take(MAX_NOTES_EXCERPT_CHARS.saturating_sub(3))
        .collect();
    Some(format!("{}...", prefix.trim_end()))
}

/// Select the highest stable `rust-srec-v*` release from the GitHub release list.
pub fn select_latest_release(
    releases: &[GitHubRelease],
    current_version: &str,
) -> Option<DesktopUpdateInfo> {
    let (latest_tuple, best) = releases
        .iter()
        .filter(|release| {
            !release.draft && !release.prerelease && release.html_url.starts_with("https://")
        })
        .filter_map(|release| {
            let version_str = release.tag_name.strip_prefix(RELEASE_TAG_PREFIX)?;
            let parsed = parse_semver(version_str)?;
            Some((parsed, release))
        })
        .max_by_key(|(parsed, _)| *parsed)?;

    let latest_version = format!("{}.{}.{}", latest_tuple.0, latest_tuple.1, latest_tuple.2);
    let update_available = is_newer_version(current_version, &latest_version);

    Some(DesktopUpdateInfo {
        current_version: current_version.to_string(),
        latest_version,
        update_available,
        release_url: best.html_url.clone(),
        published_at: best.published_at.clone(),
        notes_excerpt: truncate_notes_excerpt(best.body.as_deref()),
    })
}

pub async fn fetch_update_info() -> Result<DesktopUpdateInfo, String> {
    rust_srec::backend::install_rustls_provider();

    let current_version = env!("CARGO_PKG_VERSION");
    let client = reqwest::Client::builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .user_agent(format!("rust-srec-desktop/{current_version}"))
        .build()
        .map_err(|error| format!("Failed to initialize HTTP client: {error}"))?;

    let mut response = client
        .get(GITHUB_RELEASES_API_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| format!("Failed to check for updates: {error}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "GitHub Releases API returned HTTP {}",
            status.as_u16()
        ));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Failed to read GitHub releases response: {error}"))?
    {
        if body.len() + chunk.len() > MAX_RELEASES_RESPONSE_BYTES {
            return Err(format!(
                "GitHub releases response exceeded {} bytes",
                MAX_RELEASES_RESPONSE_BYTES
            ));
        }
        body.extend_from_slice(&chunk);
    }

    let releases = serde_json::from_slice::<Vec<GitHubRelease>>(&body)
        .map_err(|error| format!("Failed to parse GitHub releases response: {error}"))?;

    select_latest_release(&releases, current_version)
        .ok_or_else(|| "No published Rust-Srec release found".to_string())
}

#[tauri::command]
pub async fn check_for_updates() -> Result<DesktopUpdateInfo, String> {
    fetch_update_info().await
}

#[tauri::command]
pub async fn skip_update_version(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopBackendState>,
    version: Option<String>,
) -> Result<DesktopSettings, String> {
    let normalized = version
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let _write_guard = state.settings_write_lock.lock().await;
    let mut settings = state.desktop_settings();
    settings.skipped_update_version = normalized;

    state.commit_desktop_settings(&app, &settings).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag_name: &str, draft: bool, prerelease: bool) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag_name.to_string(),
            html_url: format!("https://github.com/hua0512/rust-srec/releases/tag/{tag_name}"),
            draft,
            prerelease,
            published_at: Some("2026-10-01T00:00:00Z".to_string()),
            body: Some(format!("Notes for {tag_name}")),
        }
    }

    #[test]
    fn compares_versions_and_offers_stable_release_to_prerelease_users() {
        assert!(is_newer_version("0.8.0", "0.8.1"));
        assert!(is_newer_version("0.8.0", "0.10.0"));
        assert!(!is_newer_version("0.8.0", "0.8.0"));
        assert!(!is_newer_version("0.9.0", "0.8.5"));
        assert!(!is_newer_version("0.8.0", "0.9.0-beta.1"));

        assert!(is_newer_version("0.9.0-rc.1", "0.9.0"));
        assert!(is_newer_version("0.9.0-rc.1", "0.9.1"));
        assert!(!is_newer_version("0.9.0-rc.1", "0.8.5"));
    }

    #[test]
    fn selects_highest_stable_rust_srec_release_ignoring_noise() {
        let releases = vec![
            release("mesio-v2.0.0", false, false),
            release("rust-srec-v0.11.0", true, false),
            release("rust-srec-v0.10.0-rc.1", false, true),
            release("rust-srec-v0.8.5", false, false),
            release("rust-srec-v0.9.2", false, false),
            release("rust-srec-v0.9.1", false, false),
        ];

        let selected = select_latest_release(&releases, "0.8.0").expect("should pick release");
        assert_eq!(selected.current_version, "0.8.0");
        assert_eq!(selected.latest_version, "0.9.2");
        assert!(selected.update_available);
        assert_eq!(
            selected.release_url,
            "https://github.com/hua0512/rust-srec/releases/tag/rust-srec-v0.9.2"
        );

        let up_to_date = select_latest_release(&releases, "0.9.2").expect("should pick release");
        assert!(!up_to_date.update_available);
    }
}
