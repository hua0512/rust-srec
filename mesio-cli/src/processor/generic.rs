use crate::error::AppError;
use crossbeam_channel as mpsc;
use futures::{Stream, StreamExt};
use pipeline_common::{
    PipelineError, PipelineProvider, ProtocolWriter, StreamerContext, config::PipelineConfig,
    progress::ProgressEvent,
};
use std::{path::Path, sync::Arc};
use tracing::warn;

pub async fn process_stream<C, I, P, W, S, E, F>(
    pipeline_common_config: &PipelineConfig,
    pipeline_config: C,
    stream: S,
    output_dir: &Path,
    base_name: &str,
    extension: &str,
    on_progress: Option<Arc<F>>,
) -> Result<W::Stats, AppError>
where
    C: Send + 'static,
    I: Send + 'static,
    P: PipelineProvider<Item = I, Config = C>,
    W: ProtocolWriter<F, Item = I>,
    S: Stream<Item = Result<I, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
    F: Fn(ProgressEvent) + Send + Sync + 'static,
{
    let (tx, rx) = mpsc::bounded(pipeline_common_config.channel_size);
    let (processed_tx, processed_rx) = mpsc::bounded(pipeline_common_config.channel_size);

    let context = StreamerContext::new();
    let pipeline_provider = P::with_config(context, pipeline_common_config, pipeline_config);

    let processing_task = tokio::task::spawn_blocking(move || {
        let pipeline = pipeline_provider.build_pipeline();
        let input_iter = std::iter::from_fn(move || rx.recv().map(Some).unwrap_or(None));

        let mut output = |result: Result<I, PipelineError>| {
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

        if let Err(e) = pipeline.process(input_iter, &mut output) {
            tracing::error!("Pipeline processing failed: {}", e);
        }
    });

    let mut writer = W::new(
        output_dir.to_path_buf(),
        base_name.to_string(),
        extension.to_string(),
        on_progress,
    );
    let writer_task = tokio::task::spawn_blocking(move || writer.run(processed_rx));

    let mut stream = Box::pin(stream);
    while let Some(item_result) = stream.next().await {
        let item = item_result.map_err(|e| PipelineError::Processing(e.to_string()));
        if tx.send(item).is_err() {
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
