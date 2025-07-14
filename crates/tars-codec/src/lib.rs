pub mod codec;
pub mod de;
pub mod error;
pub mod pool;
pub mod ser;
pub mod simd;
pub mod types;

pub use crate::{
    codec::TarsCodec,
    error::TarsError,
    pool::{PooledByteBuffer, PooledDeserializer, PooledSerializer, TarsCodecPool},
    simd::{bulk_ops, utf8_simd},
    types::{TarsMessage, TarsRequestHeader, TarsValue, ValidatedBytes},
};
use bytes::{Bytes, BytesMut};
use tokio_util::codec::Decoder;

/// Estimate the serialized size of a TarsMessage for optimal memory allocation
pub fn estimate_message_size(message: &TarsMessage) -> usize {
    let mut size = 4; // Length prefix

    // Header fields
    size += 3; // version (i16) 
    size += 2; // packet_type (u8)
    size += 5; // message_type (i32)
    size += 5; // request_id (i32)
    size += 2 + message.header.servant_name.len().min(255); // servant_name (estimate)
    size += 2 + message.header.func_name.len().min(255); // func_name (estimate)
    size += 5; // timeout (i32)

    // Context map
    size += 6; // map header
    for (k, v) in &message.header.context {
        size += 2 + k.len().min(255);
        size += 2 + v.len().min(255);
    }

    // Status map
    size += 6; // map header
    for (k, v) in &message.header.status {
        size += 2 + k.len().min(255);
        size += 2 + v.len().min(255);
    }

    // Body (estimate conservatively)
    size += 6; // body map header
    for (k, v) in &message.body {
        size += 2 + k.len().min(255);
        size += 6 + v.len(); // SimpleList overhead + content
    }

    size
}

/// Encode a TARS message with capacity hint for better performance
pub fn encode_request_with_capacity(
    message: &TarsMessage,
    estimated_size: usize,
) -> Result<BytesMut, TarsError> {
    let mut codec = TarsCodec;
    let mut dst = BytesMut::with_capacity(estimated_size);
    codec.encode_by_ref(message, &mut dst)?;
    Ok(dst)
}

/// Standard TARS request encoding (by reference)
pub fn encode_request(message: &TarsMessage) -> Result<BytesMut, TarsError> {
    let mut codec = TarsCodec;
    let mut dst = BytesMut::new();
    codec.encode_by_ref(message, &mut dst)?;
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
