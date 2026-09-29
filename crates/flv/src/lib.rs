//! FLV container framing and audio/video payload inspection.
//!
//! Wire formats follow [Adobe FLV 10.1, Annex E] and [Enhanced RTMP v2]
//! (v2-2026-01-31-r2). The bundled `enhanced-rtmp-v2.md` is an older reference;
//! use the upstream specification for current packet types and FourCC values.
//!
//! The synchronous and asynchronous writers both emit a back-pointer after
//! every tag, including the last tag. Enhanced payloads remain intact when
//! writing, including ModEx and multitrack wrappers. Payload inspection is
//! codec-specific and does not implement every Enhanced RTMP feature.
//!
//! Parsers cap extended headers at 64 KiB as a resource safeguard. This is an
//! implementation limit, not a restriction imposed by the FLV specification.
//!
//! [Adobe FLV 10.1, Annex E]: https://veovera.github.io/enhanced-rtmp/docs/legacy/video-file-format-v10-1-spec.pdf#page=74
//! [Enhanced RTMP v2]: https://github.com/veovera/enhanced-rtmp/blob/main/docs/enhanced/enhanced-rtmp-v2.md

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
pub use media_types::split_reason::{AudioCodecInfo, SplitReason, VideoCodecInfo};
pub use tag::{CodecKind, FlvTag, FlvTagType, TagClass};
pub use writer::FlvWriter;
pub use writer_async::FlvEncoder;
