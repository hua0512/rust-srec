//! Sequential media execution with explicit publication ownership.

use async_trait::async_trait;

use super::ProcessorOutput;
use super::outputs::{OutputBatch, accumulate_media_output};
use super::planning::OutputPlan;
use crate::Result;
use crate::pipeline::ProgressReporter;

#[async_trait]
pub(super) trait MediaItem: Sync {
    /// Whether `process` runs FFmpeg on `input` rather than passing it
    /// through; batch progress counts only these inputs.
    fn runs_ffmpeg(&self, input: &str) -> bool;

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
    progress: &ProgressReporter,
    processor: &P,
) -> Result<ProcessorOutput> {
    let mut result = ProcessorOutput {
        outputs: Vec::with_capacity(if plan.is_batch() { plan.len() } else { 0 }),
        ..Default::default()
    };
    let ffmpeg_runs = plan
        .items()
        .filter(|(input, _)| processor.runs_ffmpeg(input))
        .count();
    let mut ffmpeg_index = 0;
    for (input, output) in plan.items() {
        if processor.runs_ffmpeg(input) {
            progress.set_batch_item(ffmpeg_index, ffmpeg_runs, input);
            ffmpeg_index += 1;
        }
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

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::super::ProcessorContext;
    use super::*;
    use crate::pipeline::{JobProgressSnapshot, ProgressKind};

    /// Reports FFmpeg's end for every input it does not pass through.
    struct EndingRun(ProgressReporter);

    #[async_trait]
    impl MediaItem for EndingRun {
        fn runs_ffmpeg(&self, input: &str) -> bool {
            !input.ends_with(".xml")
        }

        async fn process(
            &self,
            input: &str,
            _output: Option<&str>,
            _batch: &mut OutputBatch,
        ) -> Result<ProcessorOutput> {
            if self.runs_ffmpeg(input) {
                let mut end = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
                end.percent = Some(100.0);
                self.0.report(end);
            }
            Ok(ProcessorOutput::default())
        }
    }

    #[tokio::test]
    async fn passed_through_inputs_take_no_share_of_batch_progress() {
        let (tx, mut rx) = mpsc::channel(4);
        let progress = ProgressReporter::new("job", tx);
        let inputs = ["seg.flv".to_owned(), "seg.xml".to_owned()];
        let ctx = ProcessorContext::noop("job");
        let Ok(plan) = OutputPlan::unary_or_mapped(&inputs, &[]) else {
            panic!("outputs may be omitted");
        };

        run_media(
            plan,
            OutputBatch::new(&inputs, &ctx),
            &progress,
            &EndingRun(progress.clone()),
        )
        .await
        .unwrap();

        let end = rx.try_recv().unwrap().snapshot;
        assert_eq!(
            (end.files_done, end.files_total, end.percent),
            (Some(0), Some(1), Some(100.0))
        );
    }
}
