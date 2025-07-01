// In tars-codec/src/types.rs

use std::collections::BTreeMap;

/// Represents a full Tars message.
pub struct TarsMessage {
    pub header: TarsRequestHeader,
    pub body: BTreeMap<String, Vec<u8>>, // The raw body payload
}

/// Represents the Tars request header.
#[derive(Debug, Clone, PartialEq)]
pub struct TarsRequestHeader {
    pub version: i16,
    pub packet_type: u8,
    pub message_type: i32,
    pub request_id: i32,
    pub servant_name: String,
    pub func_name: String,
    pub timeout: i32,
    pub context: BTreeMap<String, String>,
    pub status: BTreeMap<String, String>,
}

/// An enum representing any valid Tars value.
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq)]
pub enum TarsValue {
    Bool(bool),
    Byte(u8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    Struct(BTreeMap<u8, TarsValue>), // Using BTreeMap for ordered keys
    Map(BTreeMap<TarsValue, TarsValue>),
    List(Vec<TarsValue>),
    SimpleList(Vec<u8>),
    StructBegin,
    StructEnd,
}

impl Eq for TarsValue {}

impl PartialOrd for TarsValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TarsValue {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (TarsValue::Bool(a), TarsValue::Bool(b)) => a.cmp(b),
            (TarsValue::Byte(a), TarsValue::Byte(b)) => a.cmp(b),
            (TarsValue::Short(a), TarsValue::Short(b)) => a.cmp(b),
            (TarsValue::Int(a), TarsValue::Int(b)) => a.cmp(b),
            (TarsValue::Long(a), TarsValue::Long(b)) => a.cmp(b),
            (TarsValue::Float(a), TarsValue::Float(b)) => a.partial_cmp(b).unwrap(),
            (TarsValue::Double(a), TarsValue::Double(b)) => a.partial_cmp(b).unwrap(),
            (TarsValue::String(a), TarsValue::String(b)) => a.cmp(b),
            (TarsValue::Struct(a), TarsValue::Struct(b)) => a.cmp(b),
            (TarsValue::Map(a), TarsValue::Map(b)) => a.cmp(b),
            (TarsValue::List(a), TarsValue::List(b)) => a.cmp(b),
            (TarsValue::SimpleList(a), TarsValue::SimpleList(b)) => a.cmp(b),
            (TarsValue::StructBegin, TarsValue::StructBegin) => Ordering::Equal,
            (TarsValue::StructEnd, TarsValue::StructEnd) => Ordering::Equal,
            _ => Ordering::Less,
        }
    }
}

#[repr(u8)]
pub enum TarsType {
    Int1 = 0,
    Int2 = 1,
    Int4 = 2,
    Int8 = 3,
    Float = 4,
    Double = 5,
    String1 = 6,
    String4 = 7,
    Map = 8,
    List = 9,
    StructBegin = 10,
    StructEnd = 11,
    Zero = 12,
    SimpleList = 13,
}

impl From<TarsType> for u8 {
    fn from(t: TarsType) -> Self {
        t as u8
    }
}

impl TryFrom<u8> for TarsType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(TarsType::Int1),
            1 => Ok(TarsType::Int2),
            2 => Ok(TarsType::Int4),
            3 => Ok(TarsType::Int8),
            4 => Ok(TarsType::Float),
            5 => Ok(TarsType::Double),
            6 => Ok(TarsType::String1),
            7 => Ok(TarsType::String4),
            8 => Ok(TarsType::Map),
            9 => Ok(TarsType::List),
            10 => Ok(TarsType::StructBegin),
            11 => Ok(TarsType::StructEnd),
            12 => Ok(TarsType::Zero),
            13 => Ok(TarsType::SimpleList),
            _ => Err(()),
        }
    }
}