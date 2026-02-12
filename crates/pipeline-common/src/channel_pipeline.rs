//! # Channel-Based Pipeline Implementation
//!
//! This module provides a channel-based pipeline implementation that runs each processor
//! in its own task, connected by channels. This allows for pipeline parallelism and
//! better backpressure handling.

use crate::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error};

/// Default capacity for channels between stages
const DEFAULT_CHANNEL_CAPACITY: usize = 32;

fn stage_process_error(
    stage: &'static str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> PipelineError {
    PipelineError::StageProcess {
        stage,
        source: Box::new(source),
    }
}

fn stage_finish_error(
    stage: &'static str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> PipelineError {
    PipelineError::StageFinish {
        stage,
        source: Box::new(source),
    }
}

/// A channel-based pipeline for processing data through a series of processors.
///
/// Unlike the synchronous `Pipeline`, this implementation spawns a Tokio task for
/// each processor, connecting them with MPSC channels. This allows stages to run
/// in parallel and provides buffering between stages.
///
/// Runtime model:
/// - Each stage runs in `tokio::task::spawn_blocking`.
/// - Stage processors are expected to be synchronous (`Processor<T>`).
pub struct ChannelPipeline<T> {
    processors: Vec<Box<dyn Processor<T> + Send>>,
    context: Arc<StreamerContext>,
    channel_size: usize,
}

/// Result of spawning a pipeline
pub struct SpawnedPipeline<T> {
    pub input_tx: mpsc::Sender<Result<T, PipelineError>>,
    pub output_rx: mpsc::Receiver<Result<T, PipelineError>>,
    pub tasks: Vec<JoinHandle<Result<(), PipelineError>>>,
}

impl<T> ChannelPipeline<T>
where
    T: Send + 'static,
{
    /// Create a new empty pipeline with the given processing context.
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            processors: Vec::new(),
            context,
            channel_size: DEFAULT_CHANNEL_CAPACITY,
        }
    }

    /// Set the channel size for connections between processors.
    pub fn with_channel_size(mut self, size: usize) -> Self {
        self.channel_size = size;
        self
    }

    /// Add a processor to the end of the pipeline.
    pub fn add_processor<P: Processor<T> + Send + 'static>(mut self, processor: P) -> Self {
        self.processors.push(Box::new(processor));
        self
    }

    /// Spawns the pipeline tasks and returns the input sender, output receiver, and task handles.
    ///
    /// This allows for fully async integration where the caller drives the input and consumes the output.
    pub fn spawn(self) -> SpawnedPipeline<T> {
        let mut tasks: Vec<JoinHandle<Result<(), PipelineError>>> = Vec::new();

        // Channel for the initial input to the first processor
        let (first_tx, mut current_rx) =
            mpsc::channel::<Result<T, PipelineError>>(self.channel_size);

        // Iterate through processors and chain them
        for mut processor in self.processors {
            let (next_tx, next_rx) = mpsc::channel::<Result<T, PipelineError>>(self.channel_size);
            let context = self.context.clone();
            let processor_name = processor.name();

            // Spawn processor task
            // We use spawn_blocking because processors are synchronous
            let task = tokio::task::spawn_blocking(move || {
                let mut input_rx = current_rx;
                let tx = next_tx;
                let mut processed_items: usize = 0;
                let mut emitted_items: usize = 0;
                let mut next_progress_log_at: usize = 10_000;

                // Process items
                while let Some(item_result) = input_rx.blocking_recv() {
                    match item_result {
                        Ok(item) => {
                            let mut output_fn = |processed_item: T| {
                                if tx.blocking_send(Ok(processed_item)).is_err() {
                                    return Err(PipelineError::ChannelClosed("downstream"));
                                }
                                emitted_items = emitted_items.saturating_add(1);
                                Ok(())
                            };

                            if let Err(e) = processor.process(&context, item, &mut output_fn) {
                                error!(processor = processor_name, error = ?e, "Processor failed");
                                let msg = e.to_string();
                                let _ = tx.blocking_send(Err(stage_process_error(
                                    processor_name,
                                    std::io::Error::other(msg.clone()),
                                )));
                                return Err(stage_process_error(
                                    processor_name,
                                    std::io::Error::other(msg),
                                ));
                            }
                            processed_items = processed_items.saturating_add(1);
                            if processed_items >= next_progress_log_at {
                                debug!(
                                    processor = processor_name,
                                    processed_items, emitted_items, "Stage progress"
                                );
                                next_progress_log_at = next_progress_log_at.saturating_add(10_000);
                            }
                        }
                        Err(e) => {
                            // Forward errors
                            if tx.blocking_send(Err(e)).is_err() {
                                break;
                            }
                        }
                    }
                }

                // Finalize processor
                let mut output_fn = |processed_item: T| {
                    if tx.blocking_send(Ok(processed_item)).is_err() {
                        return Err(PipelineError::ChannelClosed("downstream during finish"));
                    }
                    Ok(())
                };

                if let Err(e) = processor.finish(&context, &mut output_fn) {
                    error!(processor = processor_name, error = ?e, "Processor finish failed");
                    let msg = e.to_string();
                    let _ = tx.blocking_send(Err(stage_finish_error(
                        processor_name,
                        std::io::Error::other(msg.clone()),
                    )));
                    return Err(stage_finish_error(
                        processor_name,
                        std::io::Error::other(msg),
                    ));
                }

                Ok(())
            });

            tasks.push(task);
            current_rx = next_rx;
        }

        SpawnedPipeline {
            input_tx: first_tx,
            output_rx: current_rx,
            tasks,
        }
    }

    /// Runs the pipeline, spawning tasks for each processor.
    ///
    /// Takes an iterator of input data and a function to handle output data.
    /// Returns an error if any processor fails.
    pub fn run<I, O, E>(self, input: I, output: &mut O) -> Result<(), PipelineError>
    where
        I: Iterator<Item = Result<T, E>> + Send + 'static,
        O: FnMut(Result<T, E>),
        E: Into<PipelineError> + From<PipelineError> + Send + 'static,
    {
        // If no processors, just pass through
        if self.processors.is_empty() {
            for item in input {
                output(item);
            }
            return Ok(());
        }

        let context = self.context.clone();
        let SpawnedPipeline {
            input_tx,
            output_rx,
            tasks,
        } = self.spawn();
        let mut output_rx = output_rx;
        let mut tasks = tasks;

        // Spawn input task to bridge iterator to channel
        let input_task = tokio::spawn(async move {
            for item in input {
                if context.token.is_cancelled() {
                    return Err(PipelineError::Cancelled);
                }
                if input_tx.send(item.map_err(|e| e.into())).await.is_err() {
                    // Downstream closed
                    break;
                }
            }
            Ok::<(), PipelineError>(())
        });
        tasks.push(input_task);

        // Pump output
        while let Some(item) = output_rx.blocking_recv() {
            match item {
                Ok(t) => output(Ok(t)),
                Err(e) => return Err(e),
            }
        }

        // We don't strictly wait for tasks here in the sync wrapper,
        // but in a real async usage we would.
        // The tasks will finish naturally.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::CancellationToken;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestProcessor {
        _name: String,
        processed_count: Arc<AtomicUsize>,
    }

    impl TestProcessor {
        fn new(name: &str, counter: Arc<AtomicUsize>) -> Self {
            Self {
                _name: name.to_string(),
                processed_count: counter,
            }
        }
    }

    impl Processor<String> for TestProcessor {
        fn name(&self) -> &'static str {
            "TestProcessor"
        }

        fn process(
            &mut self,
            _context: &Arc<StreamerContext>,
            input: String,
            output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            self.processed_count.fetch_add(1, Ordering::SeqCst);
            output(format!("{}-processed", input))
        }

        fn finish(
            &mut self,
            _context: &Arc<StreamerContext>,
            _output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_channel_pipeline_flow() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);
        let counter = Arc::new(AtomicUsize::new(0));

        let pipeline = ChannelPipeline::new(context.clone())
            .add_processor(TestProcessor::new("p1", counter.clone()));

        let input = vec![
            Ok("item1".to_string()),
            Ok("item2".to_string()),
            Ok("item3".to_string()),
        ];

        // We need to run this in a blocking task because pipeline.run is blocking (intended for spawn_blocking)
        // But here we are in async test.
        // So we wrap it.
        let pipeline_task = tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            let mut output_fn = |res: Result<String, PipelineError>| {
                results.push(res.unwrap());
            };
            pipeline.run(input.into_iter(), &mut output_fn).unwrap();
            results
        });

        let final_results = pipeline_task.await.unwrap();

        assert_eq!(final_results.len(), 3);
        assert_eq!(final_results[0], "item1-processed");
        assert_eq!(final_results[1], "item2-processed");
        assert_eq!(final_results[2], "item3-processed");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }
    struct FailingProcessor;

    impl Processor<String> for FailingProcessor {
        fn name(&self) -> &'static str {
            "FailingProcessor"
        }

        fn process(
            &mut self,
            _context: &Arc<StreamerContext>,
            _input: String,
            _output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            Err(PipelineError::Strategy(Box::new(std::io::Error::other(
                "Intentional failure",
            ))))
        }

        fn finish(
            &mut self,
            _context: &Arc<StreamerContext>,
            _output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_channel_pipeline_error_propagation() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);

        let pipeline = ChannelPipeline::new(context.clone()).add_processor(FailingProcessor);

        let input = vec![Ok("item1".to_string())];

        let pipeline_task = tokio::task::spawn_blocking(move || {
            let mut output_fn = |_res: Result<String, PipelineError>| {};
            pipeline.run(input.into_iter(), &mut output_fn)
        });

        let result = pipeline_task.await.unwrap();
        assert!(result.is_err());
        match result {
            Err(PipelineError::StageProcess { stage, source }) => {
                assert_eq!(stage, "FailingProcessor");
                assert_eq!(source.to_string(), "Intentional failure");
            }
            _ => panic!("Expected StageProcess error, got {:?}", result),
        }
    }

    struct FinishFailingProcessor;

    impl Processor<String> for FinishFailingProcessor {
        fn name(&self) -> &'static str {
            "FinishFailingProcessor"
        }

        fn process(
            &mut self,
            _context: &Arc<StreamerContext>,
            input: String,
            output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            output(input)
        }

        fn finish(
            &mut self,
            _context: &Arc<StreamerContext>,
            _output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            Err(PipelineError::Strategy(Box::new(std::io::Error::other(
                "Intentional finish failure",
            ))))
        }
    }

    #[tokio::test]
    async fn test_channel_pipeline_finish_error_propagation() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);

        let pipeline = ChannelPipeline::new(context).add_processor(FinishFailingProcessor);
        let input = vec![Ok("item1".to_string())];

        let pipeline_task = tokio::task::spawn_blocking(move || {
            let mut output_fn = |_res: Result<String, PipelineError>| {};
            pipeline.run(input.into_iter(), &mut output_fn)
        });

        let result = pipeline_task.await.unwrap();
        assert!(result.is_err());
        match result {
            Err(PipelineError::StageFinish { stage, source }) => {
                assert_eq!(stage, "FinishFailingProcessor");
                assert_eq!(source.to_string(), "Intentional finish failure");
            }
            _ => panic!("Expected StageFinish error, got {:?}", result),
        }
    }
}
