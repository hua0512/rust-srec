use tokio::task::JoinHandle;

use crate::PipelineError;

/// Unified completion error for a writer + pipeline-task run.
#[derive(Debug)]
pub enum RunCompletionError<WriterErr> {
    Writer(WriterErr),
    Pipeline(PipelineError),
}

/// Wait for all processing tasks and resolve writer/pipeline outcome deterministically.
///
/// Semantics:
/// - If writer succeeded and a pipeline task failed, return `Pipeline`.
/// - If writer failed, return `Writer` (pipeline task failures are treated as secondary).
/// - If both succeeded, return writer output.
pub async fn settle_run<WriterOut, WriterErr>(
    writer_result: Result<WriterOut, WriterErr>,
    processing_tasks: Vec<JoinHandle<Result<(), PipelineError>>>,
) -> Result<WriterOut, RunCompletionError<WriterErr>> {
    let writer_ok = writer_result.is_ok();
    let mut first_pipeline_error: Option<PipelineError> = None;

    for task in processing_tasks {
        let task_result = match task.await {
            Ok(result) => result,
            Err(join_error) if join_error.is_cancelled() => Err(PipelineError::Cancelled),
            Err(join_error) => Err(PipelineError::Strategy(Box::new(std::io::Error::other(
                format!("Pipeline task panicked: {join_error}"),
            )))),
        };

        if writer_ok
            && let Err(err) = task_result
            && first_pipeline_error.is_none()
        {
            first_pipeline_error = Some(err);
        }
    }

    match (writer_result, first_pipeline_error) {
        (Ok(_), Some(err)) => Err(RunCompletionError::Pipeline(err)),
        (Ok(output), None) => Ok(output),
        (Err(err), _) => Err(RunCompletionError::Writer(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::{RunCompletionError, settle_run};
    use crate::PipelineError;

    #[tokio::test]
    async fn settle_run_returns_pipeline_error_when_writer_succeeds() {
        let tasks = vec![tokio::spawn(async {
            Err(PipelineError::Strategy(Box::new(std::io::Error::other(
                "pipeline failed",
            ))))
        })];

        let result = settle_run::<usize, ()>(Ok(1), tasks).await;
        match result {
            Err(RunCompletionError::Pipeline(err)) => {
                assert_eq!(err.to_string(), "pipeline failed");
            }
            other => panic!("expected pipeline error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn settle_run_prioritizes_writer_error() {
        let tasks = vec![tokio::spawn(async {
            Err(PipelineError::Strategy(Box::new(std::io::Error::other(
                "pipeline failed",
            ))))
        })];

        let result = settle_run::<usize, &str>(Err("writer failed"), tasks).await;
        match result {
            Err(RunCompletionError::Writer(err)) => {
                assert_eq!(err, "writer failed");
            }
            other => panic!("expected writer error, got {:?}", other),
        }
    }
}
