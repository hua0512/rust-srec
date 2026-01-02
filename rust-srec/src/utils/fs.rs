//! Filesystem helpers shared across modules.
//!
//! These helpers provide consistent error context (operation + path) and
//! reduce duplicated `create_dir_all` / parent-directory checks.

use std::path::Path;

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

/// Ensure a directory exists, creating it (recursively) if needed.
pub async fn ensure_dir_all(path: &Path) -> Result<()> {
    ensure_dir_all_with_op("creating directory", path).await
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

/// Ensure a directory exists (synchronous variant).
pub fn ensure_dir_all_sync(path: &Path) -> Result<()> {
    ensure_dir_all_sync_with_op("creating directory", path)
}

/// Ensure a directory exists (synchronous variant) with a custom operation label.
pub fn ensure_dir_all_sync_with_op(op: &'static str, path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| io_error(op, path, e))
}
