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

    pub fn into_inner(self) -> BytesMut {
        self.buffer
    }

    pub fn into_bytes(self) -> Bytes {
        self.buffer.freeze()
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

    pub fn write_i8(&mut self, tag: u8, value: i8) -> Result<(), TarsError> {
        if value == 0 {
            self.write_head(tag, TarsType::Zero);
            return Ok(());
        }
        self.write_head(tag, TarsType::Int1);
        self.buffer.put_i8(value);
        Ok(())
    }

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
