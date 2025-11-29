use crate::error::AppError;
use crate::utils::spans;
use futures::{Stream, StreamExt};
use pipeline_common::{
    CancellationToken, PipelineError, PipelineProvider, ProtocolWriter, StreamerContext,
    config::PipelineConfig,
};
use std::pin::Pin;

use tracing::{Level, Span, span};

pub async fn process_stream<P, W>(
    pipeline_common_config: &PipelineConfig,
    pipeline_config: P::Config,
    stream: Pin<Box<dyn Stream<Item = Result<P::Item, PipelineError>> + Send>>,
    writer_message: &str,
    writer_initializer: impl FnOnce(&Span) -> W,
    token: CancellationToken,
) -> Result<W::Stats, AppError>
where
    P: PipelineProvider,
    P::Config: Send + 'static,
    P::Item: Send + 'static,
    W: ProtocolWriter<Item = P::Item>,
{
    let writer_span = span!(Level::INFO, "writer_processing");
    spans::init_writing_span(&writer_span, writer_message);

    process_stream_with_span::<P, W>(
        pipeline_common_config,
        pipeline_config,
        stream,
        writer_span,
        writer_initializer,
        token,
    )
    .await
}

pub async fn process_stream_with_span<P, W>(
    pipeline_common_config: &PipelineConfig,
    pipeline_config: P::Config,
    stream: Pin<Box<dyn Stream<Item = Result<P::Item, PipelineError>> + Send>>,
    writer_span: Span,
    writer_initializer: impl FnOnce(&Span) -> W,
    token: CancellationToken,
) -> Result<W::Stats, AppError>
where
    P: PipelineProvider,
    P::Config: Send + 'static,
    P::Item: Send + 'static,
    W: ProtocolWriter<Item = P::Item>,
{
    let context = StreamerContext::new(token.clone());
    let pipeline_provider = P::with_config(context, pipeline_common_config, pipeline_config);

    // Create span for pipeline processing under the writer span
    let processing_span = span!(parent: &writer_span, Level::INFO, "pipeline_processing");
    spans::init_processing_span(&processing_span, "Processing pipeline");

    // Build the pipeline (now ChannelPipeline)
    let pipeline = pipeline_provider.build_pipeline();

    // Spawn the pipeline tasks
    let pipeline_common::channel_pipeline::SpawnedPipeline {
        input_tx,
        output_rx,
        tasks: processing_tasks,
    } = pipeline.spawn();

    // Initialize the writer using the provided span
    let mut writer = writer_initializer(&writer_span);
    let writer_task = {
        let span = writer_span.clone();
        tokio::task::spawn_blocking(move || {
            let _enter = span.enter(); // Enter the span in the blocking task
            writer.run(output_rx)
        })
    };

    // Ensure subsequent async work executes within the writer span
    let _writer_guard = writer_span.enter();

    let mut stream = stream;
    while let Some(item_result) = stream.next().await {
        if input_tx.send(item_result).await.is_err() {
            // Upstream channel closed
            break;
        }
    }

    drop(input_tx); // Close the channel to signal completion to the processing task
    drop(_writer_guard);

    // We should also wait for processing tasks to ensure clean shutdown
    let writer_result = writer_task
        .await
        .map_err(|e| AppError::Writer(e.to_string()))?
        .map_err(|e| AppError::Writer(e.to_string()));

    // We should also wait for processing tasks to ensure clean shutdown
    // If writer failed, we still want to wait for tasks but maybe we prioritize writer error
    for task in processing_tasks {
        let task_result = task
            .await
            .map_err(|e| AppError::Pipeline(PipelineError::Processing(e.to_string())))?;

        // If writer succeeded, we care about task errors.
        // If writer failed, we might ignore task errors (which are likely "channel closed")
        if writer_result.is_ok() {
            task_result?;
        }
    }

    writer_result
}
