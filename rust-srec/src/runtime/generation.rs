//! Durable dirty-generation marker for isolated runtime workers.

use std::collections::HashSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tempfile::Builder;
use uuid::Uuid;

use crate::{Error, Result};

const MARKER_FORMAT_VERSION: u8 = 1;

/// Identifies one launch of the isolated runtime worker graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeGeneration(Uuid);

impl RuntimeGeneration {
    /// Generate a fresh runtime generation.
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }
}

impl FromStr for RuntimeGeneration {
    type Err = uuid::Error;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

impl fmt::Display for RuntimeGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Durable state for active runtime ownership and unresolved recovery debt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirtyGenerationState {
    version: u8,
    active_generation: Option<RuntimeGeneration>,
    recovery_pending: Vec<RuntimeGeneration>,
}

impl Default for DirtyGenerationState {
    fn default() -> Self {
        Self {
            version: MARKER_FORMAT_VERSION,
            active_generation: None,
            recovery_pending: Vec::new(),
        }
    }
}

impl DirtyGenerationState {
    /// Most recent generation whose state was not durably cleared.
    pub fn latest_dirty_generation(&self) -> Option<RuntimeGeneration> {
        self.active_generation
            .or_else(|| self.recovery_pending.last().copied())
    }

    /// Number of generations whose active or recovery state remains dirty.
    pub fn dirty_generation_count(&self) -> usize {
        self.recovery_pending.len() + usize::from(self.active_generation.is_some())
    }

    fn record_recovery_pending(&mut self, generation: RuntimeGeneration) {
        if !self.recovery_pending.contains(&generation) {
            self.recovery_pending.push(generation);
        }
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.version != MARKER_FORMAT_VERSION {
            return Err(Error::validation(format!(
                "unsupported dirty generation marker version {} in '{}'",
                self.version,
                path.display()
            )));
        }
        if self.active_generation.is_none() && self.recovery_pending.is_empty() {
            return Err(Error::validation(format!(
                "dirty generation marker '{}' contains no active or pending generation",
                path.display()
            )));
        }

        let mut generations = HashSet::new();
        if let Some(active) = self.active_generation {
            generations.insert(active);
        }
        if self
            .recovery_pending
            .iter()
            .any(|generation| !generations.insert(*generation))
        {
            return Err(Error::validation(format!(
                "dirty generation marker '{}' contains duplicate generations",
                path.display()
            )));
        }
        Ok(())
    }
}

/// Ownership handle for one active runtime generation.
///
/// Beginning a generation moves any interrupted active generation into the
/// recovery-pending set. A clean exit clears only this handle's active
/// generation; earlier recovery debt remains until explicitly resolved.
#[derive(Debug)]
pub struct DirtyGenerationMarker {
    path: PathBuf,
    generation: RuntimeGeneration,
}

/// Exclusive ownership of the database-adjacent runtime supervision domain.
pub struct RuntimeLease {
    _file: File,
}

impl RuntimeLease {
    /// Acquire the runtime lease associated with `marker_path` without waiting.
    pub fn acquire(marker_path: impl AsRef<Path>) -> Result<Self> {
        let path = runtime_lock_path(marker_path.as_ref());
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| Error::io_path("opening runtime ownership lock", &path, error))?;
        file.try_lock_exclusive()
            .map_err(|error| Error::io_path("acquiring runtime ownership lock", &path, error))?;
        Ok(Self { _file: file })
    }
}

impl DirtyGenerationMarker {
    /// Atomically install and sync active ownership for `generation`.
    pub fn begin(path: impl AsRef<Path>, generation: RuntimeGeneration) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut state = Self::load(&path)?.unwrap_or_default();
        if let Some(previous) = state.active_generation.take() {
            state.record_recovery_pending(previous);
        }
        state.active_generation = Some(generation);
        persist_state(&path, &state)?;

        Ok(Self { path, generation })
    }

    /// Load durable runtime state, returning `None` when no debt exists.
    pub fn load(path: impl AsRef<Path>) -> Result<Option<DirtyGenerationState>> {
        let path = path.as_ref().to_path_buf();
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(Error::io_path(
                    "reading dirty generation marker",
                    &path,
                    error,
                ));
            }
        };

        let state = match serde_json::from_str::<DirtyGenerationState>(&contents) {
            Ok(state) => state,
            Err(json_error) => match contents.trim().parse::<RuntimeGeneration>() {
                Ok(generation) => DirtyGenerationState {
                    active_generation: Some(generation),
                    ..DirtyGenerationState::default()
                },
                Err(_) => {
                    return Err(Error::validation(format!(
                        "invalid dirty generation marker '{}': {json_error}",
                        path.display()
                    )));
                }
            },
        };
        state.validate(&path)?;
        Ok(Some(state))
    }

    /// Return the generation recorded by this marker.
    pub fn generation(&self) -> RuntimeGeneration {
        self.generation
    }

    /// Clear this active generation after clean exit.
    ///
    /// Returns `true` when earlier recovery debt remains on disk.
    pub fn clear(self) -> Result<bool> {
        // Runtime ownership serializes begin/clear. This re-read additionally
        // prevents a retained stale handle from clearing a later generation.
        let Some(mut state) = Self::load(&self.path)? else {
            return Err(Error::Other(format!(
                "dirty generation marker '{}' disappeared before generation {} was cleared",
                self.path.display(),
                self.generation
            )));
        };
        if state.active_generation != Some(self.generation) {
            let current = state
                .active_generation
                .map_or_else(|| "none".to_string(), |generation| generation.to_string());
            return Err(Error::Other(format!(
                "refusing to clear dirty generation marker '{}' for generation {}; current generation is {}",
                self.path.display(),
                self.generation,
                current
            )));
        }

        state.active_generation = None;
        if !state.recovery_pending.is_empty() {
            persist_state(&self.path, &state)?;
            return Ok(true);
        }

        match std::fs::remove_file(&self.path) {
            Ok(()) => {
                sync_parent_directory(marker_parent(&self.path))?;
                Ok(false)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::Other(format!(
                    "dirty generation marker '{}' disappeared while generation {} was being cleared",
                    self.path.display(),
                    self.generation
                )))
            }
            Err(error) => Err(Error::io_path(
                "clearing dirty generation marker",
                &self.path,
                error,
            )),
        }
    }
}

fn persist_state(path: &Path, state: &DirtyGenerationState) -> Result<()> {
    let parent = marker_parent(path);
    let serialized = serde_json::to_vec(state)?;
    let mut temporary = Builder::new()
        .prefix(".runtime-generation-")
        .tempfile_in(parent)
        .map_err(|error| Error::io_path("creating dirty generation marker", path, error))?;
    temporary
        .write_all(&serialized)
        .map_err(|error| Error::io_path("writing dirty generation marker", path, error))?;
    writeln!(temporary)
        .map_err(|error| Error::io_path("writing dirty generation marker", path, error))?;
    temporary
        .flush()
        .map_err(|error| Error::io_path("flushing dirty generation marker", path, error))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| Error::io_path("syncing dirty generation marker", path, error))?;

    let persisted = temporary
        .persist(path)
        .map_err(|error| Error::io_path("replacing dirty generation marker", path, error.error))?;
    persisted
        .sync_all()
        .map_err(|error| Error::io_path("syncing dirty generation marker", path, error))?;
    sync_parent_directory(parent)
}

fn marker_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn runtime_lock_path(marker_path: &Path) -> PathBuf {
    let mut lock_path = marker_path.as_os_str().to_os_string();
    lock_path.push(".lock");
    PathBuf::from(lock_path)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<()> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| Error::io_path("syncing marker directory", parent, error))
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Result<()> {
    // std does not expose a portable way to open directories for syncing on
    // Windows. The marker file itself is synced before and after replacement.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation(value: &str) -> RuntimeGeneration {
        value.parse().expect("test generation should be valid")
    }

    #[test]
    fn marker_round_trips_active_generation() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("runtime.dirty");
        let expected = generation("5d767a8b-1ff2-49a5-b329-396660703e49");

        let marker = DirtyGenerationMarker::begin(&path, expected)
            .expect("dirty marker should be installed");
        let loaded = DirtyGenerationMarker::load(&path)
            .expect("dirty marker should be readable")
            .expect("dirty marker should exist");

        assert_eq!(marker.generation(), expected);
        assert_eq!(loaded.active_generation, Some(expected));
        assert!(loaded.recovery_pending.is_empty());
        assert_eq!(expected.to_string().parse(), Ok(expected));
    }

    #[test]
    fn begin_moves_an_interrupted_generation_to_recovery_pending() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("runtime.dirty");
        let first = generation("f4daa1f4-288c-4d06-89f2-226da96d4be3");
        let second = generation("40703585-6d3e-4934-87c2-7310dc628ab2");

        let stale = DirtyGenerationMarker::begin(&path, first)
            .expect("initial dirty marker should be installed");
        let second_marker = DirtyGenerationMarker::begin(&path, second)
            .expect("replacement dirty marker should be installed");

        let loaded = DirtyGenerationMarker::load(&path)
            .expect("replacement marker should be readable")
            .expect("replacement marker should exist");
        assert_eq!(loaded.active_generation, Some(second));
        assert_eq!(loaded.recovery_pending, vec![first]);
        assert!(stale.clear().is_err());

        assert!(
            second_marker
                .clear()
                .expect("active generation should clear while preserving recovery debt")
        );
        let recovered = DirtyGenerationMarker::load(&path)
            .expect("recovery debt should remain readable")
            .expect("recovery debt should remain present");
        assert_eq!(recovered.active_generation, None);
        assert_eq!(recovered.recovery_pending, vec![first]);

        let entries = std::fs::read_dir(directory.path())
            .expect("marker directory should be readable")
            .collect::<std::io::Result<Vec<_>>>()
            .expect("marker directory entries should be readable");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), path);
    }

    #[test]
    fn clear_removes_matching_marker() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("runtime.dirty");
        let marker =
            DirtyGenerationMarker::begin(&path, generation("c7b25329-cc9c-4451-940b-d93601813cf8"))
                .expect("dirty marker should be installed");

        assert!(
            !marker
                .clear()
                .expect("matching marker should be cleared without recovery debt")
        );

        assert!(
            DirtyGenerationMarker::load(&path)
                .expect("missing marker should load cleanly")
                .is_none()
        );
    }

    #[test]
    fn repeated_forced_generations_accumulate_recovery_debt() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("runtime.dirty");
        let first = generation("d68f2f31-7705-45df-8195-fbd4b7f6c0dc");
        let second = generation("e08dd995-9a14-4077-b558-3eecc3e7c715");
        let third = generation("874479e1-2827-4da7-bc6d-a45a2212d783");

        drop(DirtyGenerationMarker::begin(&path, first).expect("begin first generation"));
        drop(DirtyGenerationMarker::begin(&path, second).expect("begin second generation"));
        let current = DirtyGenerationMarker::begin(&path, third).expect("begin third generation");

        let state = DirtyGenerationMarker::load(&path)
            .expect("marker should be readable")
            .expect("marker should exist");
        assert_eq!(state.active_generation, Some(third));
        assert_eq!(state.recovery_pending, vec![first, second]);
        assert_eq!(state.latest_dirty_generation(), Some(third));
        assert_eq!(state.dirty_generation_count(), 3);

        assert!(current.clear().expect("third generation should clear"));
        let state = DirtyGenerationMarker::load(&path)
            .expect("recovery marker should be readable")
            .expect("recovery marker should remain");
        assert_eq!(state.active_generation, None);
        assert_eq!(state.recovery_pending, vec![first, second]);
        assert_eq!(state.latest_dirty_generation(), Some(second));
        assert_eq!(state.dirty_generation_count(), 2);
    }

    #[test]
    fn legacy_single_generation_marker_is_migrated_on_begin() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("runtime.dirty");
        let legacy = generation("bcda771d-c21e-48cf-af92-8176be6553ea");
        let current = generation("de29cf58-0f10-424e-bd39-e597b89d3f91");
        std::fs::write(&path, format!("{legacy}\n")).expect("legacy marker should be written");

        let marker =
            DirtyGenerationMarker::begin(&path, current).expect("legacy marker should migrate");
        let state = DirtyGenerationMarker::load(&path)
            .expect("migrated marker should be readable")
            .expect("migrated marker should exist");

        assert_eq!(state.active_generation, Some(current));
        assert_eq!(state.recovery_pending, vec![legacy]);
        assert!(marker.clear().expect("current generation should clear"));
    }

    #[test]
    fn load_rejects_malformed_marker_with_path_context() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("runtime.dirty");
        std::fs::write(&path, "not-a-runtime-generation\n")
            .expect("malformed marker should be written");

        let error = DirtyGenerationMarker::load(&path)
            .expect_err("malformed marker should be rejected")
            .to_string();

        assert!(error.contains("invalid dirty generation marker"));
        assert!(error.contains(&path.display().to_string()));
    }

    #[test]
    fn runtime_lease_excludes_a_second_supervisor() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let marker_path = directory.path().join("runtime.dirty");
        let first = RuntimeLease::acquire(&marker_path).expect("first lease should be acquired");

        let error = RuntimeLease::acquire(&marker_path)
            .err()
            .expect("second lease should be rejected")
            .to_string();
        assert!(error.contains("acquiring runtime ownership lock"));

        drop(first);
        RuntimeLease::acquire(&marker_path).expect("lease should be reusable after release");
        assert_eq!(
            runtime_lock_path(&marker_path),
            directory.path().join("runtime.dirty.lock")
        );
    }
}
