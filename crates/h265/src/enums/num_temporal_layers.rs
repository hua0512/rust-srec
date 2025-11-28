/// The number of temporal layers in the stream.
///
/// `0` and `1` are special values.
///
/// Any other value represents the actual number of temporal layers.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NumTemporalLayers {
    /// The stream might be temporally scalable.
    Unknown,
    /// The stream is not temporally scalable.
    NotScalable,
    /// A specific number of temporal layers, represented by the enclosed value.
    Count(u8),
}

impl From<u8> for NumTemporalLayers {
    fn from(value: u8) -> Self {
        match value {
            0 => NumTemporalLayers::Unknown,
            1 => NumTemporalLayers::NotScalable,
            _ => NumTemporalLayers::Count(value),
        }
    }
}


impl From<&NumTemporalLayers> for u8 {
    fn from(value: &NumTemporalLayers) -> Self {
        match *value {
            NumTemporalLayers::Unknown => 0,
            NumTemporalLayers::NotScalable => 1,
            NumTemporalLayers::Count(count) => count,
        }
    }
}
