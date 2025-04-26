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

    /// Process all input through the pipeline.
    ///
    /// Takes an iterator of input data and a function to handle output data.
    /// Returns an error if any processor in the pipeline fails.
    pub fn process<I, O, E>(mut self, input: I, output: &mut O) -> Result<(), PipelineError>
    where
        I: Iterator<Item = Result<T, E>>,
        O: FnMut(Result<T, E>),
        E: Into<PipelineError> + From<PipelineError>,
    {
        // Recursive processing function that passes data through the pipeline
        fn process_inner<T>(
            processors: &mut [Box<dyn Processor<T>>],
            data: T,
            output: &mut dyn FnMut(T) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            if let Some((first, rest)) = processors.split_first_mut() {
                let mut intermediate_output = |data| process_inner(rest, data, output);
                first.process(data, &mut intermediate_output)
            } else {
                output(data)
            }
        }

        // Process the input stream
        for item in input {
            match item {
                Ok(data) => {
                    // Create the internal output function inside the loop to avoid capturing issues
                    let mut internal_output = |data: T| {
                        output(Ok(data));
                        Ok(())
                    };
                    process_inner(&mut self.processors, data, &mut internal_output)?;
                }
                Err(e) => {
                    output(Err(e));
                }
            }
        }

        // Finalize processing for all processors in the chain
        let mut processors = &mut self.processors[..];
        while !processors.is_empty() {
            let (current, rest) = processors.split_first_mut().unwrap();
            let mut internal_output = |data: T| {
                output(Ok(data));
                Ok(())
            };
            let mut output_fn = |data: T| process_inner(rest, data, &mut internal_output);
            current.finish(&mut output_fn)?;
            processors = rest;
        }
        Ok(())
    }
}
