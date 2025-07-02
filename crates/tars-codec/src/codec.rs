use crate::{de::TarsDeserializer, error::TarsError, ser::TarsSerializer, types::TarsMessage};
use bytes::{BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

#[derive(Default)]
pub struct TarsCodec;

impl Encoder<TarsMessage> for TarsCodec {
    type Error = TarsError;

    fn encode(&mut self, item: TarsMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
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
        let total_len = (4 + encoded_bytes.len()) as u32;

        // Pre-allocate capacity to avoid reallocations
        dst.reserve(total_len as usize);
        dst.put_u32(total_len);
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
