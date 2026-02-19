//! Pipeline processors for post-processing tasks.

mod ass_burnin;
mod audio_extract;
mod compression;
mod copy_move;
mod danmaku_factory;
mod delete;
mod execute;
mod metadata;
mod rclone;
mod remux;
mod tdl;
mod thumbnail;
mod traits;
pub mod utils;

pub use ass_burnin::{AssBurnInConfig, AssBurnInProcessor, AssMatchStrategy};
pub use audio_extract::AudioExtractProcessor;
pub use compression::CompressionProcessor;
pub use copy_move::{CopyMoveConfig, CopyMoveOperation, CopyMoveProcessor};
pub use danmaku_factory::{DanmakuFactoryConfig, DanmakuFactoryProcessor};
pub use delete::DeleteProcessor;
pub use execute::ExecuteCommandProcessor;
pub use metadata::MetadataProcessor;
pub use rclone::RcloneProcessor;
pub use remux::RemuxProcessor;
pub use tdl::TdlUploadProcessor;
pub use thumbnail::ThumbnailProcessor;
pub use traits::{
    JobLogSink, Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType,
};
