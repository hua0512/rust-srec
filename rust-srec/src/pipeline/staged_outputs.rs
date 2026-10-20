//! Durable record of the temporary files processors stage before publishing.
//!
//! A processor writes each output to a `<stem>.tmp-<uuid>[.<ext>]` sibling and
//! renames it into place; its guard deletes the file on failure or
//! cancellation. A crash, power loss or forced exit skips that cleanup, so each
//! attempt records the path before creating the file and forgets it once the
//! worker finalizes the attempt. Rows that survive a restart are the leftovers
//! the API lists. Overwrite backups made while committing an output are not
//! recorded: they exist only for the duration of the final renames.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::warn;

use crate::database::repositories::{NewStagedOutput, StagedOutputRepository};
use crate::database::time::now_ms;

const TEMP_MARKER: &str = ".tmp-";

/// The temporary sibling a processor writes before publishing `output`.
pub(crate) fn temp_path_for(output: &Path) -> PathBuf {
    let mut name = output
        .file_stem()
        .map(std::ffi::OsString::from)
        .unwrap_or_default();
    name.push(format!("{TEMP_MARKER}{}", uuid::Uuid::new_v4()));
    if let Some(extension) = output.extension() {
        name.push(".");
        name.push(extension);
    }
    output.with_file_name(name)
}

/// Whether `path` has exactly the shape [`temp_path_for`] gives `output`.
pub(crate) fn is_temp_path_for(path: &Path, output: &Path) -> bool {
    if path.parent() != output.parent() {
        return false;
    }
    let (Some(name), Some(stem)) = (
        path.file_name().and_then(|name| name.to_str()),
        output.file_stem().and_then(|stem| stem.to_str()),
    ) else {
        return false;
    };
    let suffix = match output.extension() {
        Some(extension) => match extension.to_str() {
            Some(extension) => format!(".{extension}"),
            None => return false,
        },
        None => String::new(),
    };
    name.strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix(TEMP_MARKER))
        .and_then(|rest| rest.strip_suffix(suffix.as_str()))
        .is_some_and(|id| id.len() == 36 && uuid::Uuid::try_parse(id).is_ok())
}

/// Delete a leftover temporary file. A file that is already gone counts as
/// deleted; anything other than a regular file is refused.
pub(crate) fn remove_leftover(path: &Path) -> std::io::Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        // Copying a read-only source on Windows keeps that attribute on the
        // temporary copy, which then refuses deletion until it is cleared.
        #[cfg(windows)]
        Err(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied
                && metadata.permissions().readonly() =>
        {
            let mut permissions = metadata.permissions();
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "Windows-only attribute reset on a file that is about to be deleted"
            )]
            permissions.set_readonly(false);
            std::fs::set_permissions(path, permissions)?;
            std::fs::remove_file(path).map(|()| true)
        }
        Err(error) => Err(error),
    }
}

/// Records one attempt's temporary files. Recording is best effort: a failed
/// write only costs the listing of that file after a crash, so processing
/// continues with a warning.
#[derive(Clone, Default)]
pub(crate) struct StagedOutputRecorder {
    inner: Option<Arc<RecorderInner>>,
}

struct RecorderInner {
    repository: Arc<dyn StagedOutputRepository>,
    job_id: String,
    attempt_id: String,
    /// Set by the first recording, so finishing an attempt that staged nothing,
    /// such as an upload, costs no write.
    staged: AtomicBool,
}

impl StagedOutputRecorder {
    /// A recorder for one new attempt of `job_id`.
    pub(crate) fn for_attempt(
        repository: Arc<dyn StagedOutputRepository>,
        job_id: impl Into<String>,
    ) -> Self {
        Self {
            inner: Some(Arc::new(RecorderInner {
                repository,
                job_id: job_id.into(),
                attempt_id: uuid::Uuid::new_v4().to_string(),
                staged: AtomicBool::new(false),
            })),
        }
    }

    /// Record `temp` before the caller creates it.
    pub(crate) async fn record(&self, temp: &Path, output: &Path) {
        let Some(inner) = &self.inner else {
            return;
        };
        // Set before the write: a failure reported after the row was committed
        // must still have `finish` remove it.
        inner.staged.store(true, Ordering::Release);
        let (path, output_path) = (temp.to_string_lossy(), output.to_string_lossy());
        let staged = NewStagedOutput {
            path: &path,
            output_path: &output_path,
            job_id: &inner.job_id,
            attempt_id: &inner.attempt_id,
        };
        if let Err(error) = inner.repository.record(staged, now_ms()).await {
            warn!(
                job_id = %inner.job_id,
                path = %temp.display(),
                %error,
                "Failed to record a staged output; it will not be listed if this attempt is interrupted"
            );
        }
    }

    /// Forget this attempt's files once its worker has finalized it.
    pub(crate) async fn finish(&self) {
        let Some(inner) = &self.inner else {
            return;
        };
        if !inner.staged.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) = inner.repository.remove_attempt(&inner.attempt_id).await {
            warn!(
                job_id = %inner.job_id,
                %error,
                "Failed to forget staged outputs of a finished attempt; a restart will list them"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_paths_match_only_their_own_output() {
        let output = Path::new("/videos/stream.part1.mp4");
        let temp = temp_path_for(output);
        assert!(is_temp_path_for(&temp, output));
        assert!(!is_temp_path_for(&temp, Path::new("/videos/other.mp4")));
        assert!(!is_temp_path_for(
            &temp,
            Path::new("/elsewhere/stream.part1.mp4")
        ));
        assert!(!is_temp_path_for(output, output));

        let bare = Path::new("/videos/stream");
        assert!(is_temp_path_for(&temp_path_for(bare), bare));

        for name in [
            "stream.part1.tmp-not-a-uuid.mp4",
            "stream.part1.tmp-6d1c3d5e-8a3b-4c41-9d2e-1f0a9b7c2e11.mkv",
            "stream.part1.tmp-6d1c3d5e8a3b4c419d2e1f0a9b7c2e11.mp4",
            "stream.part1.mp4",
        ] {
            assert!(
                !is_temp_path_for(&Path::new("/videos").join(name), output),
                "{name}"
            );
        }
        assert!(is_temp_path_for(
            &Path::new("/videos").join("stream.part1.tmp-6d1c3d5e-8a3b-4c41-9d2e-1f0a9b7c2e11.mp4"),
            output
        ));
    }

    #[cfg(windows)]
    #[test]
    fn removing_a_read_only_leftover_clears_the_attribute() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("a.tmp-x.mp4");
        std::fs::write(&file, b"partial").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();

        assert!(remove_leftover(&file).unwrap());
        assert!(!file.exists());
    }
}
