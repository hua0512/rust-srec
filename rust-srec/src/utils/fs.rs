//! Filesystem helpers shared across modules.
//!
//! These helpers provide consistent error context (operation + path) and
//! reduce duplicated `create_dir_all` / parent-directory checks.

use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Convert an IO error into an application error with operation + path context.
pub fn io_error(op: &'static str, path: &Path, source: std::io::Error) -> Error {
    Error::io_path(op, path, source)
}

/// Ensure a directory exists, creating it (recursively) if needed.
pub async fn ensure_dir_all_with_op(op: &'static str, path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|e| io_error(op, path, e))
}

/// Ensure the parent directory of a file path exists.
pub async fn ensure_parent_dir(path: &Path) -> Result<()> {
    ensure_parent_dir_with_op("creating directory", path).await
}

/// Ensure the parent directory of a file path exists with a custom operation label.
pub async fn ensure_parent_dir_with_op(op: &'static str, path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    ensure_dir_all_with_op(op, parent).await
}

/// Ensure a directory exists (synchronous variant) with a custom operation label.
pub fn ensure_dir_all_sync_with_op(op: &'static str, path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| io_error(op, path, e))
}

/// Key under which two spellings of one path count as the same file when
/// outputs are merged or de-duplicated. Windows and the default macOS file
/// systems fold case, so the key is lower-cased there; other platforms keep
/// the spelling as it is. Matches the case policy of the media processors.
pub fn path_dedup_key(path: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        path.to_lowercase()
    } else {
        path.to_owned()
    }
}

/// Turn a stored `media_outputs.file_path` into a path usable by the std/tokio APIs.
///
/// Windows note: some parts of the pipeline/tooling may emit extended-length paths
/// like `\\?\C:\...`. While this is valid for Win32 APIs, it can be a portability
/// footgun across libraries and runtimes. Normalize it to a regular path when possible.
///
/// Shared by media serving, manual deletion, and periodic output retention.
pub fn normalize_media_path(file_path: &str) -> PathBuf {
    let path = PathBuf::from(file_path);
    if cfg!(windows)
        && let Some(s) = path.to_str()
    {
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            // `\\?\UNC\server\share\...` -> `\\server\share\...`
            return PathBuf::from(format!(r"\\{}", rest));
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            // `\\?\C:\...` -> `C:\...`
            return PathBuf::from(rest);
        }
    }
    path
}
