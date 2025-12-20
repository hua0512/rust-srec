//! Pipeline processors for post-processing tasks.

mod audio_extract;
mod compression;
mod copy_move;
mod delete;
mod execute;
mod metadata;
mod rclone;
mod remux;
mod thumbnail;
mod traits;
pub mod utils;

pub use audio_extract::AudioExtractProcessor;
pub use compression::CompressionProcessor;
pub use copy_move::{CopyMoveConfig, CopyMoveOperation, CopyMoveProcessor};
pub use delete::DeleteProcessor;
pub use execute::ExecuteCommandProcessor;
pub use metadata::MetadataProcessor;
pub use rclone::RcloneProcessor;
pub use remux::RemuxProcessor;
pub use thumbnail::ThumbnailProcessor;
pub use traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
