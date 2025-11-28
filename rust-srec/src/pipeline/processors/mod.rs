//! Pipeline processors for post-processing tasks.

mod traits;
mod remux;
mod upload;
mod execute;
mod thumbnail;

pub use traits::{Processor, ProcessorInput, ProcessorOutput, ProcessorType};
pub use remux::RemuxProcessor;
pub use upload::UploadProcessor;
pub use execute::ExecuteCommandProcessor;
pub use thumbnail::ThumbnailProcessor;
