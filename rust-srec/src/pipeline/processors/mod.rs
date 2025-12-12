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
pub use compression::{ArchiveFormat, CompressionConfig, CompressionProcessor};
pub use copy_move::{CopyMoveConfig, CopyMoveOperation, CopyMoveProcessor};
pub use delete::{DeleteConfig, DeleteProcessor};
pub use execute::ExecuteCommandProcessor;
pub use metadata::{MetadataConfig, MetadataProcessor};
pub use rclone::{RcloneOperation, RcloneProcessor};
pub use remux::RemuxProcessor;
pub use thumbnail::ThumbnailProcessor;
pub use traits::{Processor, ProcessorInput, ProcessorOutput, ProcessorType};
