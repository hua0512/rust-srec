use bytes::Bytes;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::sync::atomic::AtomicI32;

/// Atomic counter for generating unique request IDs across all TARS messages
static REQUEST_ID_COUNTER: AtomicI32 = AtomicI32::new(0);

/// Returns the next unique request ID for TARS messages.
/// Thread-safe and monotonically increasing.
#[inline]
pub fn next_request_id() -> i32 {
    REQUEST_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

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
#[derive(Debug, Clone)]
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

const CANONICAL_F32_NAN_BITS: u32 = 0x7fc0_0000;
const CANONICAL_F64_NAN_BITS: u64 = 0x7ff8_0000_0000_0000;

#[inline]
fn canonical_f32_bits(v: f32) -> u32 {
    if v.is_nan() {
        CANONICAL_F32_NAN_BITS
    } else if v == 0.0 {
        0
    } else {
        v.to_bits()
    }
}

#[inline]
fn canonical_f64_bits(v: f64) -> u64 {
    if v.is_nan() {
        CANONICAL_F64_NAN_BITS
    } else if v == 0.0 {
        0
    } else {
        v.to_bits()
    }
}

impl TarsValue {
    /// Canonical discriminant rank used by `Ord` and `Hash`.
    /// `String` and `StringRef` share rank `7` because they represent the same logical TARS string type.
    #[inline]
    const fn variant_rank(&self) -> u8 {
        match self {
            TarsValue::Bool(_) => 0,
            TarsValue::Byte(_) => 1,
            TarsValue::Short(_) => 2,
            TarsValue::Int(_) => 3,
            TarsValue::Long(_) => 4,
            TarsValue::Float(_) => 5,
            TarsValue::Double(_) => 6,
            TarsValue::String(_) | TarsValue::StringRef(_) => 7,
            TarsValue::Struct(_) => 8,
            TarsValue::Map(_) => 9,
            TarsValue::List(_) => 10,
            TarsValue::SimpleList(_) => 11,
            TarsValue::Binary(_) => 12,
            TarsValue::StructBegin => 13,
            TarsValue::StructEnd => 14,
        }
    }
}

impl PartialEq for TarsValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TarsValue::Bool(a), TarsValue::Bool(b)) => a == b,
            (TarsValue::Byte(a), TarsValue::Byte(b)) => a == b,
            (TarsValue::Short(a), TarsValue::Short(b)) => a == b,
            (TarsValue::Int(a), TarsValue::Int(b)) => a == b,
            (TarsValue::Long(a), TarsValue::Long(b)) => a == b,
            (TarsValue::Float(a), TarsValue::Float(b)) => {
                canonical_f32_bits(*a) == canonical_f32_bits(*b)
            }
            (TarsValue::Double(a), TarsValue::Double(b)) => {
                canonical_f64_bits(*a) == canonical_f64_bits(*b)
            }
            (TarsValue::String(a), TarsValue::String(b)) => a == b,
            (TarsValue::StringRef(a), TarsValue::StringRef(b)) => a == b,
            (TarsValue::String(a), TarsValue::StringRef(b)) => a.as_bytes() == b.as_ref(),
            (TarsValue::StringRef(a), TarsValue::String(b)) => a.as_ref() == b.as_bytes(),
            (TarsValue::Struct(a), TarsValue::Struct(b)) => a == b,
            (TarsValue::Map(a), TarsValue::Map(b)) => a == b,
            (TarsValue::List(a), TarsValue::List(b)) => a == b,
            (TarsValue::SimpleList(a), TarsValue::SimpleList(b)) => a == b,
            (TarsValue::Binary(a), TarsValue::Binary(b)) => a == b,
            (TarsValue::StructBegin, TarsValue::StructBegin) => true,
            (TarsValue::StructEnd, TarsValue::StructEnd) => true,
            _ => false,
        }
    }
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
            (TarsValue::Float(a), TarsValue::Float(b)) => f32::from_bits(canonical_f32_bits(*a))
                .total_cmp(&f32::from_bits(canonical_f32_bits(*b))),
            (TarsValue::Double(a), TarsValue::Double(b)) => f64::from_bits(canonical_f64_bits(*a))
                .total_cmp(&f64::from_bits(canonical_f64_bits(*b))),
            (TarsValue::String(a), TarsValue::String(b)) => a.as_bytes().cmp(b.as_bytes()),
            (TarsValue::StringRef(a), TarsValue::StringRef(b)) => a.as_ref().cmp(b.as_ref()),
            (TarsValue::String(a), TarsValue::StringRef(b)) => a.as_bytes().cmp(b.as_ref()),
            (TarsValue::StringRef(a), TarsValue::String(b)) => a.as_ref().cmp(b.as_bytes()),
            (TarsValue::Struct(a), TarsValue::Struct(b)) => {
                let mut a_vec: Vec<_> = a.iter().collect();
                let mut b_vec: Vec<_> = b.iter().collect();
                a_vec.sort_by(|(ka, va), (kb, vb)| ka.cmp(kb).then_with(|| va.cmp(vb)));
                b_vec.sort_by(|(ka, va), (kb, vb)| ka.cmp(kb).then_with(|| va.cmp(vb)));
                a_vec.cmp(&b_vec)
            }
            (TarsValue::Map(a), TarsValue::Map(b)) => {
                let mut a_vec: Vec<_> = a.iter().collect();
                let mut b_vec: Vec<_> = b.iter().collect();
                a_vec.sort_by(|(ka, va), (kb, vb)| ka.cmp(kb).then_with(|| va.cmp(vb)));
                b_vec.sort_by(|(ka, va), (kb, vb)| ka.cmp(kb).then_with(|| va.cmp(vb)));
                a_vec.cmp(&b_vec)
            }
            (TarsValue::List(a), TarsValue::List(b)) => a.as_slice().cmp(b.as_slice()),
            (TarsValue::SimpleList(a), TarsValue::SimpleList(b)) => a.cmp(b),
            (TarsValue::Binary(a), TarsValue::Binary(b)) => a.cmp(b),
            (TarsValue::StructBegin, TarsValue::StructBegin)
            | (TarsValue::StructEnd, TarsValue::StructEnd) => Ordering::Equal,
            _ => self.variant_rank().cmp(&other.variant_rank()),
        }
    }
}

impl Hash for TarsValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.variant_rank().hash(state);
        match self {
            TarsValue::Bool(v) => v.hash(state),
            TarsValue::Byte(v) => v.hash(state),
            TarsValue::Short(v) => v.hash(state),
            TarsValue::Int(v) => v.hash(state),
            TarsValue::Long(v) => v.hash(state),
            TarsValue::Float(v) => canonical_f32_bits(*v).hash(state),
            TarsValue::Double(v) => canonical_f64_bits(*v).hash(state),
            TarsValue::String(v) => {
                let bytes: &[u8] = v.as_bytes();
                bytes.hash(state);
            }
            TarsValue::StringRef(v) => {
                let bytes: &[u8] = v.as_ref();
                bytes.hash(state);
            }
            TarsValue::Struct(v) => {
                v.len().hash(state);
                let mut combined = 0u64;
                for (k, val) in v {
                    let mut entry_hasher = std::collections::hash_map::DefaultHasher::new();
                    k.hash(&mut entry_hasher);
                    val.hash(&mut entry_hasher);
                    combined = combined.wrapping_add(entry_hasher.finish());
                }
                combined.hash(state);
            }
            TarsValue::Map(v) => {
                v.len().hash(state);
                let mut combined = 0u64;
                for (k, val) in v {
                    let mut entry_hasher = std::collections::hash_map::DefaultHasher::new();
                    k.hash(&mut entry_hasher);
                    val.hash(&mut entry_hasher);
                    combined = combined.wrapping_add(entry_hasher.finish());
                }
                combined.hash(state);
            }
            TarsValue::List(v) => {
                v.len().hash(state);
                for item in v {
                    item.hash(state);
                }
            }
            TarsValue::SimpleList(v) => v.hash(state),
            TarsValue::Binary(v) => v.hash(state),
            TarsValue::StructBegin | TarsValue::StructEnd => {}
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

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;

    use super::*;

    fn hash_value(v: &TarsValue) -> u64 {
        let mut hasher = DefaultHasher::new();
        v.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn tars_value_ord_eq_and_hash_invariants_hold_across_variants() {
        let values = vec![
            TarsValue::Bool(false),
            TarsValue::Bool(true),
            TarsValue::Byte(1),
            TarsValue::Short(-2),
            TarsValue::Int(42),
            TarsValue::Long(-100),
            TarsValue::Float(-0.0),
            TarsValue::Float(0.0),
            TarsValue::Float(f32::NAN),
            TarsValue::Float(f32::from_bits(0xffc0_0001)),
            TarsValue::Double(-0.0),
            TarsValue::Double(0.0),
            TarsValue::Double(f64::NAN),
            TarsValue::Double(f64::from_bits(0xfff8_0000_0000_0001)),
            TarsValue::String("alpha".to_string()),
            TarsValue::StringRef(Bytes::from_static(b"alpha")),
            TarsValue::StringRef(Bytes::from_static(b"beta")),
            TarsValue::SimpleList(Bytes::from_static(b"raw")),
            TarsValue::Binary(Bytes::from_static(b"raw")),
            TarsValue::StructBegin,
            TarsValue::StructEnd,
        ];

        for a in &values {
            for b in &values {
                assert_eq!(
                    a.cmp(b),
                    b.cmp(a).reverse(),
                    "antisymmetry failed: {a:?} vs {b:?}"
                );
                assert_eq!(
                    a == b,
                    a.cmp(b) == Ordering::Equal,
                    "Eq/Ord mismatch: {a:?} vs {b:?}"
                );
                if a == b {
                    assert_eq!(
                        hash_value(a),
                        hash_value(b),
                        "Eq/Hash mismatch: {a:?} vs {b:?}"
                    );
                }
            }
        }

        let mut forward = values.clone();
        let mut reverse: Vec<_> = values.into_iter().rev().collect();
        forward.sort();
        reverse.sort();
        assert_eq!(forward, reverse);

        let mut map_a = FxHashMap::with_capacity_and_hasher(4, Default::default());
        let mut map_b = FxHashMap::with_capacity_and_hasher(64, Default::default());
        for i in 0..8 {
            map_a.insert(TarsValue::String(format!("k{i}")), TarsValue::Int(i));
        }
        for i in (0..8).rev() {
            map_b.insert(
                TarsValue::StringRef(Bytes::from(format!("k{i}"))),
                TarsValue::Int(i),
            );
        }
        let val_map_a = TarsValue::Map(map_a);
        let val_map_b = TarsValue::Map(map_b);
        assert_eq!(val_map_a, val_map_b);
        assert_eq!(val_map_a.cmp(&val_map_b), Ordering::Equal);
        assert_eq!(hash_value(&val_map_a), hash_value(&val_map_b));
    }
}
