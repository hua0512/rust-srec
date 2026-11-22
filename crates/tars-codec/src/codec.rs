use std::io;

use bytes::{BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::{de::TarsDeserializer, error::TarsError, ser::TarsSerializer, types::TarsMessage};

/// Maximum allowed TARS frame size (16 MiB), including the 4-byte length prefix.
pub const MAX_TARS_FRAME_SIZE: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub struct TarsCodec;

impl Encoder<TarsMessage> for TarsCodec {
    type Error = TarsError;

    fn encode(&mut self, item: TarsMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode_by_ref(&item, dst)
    }
}

impl TarsCodec {
    /// Encode a TarsMessage by reference (more efficient for reuse)
    pub fn encode_by_ref(
        &mut self,
        item: &TarsMessage,
        dst: &mut BytesMut,
    ) -> Result<(), TarsError> {
        let mut serializer = TarsSerializer::new();

        let mut body_serializer = TarsSerializer::new();
        body_serializer.write_map(0, &item.body)?;
        let body_bytes = body_serializer.into_bytes();

        serializer.write_i16(1, item.header.version)?;
        serializer.write_u8(2, item.header.packet_type)?;
        serializer.write_i32(3, item.header.message_type)?;
        serializer.write_i32(4, item.header.request_id)?;
        serializer.write_string(5, &item.header.servant_name)?;
        serializer.write_string(6, &item.header.func_name)?;
        serializer.write_simple_list(7, &body_bytes)?;
        serializer.write_i32(8, item.header.timeout)?;
        serializer.write_map(9, &item.header.context)?;
        serializer.write_map(10, &item.header.status)?;

        let encoded_bytes = serializer.into_bytes();
        let total_len = 4 + encoded_bytes.len();
        if total_len > MAX_TARS_FRAME_SIZE {
            return Err(TarsError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("TARS frame length {total_len} exceeds maximum {MAX_TARS_FRAME_SIZE}"),
            )));
        }

        // Pre-allocate capacity to avoid reallocations
        dst.reserve(total_len);
        // Fits in `u32`: bounded by `MAX_TARS_FRAME_SIZE` above.
        dst.put_u32(total_len as u32);
        dst.extend_from_slice(&encoded_bytes);

        Ok(())
    }
}

impl Decoder for TarsCodec {
    type Item = TarsMessage;
    type Error = TarsError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None);
        }

        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&src[..4]);
        let len = u32::from_be_bytes(len_bytes) as usize;

        if len < 4 {
            return Err(TarsError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid TARS frame length {len}: must be at least 4 bytes"),
            )));
        }
        if len > MAX_TARS_FRAME_SIZE {
            return Err(TarsError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("TARS frame length {len} exceeds maximum {MAX_TARS_FRAME_SIZE}"),
            )));
        }

        if src.len() < len {
            src.reserve(len - src.len());
            return Ok(None);
        }

        let data = src.split_to(len).freeze();
        let mut de = TarsDeserializer::new(data.slice(4..));
        let message = de.read_message()?;
        Ok(Some(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_rejects_short_or_oversized_frame_length_before_reserving_or_slicing() {
        let mut codec = TarsCodec;

        for invalid_len in [0u32, 1, 2, 3] {
            let mut src = BytesMut::from(&invalid_len.to_be_bytes()[..]);
            let err = codec.decode(&mut src).unwrap_err();
            assert!(
                matches!(err, TarsError::Io(ref io_err) if io_err.kind() == io::ErrorKind::InvalidData),
                "unexpected error for len {invalid_len}: {err:?}"
            );
        }

        for oversized_len in [(MAX_TARS_FRAME_SIZE as u32) + 1, u32::MAX] {
            let mut src = BytesMut::from(&oversized_len.to_be_bytes()[..]);
            let initial_cap = src.capacity();
            let err = codec.decode(&mut src).unwrap_err();
            assert!(
                matches!(err, TarsError::Io(ref io_err) if io_err.kind() == io::ErrorKind::InvalidData),
                "unexpected error for len {oversized_len}: {err:?}"
            );
            assert_eq!(src.capacity(), initial_cap);
        }
    }
}
