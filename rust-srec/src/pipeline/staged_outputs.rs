//! Durable record of the temporary files processors stage before publishing.
//!
//! A processor writes each output to a `<stem>.tmp-<uuid>[.<ext>]` sibling and
//! renames it into place; its guard deletes the file on failure or
//! cancellation. A crash, power loss or forced exit skips that cleanup, so each
//! attempt records the path before creating the file and forgets it once the
//! worker finalizes the attempt. Rows that survive a restart are the leftovers
//! the API lists. Overwrite backups made while committing an output are not
//! recorded: they exist only for the duration of the final renames.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tracing::warn;

use crate::database::repositories::{NewStagedOutput, StagedOutputRepository};
use crate::database::time::now_ms;
pub(crate) use crate::domain::processor::{
    StagedOutputSink, StagedOutputTracker, is_temp_path_for, remove_leftover,
};

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

#[async_trait]
impl StagedOutputSink for RecorderInner {
    async fn record(&self, temp: &Path, output: &Path) {
        // Set before the write: a failure reported after the row was committed
        // must still have `finish` remove it.
        self.staged.store(true, Ordering::Release);
        let (path, output_path) = (temp.to_string_lossy(), output.to_string_lossy());
        let staged = NewStagedOutput {
            path: &path,
            output_path: &output_path,
            job_id: &self.job_id,
            attempt_id: &self.attempt_id,
        };
        if let Err(error) = self.repository.record(staged, now_ms()).await {
            warn!(
                job_id = %self.job_id,
                path = %temp.display(),
                %error,
                "Failed to record a staged output; it will not be listed if this attempt is interrupted"
            );
        }
    }

    async fn finish(&self) {
        if !self.staged.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) = self.repository.remove_attempt(&self.attempt_id).await {
            warn!(
                job_id = %self.job_id,
                %error,
                "Failed to forget staged outputs of a finished attempt; a restart will list them"
            );
        }
    }
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

    /// Forget this attempt's files once its worker has finalized it.
    pub(crate) async fn finish(&self) {
        if let Some(inner) = &self.inner {
            inner.finish().await;
        }
    }

    /// Erase the SQLite repository behind a [`StagedOutputTracker`].
    pub(crate) fn tracker(&self) -> StagedOutputTracker {
        match &self.inner {
            Some(inner) => StagedOutputTracker::new(Arc::clone(inner) as Arc<dyn StagedOutputSink>),
            None => StagedOutputTracker::default(),
        }
    }
}

impl From<StagedOutputRecorder> for StagedOutputTracker {
    fn from(recorder: StagedOutputRecorder) -> Self {
        recorder.tracker()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::SqlxStagedOutputRepository;
    use crate::domain::processor::temp_path_for;

    #[tokio::test]
    async fn recorder_persists_staged_paths_and_clears_finished_attempts() {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let repository = Arc::new(SqlxStagedOutputRepository::new(pool.clone(), pool));
        let recorder = StagedOutputRecorder::for_attempt(repository.clone(), "job-1");
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("video.mp4");
        let temp = temp_path_for(&output);

        recorder.tracker().record(&temp, &output).await;

        repository.mark_interrupted(1).await.unwrap();
        let rows = repository
            .list_interrupted(Some("job-1"), 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, temp.to_string_lossy());
        assert_eq!(rows[0].output_path, output.to_string_lossy());
        assert!(is_temp_path_for(&temp, &output));

        // A finished attempt leaves nothing for the next startup to list.
        let next = StagedOutputRecorder::for_attempt(repository.clone(), "job-1");
        let other = directory.path().join("other.mp4");
        next.tracker().record(&temp_path_for(&other), &other).await;
        next.finish().await;
        assert_eq!(repository.mark_interrupted(2).await.unwrap(), 0);
    }
}
