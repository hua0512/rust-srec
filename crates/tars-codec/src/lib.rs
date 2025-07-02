pub mod codec;
pub mod de;
pub mod error;
pub mod ser;
pub mod types;

pub use crate::{
    codec::TarsCodec,
    error::TarsError,
    types::{TarsMessage, TarsRequestHeader, TarsValue},
};
use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

pub fn encode_request(message: TarsMessage) -> Result<BytesMut, TarsError> {
    let mut codec = TarsCodec;
    let mut dst = BytesMut::new();
    codec.encode(message, &mut dst)?;
    Ok(dst)
}

pub fn decode_response(src: &mut BytesMut) -> Result<Option<TarsMessage>, TarsError> {
    let mut codec = TarsCodec;
    codec.decode(src)
}
