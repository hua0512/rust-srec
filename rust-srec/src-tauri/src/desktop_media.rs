use std::path::{Path, PathBuf};

use tauri_plugin_opener::OpenerExt;

use crate::DesktopBackendState;
use crate::desktop_settings::BackendMode;

const PLAYABLE_MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "flv", "ts", "m2ts", "webm", "mov", "m4v", "mp3", "wav", "ogg", "aac", "m4a",
    "flac", "m3u8",
];

#[cfg(target_os = "macos")]
const MACOS_EXECUTABLE_BUNDLE_EXTENSIONS: &[&str] = &[
    "app", "pkg", "mpkg", "saver", "prefpane", "workflow", "action",
];

#[cfg(target_os = "macos")]
fn is_macos_executable_bundle(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| MACOS_EXECUTABLE_BUNDLE_EXTENSIONS.contains(&ext.as_str()))
}

/// The part of an output-folder setting that names a real directory.
///
/// The setting may be a template such as `/rec/{streamer}/%Y-%m-%d`; this is
/// everything up to and including the separator before the first component
/// that holds a `{placeholder}` or a `%` time code, or the whole setting when
/// there is none. Empty when the first component is already a placeholder.
pub(crate) fn output_folder_root(output_folder: &str) -> &str {
    let Some(placeholder) = output_folder.find(['{', '%']) else {
        return output_folder;
    };
    match output_folder[..placeholder].rfind(['/', '\\']) {
        Some(separator) => &output_folder[..=separator],
        None => "",
    }
}

/// Ensure a recorded media output is a playable audio/video file with an
/// allowlisted media extension before handing it to the OS default handler.
pub fn validate_playable_media_output(file_type: &str, path: &Path) -> Result<(), String> {
    if !(file_type.eq_ignore_ascii_case("VIDEO") || file_type.eq_ignore_ascii_case("AUDIO")) {
        return Err(format!("Unsupported media output type '{file_type}'"));
    }

    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "Media file has no valid extension".to_string())?;

    if !PLAYABLE_MEDIA_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("Unsupported media file extension '.{ext}'"));
    }

    Ok(())
}

/// Validate that `path` points to an existing non-bundle directory so the
/// system file manager opener is never handed a regular file, symlink to a
/// file, or macOS executable bundle.
pub fn validate_existing_directory(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Directory path cannot be empty".to_string());
    }

    let candidate = PathBuf::from(trimmed);
    #[cfg(target_os = "macos")]
    if is_macos_executable_bundle(&candidate) {
        return Err("Refusing to open an application bundle as a folder".to_string());
    }

    // `dunce` drops the Windows verbatim prefix (`\\?\`) where the path stays
    // valid without it, which is the form Explorer and other programs expect.
    let canonical = dunce::canonicalize(&candidate)
        .map_err(|e| format!("Directory does not exist or cannot be accessed: {e}"))?;

    #[cfg(target_os = "macos")]
    if is_macos_executable_bundle(&canonical) {
        return Err("Refusing to open an application bundle as a folder".to_string());
    }

    let metadata = std::fs::metadata(&canonical)
        .map_err(|e| format!("Directory does not exist or cannot be accessed: {e}"))?;
    if !metadata.is_dir() {
        return Err("Path is not a directory".to_string());
    }

    Ok(canonical)
}

/// Resolve a recorded output to the file the system player may open.
///
/// The type and extension are checked on the stored path and again on the
/// canonical one, so a media-named symlink to anything else is refused.
fn resolve_playable_media_path(file_type: &str, path: &Path) -> Result<PathBuf, String> {
    validate_playable_media_output(file_type, path)?;

    let canonical =
        dunce::canonicalize(path).map_err(|e| format!("Recording file is not accessible: {e}"))?;
    validate_playable_media_output(file_type, &canonical)?;

    if !canonical.is_file() {
        return Err("Recording path is not a file".to_string());
    }
    Ok(canonical)
}

/// Open a validated directory in the platform's file manager.
pub(crate) fn open_validated_directory(app: &tauri::AppHandle, dir: &Path) -> Result<(), String> {
    let dir_str = dir.to_string_lossy().into_owned();
    #[cfg(target_os = "macos")]
    let app_name = Some("Finder");
    #[cfg(not(target_os = "macos"))]
    let app_name: Option<&str> = None;

    app.opener()
        .open_path(dir_str.clone(), app_name)
        .map_err(|e| format!("Failed to open '{dir_str}': {e}"))
}

#[tauri::command]
pub async fn open_recording_in_player(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopBackendState>,
    output_id: String,
) -> Result<(), String> {
    if state.active_backend_mode != BackendMode::Local {
        return Err("Not available when connected to a remote server".to_string());
    }

    let output_id = output_id.trim();
    if output_id.is_empty() {
        return Err("Recording ID cannot be empty".to_string());
    }

    let container = state
        .backend()
        .ok_or_else(|| "Backend is not ready yet".to_string())?;

    let (file_type, path) = container
        .media_output_location(output_id)
        .await
        .map_err(|e| format!("Recording not found: {e}"))?;

    let open_path = tauri::async_runtime::spawn_blocking(move || {
        resolve_playable_media_path(&file_type, &path)
    })
    .await
    .map_err(|e| format!("Failed to check the recording file: {e}"))??;

    let path_str = open_path.to_string_lossy().into_owned();
    app.opener()
        .open_path(path_str.clone(), None::<&str>)
        .map_err(|e| format!("Failed to open '{path_str}': {e}"))
}

#[tauri::command]
pub async fn open_directory_in_explorer(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopBackendState>,
    path: String,
) -> Result<(), String> {
    // Folder settings come from the backend. With a remote backend they name
    // directories on another machine, and resolving one here could reach out
    // to a network share of that server's choosing.
    if state.active_backend_mode != BackendMode::Local {
        return Err("Not available when connected to a remote server".to_string());
    }

    let dir = tauri::async_runtime::spawn_blocking(move || validate_existing_directory(&path))
        .await
        .map_err(|e| format!("Failed to validate directory: {e}"))??;

    open_validated_directory(&app, &dir)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "rust-srec-desktop-media-{label}-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn accepts_media_extensions_in_any_case_and_rejects_other_files() {
        assert!(validate_playable_media_output("video", Path::new("/rec/stream.MKV")).is_ok());

        assert!(validate_playable_media_output("DANMU", Path::new("/rec/chat.xml")).is_err());
        assert!(validate_playable_media_output("THUMBNAIL", Path::new("/rec/thumb.mp4")).is_err());
        assert!(validate_playable_media_output("VIDEO", Path::new("/rec/payload.exe")).is_err());
        assert!(validate_playable_media_output("AUDIO", Path::new("/rec/no_extension")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_media_named_symlink_to_another_kind_of_file() {
        let dir = TempDir::new("symlink");
        let script = dir.path().join("payload.sh");
        std::fs::write(&script, b"#!/bin/sh\n").expect("write script");
        let link = dir.path().join("clip.mp4");
        std::os::unix::fs::symlink(&script, &link).expect("create symlink");
        let recording = dir.path().join("real.mp4");
        std::fs::write(&recording, b"media").expect("write recording");

        assert!(resolve_playable_media_path("VIDEO", &link).is_err());
        assert_eq!(
            resolve_playable_media_path("VIDEO", &recording).expect("playable recording"),
            std::fs::canonicalize(&recording).expect("canonicalize")
        );
    }

    #[test]
    fn validates_existing_directory_and_rejects_files_or_missing_paths() {
        let dir = TempDir::new("dir-validation");
        let dir_str = dir.path().to_string_lossy().into_owned();

        assert_eq!(
            validate_existing_directory(&format!("  {dir_str}  ")).expect("valid dir"),
            dunce::canonicalize(dir.path()).expect("canonicalize")
        );
        assert!(validate_existing_directory("   ").is_err());

        let file_path = dir.path().join("tool.exe");
        std::fs::write(&file_path, b"not a dir").expect("write temp file");
        assert!(validate_existing_directory(&file_path.to_string_lossy()).is_err());

        let missing_path = dir.path().join("does-not-exist");
        assert!(validate_existing_directory(&missing_path.to_string_lossy()).is_err());
    }

    #[test]
    fn output_folder_root_stops_before_the_first_placeholder() {
        assert_eq!(output_folder_root("/rec/live"), "/rec/live");
        assert_eq!(output_folder_root("/rec/{streamer}/%Y-%m-%d"), "/rec/");
        assert_eq!(output_folder_root("/rec/live-%Y/{title}"), "/rec/");
        assert_eq!(output_folder_root(r"D:\{streamer}"), r"D:\");
        assert_eq!(output_folder_root("{streamer}/%Y"), "");
    }
}
