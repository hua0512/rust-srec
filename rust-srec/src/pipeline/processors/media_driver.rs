//! Sequential media execution with explicit publication ownership.

use async_trait::async_trait;

use super::ProcessorOutput;
use super::outputs::{OutputBatch, accumulate_media_output};
use super::planning::OutputPlan;
use crate::Result;

#[async_trait]
pub(super) trait MediaItem: Sync {
    async fn process(
        &self,
        input: &str,
        output: Option<&str>,
        batch: &mut OutputBatch,
    ) -> Result<ProcessorOutput>;
}

pub(super) async fn run_media<P: MediaItem>(
    plan: OutputPlan<'_>,
    mut batch: OutputBatch,
    processor: &P,
) -> Result<ProcessorOutput> {
    let mut result = ProcessorOutput {
        outputs: Vec::with_capacity(if plan.is_batch() { plan.len() } else { 0 }),
        ..Default::default()
    };
    for (input, output) in plan.items() {
        let one = processor.process(input, output, &mut batch).await?;
        if plan.is_batch() {
            accumulate_media_output(&mut result, one);
        } else {
            result = one;
        }
    }
    batch.commit().await?;
    Ok(result)
}
