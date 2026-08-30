//! Durable dirty-generation marker for isolated runtime workers.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteConnectOptions;
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

/// How many generations still owe recovery.
///
/// `RecoveryDebt::record` folds the generations it stops retaining into a
/// saturating counter, so a total can be a lower bound instead of an exact
/// figure. `Display` and `FromStr` round-trip that distinction, which is how
/// `supervise_command_with_monitor` hands the ledger size to the worker.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirtyGenerationCount {
    generations: usize,
    lower_bound: bool,
}

/// Wire and display prefix for a count that `RecoveryDebt::dropped` saturated.
const LOWER_BOUND_PREFIX: &str = "at least ";

impl DirtyGenerationCount {
    /// A count of exactly `generations` unresolved generations.
    pub fn exactly(generations: usize) -> Self {
        Self {
            generations,
            lower_bound: false,
        }
    }

    /// A count of `generations` or more, used once `RecoveryDebt::dropped`
    /// stops counting further generations.
    fn at_least(generations: usize) -> Self {
        Self {
            generations,
            lower_bound: true,
        }
    }

    /// Count one more unresolved generation, preserving the lower-bound mark.
    fn increment(self) -> Self {
        Self {
            generations: self.generations.saturating_add(1),
            ..self
        }
    }
}

impl fmt::Display for DirtyGenerationCount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.lower_bound {
            formatter.write_str(LOWER_BOUND_PREFIX)?;
        }
        self.generations.fmt(formatter)
    }
}

impl FromStr for DirtyGenerationCount {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        let value = value.trim();
        let (generations, lower_bound) = match value.strip_prefix(LOWER_BOUND_PREFIX) {
            Some(rest) => (rest.trim_start(), true),
            None => (value, false),
        };
        let generations = generations.parse::<usize>().map_err(|error| {
            Error::validation(format!("invalid dirty generation count '{value}': {error}"))
        })?;
        Ok(Self {
            generations,
            lower_bound,
        })
    }
}

/// Generations that never durably cleared their active state, retained in
/// constant space.
///
/// `record` keeps only `earliest` and `latest`; a generation that displaces
/// `latest` increments `dropped` instead of being stored, so a restart loop
/// cannot grow the serialized marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryDebt {
    earliest: RuntimeGeneration,
    latest: RuntimeGeneration,
    dropped: u32,
}

impl RecoveryDebt {
    fn new(generation: RuntimeGeneration) -> Self {
        Self {
            earliest: generation,
            latest: generation,
            dropped: 0,
        }
    }

    /// Fold `generation` into the retained endpoints.
    ///
    /// Re-recording an endpoint is a no-op, so repeating
    /// `DirtyGenerationMarker::begin` for one generation cannot inflate
    /// `dropped`. A generation already folded into `dropped` is no longer
    /// identifiable, which is safe because `RuntimeGeneration::generate` gives
    /// every launch a fresh UUID.
    fn record(&mut self, generation: RuntimeGeneration) {
        if self.contains(generation) {
            return;
        }
        if self.latest != self.earliest {
            // `latest` is displaced rather than kept, so it is only counted.
            self.dropped = self.dropped.saturating_add(1);
        }
        self.latest = generation;
    }

    fn contains(self, generation: RuntimeGeneration) -> bool {
        generation == self.earliest || generation == self.latest
    }

    /// Generations owed recovery: the retained endpoints plus `dropped`.
    fn count(self) -> DirtyGenerationCount {
        let retained = if self.earliest == self.latest { 1 } else { 2 };
        let generations = usize::try_from(self.dropped)
            .unwrap_or(usize::MAX)
            .saturating_add(retained);
        if self.dropped == u32::MAX {
            DirtyGenerationCount::at_least(generations)
        } else {
            DirtyGenerationCount::exactly(generations)
        }
    }
}

/// Durable state for active runtime ownership and unresolved recovery debt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirtyGenerationState {
    version: u8,
    active_generation: Option<RuntimeGeneration>,
    recovery_pending: Option<RecoveryDebt>,
}

impl Default for DirtyGenerationState {
    fn default() -> Self {
        Self {
            version: MARKER_FORMAT_VERSION,
            active_generation: None,
            recovery_pending: None,
        }
    }
}

impl DirtyGenerationState {
    /// Most recent generation whose state was not durably cleared.
    pub fn latest_dirty_generation(&self) -> Option<RuntimeGeneration> {
        self.active_generation
            .or_else(|| self.recovery_pending.map(|debt| debt.latest))
    }

    /// Number of generations whose active or recovery state remains dirty.
    pub fn dirty_generation_count(&self) -> DirtyGenerationCount {
        let debt = self
            .recovery_pending
            .map_or_else(DirtyGenerationCount::default, RecoveryDebt::count);
        if self.active_generation.is_some() {
            debt.increment()
        } else {
            debt
        }
    }

    fn record_recovery_pending(&mut self, generation: RuntimeGeneration) {
        match &mut self.recovery_pending {
            Some(debt) => debt.record(generation),
            None => self.recovery_pending = Some(RecoveryDebt::new(generation)),
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
        if self.active_generation.is_none() && self.recovery_pending.is_none() {
            return Err(Error::validation(format!(
                "dirty generation marker '{}' contains no active or pending generation",
                path.display()
            )));
        }
        // `RecoveryDebt::record` only increments `dropped` once a second
        // endpoint exists, so a single retained generation cannot have dropped
        // intermediates.
        if let Some(debt) = self.recovery_pending
            && debt.earliest == debt.latest
            && debt.dropped != 0
        {
            return Err(Error::validation(format!(
                "dirty generation marker '{}' counts dropped generations without a retained range",
                path.display()
            )));
        }
        if let Some(debt) = self.recovery_pending
            && self
                .active_generation
                .is_some_and(|active| debt.contains(active))
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
/// recovery-pending ledger. A clean exit clears only this handle's active
/// generation; earlier recovery debt remains until explicitly resolved.
#[derive(Debug)]
pub struct DirtyGenerationMarker {
    path: PathBuf,
    generation: RuntimeGeneration,
    unresolved_generations: DirtyGenerationCount,
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
        // `WouldBlock` is contention — another runtime already owns this
        // database — while `Error` means the lock could not be attempted at
        // all. Only the first is an operator's own doing, so they get distinct
        // messages instead of one generic lock failure.
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => Error::Other(format!(
                "another rust-srec runtime already owns {}",
                path.display()
            )),
            std::fs::TryLockError::Error(error) => {
                Error::io_path("acquiring runtime ownership lock", &path, error)
            }
        })?;
        Ok(Self { _file: file })
    }

    /// Acquire the lease guarding the runtime that owns `database_url`.
    ///
    /// Every entry point that opens a database must acquire this before it
    /// starts recording against it: the standalone supervisor, and the desktop
    /// app that hosts the same backend in-process. Keying on the database
    /// rather than on each entry point's own state directory is what makes a
    /// desktop instance and a standalone server exclude one another when they
    /// point at the same file.
    pub fn acquire_for_database(database_url: &str) -> Result<Self> {
        Self::acquire(marker_path_from_database_url(database_url)?)
    }
}

/// Derive the dirty-generation marker path that accompanies `database_url`.
///
/// `RuntimeLease::acquire` appends `.lock` to this, so both the marker and the
/// ownership lock stay beside the SQLite file they describe and every entry
/// point derives the same pair from the same URL.
pub(crate) fn marker_path_from_database_url(database_url: &str) -> Result<PathBuf> {
    let options = SqliteConnectOptions::from_str(database_url).map_err(|error| {
        Error::config(format!(
            "cannot derive runtime marker path from DATABASE_URL: {error}"
        ))
    })?;
    let database_path = options.get_filename();
    let parent = database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut marker_name = database_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("rust-srec"))
        .to_os_string();
    marker_name.push(".runtime-generation.dirty");
    Ok(parent.join(marker_name))
}

impl DirtyGenerationMarker {
    /// Atomically install and sync active ownership for `generation`.
    pub fn begin(path: impl AsRef<Path>, generation: RuntimeGeneration) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut state = Self::load(&path)?.unwrap_or_default();
        if let Some(previous) = state.active_generation.replace(generation)
            && previous != generation
        {
            // An active generation that never reached `clear` owes recovery.
            // Re-installing the generation already recorded as active leaves
            // the ledger untouched instead of listing it twice.
            state.record_recovery_pending(previous);
        }
        persist_state(&path, &state)?;

        Ok(Self {
            path,
            generation,
            unresolved_generations: state.dirty_generation_count(),
        })
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

    /// Generations owing recovery while this marker is installed, counting
    /// `generation` itself. This is the figure `begin` persisted; `clear`
    /// reports what survives a clean exit.
    pub fn unresolved_generations(&self) -> DirtyGenerationCount {
        self.unresolved_generations
    }

    /// Clear this active generation after clean exit.
    ///
    /// Returns the recovery debt still on disk, or `None` when the marker was
    /// removed because no earlier generation owes recovery.
    pub fn clear(self) -> Result<Option<DirtyGenerationCount>> {
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
        if state.recovery_pending.is_some() {
            persist_state(&self.path, &state)?;
            return Ok(Some(state.dirty_generation_count()));
        }

        match std::fs::remove_file(&self.path) {
            Ok(()) => {
                sync_parent_directory(marker_parent(&self.path))?;
                Ok(None)
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
        assert_eq!(
            marker.unresolved_generations(),
            DirtyGenerationCount::exactly(1)
        );
        assert_eq!(loaded.active_generation, Some(expected));
        assert_eq!(loaded.recovery_pending, None);
        assert_eq!(
            loaded.dirty_generation_count(),
            DirtyGenerationCount::exactly(1)
        );
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
        assert_eq!(loaded.recovery_pending, Some(RecoveryDebt::new(first)));
        assert!(stale.clear().is_err());

        assert_eq!(
            second_marker
                .clear()
                .expect("active generation should clear while preserving recovery debt"),
            Some(DirtyGenerationCount::exactly(1))
        );
        let recovered = DirtyGenerationMarker::load(&path)
            .expect("recovery debt should remain readable")
            .expect("recovery debt should remain present");
        assert_eq!(recovered.active_generation, None);
        assert_eq!(recovered.recovery_pending, Some(RecoveryDebt::new(first)));
        assert_eq!(recovered.latest_dirty_generation(), Some(first));

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

        assert_eq!(
            marker
                .clear()
                .expect("matching marker should be cleared without recovery debt"),
            None
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
        let debt = RecoveryDebt {
            earliest: first,
            latest: second,
            dropped: 0,
        };
        assert_eq!(state.active_generation, Some(third));
        assert_eq!(state.recovery_pending, Some(debt));
        assert_eq!(state.latest_dirty_generation(), Some(third));
        assert_eq!(
            state.dirty_generation_count(),
            DirtyGenerationCount::exactly(3)
        );

        assert_eq!(
            current.clear().expect("third generation should clear"),
            Some(DirtyGenerationCount::exactly(2))
        );
        let state = DirtyGenerationMarker::load(&path)
            .expect("recovery marker should be readable")
            .expect("recovery marker should remain");
        assert_eq!(state.active_generation, None);
        assert_eq!(state.recovery_pending, Some(debt));
        assert_eq!(state.latest_dirty_generation(), Some(second));
        assert_eq!(
            state.dirty_generation_count(),
            DirtyGenerationCount::exactly(2)
        );
    }

    #[test]
    fn recording_one_generation_twice_keeps_a_single_entry() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("runtime.dirty");
        let repeated = generation("2f2f1f2a-2f31-4d0e-9a2f-0f5a9a4a1c77");

        drop(DirtyGenerationMarker::begin(&path, repeated).expect("begin repeated generation"));
        let marker = DirtyGenerationMarker::begin(&path, repeated)
            .expect("re-beginning one generation should be accepted");

        let state = DirtyGenerationMarker::load(&path)
            .expect("marker should be readable")
            .expect("marker should exist");
        assert_eq!(state.active_generation, Some(repeated));
        assert_eq!(state.recovery_pending, None);
        assert_eq!(
            state.dirty_generation_count(),
            DirtyGenerationCount::exactly(1)
        );
        assert_eq!(
            marker
                .clear()
                .expect("re-begun generation should clear without debt"),
            None
        );

        // `RecoveryDebt::record` is idempotent for both retained endpoints:
        // re-recording one neither duplicates it nor counts a dropped
        // generation.
        let other = generation("6b0f9e4c-32bd-4c6e-8f6f-6f5f38a0b1d2");
        let mut debt_state = DirtyGenerationState::default();
        debt_state.record_recovery_pending(repeated);
        debt_state.record_recovery_pending(other);
        debt_state.record_recovery_pending(repeated);
        debt_state.record_recovery_pending(other);

        assert_eq!(
            debt_state.recovery_pending,
            Some(RecoveryDebt {
                earliest: repeated,
                latest: other,
                dropped: 0,
            })
        );
        assert_eq!(
            debt_state.dirty_generation_count(),
            DirtyGenerationCount::exactly(2)
        );
    }

    #[test]
    fn repeated_crash_cycles_keep_the_marker_bounded() {
        const CYCLES: usize = 64;

        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("runtime.dirty");
        let earliest = RuntimeGeneration::generate();
        drop(DirtyGenerationMarker::begin(&path, earliest).expect("begin first generation"));

        let mut size_at_four_cycles = 0;
        let mut latest = earliest;
        for cycle in 2..=CYCLES {
            latest = RuntimeGeneration::generate();
            // Dropping the handle without `clear` leaves the generation active
            // on disk, which the next `begin` folds into the debt.
            drop(DirtyGenerationMarker::begin(&path, latest).expect("begin crashed generation"));
            if cycle == 4 {
                size_at_four_cycles = marker_size(&path);
            }
        }
        let size_at_all_cycles = marker_size(&path);

        // Only the decimal width of `RecoveryDebt::dropped` may differ between
        // 4 and 64 crash cycles; no generation is appended.
        assert!(
            size_at_all_cycles <= size_at_four_cycles + 2,
            "marker grew from {size_at_four_cycles} to {size_at_all_cycles} bytes across {CYCLES} cycles"
        );

        let state = DirtyGenerationMarker::load(&path)
            .expect("marker should be readable")
            .expect("marker should exist");
        let debt = state
            .recovery_pending
            .expect("crashed generations should leave recovery debt");
        assert_eq!(state.active_generation, Some(latest));
        assert_eq!(debt.earliest, earliest);
        assert_eq!(debt.dropped, u32::try_from(CYCLES - 3).unwrap());
        assert_eq!(state.latest_dirty_generation(), Some(latest));
        assert_eq!(
            state.dirty_generation_count(),
            DirtyGenerationCount::exactly(CYCLES)
        );
    }

    #[test]
    fn dropped_generations_stop_growing_the_serialized_state() {
        const EARLY: usize = 8;
        const LATE: usize = 10_000;

        let earliest = RuntimeGeneration::generate();
        let mut state = DirtyGenerationState::default();
        state.record_recovery_pending(earliest);
        for _ in 1..EARLY {
            state.record_recovery_pending(RuntimeGeneration::generate());
        }
        let early_size = serde_json::to_vec(&state)
            .expect("state should serialize")
            .len();

        for _ in EARLY..LATE {
            state.record_recovery_pending(RuntimeGeneration::generate());
        }
        let late_size = serde_json::to_vec(&state)
            .expect("state should serialize")
            .len();

        assert!(
            late_size <= early_size + 4,
            "serialized state grew from {early_size} to {late_size} bytes across {LATE} generations"
        );
        let debt = state
            .recovery_pending
            .expect("recorded generations should leave recovery debt");
        assert_eq!(debt.earliest, earliest);
        assert_eq!(debt.dropped, u32::try_from(LATE - 2).unwrap());
        assert_eq!(
            state.dirty_generation_count(),
            DirtyGenerationCount::exactly(LATE)
        );
    }

    #[test]
    fn a_saturated_dropped_counter_reports_a_lower_bound() {
        let mut state = DirtyGenerationState {
            recovery_pending: Some(RecoveryDebt {
                earliest: RuntimeGeneration::generate(),
                latest: RuntimeGeneration::generate(),
                dropped: u32::MAX - 1,
            }),
            ..DirtyGenerationState::default()
        };
        state.record_recovery_pending(RuntimeGeneration::generate());
        state.record_recovery_pending(RuntimeGeneration::generate());

        let debt = state.recovery_pending.expect("debt should remain recorded");
        assert_eq!(debt.dropped, u32::MAX);

        let count = state.dirty_generation_count();
        let saturated = usize::try_from(u32::MAX)
            .unwrap_or(usize::MAX)
            .saturating_add(2);
        assert_eq!(count, DirtyGenerationCount::at_least(saturated));
        assert_eq!(count.to_string(), format!("at least {saturated}"));
    }

    #[test]
    fn dirty_generation_count_round_trips_through_its_text_form() {
        let exact = DirtyGenerationCount::exactly(3);
        let bounded = DirtyGenerationCount::at_least(7);

        assert_eq!(exact.to_string(), "3");
        assert_eq!(bounded.to_string(), "at least 7");
        assert_eq!(
            exact
                .to_string()
                .parse::<DirtyGenerationCount>()
                .expect("exact count should parse"),
            exact
        );
        assert_eq!(
            bounded
                .to_string()
                .parse::<DirtyGenerationCount>()
                .expect("lower-bound count should parse"),
            bounded
        );
        assert!("many".parse::<DirtyGenerationCount>().is_err());
        assert!("at least many".parse::<DirtyGenerationCount>().is_err());
    }

    #[test]
    fn load_rejects_dropped_generations_without_a_retained_range() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("runtime.dirty");
        let only = generation("8f0fbc4f-9d2c-4a0b-9d4f-6a2f7a1c0b55");
        let corrupt = DirtyGenerationState {
            recovery_pending: Some(RecoveryDebt {
                earliest: only,
                latest: only,
                dropped: 3,
            }),
            ..DirtyGenerationState::default()
        };
        std::fs::write(
            &path,
            serde_json::to_vec(&corrupt).expect("state should serialize"),
        )
        .expect("corrupt marker should be written");

        let error = DirtyGenerationMarker::load(&path)
            .expect_err("inconsistent debt should be rejected")
            .to_string();

        assert!(error.contains("counts dropped generations without a retained range"));
        assert!(error.contains(&path.display().to_string()));
    }

    fn marker_size(path: &Path) -> u64 {
        std::fs::metadata(path)
            .expect("marker metadata should be readable")
            .len()
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
        assert_eq!(state.recovery_pending, Some(RecoveryDebt::new(legacy)));
        assert_eq!(
            marker.clear().expect("current generation should clear"),
            Some(DirtyGenerationCount::exactly(1))
        );
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
        assert!(
            error.contains("another rust-srec runtime already owns"),
            "contention should name the existing owner: {error}"
        );

        drop(first);
        RuntimeLease::acquire(&marker_path).expect("lease should be reusable after release");
        assert_eq!(
            runtime_lock_path(&marker_path),
            directory.path().join("runtime.dirty.lock")
        );
    }

    /// The desktop app hosts the backend in-process while the standalone server
    /// runs it under the supervisor. Both acquire the lease from the database
    /// URL, so pointing them at one file must let only the first one start.
    #[test]
    fn runtime_lease_is_shared_by_every_entry_point_on_one_database() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let database_path = directory.path().join("srec.db");
        let database_url = format!("sqlite:{}?mode=rwc", database_path.to_string_lossy());

        let desktop = RuntimeLease::acquire_for_database(&database_url)
            .expect("the first entry point should take the lease");

        // The supervisor derives its own path from the same URL rather than
        // being handed one, which is what makes the exclusion mutual.
        let supervisor_marker = marker_path_from_database_url(&database_url)
            .expect("marker path should derive from the database URL");
        let error = RuntimeLease::acquire(&supervisor_marker)
            .err()
            .expect("a second entry point on the same database must be rejected")
            .to_string();
        assert!(
            error.contains("another rust-srec runtime already owns"),
            "contention should name the existing owner: {error}"
        );

        drop(desktop);
        RuntimeLease::acquire(&supervisor_marker)
            .expect("the lease should transfer once the first entry point exits");
    }

    #[test]
    fn separate_databases_do_not_share_a_lease() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let first_url = format!(
            "sqlite:{}?mode=rwc",
            directory.path().join("one.db").to_string_lossy()
        );
        let second_url = format!(
            "sqlite:{}?mode=rwc",
            directory.path().join("two.db").to_string_lossy()
        );

        let _first = RuntimeLease::acquire_for_database(&first_url)
            .expect("the first database should be leased");
        RuntimeLease::acquire_for_database(&second_url)
            .expect("an unrelated database must not be blocked by the first lease");
    }
}
