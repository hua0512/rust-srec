//! # Generic Processor Trait
//!
//! This module defines the generic `Processor<T>` trait which is the foundation
//! of the pipeline processing system. Each processor can transform data items
//! of type T and pass them to the next processor in the chain.
//!
//! ## Usage
//!
//! Implement the `Processor<T>` trait for your custom operators to add them
//! to a processing pipeline.
//!

use crate::PipelineError;

/// A generic processor trait for handling data of type T.
///
/// Processors form the building blocks of a pipeline. Each processor takes
/// an input item, processes it, and emits zero or more outputs through a
/// callback function.
pub trait Processor<T> {
    /// Process an input item and produce output.
    ///
    /// This method takes an input item of type T and a callback function
    /// that it can call to emit output items. The callback will typically
    /// pass the item to the next processor in the chain.
    ///
    /// # Arguments
    ///
    /// * `input` - The input data item to process
    /// * `output` - A mutable function that accepts processed items
    ///
    /// # Returns
    ///
    /// `Result<(), PipelineError>` - Success or an error if processing failed
    fn process(
        &mut self,
        input: T,
        output: &mut dyn FnMut(T) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError>;

    /// Called when processing is complete to flush any buffered data.
    ///
    /// This method is called at the end of the data stream to allow processors
    /// to emit any remaining items they might be buffering.
    ///
    /// # Arguments
    ///
    /// * `output` - A mutable function that accepts processed items
    ///
    /// # Returns
    ///
    /// `Result<(), PipelineError>` - Success or an error if finalization failed
    fn finish(
        &mut self,
        output: &mut dyn FnMut(T) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError>;

    /// Get the name of this processor for logging and debugging.
    fn name(&self) -> &'static str;
}

// /// Trait for automatically adapting types that implement a specific processor trait
// /// to the generic Processor trait.
// ///
// /// This provides a path for existing processor implementations to adopt the
// /// generic trait without changing their code.
// pub trait ProcessorAdapter<T, E>: Sized {
//     /// Convert from specific error type to the generic PipelineError
//     fn adapt_error(error: E) -> PipelineError;

//     /// Convert from a specific output callback signature to the generic one
//     fn adapt_output<'a, F>(output: &'a mut F) -> impl FnMut(T) -> Result<(), PipelineError> + 'a
//     where
//         F: FnMut(T) -> Result<(), E>;
// }
