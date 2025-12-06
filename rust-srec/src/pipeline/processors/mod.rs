//! Pipeline processors for post-processing tasks.

mod execute;
mod remux;
mod thumbnail;
mod traits;
mod upload;

pub use execute::ExecuteCommandProcessor;
pub use remux::RemuxProcessor;
pub use thumbnail::ThumbnailProcessor;
pub use traits::{Processor, ProcessorInput, ProcessorOutput, ProcessorType};
pub use upload::UploadProcessor;
