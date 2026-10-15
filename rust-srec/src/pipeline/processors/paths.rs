//! External-command spelling and distinct collision policies.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
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

#[derive(Clone, Copy)]
enum StagedIdentity {
    Present(u64),
    Missing,
    Unavailable,
}

pub(super) struct StagedPath {
    path: PathBuf,
    key: Option<PathBuf>,
    identity: StagedIdentity,
}

impl StagedPath {
    pub(super) fn prepare(path: PathBuf) -> Self {
        let identity = match same_file::Handle::from_path(&path) {
            Ok(handle) => {
                let mut hasher = DefaultHasher::new();
                handle.hash(&mut hasher);
                StagedIdentity::Present(hasher.finish())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StagedIdentity::Missing,
            Err(_) => StagedIdentity::Unavailable,
        };
        // A failed probe must be retried by staged_aliases, whose native
        // identity match can legitimately short-circuit path resolution.
        let key = staged_comparison_key(&path).ok().map(|key| {
            if cfg!(windows) {
                PathBuf::from(key.to_string_lossy().to_ascii_lowercase())
            } else {
                key
            }
        });
        Self {
            path,
            key,
            identity,
        }
    }
}

/// A snapshot of paths for one validation phase. Fingerprints avoid retaining
/// one open file per input; matching buckets are confirmed with native identity
/// checks, so a hash collision never establishes an alias. Failed probes also
/// use the original pair check, which retains its error and short-circuit rules.
#[derive(Default)]
pub(super) struct StagedPathIndex {
    paths: Vec<PathBuf>,
    keys: HashMap<PathBuf, Vec<usize>>,
    identities: HashMap<u64, Vec<usize>>,
    unavailable_keys: Vec<usize>,
    unavailable_identities: Vec<usize>,
}

impl StagedPathIndex {
    pub(super) fn len(&self) -> usize {
        self.paths.len()
    }

    pub(super) fn insert(&mut self, path: StagedPath) {
        let index = self.paths.len();
        if let Some(key) = path.key {
            self.keys.entry(key).or_default().push(index);
        } else {
            self.unavailable_keys.push(index);
        }
        match path.identity {
            StagedIdentity::Present(identity) => {
                self.identities.entry(identity).or_default().push(index);
            }
            StagedIdentity::Unavailable => self.unavailable_identities.push(index),
            StagedIdentity::Missing => {}
        }
        self.paths.push(path.path);
    }

    pub(super) fn aliases(&self, path: &StagedPath) -> Result<bool> {
        let Some(key) = &path.key else {
            return self.check_pairs(&path.path, 0..self.paths.len());
        };
        if matches!(path.identity, StagedIdentity::Unavailable) {
            return self.check_pairs(&path.path, 0..self.paths.len());
        }

        let mut candidates = self.unavailable_keys.clone();
        if let Some(matches) = self.keys.get(key) {
            candidates.extend(matches);
        }
        if let StagedIdentity::Present(identity) = path.identity {
            candidates.extend(&self.unavailable_identities);
            if let Some(matches) = self.identities.get(&identity) {
                candidates.extend(matches);
            }
        }
        // Error/alias precedence follows source order even when a candidate was
        // selected by both its spelling and native identity.
        candidates.sort_unstable();
        candidates.dedup();
        self.check_pairs(&path.path, candidates)
    }

    fn check_pairs(
        &self,
        path: &Path,
        candidates: impl IntoIterator<Item = usize>,
    ) -> Result<bool> {
        for index in candidates {
            if staged_aliases(path, &self.paths[index])? {
                return Ok(true);
            }
        }
        Ok(false)
    }
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

    #[test]
    fn staged_index_preserves_pairwise_aliases_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.mp4");
        let second = dir.path().join("second.mp4");
        let hardlink = dir.path().join("hardlink.mp4");
        let missing = dir.path().join("missing.mp4");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        std::fs::hard_link(&first, &hardlink).unwrap();
        let invalid = PathBuf::from("invalid\0path");

        for sources in [
            vec![first.clone(), missing.clone()],
            vec![invalid.clone(), first.clone()],
            vec![first.clone(), invalid.clone()],
        ] {
            let mut index = StagedPathIndex::default();
            for source in &sources {
                index.insert(StagedPath::prepare(source.clone()));
            }
            for candidate in [&first, &second, &hardlink, &missing, &invalid] {
                let pairwise = sources.iter().try_fold(false, |aliased, source| {
                    if aliased {
                        Ok(true)
                    } else {
                        staged_aliases(candidate, source)
                    }
                });
                let indexed = index.aliases(&StagedPath::prepare(candidate.clone()));
                assert_eq!(
                    indexed.map_err(|error| error.to_string()),
                    pairwise.map_err(|error| error.to_string()),
                    "candidate={candidate:?}, sources={sources:?}",
                );
            }
        }
    }

    #[test]
    fn staged_identity_fingerprint_collisions_do_not_establish_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.mp4");
        let second = dir.path().join("second.mp4");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let prepared = StagedPath::prepare(first);
        let mut candidate = StagedPath::prepare(second);
        candidate.identity = prepared.identity;
        let mut index = StagedPathIndex::default();
        index.insert(prepared);
        assert!(!index.aliases(&candidate).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn staged_index_does_not_retain_file_handles() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.mp4");
        std::fs::write(&input, b"source").unwrap();
        let mut index = StagedPathIndex::default();
        index.insert(StagedPath::prepare(input.clone()));
        let _exclusive = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&input)
            .expect("prepared identities must release their file handles");
        assert_eq!(index.len(), 1);
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
