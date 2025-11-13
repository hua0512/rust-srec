//! # Generic Pipeline Implementation
//!
//! This module provides a generic pipeline implementation that chains together
//! processors to form a complete data processing workflow.
//!
//! ## Usage
//!
//! Create a new `Pipeline<T>` and add processors that implement the `Processor<T>`
//! trait. Then process a stream of data through the pipeline.
//!

use crate::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use tracing_indicatif::span_ext::IndicatifSpanExt;

/// Default capacity hint for intermediate vectors
const DEFAULT_STAGE_CAPACITY: usize = 8;

/// A generic pipeline for processing data through a series of processors.
///
/// The pipeline coordinates a sequence of processors, with each processor
/// receiving outputs from the previous one in the chain.
pub struct Pipeline<T> {
    processors: Vec<Box<dyn Processor<T>>>,
    context: Arc<StreamerContext>,
}

impl<T> Pipeline<T> {
    /// Create a new empty pipeline with the given processing context.
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            processors: Vec::new(),
            context,
        }
    }

    /// Add a processor to the end of the pipeline.
    ///
    /// Returns self for method chaining.
    pub fn add_processor<P: Processor<T> + 'static>(mut self, processor: P) -> Self {
        self.processors.push(Box::new(processor));
        self
    }

    /// Runs the pipeline, processing all input and then finalizing the processors.
    ///
    /// Takes an iterator of input data and a function to handle output data.
    /// Returns an error if any processor in the pipeline fails.
    /// On cancellation, it stops processing new items but still finalizes all processors.
    ///
    /// # Optimizations
    ///
    /// - Reuses Vec allocations across stages via `clear()` and `swap()`
    /// - Pre-allocates with capacity hints
    /// - Early exits when stages produce no output
    pub fn run<I, O, E>(mut self, input: I, output: &mut O) -> Result<(), PipelineError>
    where
        I: Iterator<Item = Result<T, E>>,
        O: FnMut(Result<T, E>),
        E: Into<PipelineError> + From<PipelineError>,
    {
        let processing_result = self.process_items(input, output);

        // Finalize processing for all processors in the chain. This runs even if processing was
        // cancelled or errored.
        let finalization_result = self.finalize_processors(output);

        // Prioritize returning the finalization error if one occurred.
        finalization_result?;

        // Otherwise, return the result of the main processing.
        processing_result
    }

    /// Process input items through the pipeline
    fn process_items<I, O, E>(&mut self, input: I, output: &mut O) -> Result<(), PipelineError>
    where
        I: Iterator<Item = Result<T, E>>,
        O: FnMut(Result<T, E>),
        E: Into<PipelineError> + From<PipelineError>,
    {
        // Pre-allocate vectors for stage processing - these will be reused
        let mut current_stage_items: Vec<T> = Vec::with_capacity(DEFAULT_STAGE_CAPACITY);
        let mut next_stage_items: Vec<T> = Vec::with_capacity(DEFAULT_STAGE_CAPACITY);

        let mut item_index: usize = 0;

        for item_result in input {
            // Check for cancellation before processing each input item
            if self.context.token.is_cancelled() {
                return Err(PipelineError::Cancelled);
            }

            match item_result {
                Ok(initial_data) => {
                    // Reset and prepare for new item
                    current_stage_items.clear();
                    current_stage_items.push(initial_data);

                    // Process through each stage
                    for processor_index in 0..self.processors.len() {
                        next_stage_items.clear(); // Reuse allocation
                        let processor = &mut self.processors[processor_index];

                        // Process all items in current stage
                        for item_to_process in current_stage_items.drain(..) {
                            let mut processor_output_handler = |processed_item: T| {
                                next_stage_items.push(processed_item);
                                Ok(())
                            };

                            if let Err(e) = processor.process(
                                &self.context,
                                item_to_process,
                                &mut processor_output_handler,
                            ) {
                                // Enhanced error context
                                tracing::error!(
                                    processor = processor.name(),
                                    processor_index,
                                    item_index,
                                    "Processor failed during processing"
                                );
                                return Err(e);
                            }
                        }

                        // Swap vectors for next iteration (avoids reallocation)
                        std::mem::swap(&mut current_stage_items, &mut next_stage_items);

                        // Early exit if stage produced no output
                        if current_stage_items.is_empty() {
                            break;
                        }
                    }

                    // Emit final outputs
                    for final_item in current_stage_items.drain(..) {
                        output(Ok(final_item));
                    }
                }
                Err(e) => {
                    // Pass through errors from input
                    output(Err(e));
                }
            }

            item_index += 1;

            // Update progress on current span
            let span = tracing::Span::current();
            span.pb_set_position(item_index as u64);
            span.pb_set_message(&format!("Processing item {}", item_index));
        }

        Ok(())
    }

    /// Finalize all processors and route flushed data through remaining stages
    fn finalize_processors<O, E>(&mut self, output: &mut O) -> Result<(), PipelineError>
    where
        O: FnMut(Result<T, E>),
        E: From<PipelineError>,
    {
        // Pre-allocate vectors for finalization - these will be reused
        let mut items_flushed_by_current: Vec<T> = Vec::with_capacity(DEFAULT_STAGE_CAPACITY);
        let mut items_for_subsequent: Vec<T> = Vec::with_capacity(DEFAULT_STAGE_CAPACITY);
        let mut next_stage_items: Vec<T> = Vec::with_capacity(DEFAULT_STAGE_CAPACITY);
        let mut final_flushed_outputs: Vec<T> = Vec::with_capacity(DEFAULT_STAGE_CAPACITY);

        for i in 0..self.processors.len() {
            // Split processors into current and subsequent
            let (current_processor_slice, subsequent_processors_slice) =
                self.processors.split_at_mut(i + 1);
            let current_processor = &mut current_processor_slice[i];

            // Flush current processor
            items_flushed_by_current.clear();
            let mut current_finish_handler = |flushed_item: T| {
                items_flushed_by_current.push(flushed_item);
                Ok(())
            };

            if let Err(e) = current_processor.finish(&self.context, &mut current_finish_handler) {
                // Enhanced error context for finalization
                tracing::error!(
                    processor = current_processor.name(),
                    processor_index = i,
                    "Processor failed during finalization"
                );
                return Err(e);
            }

            // Move flushed items to processing vector
            items_for_subsequent.clear();
            items_for_subsequent.append(&mut items_flushed_by_current);

            // Process flushed items through all remaining processors
            for (subsequent_index, subsequent_processor) in
                subsequent_processors_slice.iter_mut().enumerate()
            {
                next_stage_items.clear();

                for item_to_process in items_for_subsequent.drain(..) {
                    let mut subsequent_process_handler = |processed_item: T| {
                        next_stage_items.push(processed_item);
                        Ok(())
                    };

                    if let Err(e) = subsequent_processor.process(
                        &self.context,
                        item_to_process,
                        &mut subsequent_process_handler,
                    ) {
                        tracing::error!(
                            processor = subsequent_processor.name(),
                            processor_index = i + 1 + subsequent_index,
                            "Processor failed during finalization cascade"
                        );
                        return Err(e);
                    }
                }

                // Swap for next iteration
                std::mem::swap(&mut items_for_subsequent, &mut next_stage_items);

                // Early exit if no items left
                if items_for_subsequent.is_empty() {
                    break;
                }
            }

            // Collect final outputs from this processor's finalization
            final_flushed_outputs.append(&mut items_for_subsequent);
        }

        // Emit all finalized outputs
        for final_item in final_flushed_outputs {
            output(Ok(final_item));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::CancellationToken;

    // Simple processor that increments a counter
    struct IncrementProcessor;

    impl Processor<u32> for IncrementProcessor {
        fn process(
            &mut self,
            _context: &Arc<StreamerContext>,
            input: u32,
            output: &mut dyn FnMut(u32) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            output(input + 1)
        }

        fn finish(
            &mut self,
            _context: &Arc<StreamerContext>,
            _output: &mut dyn FnMut(u32) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            Ok(())
        }

        fn name(&self) -> &'static str {
            "IncrementProcessor"
        }
    }

    // Processor that duplicates input (fan-out of 2)
    struct DuplicateProcessor;

    impl Processor<u32> for DuplicateProcessor {
        fn process(
            &mut self,
            _context: &Arc<StreamerContext>,
            input: u32,
            output: &mut dyn FnMut(u32) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            output(input)?;
            output(input)
        }

        fn finish(
            &mut self,
            _context: &Arc<StreamerContext>,
            _output: &mut dyn FnMut(u32) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            Ok(())
        }

        fn name(&self) -> &'static str {
            "DuplicateProcessor"
        }
    }

    // Processor that buffers items and flushes on finish
    struct BufferingProcessor {
        buffer: Vec<u32>,
    }

    impl BufferingProcessor {
        fn new() -> Self {
            Self { buffer: Vec::new() }
        }
    }

    impl Processor<u32> for BufferingProcessor {
        fn process(
            &mut self,
            _context: &Arc<StreamerContext>,
            input: u32,
            _output: &mut dyn FnMut(u32) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            self.buffer.push(input);
            Ok(())
        }

        fn finish(
            &mut self,
            _context: &Arc<StreamerContext>,
            output: &mut dyn FnMut(u32) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            for item in self.buffer.drain(..) {
                output(item)?;
            }
            Ok(())
        }

        fn name(&self) -> &'static str {
            "BufferingProcessor"
        }
    }

    #[test]
    fn test_simple_pipeline() {
        let context = Arc::new(StreamerContext::new(CancellationToken::new()));
        let pipeline = Pipeline::new(context).add_processor(IncrementProcessor);

        let input = vec![Ok(1), Ok(2), Ok(3)];
        let mut results = Vec::new();
        let mut output = |res: Result<u32, PipelineError>| {
            results.push(res.unwrap());
        };

        pipeline.run(input.into_iter(), &mut output).unwrap();

        assert_eq!(results, vec![2, 3, 4]);
    }

    #[test]
    fn test_chained_processors() {
        let context = Arc::new(StreamerContext::new(CancellationToken::new()));
        let pipeline = Pipeline::new(context)
            .add_processor(IncrementProcessor)
            .add_processor(IncrementProcessor)
            .add_processor(IncrementProcessor);

        let input = vec![Ok(0)];
        let mut results = Vec::new();
        let mut output = |res: Result<u32, PipelineError>| {
            results.push(res.unwrap());
        };

        pipeline.run(input.into_iter(), &mut output).unwrap();

        assert_eq!(results, vec![3]);
    }

    #[test]
    fn test_fan_out() {
        let context = Arc::new(StreamerContext::new(CancellationToken::new()));
        let pipeline = Pipeline::new(context)
            .add_processor(DuplicateProcessor)
            .add_processor(IncrementProcessor);

        let input = vec![Ok(1), Ok(2)];
        let mut results = Vec::new();
        let mut output = |res: Result<u32, PipelineError>| {
            results.push(res.unwrap());
        };

        pipeline.run(input.into_iter(), &mut output).unwrap();

        // Each input duplicated then incremented: 1→[1,1]→[2,2], 2→[2,2]→[3,3]
        assert_eq!(results, vec![2, 2, 3, 3]);
    }

    #[test]
    fn test_finalization_with_buffering() {
        let context = Arc::new(StreamerContext::new(CancellationToken::new()));
        let pipeline = Pipeline::new(context)
            .add_processor(BufferingProcessor::new())
            .add_processor(IncrementProcessor);

        let input = vec![Ok(1), Ok(2), Ok(3)];
        let mut results = Vec::new();
        let mut output = |res: Result<u32, PipelineError>| {
            results.push(res.unwrap());
        };

        pipeline.run(input.into_iter(), &mut output).unwrap();

        // All items buffered, then flushed and incremented
        assert_eq!(results, vec![2, 3, 4]);
    }

    #[test]
    fn test_cancellation() {
        let token = CancellationToken::new();
        let context = Arc::new(StreamerContext::new(token.clone()));

        // Cancel immediately
        token.cancel();

        let pipeline = Pipeline::new(context).add_processor(IncrementProcessor);

        let input = vec![Ok(1), Ok(2), Ok(3)];
        let mut results = Vec::new();
        let mut output = |res: Result<u32, PipelineError>| {
            results.push(res.unwrap());
        };

        let result = pipeline.run(input.into_iter(), &mut output);

        assert!(matches!(result, Err(PipelineError::Cancelled)));
    }

    #[test]
    fn test_empty_pipeline() {
        let context = Arc::new(StreamerContext::new(CancellationToken::new()));
        let pipeline = Pipeline::<u32>::new(context);

        let input = vec![Ok(1), Ok(2), Ok(3)];
        let mut results = Vec::new();
        let mut output = |res: Result<u32, PipelineError>| {
            results.push(res.unwrap());
        };

        pipeline.run(input.into_iter(), &mut output).unwrap();

        // Empty pipeline passes through unchanged
        assert_eq!(results, vec![1, 2, 3]);
    }
}
