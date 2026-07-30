pub mod codec;
pub mod de;
pub mod error;
pub mod ser;
pub mod types;

pub use crate::{
    codec::TarsCodec,
    error::TarsError,
    types::{TarsMessage, TarsRequestHeader, TarsValue, ValidatedBytes, next_request_id},
};
use bytes::{Bytes, BytesMut};
use tokio_util::codec::Decoder;

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

/// Decode a TarsValue from bytes
pub fn decode_tars_value(bytes: Bytes) -> Result<TarsValue, TarsError> {
    let mut deserializer = de::TarsDeserializer::new(bytes);
    let (_, value) = deserializer.read_value()?;
    Ok(value)
}

/// Decode a TarsValue::Struct from naked bytes (no outer StructBegin/StructEnd).
pub fn decode_tars_struct(bytes: Bytes) -> Result<TarsValue, TarsError> {
    let mut deserializer = de::TarsDeserializer::new(bytes);
    let fields = deserializer.read_struct_naked()?;
    Ok(TarsValue::Struct(fields))
}

/// Encode a TarsValue to bytes without using serde.
/// If it matches TarsValue::Struct, it will only encode its fields.
pub fn encode_tars_value(value: &TarsValue) -> Result<BytesMut, TarsError> {
    let mut serializer = ser::TarsSerializer::new();
    if let TarsValue::Struct(fields) = value {
        serializer.write_struct_fields(fields)?;
    } else {
        serializer.write_value(0, value)?;
    }
    Ok(serializer.into_inner())
}

/// Encode a TarsValue to bytes wrapped in StructBegin/StructEnd.
pub fn encode_tars_value_wrapped(value: &TarsValue) -> Result<BytesMut, TarsError> {
    let mut serializer = ser::TarsSerializer::new();
    serializer.write_value(0, value)?;
    Ok(serializer.into_inner())
}
