//! Stage complete processor outputs before publishing them, with batch rollback.

use std::path::{Path, PathBuf};

use tracing::warn;

use crate::{Error, Result};

pub(super) struct TempOutputGuard {
    path: PathBuf,
    cleanup: bool,
    /// Set only once ownership has moved into a blocking commit/promotion closure.
    cleanup_on_current_thread: bool,
}

impl TempOutputGuard {
    pub(super) fn new(output_path: &Path) -> Self {
        let mut name = output_path
            .file_stem()
            .map(std::ffi::OsString::from)
            .unwrap_or_default();
        name.push(format!(".tmp-{}", uuid::Uuid::new_v4()));
        if let Some(extension) = output_path.extension() {
            name.push(".");
            name.push(extension);
        }
        Self {
            path: output_path.with_file_name(name),
            cleanup: true,
            cleanup_on_current_thread: false,
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempOutputGuard {
    fn drop(&mut self) {
        if !self.cleanup {
            return;
        }
        let path = self.path.clone();
        dispatch_cleanup(self.cleanup_on_current_thread, move || {
            remove_temp_output(&path)
        });
    }
}

fn dispatch_cleanup(in_blocking_commit: bool, cleanup: impl FnOnce() + Send + 'static) {
    if !in_blocking_commit && let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn_blocking(cleanup);
    } else {
        cleanup();
    }
}

fn remove_temp_output(path: &Path) {
    for attempt in 0..=20 {
        if attempt > 0 {
            // Windows may briefly retain a file handle after process-tree termination.
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        match std::fs::remove_file(path) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) if attempt == 20 => {
                warn!(%error, path = %path.display(), "Failed to remove temporary processor output");
            }
            Err(_) => {}
        }
    }
}

fn verified_size(path: &Path) -> Result<u64> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| Error::io_path("inspect processor output", path, error))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(Error::PipelineError(format!(
            "Processor did not produce a nonempty regular file: {}",
            path.display()
        )));
    }
    Ok(metadata.len())
}

pub(super) async fn output_size(path: &Path) -> Result<u64> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || verified_size(&path))
        .await
        .map_err(|error| Error::PipelineError(format!("Output validation failed: {error}")))?
}

fn promote(temp: &Path, output: &Path, overwrite: bool) -> Result<()> {
    if overwrite {
        std::fs::rename(temp, output)
            .map_err(|error| Error::io_path("promote processor output", output, error))
    } else {
        // tempfile uses native no-replace rename where available (including Windows,
        // Linux and macOS), with a hard-link fallback on older platforms. Unlike an
        // existence check followed by rename, it never overwrites a competing output.
        let path = tempfile::TempPath::try_from_path(temp)
            .map_err(|error| Error::io_path("prepare output promotion", temp, error))?;
        path.persist_noclobber(output).map_err(|failure| {
            let error = failure.error;
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Error::PipelineError(format!(
                    "Output file already exists and overwrite is disabled: {}",
                    output.display()
                ))
            } else {
                Error::io_path("publish processor output", output, error)
            }
        })
    }
}

pub(super) async fn promote_output(
    temp: TempOutputGuard,
    output: &Path,
    overwrite: bool,
) -> Result<()> {
    let output = output.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut temp = temp;
        temp.cleanup_on_current_thread = true;
        verified_size(temp.path())?;
        promote(temp.path(), &output, overwrite)
    })
    .await
    .map_err(|error| Error::PipelineError(format!("Output promotion failed: {error}")))?
}

fn comparison_key(path: &Path) -> Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(path) => return Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::io_path("resolve output path", path, error)),
    }
    let absolute = std::path::absolute(path)
        .map_err(|error| Error::io_path("resolve output path", path, error))?;
    if let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) {
        match std::fs::canonicalize(parent) {
            Ok(parent) => return Ok(parent.join(name)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io_path("resolve output parent", parent, error)),
        }
    }
    Ok(absolute)
}

fn aliases(left: &Path, right: &Path) -> Result<bool> {
    match same_file::is_same_file(left, right) {
        Ok(true) => return Ok(true),
        Ok(false) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::io_path("compare output identity", left, error)),
    }
    let left = comparison_key(left)?;
    let right = comparison_key(right)?;
    Ok(if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    })
}

fn validate_destination(
    output: &Path,
    sources: &[PathBuf],
    other_outputs: &[PathBuf],
) -> Result<()> {
    for source in sources {
        if aliases(output, source)? {
            return Err(Error::PipelineError(format!(
                "Output path aliases an input file: {}",
                output.display()
            )));
        }
    }
    for other in other_outputs {
        if aliases(output, other)? {
            return Err(Error::PipelineError(format!(
                "Multiple outputs name the same file: {}",
                output.display()
            )));
        }
    }
    Ok(())
}

struct PendingOutput {
    temp: TempOutputGuard,
    output: PathBuf,
    overwrite: bool,
    original: Option<TempOutputGuard>,
    published_identity: Option<same_file::Handle>,
}

pub(super) struct OutputBatch {
    sources: Vec<PathBuf>,
    pending: Vec<PendingOutput>,
    committed: bool,
}

impl OutputBatch {
    pub(super) fn new(inputs: &[String]) -> Self {
        Self {
            sources: inputs.iter().map(PathBuf::from).collect(),
            pending: Vec::new(),
            committed: false,
        }
    }

    pub(super) fn protect(&mut self, input: &Path) {
        self.sources.push(input.to_owned());
    }

    pub(super) fn discard(&mut self, temp: &Path) {
        self.pending.retain(|entry| entry.temp.path() != temp);
    }

    pub(super) async fn stage(&mut self, output: &Path, overwrite: bool) -> Result<PathBuf> {
        let sources = self.sources.clone();
        let other_outputs: Vec<_> = self
            .pending
            .iter()
            .map(|entry| entry.output.clone())
            .collect();
        let output = output.to_owned();
        let checked_output = output.clone();
        tokio::task::spawn_blocking(move || {
            validate_destination(&checked_output, &sources, &other_outputs)
        })
        .await
        .map_err(|error| {
            Error::PipelineError(format!("Output path validation failed: {error}"))
        })??;
        let temp = TempOutputGuard::new(&output);
        let path = temp.path().to_owned();
        self.pending.push(PendingOutput {
            temp,
            output,
            overwrite,
            original: None,
            published_identity: None,
        });
        Ok(path)
    }

    pub(super) async fn commit(self) -> Result<()> {
        // Once publication starts, the owned transaction finishes or rolls back
        // independently of cancellation. It can only publish complete files.
        tokio::task::spawn_blocking(move || self.commit_blocking())
            .await
            .map_err(|error| Error::PipelineError(format!("Output commit failed: {error}")))?
    }

    fn commit_blocking(mut self) -> Result<()> {
        for entry in &mut self.pending {
            entry.temp.cleanup_on_current_thread = true;
        }
        let mut outputs = Vec::new();
        for entry in &self.pending {
            verified_size(entry.temp.path())?;
            validate_destination(&entry.output, &self.sources, &outputs)?;
            outputs.push(entry.output.clone());
        }
        for entry in &mut self.pending {
            let identity = same_file::Handle::from_path(entry.temp.path()).map_err(|error| {
                Error::io_path("inspect output identity", entry.temp.path(), error)
            })?;
            if entry.overwrite {
                match std::fs::symlink_metadata(&entry.output) {
                    Ok(metadata) => {
                        if !metadata.is_file() {
                            return Err(Error::PipelineError(format!(
                                "Output destination is not a regular file: {}",
                                entry.output.display()
                            )));
                        }
                        let mut backup = TempOutputGuard::new(&entry.output);
                        backup.cleanup_on_current_thread = true;
                        if std::fs::hard_link(&entry.output, backup.path()).is_err() {
                            // Filesystems without hard links still support replacement.
                            std::fs::copy(&entry.output, backup.path()).map_err(|error| {
                                Error::io_path("preserve existing output", &entry.output, error)
                            })?;
                        }
                        entry.original = Some(backup);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(Error::io_path(
                            "inspect output destination",
                            &entry.output,
                            error,
                        ));
                    }
                }
            }
            promote(entry.temp.path(), &entry.output, entry.overwrite)?;
            entry.published_identity = Some(identity);
        }
        self.committed = true;
        Ok(())
    }
}

impl Drop for OutputBatch {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // Published entries exist only inside commit_blocking, so rollback IO stays
        // on that worker. Dropping an uncommitted async batch only drops staged guards.
        for entry in self.pending.iter_mut().rev() {
            let Some(identity) = &entry.published_identity else {
                continue;
            };
            let still_owned = same_file::Handle::from_path(&entry.output)
                .is_ok_and(|current| current == *identity);
            if !still_owned {
                if let Some(original) = &mut entry.original {
                    original.cleanup = false;
                    warn!(path = %entry.output.display(), backup = %original.path().display(), "Output changed during rollback; retained original backup");
                }
                continue;
            }
            let result = if let Some(original) = &entry.original {
                std::fs::rename(original.path(), &entry.output)
            } else {
                std::fs::remove_file(&entry.output)
            };
            if let Err(error) = result {
                if let Some(original) = &mut entry.original {
                    original.cleanup = false;
                }
                warn!(%error, path = %entry.output.display(), backup = ?entry.original.as_ref().map(|original| original.path()), "Failed to roll back processor output");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn temporary_output_cleanup_does_not_block_async_runtime() {
        let runtime_thread = std::thread::current().id();
        let (started, observed) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::sync_channel(1);
        let (finished, completion) = tokio::sync::oneshot::channel();
        dispatch_cleanup(false, move || {
            started.send(std::thread::current().id()).unwrap();
            blocked
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            finished.send(()).unwrap();
        });
        let cleanup_thread = tokio::time::timeout(std::time::Duration::from_secs(1), observed)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(cleanup_thread, runtime_thread);
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), completion)
            .await
            .unwrap()
            .unwrap();
    }

    async fn staged(
        batch: &mut OutputBatch,
        output: &Path,
        overwrite: bool,
        bytes: &[u8],
    ) -> PathBuf {
        let path = batch.stage(output, overwrite).await.unwrap();
        tokio::fs::write(&path, bytes).await.unwrap();
        path
    }

    fn assert_no_temps(dir: &Path) {
        assert!(std::fs::read_dir(dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp-")
        }));
    }

    #[tokio::test]
    async fn missing_or_empty_outputs_do_not_replace_existing_files() {
        for missing in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let output = dir.path().join("output.mp4");
            std::fs::write(&output, b"original").unwrap();
            let mut batch = OutputBatch::new(&[]);
            let temp = batch.stage(&output, true).await.unwrap();
            if !missing {
                std::fs::write(&temp, b"").unwrap();
            }
            assert!(batch.commit().await.is_err());
            assert_eq!(std::fs::read(output).unwrap(), b"original");
            assert_no_temps(dir.path());
        }
    }

    #[tokio::test]
    async fn incomplete_batch_drop_removes_only_staged_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.mp4");
        let output = dir.path().join("output.mp4");
        std::fs::write(&input, b"source").unwrap();
        std::fs::write(&output, b"existing").unwrap();
        let mut batch = OutputBatch::new(&[input.to_string_lossy().into_owned()]);
        let temp = staged(&mut batch, &output, true, b"partial").await;
        drop(batch);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while tokio::fs::try_exists(&temp).await.unwrap() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(input).unwrap(), b"source");
        assert_eq!(std::fs::read(output).unwrap(), b"existing");
        assert_no_temps(dir.path());
    }

    #[tokio::test]
    async fn failed_publication_rolls_back_new_and_overwritten_outputs() {
        for replace in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let first = dir.path().join("first.mp4");
            let second = dir.path().join("second.mp4");
            if replace {
                std::fs::write(&first, b"original").unwrap();
            }
            let mut batch = OutputBatch::new(&[]);
            staged(&mut batch, &first, replace, b"completed first").await;
            staged(&mut batch, &second, false, b"completed second").await;
            std::fs::write(&second, b"unrelated").unwrap();
            assert!(batch.commit().await.is_err());
            if replace {
                assert_eq!(std::fs::read(first).unwrap(), b"original");
            } else {
                assert!(!first.exists());
            }
            assert_eq!(std::fs::read(second).unwrap(), b"unrelated");
            assert_no_temps(dir.path());
        }
    }

    #[tokio::test]
    async fn no_overwrite_publication_has_one_winner() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("output.mp4");
        let mut first = OutputBatch::new(&[]);
        let mut second = OutputBatch::new(&[]);
        staged(&mut first, &output, false, b"first").await;
        staged(&mut second, &output, false, b"second").await;
        let (first, second) = tokio::join!(first.commit(), second.commit());
        assert_ne!(first.is_ok(), second.is_ok());
        let bytes = std::fs::read(output).unwrap();
        assert!(bytes == b"first" || bytes == b"second");
        assert_no_temps(dir.path());
    }

    #[tokio::test]
    async fn source_aliases_and_duplicate_destinations_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.mp4");
        let hardlink = dir.path().join("alias.mp4");
        std::fs::write(&input, b"source").unwrap();
        std::fs::hard_link(&input, &hardlink).unwrap();
        let mut batch = OutputBatch::new(&[input.to_string_lossy().into_owned()]);
        assert!(batch.stage(&input, true).await.is_err());
        assert!(batch.stage(&hardlink, true).await.is_err());
        assert!(
            batch
                .stage(&dir.path().join(".").join("input.mp4"), true)
                .await
                .is_err()
        );
        #[cfg(unix)]
        {
            let symlink = dir.path().join("symlink.mp4");
            std::os::unix::fs::symlink(&input, &symlink).unwrap();
            assert!(batch.stage(&symlink, true).await.is_err());
        }
        let output = dir.path().join("output.mp4");
        batch.stage(&output, true).await.unwrap();
        assert!(
            batch
                .stage(&dir.path().join(".").join("output.mp4"), true)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(input).unwrap(), b"source");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn temporary_cleanup_retries_until_exclusive_handle_closes() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let guard = TempOutputGuard::new(&dir.path().join("output.mp4"));
        let temp = guard.path().to_owned();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(&temp)
            .unwrap();
        drop(guard);
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        drop(file);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while temp.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_no_temps(dir.path());
    }
}
