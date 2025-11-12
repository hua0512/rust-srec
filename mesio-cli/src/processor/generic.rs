use crate::error::AppError;
use futures::{Stream, StreamExt};
use pipeline_common::{
    config::PipelineConfig, CancellationToken, PipelineError, PipelineProvider, ProtocolWriter,
    StreamerContext,
};
use std::pin::Pin;
use std::sync::mpsc;
use tracing::warn;

pub async fn process_stream<P, W>(
    pipeline_common_config: &PipelineConfig,
    pipeline_config: P::Config,
    stream: Pin<Box<dyn Stream<Item = Result<P::Item, PipelineError>> + Send>>,
    writer_initializer: impl FnOnce() -> W,
    token: CancellationToken,
) -> Result<W::Stats, AppError>
where
    P: PipelineProvider,
    P::Config: Send + 'static,
    P::Item: Send + 'static,
    W: ProtocolWriter<Item = P::Item>,
{
    let (tx, rx) = mpsc::sync_channel(pipeline_common_config.channel_size);
    let (processed_tx, processed_rx) = mpsc::sync_channel(pipeline_common_config.channel_size);

    let context = StreamerContext::new(token.clone());
    let pipeline_provider = P::with_config(context, pipeline_common_config, pipeline_config);

    let processing_task = tokio::task::spawn_blocking(move || {
        let pipeline = pipeline_provider.build_pipeline();
        let input_iter = std::iter::from_fn(move || rx.recv().map(Some).unwrap_or(None));

        let mut output = |result: Result<P::Item, PipelineError>| {
            if let Err(ref send_error) = processed_tx.send(result) {
                // Downstream channel closed, stop processing
                // get error and log it
                if let Err(e) = send_error.0.as_ref() {
                    warn!("Output channel closed, stopping processing: {e}");
                } else {
                    warn!("Output channel closed, stopping processing");
                }
            }
        };

        if let Err(e) = pipeline.run(input_iter, &mut output) {
            tracing::error!("Pipeline processing failed: {}", e);
        }
    });

    let mut writer = writer_initializer();
    let writer_task = tokio::task::spawn_blocking(move || writer.run(processed_rx));

    let mut stream = stream;
    while let Some(item_result) = stream.next().await {
        if tx.send(item_result).is_err() {
            // Upstream channel closed
            break;
        }
    }

    drop(tx); // Close the channel to signal completion to the processing task

    processing_task
        .await
        .map_err(|e| AppError::Pipeline(PipelineError::Processing(e.to_string())))?;
    let writer_result = writer_task
        .await
        .map_err(|e| AppError::Writer(e.to_string()))?
        .map_err(|e| AppError::Writer(e.to_string()))?;

    Ok(writer_result)
}
