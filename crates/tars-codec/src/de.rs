use crate::{
    error::TarsError,
    types::{TarsRequestHeader, TarsType, TarsValue},
};
use bytes::{Buf, BytesMut};
use std::collections::{BTreeMap, HashMap};

pub struct TarsDeserializer<'a> {
    pub buffer: &'a [u8],
    pos: usize,
}

impl<'a> TarsDeserializer<'a> {
    pub fn new(buffer: &'a [u8]) -> Self {
        Self { buffer, pos: 0 }
    }

    pub fn read_head(&mut self) -> Result<(u8, TarsType), TarsError> {
        let head = self.buffer.get_u8();
        let type_id = (head & 0x0F)
            .try_into()
            .map_err(|()| TarsError::InvalidTypeId(head & 0x0F))?;
        let mut tag = (head & 0xF0) >> 4;
        if tag == 15 {
            tag = self.buffer.get_u8();
        }
        Ok((tag, type_id))
    }

    pub fn read_bool(&mut self) -> Result<bool, TarsError> {
        Ok(self.buffer.get_u8() != 0)
    }

    pub fn read_i8(&mut self) -> Result<i8, TarsError> {
        Ok(self.buffer.get_i8())
    }

    pub fn read_i16(&mut self) -> Result<i16, TarsError> {
        Ok(self.buffer.get_i16())
    }

    pub fn read_i32(&mut self) -> Result<i32, TarsError> {
        Ok(self.buffer.get_i32())
    }

    pub fn read_i64(&mut self) -> Result<i64, TarsError> {
        Ok(self.buffer.get_i64())
    }

    pub fn read_f32(&mut self) -> Result<f32, TarsError> {
        Ok(self.buffer.get_f32())
    }

    pub fn read_f64(&mut self) -> Result<f64, TarsError> {
        Ok(self.buffer.get_f64())
    }

    pub fn read_string(&mut self, len: usize) -> Result<String, TarsError> {
        let mut buf = vec![0; len];
        self.buffer.copy_to_slice(&mut buf);
        Ok(String::from_utf8(buf)?)
    }

    pub fn read_struct(&mut self) -> Result<BTreeMap<u8, TarsValue>, TarsError> {
        let mut map = BTreeMap::new();
        loop {
            let (tag, type_id) = self.read_head()?;
            if let TarsType::StructEnd = type_id {
                break;
            }
            let value = self.read_value_by_type(type_id)?;
            map.insert(tag, value);
        }
        Ok(map)
    }

    pub fn read_map(&mut self) -> Result<BTreeMap<TarsValue, TarsValue>, TarsError> {
        let mut map = BTreeMap::new();
        let len = self.read_i32()? as usize;
        for _ in 0..len {
            let key = self.read_value()?;
            let value = self.read_value()?;
            map.insert(key, value);
        }
        Ok(map)
    }

    pub fn read_list(&mut self) -> Result<Vec<TarsValue>, TarsError> {
        let len = self.read_i32()? as usize;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            list.push(self.read_value()?);
        }
        Ok(list)
    }

    pub fn read_simple_list(&mut self) -> Result<Vec<u8>, TarsError> {
        self.read_head()?;
        let len = self.read_i32()? as usize;
        let mut buf = vec![0; len];
        self.buffer.copy_to_slice(&mut buf);
        Ok(buf)
    }

    pub fn read_value(&mut self) -> Result<TarsValue, TarsError> {
        let (_tag, type_id) = self.read_head()?;
        self.read_value_by_type(type_id)
    }

    pub fn read_value_by_type(&mut self, type_id: TarsType) -> Result<TarsValue, TarsError> {
        match type_id {
            TarsType::Int1 => Ok(TarsValue::Byte(self.read_i8()? as u8)),
            TarsType::Int2 => Ok(TarsValue::Short(self.read_i16()?)),
            TarsType::Int4 => Ok(TarsValue::Int(self.read_i32()?)),
            TarsType::Int8 => Ok(TarsValue::Long(self.read_i64()?)),
            TarsType::Float => Ok(TarsValue::Float(self.read_f32()?)),
            TarsType::Double => Ok(TarsValue::Double(self.read_f64()?)),
            TarsType::String1 => {
                let len = self.buffer.get_u8() as usize;
                Ok(TarsValue::String(self.read_string(len)?))
            }
            TarsType::String4 => {
                let len = self.buffer.get_u32() as usize;
                Ok(TarsValue::String(self.read_string(len)?))
            }
            TarsType::Map => Ok(TarsValue::Map(self.read_map()?)),
            TarsType::List => Ok(TarsValue::List(self.read_list()?)),
            TarsType::StructBegin => Ok(TarsValue::Struct(self.read_struct()?)),
            TarsType::StructEnd => Err(TarsError::InvalidTag(11)),
            TarsType::Zero => Ok(TarsValue::Int(0)),
            TarsType::SimpleList => Ok(TarsValue::SimpleList(self.read_simple_list()?)),
        }
    }

    pub fn read_request_header(
        &mut self,
    ) -> Result<(TarsRequestHeader, BTreeMap<u8, TarsValue>), TarsError> {
        let mut map = self.read_struct()?;
        let header = TarsRequestHeader {
            version: map
                .remove(&1)
                .ok_or(TarsError::MissingRequiredField(1))?
                .try_into()?,
            packet_type: map
                .remove(&2)
                .ok_or(TarsError::MissingRequiredField(2))?
                .try_into()?,
            message_type: map
                .remove(&3)
                .ok_or(TarsError::MissingRequiredField(3))?
                .try_into()?,
            request_id: map
                .remove(&4)
                .ok_or(TarsError::MissingRequiredField(4))?
                .try_into()?,
            servant_name: map
                .remove(&5)
                .ok_or(TarsError::MissingRequiredField(5))?
                .try_into()?,
            func_name: map
                .remove(&6)
                .ok_or(TarsError::MissingRequiredField(6))?
                .try_into()?,
            timeout: map.remove(&8).unwrap_or(TarsValue::Int(0)).try_into()?,
            context: map
                .remove(&9)
                .unwrap_or(TarsValue::Map(BTreeMap::new()))
                .try_into()?,
            status: map
                .remove(&10)
                .unwrap_or(TarsValue::Map(BTreeMap::new()))
                .try_into()?,
        };
        Ok((header, map))
    }
}

impl TryFrom<TarsValue> for i16 {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        match value {
            TarsValue::Short(v) => Ok(v),
            _ => Err(TarsError::TypeMismatch {
                expected: "Short",
                actual: "Other",
            }),
        }
    }
}

impl TryFrom<TarsValue> for u8 {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        match value {
            TarsValue::Byte(v) => Ok(v),
            _ => Err(TarsError::TypeMismatch {
                expected: "Byte",
                actual: "Other",
            }),
        }
    }
}

impl TryFrom<TarsValue> for i32 {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        match value {
            TarsValue::Int(v) => Ok(v),
            _ => Err(TarsError::TypeMismatch {
                expected: "Int",
                actual: "Other",
            }),
        }
    }
}

impl TryFrom<TarsValue> for String {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        match value {
            TarsValue::String(v) => Ok(v),
            _ => Err(TarsError::TypeMismatch {
                expected: "String",
                actual: "Other",
            }),
        }
    }
}

impl TryFrom<TarsValue> for BTreeMap<String, String> {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        match value {
            TarsValue::Map(map) => {
                let mut result = BTreeMap::new();
                for (k, v) in map {
                    result.insert(k.try_into()?, v.try_into()?);
                }
                Ok(result)
            }
            _ => Err(TarsError::TypeMismatch {
                expected: "Map",
                actual: "Other",
            }),
        }
    }
}

impl TryFrom<TarsValue> for Vec<u8> {
    type Error = TarsError;

    fn try_from(value: TarsValue) -> Result<Self, Self::Error> {
        match value {
            TarsValue::SimpleList(v) => Ok(v),
            _ => Err(TarsError::TypeMismatch {
                expected: "SimpleList",
                actual: "Other",
            }),
        }
    }
}
