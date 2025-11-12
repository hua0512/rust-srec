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

/// A generic pipeline for processing data through a series of processors.
///
/// The pipeline coordinates a sequence of processors, with each processor
/// receiving outputs from the previous one in the chain.
pub struct Pipeline<T> {
    processors: Vec<Box<dyn Processor<T>>>,
    #[allow(dead_code)]
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
    pub fn run<I, O, E>(mut self, input: I, output: &mut O) -> Result<(), PipelineError>
    where
        I: Iterator<Item = Result<T, E>>,
        O: FnMut(Result<T, E>),
        E: Into<PipelineError> + From<PipelineError>,
    {
        let processing_result = {
            let mut result: Result<(), PipelineError> = Ok(());
            'processing_loop: for item_result in input {
                if self.context.token.is_cancelled() {
                    result = Err(PipelineError::Cancelled);
                    break 'processing_loop;
                }

                match item_result {
                    Ok(initial_data) => {
                        let mut current_stage_items: Vec<T> = vec![initial_data];

                        for processor_index in 0..self.processors.len() {
                            let mut next_stage_items: Vec<T> = Vec::new();
                            let processor = &mut self.processors[processor_index];

                            for item_to_process in current_stage_items {
                                let mut processor_output_handler = |processed_item: T| {
                                    next_stage_items.push(processed_item);
                                    Ok(())
                                };

                                if let Err(e) = processor.process(
                                    &self.context,
                                    item_to_process,
                                    &mut processor_output_handler,
                                ) {
                                    result = Err(e);
                                    break 'processing_loop;
                                }
                            }
                            current_stage_items = next_stage_items;

                            if current_stage_items.is_empty() {
                                break;
                            }
                        }

                        for final_item in current_stage_items {
                            output(Ok(final_item));
                        }
                    }
                    Err(e) => {
                        output(Err(e));
                    }
                }
            }
            result
        };

        // Finalize processing for all processors in the chain. This runs even if processing was
        // cancelled or errored.
        let finalization_result = (|| -> Result<(), PipelineError> {
            let mut final_flushed_outputs: Vec<T> = Vec::new();

            for i in 0..self.processors.len() {
                let (current_processor_slice, subsequent_processors_slice) =
                    self.processors.split_at_mut(i + 1);
                let current_processor = &mut current_processor_slice[i];

                let mut items_flushed_by_current: Vec<T> = Vec::new();
                let mut current_finish_handler = |flushed_item: T| {
                    items_flushed_by_current.push(flushed_item);
                    Ok(())
                };
                current_processor.finish(&self.context, &mut current_finish_handler)?;

                let mut items_for_subsequent_processing = items_flushed_by_current;

                for subsequent_processor in subsequent_processors_slice {
                    let mut next_stage_flushed_items: Vec<T> = Vec::new();

                    for item_to_process in items_for_subsequent_processing {
                        let mut subsequent_process_handler = |processed_item: T| {
                            next_stage_flushed_items.push(processed_item);
                            Ok(())
                        };
                        subsequent_processor.process(
                            &self.context,
                            item_to_process,
                            &mut subsequent_process_handler,
                        )?;
                    }
                    items_for_subsequent_processing = next_stage_flushed_items;

                    if items_for_subsequent_processing.is_empty() {
                        break;
                    }
                }

                final_flushed_outputs.extend(items_for_subsequent_processing);
            }

            for final_item in final_flushed_outputs {
                output(Ok(final_item));
            }

            Ok(())
        })();

        // Prioritize returning the finalization error if one occurred.
        finalization_result?;

        // Otherwise, return the result of the main processing.
        processing_result
    }

    // Recursive implementation
    // /// Process all input through the pipeline.
    // ///
    // /// Takes an iterator of input data and a function to handle output data.
    // /// Returns an error if any processor in the pipeline fails.
    // pub fn process<I, O, E>(mut self, input: I, output: &mut O) -> Result<(), PipelineError>
    // where
    //     I: Iterator<Item = Result<T, E>>,
    //     O: FnMut(Result<T, E>),
    //     E: Into<PipelineError> + From<PipelineError>,
    // {
    //     // Recursive processing function that passes data through the pipeline
    //     fn process_inner<T>(
    //         processors: &mut [Box<dyn Processor<T>>],
    //         data: T,
    //         output: &mut dyn FnMut(T) -> Result<(), PipelineError>,
    //     ) -> Result<(), PipelineError> {
    //         if let Some((first, rest)) = processors.split_first_mut() {
    //             let mut intermediate_output = |data| process_inner(rest, data, output);
    //             first.process(data, &mut intermediate_output)
    //         } else {
    //             output(data)
    //         }
    //     }

    //     // Process the input stream
    //     for item in input {
    //         match item {
    //             Ok(data) => {
    //                 // Create the internal output function inside the loop to avoid capturing issues
    //                 let mut internal_output = |data: T| {
    //                     output(Ok(data));
    //                     Ok(())
    //                 };
    //                 process_inner(&mut self.processors, data, &mut internal_output)?;
    //             }
    //             Err(e) => {
    //                 output(Err(e));
    //             }
    //         }
    //     }

    //     // Finalize processing for all processors in the chain
    //     let mut processors = &mut self.processors[..];
    //     while !processors.is_empty() {
    //         let (current, rest) = processors.split_first_mut().unwrap();
    //         let mut internal_output = |data: T| {
    //             output(Ok(data));
    //             Ok(())
    //         };
    //         let mut output_fn = |data: T| process_inner(rest, data, &mut internal_output);
    //         current.finish(&mut output_fn)?;
    //         processors = rest;
    //     }
    //     Ok(())
    // }
}
