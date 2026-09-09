//! Sequential media execution with explicit publication ownership.

use async_trait::async_trait;
use tracing::warn;

use super::ProcessorOutput;
use super::outputs::{OutputBatch, accumulate_media_output};
use super::planning::OutputPlan;
use crate::Result;

#[async_trait]
pub(super) trait Publication: Send + Sized {
    async fn commit(self) -> Result<()>;
    async fn rollback(&mut self, produced: &[String]);
}

pub(super) struct StagedPublication(pub(super) OutputBatch);

#[async_trait]
impl Publication for StagedPublication {
    async fn commit(self) -> Result<()> {
        self.0.commit().await
    }
    async fn rollback(&mut self, _produced: &[String]) {
        // OutputBatch's guards clean staged files on drop. Published outputs
        // exist only inside its cancellation-independent commit/rollback task.
    }
}

pub(super) struct IncrementalPublication;

#[async_trait]
impl Publication for IncrementalPublication {
    async fn commit(self) -> Result<()> {
        Ok(())
    }
    async fn rollback(&mut self, produced: &[String]) {
        // Remux publishes each item immediately. Preserve its explicit-error
        // cleanup; dropping the driver does not roll back published outputs.
        for path in produced {
            if let Err(error) = tokio::fs::remove_file(path).await {
                warn!(path = %path, error = %error, "Failed to remove remux output after batch failure");
            }
        }
    }
}

#[async_trait]
pub(super) trait MediaItem: Sync {
    type Publication: Publication;
    async fn process(
        &self,
        input: &str,
        output: Option<&str>,
        publication: &mut Self::Publication,
    ) -> Result<ProcessorOutput>;
}

pub(super) async fn run_media<P: MediaItem>(
    plan: OutputPlan<'_>,
    mut publication: P::Publication,
    processor: &P,
) -> Result<ProcessorOutput> {
    let mut result = ProcessorOutput {
        outputs: Vec::with_capacity(if plan.is_batch() { plan.len() } else { 0 }),
        ..Default::default()
    };
    for (input, output) in plan.items() {
        match processor.process(input, output, &mut publication).await {
            Ok(one) if !plan.is_batch() => result = one,
            Ok(one) => accumulate_media_output(&mut result, one),
            Err(error) => {
                publication.rollback(&result.items_produced).await;
                return Err(error);
            }
        }
    }
    publication.commit().await?;
    Ok(result)
}
