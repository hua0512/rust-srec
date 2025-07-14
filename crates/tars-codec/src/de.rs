use crate::{
    error::TarsError,
    types::{TarsMessage, TarsRequestHeader, TarsType, TarsValue},
};
use bytes::{Buf, Bytes};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

pub struct TarsDeserializer {
    buffer: Bytes,
    /// When true, strings are parsed as StringRef (zero-copy) instead of String
    pub zero_copy_strings: bool,
}

impl TarsDeserializer {
    pub fn new(buffer: Bytes) -> Self {
        Self {
            buffer,
            zero_copy_strings: false, // Default to backward compatibility
        }
    }

    pub fn new_zero_copy(buffer: Bytes) -> Self {
        Self {
            buffer,
            zero_copy_strings: true,
        }
    }

    /// Reset the deserializer with new data for object pool reuse
    pub fn reset(&mut self, buffer: Bytes) {
        self.buffer = buffer;
        // Keep the zero_copy_strings setting from original construction
    }

    pub fn read_message(&mut self) -> Result<TarsMessage, TarsError> {
        let mut header = TarsRequestHeader {
            version: 0,
            packet_type: 0,
            message_type: 0,
            request_id: 0,
            servant_name: String::new(),
            func_name: String::new(),
            timeout: 0,
            context: Default::default(),
            status: Default::default(),
        };
        let mut body = Default::default();

        while !self.is_empty() {
            let (tag, value) = self.read_value()?;
            match tag {
                1 => header.version = value.try_into_i16()?,
                2 => header.packet_type = value.try_into_u8()?,
                3 => header.message_type = value.try_into_i32()?,
                4 => header.request_id = value.try_into_i32()?,
                5 => header.servant_name = value.try_into_string()?,
                6 => header.func_name = value.try_into_string()?,
                7 => {
                    let body_bytes = value.try_into_simple_list()?;
                    let mut body_de = TarsDeserializer::new(body_bytes);
                    let (_tag, body_value) = body_de.read_value()?;
                    let body_map = body_value.try_into_map()?;
                    body = body_map
                        .into_iter()
                        .map(|(k, v)| {
                            let k = k.try_into_string()?;
                            let v = v.try_into_simple_list()?;
                            Ok((k, v))
                        })
                        .collect::<Result<_, TarsError>>()?;
                }
                8 => header.timeout = value.try_into_i32()?,
                9 => {
                    header.context = value
                        .try_into_map()?
                        .into_iter()
                        .map(|(k, v)| Ok((k.try_into_string()?, v.try_into_string()?)))
                        .collect::<Result<_, TarsError>>()?
                }
                10 => {
                    header.status = value
                        .try_into_map()?
                        .into_iter()
                        .map(|(k, v)| Ok((k.try_into_string()?, v.try_into_string()?)))
                        .collect::<Result<_, TarsError>>()?
                }
                _ => {} // Ignore unknown tags
            }
        }

        Ok(TarsMessage { header, body })
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    #[inline]
    pub fn read_head(&mut self) -> Result<(u8, TarsType), TarsError> {
        let head = self.buffer.get_u8();
        let type_id =
            TarsType::try_from(head & 0x0F).map_err(|()| TarsError::InvalidTypeId(head & 0x0F))?;
        let tag = (head & 0xF0) >> 4;
        if tag == 15 {
            let extended_tag = self.buffer.get_u8();
            Ok((extended_tag, type_id))
        } else {
            Ok((tag, type_id))
        }
    }

    pub fn read_bool(&mut self) -> Result<bool, TarsError> {
        Ok(self.buffer.get_u8() != 0)
    }

    #[inline]
    pub fn read_i8(&mut self) -> Result<i8, TarsError> {
        Ok(self.buffer.get_i8())
    }

    #[inline]
    pub fn read_i16(&mut self) -> Result<i16, TarsError> {
        Ok(self.buffer.get_i16())
    }

    #[inline]
    pub fn read_i32(&mut self) -> Result<i32, TarsError> {
        Ok(self.buffer.get_i32())
    }

    #[inline]
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
        if len == 0 {
            return Ok(String::new());
        }

        // Avoid temporary buffer allocation for small strings
        if len <= 256 {
            let bytes = self.buffer.split_to(len);
            // Direct conversion from bytes without intermediate vec
            // println!("TarsDeserializer::read_string: {:?}", bytes);
            std::str::from_utf8(&bytes)
                .map(|s| s.to_string())
                .map_err(TarsError::from)
        } else {
            // For larger strings, use the original approach
            let mut buf = vec![0; len];
            self.buffer.copy_to_slice(&mut buf);
            // println!("TarsDeserializer::read_string: {:?}", buf);
            String::from_utf8(buf).map_err(TarsError::from)
        }
    }

    /// Zero-copy string reading - returns Bytes instead of String
    pub fn read_string_ref(&mut self, len: usize) -> Result<Bytes, TarsError> {
        if len == 0 {
            return Ok(Bytes::new());
        }

        let bytes = self.buffer.split_to(len);
        // println!("TarsDeserializer::read_string_ref: {:?}", bytes);
        // Validate UTF-8 without allocating
        std::str::from_utf8(&bytes).map_err(TarsError::from)?;
        Ok(bytes)
    }

    pub fn read_struct(&mut self) -> Result<FxHashMap<u8, TarsValue>, TarsError> {
        let mut map = FxHashMap::default();
        loop {
            let (tag, type_id) = self.read_head()?;
            if type_id == TarsType::StructEnd {
                break;
            }
            let value = self.read_value_by_type(type_id, tag)?;
            map.insert(tag, value);
        }
        Ok(map)
    }

    pub fn read_map(&mut self) -> Result<FxHashMap<TarsValue, TarsValue>, TarsError> {
        let mut map = FxHashMap::default();
        let (_len_tag, len_type) = self.read_head()?;
        let len = self.read_value_by_type(len_type, 0)?.try_into_i32()? as usize;
        for _ in 0..len {
            let (k_tag, k_type) = self.read_head()?;
            let k = self.read_value_by_type(k_type, k_tag)?;
            let (v_tag, v_type) = self.read_head()?;
            let v = self.read_value_by_type(v_type, v_tag)?;
            map.insert(k, v);
        }
        Ok(map)
    }

    pub fn read_list(&mut self) -> Result<SmallVec<[Box<TarsValue>; 4]>, TarsError> {
        let (_len_tag, len_type) = self.read_head()?;
        let len = self.read_value_by_type(len_type, 0)?.try_into_i32()? as usize;
        // Pre-allocate with known capacity
        let mut vec = SmallVec::with_capacity(len);
        for _ in 0..len {
            let (tag, type_id) = self.read_head()?;
            let value = self.read_value_by_type(type_id, tag)?;
            vec.push(Box::new(value));
        }
        Ok(vec)
    }

    pub fn read_simple_list(&mut self) -> Result<Bytes, TarsError> {
        self.read_head()?; // Should be (0, Int1) type
        let (_len_tag, len_type) = self.read_head()?;
        let len = self.read_value_by_type(len_type, 0)?.try_into_i32()? as usize;
        let bytes = self.buffer.split_to(len);
        Ok(bytes)
    }

    pub fn read_value(&mut self) -> Result<(u8, TarsValue), TarsError> {
        let (tag, type_id) = self.read_head()?;
        let value = self.read_value_by_type(type_id, tag)?;
        Ok((tag, value))
    }

    pub fn read_value_by_type(
        &mut self,
        type_id: TarsType,
        _tag: u8,
    ) -> Result<TarsValue, TarsError> {
        match type_id {
            TarsType::Zero => Ok(TarsValue::Byte(0)),
            TarsType::Int1 => {
                let val = self.read_i8()?;
                Ok(TarsValue::Byte(val as u8))
            }
            TarsType::Int2 => self.read_i16().map(TarsValue::Short),
            TarsType::Int4 => self.read_i32().map(TarsValue::Int),
            TarsType::Int8 => self.read_i64().map(TarsValue::Long),
            TarsType::Float => self.read_f32().map(TarsValue::Float),
            TarsType::Double => self.read_f64().map(TarsValue::Double),
            TarsType::String1 => {
                let len = self.buffer.get_u8() as usize;
                if self.zero_copy_strings {
                    self.read_string_ref(len).map(TarsValue::StringRef)
                } else {
                    self.read_string(len).map(TarsValue::String)
                }
            }
            TarsType::String4 => {
                let len = self.buffer.get_u32() as usize;
                if self.zero_copy_strings {
                    self.read_string_ref(len).map(TarsValue::StringRef)
                } else {
                    self.read_string(len).map(TarsValue::String)
                }
            }
            TarsType::StructBegin => self.read_struct().map(TarsValue::Struct),
            TarsType::Map => self.read_map().map(TarsValue::Map),
            TarsType::List => self.read_list().map(TarsValue::List),
            TarsType::SimpleList => self.read_simple_list().map(TarsValue::SimpleList),
            TarsType::StructEnd => Ok(TarsValue::StructEnd),
        }
    }
}

impl TarsValue {
    pub fn try_into_i16(self) -> Result<i16, TarsError> {
        match self {
            TarsValue::Short(v) => Ok(v),
            TarsValue::Byte(v) => Ok(v as i16),
            _ => Err(TarsError::TypeMismatch {
                expected: "Short",
                actual: "Other",
            }),
        }
    }

    pub fn try_into_u8(self) -> Result<u8, TarsError> {
        match self {
            TarsValue::Byte(v) => Ok(v),
            _ => Err(TarsError::TypeMismatch {
                expected: "Byte",
                actual: "Other",
            }),
        }
    }

    pub fn try_into_i32(self) -> Result<i32, TarsError> {
        match self {
            TarsValue::Int(v) => Ok(v),
            TarsValue::Short(v) => Ok(v as i32),
            TarsValue::Byte(v) => Ok(v as i32),
            _ => Err(TarsError::TypeMismatch {
                expected: "Int",
                actual: "Other",
            }),
        }
    }

    pub fn try_into_string(self) -> Result<String, TarsError> {
        match self {
            TarsValue::String(v) => Ok(v),
            TarsValue::StringRef(bytes) => {
                // println!("TarsValue::StringRef: {:?}", bytes);
                String::from_utf8(bytes.to_vec()).map_err(TarsError::from)
            }
            _ => Err(TarsError::TypeMismatch {
                expected: "String",
                actual: "Other",
            }),
        }
    }

    pub fn try_into_map(self) -> Result<FxHashMap<TarsValue, TarsValue>, TarsError> {
        if let TarsValue::Map(v) = self {
            Ok(v)
        } else {
            Err(TarsError::TypeMismatch {
                expected: "Map",
                actual: "Other",
            })
        }
    }

    pub fn try_into_simple_list(self) -> Result<Bytes, TarsError> {
        if let TarsValue::SimpleList(v) = self {
            Ok(v)
        } else {
            Err(TarsError::TypeMismatch {
                expected: "SimpleList",
                actual: "Other",
            })
        }
    }
}

pub trait TarsDeserializable: Sized {
    fn deserialize(deserializer: &mut TarsDeserializer) -> Result<Self, TarsError>;
}

impl TarsDeserializable for TarsValue {
    fn deserialize(deserializer: &mut TarsDeserializer) -> Result<Self, TarsError> {
        let (_, value) = deserializer.read_value()?;
        Ok(value)
    }
}

pub fn from_bytes(buffer: Bytes) -> Result<TarsValue, TarsError> {
    let mut deserializer = TarsDeserializer::new(buffer);
    TarsValue::deserialize(&mut deserializer)
}
