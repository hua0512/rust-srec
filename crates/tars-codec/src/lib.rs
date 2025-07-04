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
use bytes::{Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

/// Encode a TARS message with capacity hint for better performance
pub fn encode_request_with_capacity(message: TarsMessage, estimated_size: usize) -> Result<BytesMut, TarsError> {
    let mut codec = TarsCodec;
    let mut dst = BytesMut::with_capacity(estimated_size);
    codec.encode(message, &mut dst)?;
    Ok(dst)
}

/// Standard TARS request encoding
pub fn encode_request(message: TarsMessage) -> Result<BytesMut, TarsError> {
    let mut codec = TarsCodec;
    let mut dst = BytesMut::new();
    codec.encode(message, &mut dst)?;
    Ok(dst)
}

/// Standard TARS response decoding
pub fn decode_response(src: &mut BytesMut) -> Result<Option<TarsMessage>, TarsError> {
    let mut codec = TarsCodec;
    codec.decode(src)
}

/// High-performance TARS response decoding from owned bytes
pub fn decode_response_from_bytes(bytes: Bytes) -> Result<TarsMessage, TarsError> {
    if bytes.len() < 4 {
        return Err(TarsError::Unknown);
    }
    
    let len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() != len {
        return Err(TarsError::Unknown);
    }
    
    let mut de = de::TarsDeserializer::new(bytes.slice(4..));
    de.read_message()
}

/// Zero-copy TARS response decoding - avoids string allocations where possible
pub fn decode_response_zero_copy(bytes: Bytes) -> Result<TarsMessage, TarsError> {
    if bytes.len() < 4 {
        return Err(TarsError::Unknown);
    }
    
    let len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() != len {
        return Err(TarsError::Unknown);
    }
    
    let mut de = de::TarsDeserializer::new_zero_copy(bytes.slice(4..));
    de.read_message()
}
