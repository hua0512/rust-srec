use crate::{
    error::TarsError,
    types::{TarsType, TarsValue},
};
use bytes::{BufMut, Bytes, BytesMut};

pub struct TarsSerializer {
    buffer: BytesMut,
}

impl Default for TarsSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl TarsSerializer {
    pub fn new() -> Self {
        Self {
            buffer: BytesMut::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: BytesMut::with_capacity(capacity),
        }
    }

    /// Estimate the size needed to serialize a TarsValue for capacity planning
    pub fn estimate_size(value: &TarsValue) -> usize {
        match value {
            TarsValue::Bool(_) => 2,   // head + 1 byte
            TarsValue::Byte(_) => 2,   // head + 1 byte
            TarsValue::Short(_) => 3,  // head + 2 bytes (worst case)
            TarsValue::Int(_) => 5,    // head + 4 bytes (worst case)
            TarsValue::Long(_) => 9,   // head + 8 bytes (worst case)
            TarsValue::Float(_) => 5,  // head + 4 bytes
            TarsValue::Double(_) => 9, // head + 8 bytes
            TarsValue::String(s) => {
                let len = s.len();
                if len <= 255 {
                    2 + len // head + 1-byte length + content
                } else {
                    5 + len // head + 4-byte length + content
                }
            }
            TarsValue::StringRef(bytes) => {
                let len = bytes.len();
                if len <= 255 {
                    2 + len // head + 1-byte length + content
                } else {
                    5 + len // head + 4-byte length + content
                }
            }
            TarsValue::Struct(map) => {
                let mut size = 2; // struct begin + end
                for v in map.values() {
                    size += Self::estimate_size(v);
                }
                size
            }
            TarsValue::Map(map) => {
                let mut size = 6; // head + length encoding (worst case)
                for (k, v) in map {
                    size += Self::estimate_size(k) + Self::estimate_size(v);
                }
                size
            }
            TarsValue::List(list) => {
                let mut size = 6; // head + length encoding (worst case)
                for item in list {
                    size += Self::estimate_size(item);
                }
                size
            }
            TarsValue::SimpleList(bytes) => 6 + bytes.len(), // head + length + content
            TarsValue::Binary(bytes) => 6 + bytes.len(),     // head + length + content
            TarsValue::StructBegin | TarsValue::StructEnd => 1,
        }
    }

    pub fn into_inner(self) -> BytesMut {
        self.buffer
    }

    pub fn into_bytes(self) -> Bytes {
        self.buffer.freeze()
    }

    /// Reset the serializer for reuse (for object pooling)
    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    /// Get a reference to the internal buffer without cloning
    pub fn buffer(&self) -> &BytesMut {
        &self.buffer
    }

    /// Get the current length of the serialized data
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Encode a complete TarsMessage to the internal buffer (zero-copy)
    pub fn encode_message(&mut self, message: &crate::TarsMessage) -> Result<&BytesMut, TarsError> {
        self.buffer.clear();
        self.encode_message_to_internal_buffer(message)?;
        Ok(&self.buffer)
    }

    /// Encode a complete TarsMessage and return owned bytes (consumes message)
    pub fn encode_message_owned(
        &mut self,
        message: crate::TarsMessage,
    ) -> Result<BytesMut, TarsError> {
        self.buffer.clear();
        self.encode_message_to_internal_buffer(&message)?;
        Ok(self.buffer.clone()) // Only clone when explicitly requested
    }

    /// Encode a message directly into the provided buffer (most efficient)
    pub fn encode_message_to_buffer(
        &mut self,
        message: &crate::TarsMessage,
        buffer: &mut BytesMut,
    ) -> Result<(), TarsError> {
        // Encode directly to the target buffer
        let temp_buffer = std::mem::replace(&mut self.buffer, BytesMut::new());
        self.buffer = std::mem::replace(buffer, temp_buffer);

        self.encode_message_to_internal_buffer(message)?;

        // Swap back, keeping the encoded data in the target buffer
        let encoded_buffer =
            std::mem::replace(&mut self.buffer, std::mem::replace(buffer, BytesMut::new()));
        *buffer = encoded_buffer;

        Ok(())
    }

    /// Internal helper that encodes to whatever buffer is currently set as self.buffer
    fn encode_message_to_internal_buffer(
        &mut self,
        message: &crate::TarsMessage,
    ) -> Result<(), TarsError> {
        // Write the header fields
        self.write_i16(1, message.header.version)?;
        self.write_u8(2, message.header.packet_type)?;
        self.write_i32(3, message.header.message_type)?;
        self.write_i32(4, message.header.request_id)?;
        self.write_string(5, &message.header.servant_name)?;
        self.write_string(6, &message.header.func_name)?;
        self.write_i32(7, message.header.timeout)?;

        // Write context and status as maps if not empty
        if !message.header.context.is_empty() {
            self.write_map(8, &message.header.context)?;
        }
        if !message.header.status.is_empty() {
            self.write_map(9, &message.header.status)?;
        }

        // Write body data
        for (key, data) in &message.body {
            // For simplicity, write as binary data with string key as tag
            let tag = key.bytes().next().unwrap_or(10);
            self.buffer.put_u8(tag);
            self.buffer.put_u8(TarsType::SimpleList as u8);
            if data.len() <= 255 {
                self.buffer.put_u8(data.len() as u8);
            } else {
                self.buffer.put_u32(data.len() as u32);
            }
            self.buffer.extend_from_slice(data);
        }

        Ok(())
    }

    /// Legacy method - kept for backward compatibility but now delegates to owned version
    pub fn encode_message_legacy(
        &mut self,
        message: crate::TarsMessage,
    ) -> Result<BytesMut, TarsError> {
        self.encode_message_owned(message)
    }

    pub fn write_head(&mut self, tag: u8, type_id: TarsType) {
        if tag < 15 {
            let head = (tag << 4) | u8::from(type_id);
            self.buffer.put_u8(head);
        } else {
            let head = (15 << 4) | u8::from(type_id);
            self.buffer.put_u8(head);
            self.buffer.put_u8(tag);
        }
    }

    pub fn write_bool(&mut self, tag: u8, value: bool) -> Result<(), TarsError> {
        self.write_head(tag, TarsType::Zero);
        self.buffer.put_u8(if value { 1 } else { 0 });
        Ok(())
    }

    #[inline]
    pub fn write_i8(&mut self, tag: u8, value: i8) -> Result<(), TarsError> {
        if value == 0 {
            self.write_head(tag, TarsType::Zero);
            return Ok(());
        }
        self.write_head(tag, TarsType::Int1);
        self.buffer.put_i8(value);
        Ok(())
    }

    #[inline]
    pub fn write_u8(&mut self, tag: u8, value: u8) -> Result<(), TarsError> {
        self.write_i8(tag, value as i8)
    }

    pub fn write_i16(&mut self, tag: u8, value: i16) -> Result<(), TarsError> {
        if (-128..=127).contains(&value) {
            self.write_i8(tag, value as i8)?;
        } else {
            self.write_head(tag, TarsType::Int2);
            self.buffer.put_i16(value);
        }
        Ok(())
    }

    pub fn write_i32(&mut self, tag: u8, value: i32) -> Result<(), TarsError> {
        if (-32768..=32767).contains(&value) {
            self.write_i16(tag, value as i16)?;
        } else {
            self.write_head(tag, TarsType::Int4);
            self.buffer.put_i32(value);
        }
        Ok(())
    }

    pub fn write_i64(&mut self, tag: u8, value: i64) -> Result<(), TarsError> {
        if (-2147483648..=2147483647).contains(&value) {
            self.write_i32(tag, value as i32)?;
        } else {
            self.write_head(tag, TarsType::Int8);
            self.buffer.put_i64(value);
        }
        Ok(())
    }

    pub fn write_f32(&mut self, tag: u8, value: f32) -> Result<(), TarsError> {
        self.write_head(tag, TarsType::Float);
        self.buffer.put_f32(value);
        Ok(())
    }

    pub fn write_f64(&mut self, tag: u8, value: f64) -> Result<(), TarsError> {
        self.write_head(tag, TarsType::Double);
        self.buffer.put_f64(value);
        Ok(())
    }

    pub fn write_string(&mut self, tag: u8, value: &str) -> Result<(), TarsError> {
        let len = value.len();
        if len <= 255 {
            self.write_head(tag, TarsType::String1);
            self.buffer.put_u8(len as u8);
        } else {
            self.write_head(tag, TarsType::String4);
            self.buffer.put_u32(len as u32);
        }
        self.buffer.put_slice(value.as_bytes());
        Ok(())
    }

    pub fn write_struct(
        &mut self,
        tag: u8,
        value: &rustc_hash::FxHashMap<u8, TarsValue>,
    ) -> Result<(), TarsError> {
        self.write_head(tag, TarsType::StructBegin);
        for (tag, value) in value {
            self.write_value(*tag, value)?;
        }
        self.write_head(0, TarsType::StructEnd);
        Ok(())
    }

    pub fn write_map<K, V>(
        &mut self,
        tag: u8,
        value: &rustc_hash::FxHashMap<K, V>,
    ) -> Result<(), TarsError>
    where
        K: TarsSerializable,
        V: TarsSerializable,
    {
        self.write_head(tag, TarsType::Map);
        self.write_i32(0, value.len() as i32)?;
        for (k, v) in value {
            k.serialize(self, 0)?;
            v.serialize(self, 1)?;
        }
        Ok(())
    }

    pub fn write_list(&mut self, tag: u8, value: &[Box<TarsValue>]) -> Result<(), TarsError> {
        self.write_head(tag, TarsType::List);
        self.write_i32(0, value.len() as i32)?;
        for item in value {
            self.write_value(0, item.as_ref())?;
        }
        Ok(())
    }

    pub fn write_simple_list(&mut self, tag: u8, value: &[u8]) -> Result<(), TarsError> {
        self.write_head(tag, TarsType::SimpleList);
        self.write_head(0, TarsType::Int1);
        self.write_i32(0, value.len() as i32)?;
        self.buffer.put_slice(value);
        Ok(())
    }

    pub fn write_value(&mut self, tag: u8, value: &TarsValue) -> Result<(), TarsError> {
        match value {
            TarsValue::Bool(v) => self.write_bool(tag, *v),
            TarsValue::Byte(v) => self.write_i8(tag, *v as i8),
            TarsValue::Short(v) => self.write_i16(tag, *v),
            TarsValue::Int(v) => self.write_i32(tag, *v),
            TarsValue::Long(v) => self.write_i64(tag, *v),
            TarsValue::Float(v) => self.write_f32(tag, *v),
            TarsValue::Double(v) => self.write_f64(tag, *v),
            TarsValue::String(v) => self.write_string(tag, v),
            TarsValue::StringRef(bytes) => {
                // Convert bytes back to &str for serialization
                match std::str::from_utf8(bytes) {
                    Ok(s) => self.write_string(tag, s),
                    Err(e) => {
                        println!("Invalid UTF-8 sequence: {bytes:?}");
                        Err(TarsError::InvalidUtf8(e))
                    }
                }
            }
            TarsValue::Struct(v) => self.write_struct(tag, v),
            TarsValue::Map(v) => self.write_map(tag, v),
            TarsValue::List(v) => self.write_list(tag, v),
            TarsValue::SimpleList(v) => self.write_simple_list(tag, v),
            TarsValue::Binary(v) => self.write_simple_list(tag, v),
            &TarsValue::StructBegin => {
                self.write_head(tag, TarsType::StructBegin);
                Ok(())
            }
            &TarsValue::StructEnd => {
                self.write_head(tag, TarsType::StructEnd);
                Ok(())
            }
        }
    }
}

pub trait TarsSerializable {
    fn serialize(&self, serializer: &mut TarsSerializer, tag: u8) -> Result<(), TarsError>;
}

impl TarsSerializable for String {
    fn serialize(&self, serializer: &mut TarsSerializer, tag: u8) -> Result<(), TarsError> {
        serializer.write_string(tag, self)
    }
}

impl TarsSerializable for TarsValue {
    fn serialize(&self, serializer: &mut TarsSerializer, tag: u8) -> Result<(), TarsError> {
        serializer.write_value(tag, self)
    }
}

impl TarsSerializable for Vec<u8> {
    fn serialize(&self, serializer: &mut TarsSerializer, tag: u8) -> Result<(), TarsError> {
        serializer.write_simple_list(tag, self)
    }
}

impl TarsSerializable for Bytes {
    fn serialize(&self, serializer: &mut TarsSerializer, tag: u8) -> Result<(), TarsError> {
        serializer.write_simple_list(tag, self)
    }
}

impl TarsSerializable for &str {
    fn serialize(&self, serializer: &mut TarsSerializer, tag: u8) -> Result<(), TarsError> {
        serializer.write_string(tag, self)
    }
}

pub fn to_bytes_mut(value: &TarsValue) -> Result<Bytes, TarsError> {
    let mut serializer = TarsSerializer::new();
    serializer.write_value(0, value)?;
    Ok(serializer.into_bytes())
}
