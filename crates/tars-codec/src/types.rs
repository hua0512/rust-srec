use bytes::Bytes;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::hash::{Hash, Hasher};

/// Represents a full Tars message.
#[derive(Debug)]
pub struct TarsMessage {
    pub header: TarsRequestHeader,
    pub body: FxHashMap<String, Bytes>, // The raw body payload with fast hashing
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
    pub context: FxHashMap<String, String>,
    pub status: FxHashMap<String, String>,
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
    /// Zero-copy string data - avoids allocation until conversion needed
    StringRef(Bytes),
    Struct(FxHashMap<u8, TarsValue>),
    Map(FxHashMap<TarsValue, TarsValue>),
    List(SmallVec<[Box<TarsValue>; 4]>), // Most lists are small, avoid heap allocation
    SimpleList(Bytes),
    /// Zero-copy binary data
    Binary(Bytes),
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
            (TarsValue::Float(a), TarsValue::Float(b)) => {
                // Safe float comparison that handles NaN values
                a.partial_cmp(b).unwrap_or_else(|| {
                    if a.is_nan() && b.is_nan() {
                        Ordering::Equal
                    } else if a.is_nan() {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    }
                })
            }
            (TarsValue::Double(a), TarsValue::Double(b)) => {
                // Safe double comparison that handles NaN values
                a.partial_cmp(b).unwrap_or_else(|| {
                    if a.is_nan() && b.is_nan() {
                        Ordering::Equal
                    } else if a.is_nan() {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    }
                })
            }
            (TarsValue::String(a), TarsValue::String(b)) => a.cmp(b),
            (TarsValue::StringRef(a), TarsValue::StringRef(b)) => a.cmp(b),
            (TarsValue::String(a), TarsValue::StringRef(b)) => a.as_bytes().cmp(&**b),
            (TarsValue::StringRef(a), TarsValue::String(b)) => (**a).cmp(b.as_bytes()),
            (TarsValue::Struct(a), TarsValue::Struct(b)) => {
                // Compare HashMaps by converting to sorted vectors
                let mut a_vec: Vec<_> = a.iter().collect();
                let mut b_vec: Vec<_> = b.iter().collect();
                a_vec.sort_by_key(|(k, _)| **k);
                b_vec.sort_by_key(|(k, _)| **k);
                a_vec.cmp(&b_vec)
            }
            (TarsValue::Map(a), TarsValue::Map(b)) => {
                // Compare HashMaps by converting to sorted vectors
                let mut a_vec: Vec<_> = a.iter().collect();
                let mut b_vec: Vec<_> = b.iter().collect();
                a_vec.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
                b_vec.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
                a_vec.cmp(&b_vec)
            }
            (TarsValue::List(a), TarsValue::List(b)) => {
                // Compare SmallVec of Box<TarsValue>
                let a_slice: &[Box<TarsValue>] = a.as_slice();
                let b_slice: &[Box<TarsValue>] = b.as_slice();
                a_slice.cmp(b_slice)
            }
            (TarsValue::SimpleList(a), TarsValue::SimpleList(b)) => a.cmp(b),
            (TarsValue::Binary(a), TarsValue::Binary(b)) => a.cmp(b),
            (TarsValue::StructBegin, TarsValue::StructBegin) => Ordering::Equal,
            (TarsValue::StructEnd, TarsValue::StructEnd) => Ordering::Equal,
            _ => Ordering::Less,
        }
    }
}

impl Hash for TarsValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            TarsValue::Bool(v) => {
                0u8.hash(state);
                v.hash(state);
            }
            TarsValue::Byte(v) => {
                1u8.hash(state);
                v.hash(state);
            }
            TarsValue::Short(v) => {
                2u8.hash(state);
                v.hash(state);
            }
            TarsValue::Int(v) => {
                3u8.hash(state);
                v.hash(state);
            }
            TarsValue::Long(v) => {
                4u8.hash(state);
                v.hash(state);
            }
            TarsValue::Float(v) => {
                5u8.hash(state);
                // Safe float hashing by converting to bits
                if v.is_nan() {
                    0u32.hash(state); // All NaN values hash to the same value
                } else {
                    v.to_bits().hash(state);
                }
            }
            TarsValue::Double(v) => {
                6u8.hash(state);
                // Safe double hashing by converting to bits
                if v.is_nan() {
                    0u64.hash(state); // All NaN values hash to the same value
                } else {
                    v.to_bits().hash(state);
                }
            }
            TarsValue::String(v) => {
                7u8.hash(state);
                v.hash(state);
            }
            TarsValue::StringRef(v) => {
                14u8.hash(state);
                v.hash(state);
            }
            TarsValue::Struct(v) => {
                8u8.hash(state);
                // Optimized hashing for small structs - avoid sorting when possible
                if v.len() <= 4 {
                    // For small structs, hash in tag order (usually already sorted)
                    let mut tags: Vec<_> = v.keys().collect();
                    tags.sort_unstable();
                    for &tag in tags {
                        tag.hash(state);
                        v[&tag].hash(state);
                    }
                } else {
                    // For larger structs, use the sorting approach
                    let mut pairs: Vec<_> = v.iter().collect();
                    pairs.sort_by_key(|(k, _)| **k);
                    for (k, val) in pairs {
                        k.hash(state);
                        val.hash(state);
                    }
                }
            }
            TarsValue::Map(v) => {
                9u8.hash(state);
                // Optimized hashing for small maps
                if v.len() <= 3 {
                    // For small maps, collect and sort with faster unstable sort
                    let mut pairs: Vec<_> = v.iter().collect();
                    pairs.sort_unstable_by(|(k1, _), (k2, _)| k1.cmp(k2));
                    // Hash length first for better distribution
                    v.len().hash(state);
                    for (k, val) in pairs {
                        k.hash(state);
                        val.hash(state);
                    }
                } else {
                    // For larger maps, use a simplified hash approach
                    // Hash the length and a sample of key-value pairs
                    v.len().hash(state);
                    let mut key_hash = 0u64;
                    let mut val_hash = 0u64;
                    for (k, val) in v.iter().take(3) {
                        // XOR hash values for order independence (approximate)
                        key_hash ^= {
                            let mut hasher = std::collections::hash_map::DefaultHasher::new();
                            k.hash(&mut hasher);
                            hasher.finish()
                        };
                        val_hash ^= {
                            let mut hasher = std::collections::hash_map::DefaultHasher::new();
                            val.hash(&mut hasher);
                            hasher.finish()
                        };
                    }
                    key_hash.hash(state);
                    val_hash.hash(state);
                }
            }
            TarsValue::List(v) => {
                10u8.hash(state);
                for item in v {
                    item.hash(state);
                }
            }
            TarsValue::SimpleList(v) => {
                11u8.hash(state);
                v.hash(state);
            }
            TarsValue::Binary(v) => {
                15u8.hash(state);
                v.hash(state);
            }
            TarsValue::StructBegin => {
                12u8.hash(state);
            }
            TarsValue::StructEnd => {
                13u8.hash(state);
            }
        }
    }
}

impl TarsValue {
    /// Fast path for getting i32 values without error handling
    #[inline]
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            TarsValue::Int(v) => Some(*v),
            TarsValue::Short(v) => Some(*v as i32),
            TarsValue::Byte(v) => Some(*v as i32),
            _ => None,
        }
    }

    /// Zero-copy string access - returns &str without allocation
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            TarsValue::String(s) => Some(s),
            TarsValue::StringRef(bytes) => std::str::from_utf8(bytes).ok(),
            _ => None,
        }
    }

    /// Get string as owned String (allocates only if necessary)
    pub fn into_string(self) -> Option<String> {
        match self {
            TarsValue::String(s) => Some(s),
            TarsValue::StringRef(bytes) => String::from_utf8(bytes.to_vec()).ok(),
            _ => None,
        }
    }

    /// Fast path for getting bytes values without cloning
    #[inline]
    pub fn as_bytes(&self) -> Option<&Bytes> {
        match self {
            TarsValue::SimpleList(b) => Some(b),
            TarsValue::Binary(b) => Some(b),
            TarsValue::StringRef(b) => Some(b),
            _ => None,
        }
    }

    /// Check if this is a zero value (for optimization)
    #[inline]
    pub fn is_zero(&self) -> bool {
        matches!(
            self,
            TarsValue::Byte(0) | TarsValue::Short(0) | TarsValue::Int(0) | TarsValue::Long(0)
        )
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Eq, Hash)]
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

/// A bytes wrapper that guarantees the content is valid UTF-8
/// Used to avoid redundant validation in hot paths
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValidatedBytes(pub(crate) Bytes);

impl ValidatedBytes {
    /// Create a new ValidatedBytes after checking UTF-8 validity
    pub fn new(bytes: Bytes) -> Result<Self, std::str::Utf8Error> {
        std::str::from_utf8(&bytes)?;
        Ok(Self(bytes))
    }

    /// Get the underlying bytes
    pub fn bytes(&self) -> &Bytes {
        &self.0
    }

    /// Convert to underlying bytes
    pub fn into_bytes(self) -> Bytes {
        self.0
    }

    /// Get string slice without validation (safe because we validated during construction)
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: We validated UTF-8 during construction
        unsafe { std::str::from_utf8_unchecked(&self.0) }
    }

    /// Convert to owned String
    pub fn into_string(self) -> String {
        // SAFETY: We validated UTF-8 during construction
        unsafe { String::from_utf8_unchecked(self.0.to_vec()) }
    }
}

impl Hash for ValidatedBytes {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl From<ValidatedBytes> for Bytes {
    fn from(validated: ValidatedBytes) -> Self {
        validated.0
    }
}
