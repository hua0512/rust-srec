use crate::error::{AppError, is_broken_pipe_error};
use crate::utils::spans;
use futures::{Stream, StreamExt};
use pipeline_common::{
    CancellationToken, FormatStrategy, PipelineError, PipelineProvider, ProtocolWriter,
    StreamerContext, WriterConfig, WriterStats, WriterTask, config::PipelineConfig,
};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{Level, Span, span, warn};

pub async fn process_stream<P, W>(
    pipeline_common_config: &PipelineConfig,
    pipeline_config: P::Config,
    stream: Pin<Box<dyn Stream<Item = Result<P::Item, PipelineError>> + Send>>,
    writer_message: &str,
    writer_initializer: impl FnOnce(&Span) -> W,
    token: CancellationToken,
) -> Result<WriterStats, AppError>
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
) -> Result<WriterStats, AppError>
where
    P: PipelineProvider,
    P::Config: Send + 'static,
    P::Item: Send + 'static,
    W: ProtocolWriter<Item = P::Item>,
{
    let context = Arc::new(StreamerContext::new(token.clone()));
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
        let task_result = task.await.map_err(|e| {
            AppError::Pipeline(PipelineError::Strategy(Box::new(std::io::Error::other(
                e.to_string(),
            ))))
        })?;

        // If writer succeeded, we care about task errors.
        // If writer failed, we might ignore task errors (which are likely "channel closed")
        if writer_result.is_ok() {
            task_result?;
        }
    }

    writer_result
}

/// Statistics from pipe stream processing
#[derive(Debug, Clone)]
pub struct PipeStreamStats {
    pub items_written: usize,
    pub segment_count: u32,
    pub bytes_written: u64,
}

/// Spawn a blocking writer task that reads from a channel and writes to stdout.
/// Generic over the data type `D` and strategy `S`.
/// When a broken pipe is detected, the cancellation token is triggered to stop upstream processing.
fn spawn_pipe_writer_task<D, S>(
    rx: tokio::sync::mpsc::Receiver<Result<D, PipelineError>>,
    strategy: S,
    extension: &str,
    token: CancellationToken,
) -> tokio::task::JoinHandle<Result<PipeStreamStats, (String, bool)>>
where
    D: Send + 'static,
    S: FormatStrategy<D>,
    S::Writer: Send,
{
    let config = WriterConfig::new(
        PathBuf::from("."),
        "stdout".to_string(),
        extension.to_string(),
    );
    let mut writer_task_instance = WriterTask::new(config, strategy);
    let current_span = Span::current();

    tokio::task::spawn_blocking(move || {
        let _enter = current_span.enter();
        let mut rx = rx;
        let mut broken_pipe_detected = false;

        while let Some(item_result) = rx.blocking_recv() {
            match item_result {
                Ok(item) => {
                    if let Err(e) = writer_task_instance.process_item(item) {
                        let err_str = e.to_string();
                        if is_broken_pipe_error(&err_str) {
                            warn!("Pipe closed by consumer (broken pipe), cancelling upstream");
                            token.cancel();
                            broken_pipe_detected = true;
                            break;
                        }
                        return Err((format!("Writer error: {}", err_str), false));
                    }
                }
                Err(e) => {
                    return Err((format!("Pipeline error: {}", e), false));
                }
            }
        }

        if let Err(e) = writer_task_instance.close() {
            let err_str = e.to_string();
            if is_broken_pipe_error(&err_str) {
                warn!("Broken pipe during close: consumer already disconnected");
                token.cancel();
                broken_pipe_detected = true;
            } else {
                return Err((format!("Close error: {}", err_str), false));
            }
        }

        // Return error if broken pipe was detected at any point
        if broken_pipe_detected {
            return Err(("Broken pipe".to_string(), true));
        }

        let state = writer_task_instance.get_state();
        Ok(PipeStreamStats {
            items_written: state.items_written_total,
            segment_count: state.file_sequence_number,
            bytes_written: state.bytes_written_total,
        })
    })
}

/// Convert writer task result to AppError
fn handle_pipe_writer_result(
    result: Result<Result<PipeStreamStats, (String, bool)>, tokio::task::JoinError>,
) -> Result<PipeStreamStats, AppError> {
    match result {
        Ok(Ok(stats)) => Ok(stats),
        Ok(Err((msg, is_broken_pipe))) => {
            if is_broken_pipe {
                // Broken pipe is expected behavior when consumer closes
                Err(AppError::BrokenPipe)
            } else {
                Err(AppError::Writer(msg))
            }
        }
        Err(e) => Err(AppError::Writer(e.to_string())),
    }
}

/// Process a stream to pipe output (stdout) using the provided strategy.
/// This is a generic helper that handles channel creation, writer task spawning,
/// stream forwarding, and broken pipe handling.
/// When a broken pipe is detected, upstream reading is stopped via cancellation.
pub async fn process_pipe_stream<D, S>(
    stream: Pin<Box<dyn Stream<Item = Result<D, PipelineError>> + Send>>,
    pipeline_config: &PipelineConfig,
    strategy: S,
    extension: &str,
) -> Result<PipeStreamStats, AppError>
where
    D: Send + 'static,
    S: FormatStrategy<D>,
    S::Writer: Send,
{
    let token = CancellationToken::new();
    let (tx, rx) = tokio::sync::mpsc::channel(pipeline_config.channel_size);
    let writer_task = spawn_pipe_writer_task(rx, strategy, extension, token.clone());

    let mut stream = stream;
    while let Some(item_result) = stream.next().await {
        // Check if cancellation was requested (e.g., broken pipe)
        if token.is_cancelled() {
            break;
        }
        if tx.send(item_result).await.is_err() {
            break;
        }
    }
    drop(tx);

    handle_pipe_writer_result(writer_task.await)
}

/// Process a stream through a pipeline and then to pipe output (stdout).
/// Chains the pipeline processing with pipe output using the provided strategy.
/// When a broken pipe is detected, the entire pipeline and upstream reading are stopped.
pub async fn process_pipe_stream_with_processing<P, S>(
    stream: Pin<Box<dyn Stream<Item = Result<P::Item, PipelineError>> + Send>>,
    pipeline_config: &PipelineConfig,
    pipeline_type_config: P::Config,
    strategy: S,
    extension: &str,
) -> Result<PipeStreamStats, AppError>
where
    P: PipelineProvider,
    P::Config: Send + 'static,
    P::Item: Send + 'static,
    S: FormatStrategy<P::Item>,
    S::Writer: Send,
{
    // Create a shared cancellation token for the entire pipe processing chain
    let token = CancellationToken::new();
    let context = Arc::new(StreamerContext::new(token.clone()));
    let pipeline_provider = P::with_config(context, pipeline_config, pipeline_type_config);
    let pipeline = pipeline_provider.build_pipeline();

    let pipeline_common::channel_pipeline::SpawnedPipeline {
        input_tx,
        output_rx,
        tasks: processing_tasks,
    } = pipeline.spawn();

    // Pass the token to the writer task so it can cancel on broken pipe
    let writer_task = spawn_pipe_writer_task(output_rx, strategy, extension, token.clone());

    let mut stream = stream;
    while let Some(item_result) = stream.next().await {
        // Check if cancellation was requested (e.g., broken pipe detected by writer)
        if token.is_cancelled() {
            break;
        }
        if input_tx.send(item_result).await.is_err() {
            break;
        }
    }
    drop(input_tx);

    let writer_result = handle_pipe_writer_result(writer_task.await);

    // Wait for processing tasks to ensure clean shutdown
    for task in processing_tasks {
        let task_result = task.await.map_err(|e| {
            AppError::Pipeline(PipelineError::Strategy(Box::new(std::io::Error::other(
                e.to_string(),
            ))))
        })?;

        // If writer succeeded, we care about task errors
        // If writer failed (including broken pipe), we ignore task errors
        // (which are likely "channel closed" or "cancelled")
        if writer_result.is_ok() {
            task_result?;
        }
    }

    writer_result
}
