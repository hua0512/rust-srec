//! External-command spelling and distinct collision policies.

use std::path::{Path, PathBuf};

use crate::{Error, Result};

#[derive(Clone, Copy)]
pub(super) enum CasePolicy {
    Windows,
    WindowsAndMacOs,
}

pub(super) fn spelling_equal(left: &str, right: &str, policy: CasePolicy) -> bool {
    let fold =
        cfg!(windows) || matches!(policy, CasePolicy::WindowsAndMacOs) && cfg!(target_os = "macos");
    if fold {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

/// ASS command/manifest spelling remains lexical, including symlink components.
pub(super) fn lexical_absolute(path: &str) -> String {
    let path_obj = Path::new(path);
    if path_obj.is_absolute() {
        return path.to_owned();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path_obj).to_string_lossy().into_owned(),
        Err(_) => path.to_owned(),
    }
}

fn canonical(path: &Path, operation: &'static str) -> Result<Option<PathBuf>> {
    match std::fs::canonicalize(path) {
        Ok(resolved) => Ok(Some(resolved)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::io_path(operation, path, error)),
    }
}

fn remux_command_path_blocking(path: &str) -> String {
    let path_obj = Path::new(path);
    if path_obj.is_absolute() {
        return path.to_owned();
    }
    if matches!(std::fs::exists(path_obj), Ok(true))
        && let Ok(Some(resolved)) = canonical(path_obj, "resolve remux command path")
    {
        return resolved.to_string_lossy().into_owned();
    }
    lexical_absolute(path)
}

/// Preserve absolute command strings. Relative existing inputs resolve fully;
/// missing/unreadable paths retain the command's lexical fallback.
pub(super) async fn remux_command_path(path: &str) -> String {
    let fallback = path.to_owned();
    let path = fallback.clone();
    tokio::task::spawn_blocking(move || remux_command_path_blocking(&path))
        .await
        .unwrap_or(fallback)
}

/// Remux comparison is best effort; staged destination validation is strict.
pub(super) async fn remux_comparison_key(path: &str) -> String {
    let fallback = path.to_owned();
    let path = fallback.clone();
    tokio::task::spawn_blocking(move || {
        let path_obj = Path::new(&path);
        if let Ok(Some(resolved)) = canonical(path_obj, "resolve remux path") {
            return resolved.to_string_lossy().into_owned();
        }
        if let (Some(parent), Some(name)) = (path_obj.parent(), path_obj.file_name())
            && let Ok(Some(resolved)) = canonical(parent, "resolve remux parent")
        {
            return resolved.join(name).to_string_lossy().into_owned();
        }
        remux_command_path_blocking(&path)
    })
    .await
    .unwrap_or(fallback)
}

fn staged_comparison_key(path: &Path) -> Result<PathBuf> {
    if let Some(resolved) = canonical(path, "resolve output path")? {
        return Ok(resolved);
    }
    let absolute = std::path::absolute(path)
        .map_err(|error| Error::io_path("resolve output path", path, error))?;
    if let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name())
        && let Some(resolved) = canonical(parent, "resolve output parent")?
    {
        return Ok(resolved.join(name));
    }
    Ok(absolute)
}

/// Native identity catches existing hardlinks and symlinks; missing leaves use
/// resolved parents. Non-NotFound I/O errors must not establish distinct files.
pub(super) fn staged_aliases(left: &Path, right: &Path) -> Result<bool> {
    match same_file::is_same_file(left, right) {
        Ok(true) => return Ok(true),
        Ok(false) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::io_path("compare output identity", left, error)),
    }
    let left = staged_comparison_key(left)?;
    let right = staged_comparison_key(right)?;
    Ok(if cfg!(windows) {
        spelling_equal(
            &left.to_string_lossy(),
            &right.to_string_lossy(),
            CasePolicy::Windows,
        )
    } else {
        left == right
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_policies_keep_platform_case_contracts() {
        assert_eq!(
            spelling_equal("A/Clip.MP4", "a/clip.mp4", CasePolicy::Windows),
            cfg!(windows)
        );
        assert_eq!(
            spelling_equal("A/Clip.MP4", "a/clip.mp4", CasePolicy::WindowsAndMacOs),
            cfg!(any(windows, target_os = "macos"))
        );
        assert!(!spelling_equal("a/./clip", "a/clip", CasePolicy::Windows));
        assert!(staged_aliases(Path::new("invalid\0input"), Path::new("invalid\0output")).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_spelling_and_missing_leaf_identity_remain_distinct_policies() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let cwd = std::env::current_dir().unwrap();
            let dir = tempfile::tempdir_in(&cwd).unwrap();
            let real = dir.path().join("real");
            std::fs::create_dir(&real).unwrap();
            std::fs::write(real.join("input.mp4"), b"input").unwrap();
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let relative = link
                .join("input.mp4")
                .strip_prefix(&cwd)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            assert_eq!(
                lexical_absolute(&relative),
                cwd.join(&relative).to_string_lossy()
            );
            assert_eq!(
                remux_command_path(&relative).await,
                std::fs::canonicalize(real.join("input.mp4"))
                    .unwrap()
                    .to_string_lossy()
            );
            let missing = link.join("new.mp4");
            assert!(!missing.exists());
            assert!(staged_aliases(&missing, &real.join("new.mp4")).unwrap());
            assert_eq!(
                remux_comparison_key(&missing.to_string_lossy()).await,
                real.join("new.mp4").to_string_lossy()
            );
        })
        .await
        .expect("path contract checks must finish");
    }
}
