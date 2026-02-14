mod aac;
pub mod audio;
pub mod av1;
pub mod avc;
pub mod data;
pub mod encode;
pub mod error;
// The previous `file` module contained an owned FLV file representation.
// After the refactor to a single `FlvTag` representation, that module became redundant.
pub mod framing;
pub mod header;
pub mod hevc;
pub mod parser;
pub mod parser_async;
pub mod resolution;
pub mod script;
pub mod tag;
pub mod video;
pub mod writer;
pub mod writer_async;

pub use data::FlvData;
pub use error::FlvError;
pub use header::FlvHeader;
pub use tag::{FlvTag, FlvTagType};
pub use writer::FlvWriter;
pub use writer_async::FlvEncoder;
