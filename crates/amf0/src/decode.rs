use std::borrow::Cow;
use std::io;

use super::{Amf0Marker, Amf0ReadError, Amf0Value};

/// Result of lossy decoding that may skip over invalid bytes.
///
/// When using [`Amf0Decoder::decode_all_lossy`], the decoder will attempt to
/// recover from certain errors by skipping unknown or unsupported marker bytes
/// and continuing to decode subsequent values.
pub struct LossyDecodeResult<'a> {
    /// Successfully decoded values.
    pub values: Vec<Amf0Value<'a>>,
    /// Total number of bytes skipped over during recovery.
    pub bytes_skipped: usize,
    /// The final non-recoverable error, if any (e.g., EOF or UTF-8 error).
    pub error: Option<Amf0ReadError>,
}

/// Maximum object/array nesting depth accepted by [`Amf0Decoder::decode`].
const MAX_DEPTH: usize = 256;

/// An AMF0 Decoder.
///
/// This decoder takes a reference to a byte slice and reads the AMF0 data from
/// it. All returned objects are references to the original byte slice, making
/// it very cheap to use.
pub struct Amf0Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Amf0Decoder<'a> {
    /// Create a new AMF0 decoder.
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            depth: 0,
        }
    }

    /// Check if the decoder has reached the end of the AMF0 data.
    pub const fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Read `len` bytes from the buffer, advancing the position.
    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], Amf0ReadError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| {
                Amf0ReadError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "not enough data",
                ))
            })?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    /// Read a single byte, advancing the position.
    fn read_u8(&mut self) -> Result<u8, Amf0ReadError> {
        let bytes = self.read_bytes(1)?;
        Ok(bytes[0])
    }

    /// Read a big-endian u16, advancing the position.
    fn read_u16_be(&mut self) -> Result<u16, Amf0ReadError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// Read a big-endian u32, advancing the position.
    fn read_u32_be(&mut self) -> Result<u32, Amf0ReadError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Read a big-endian f64, advancing the position.
    fn read_f64_be(&mut self) -> Result<f64, Amf0ReadError> {
        let bytes = self.read_bytes(8)?;
        Ok(f64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    /// Read a big-endian i16, advancing the position.
    fn read_i16_be(&mut self) -> Result<i16, Amf0ReadError> {
        let bytes = self.read_bytes(2)?;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// Read a big-endian u24 (3 bytes), advancing the position.
    fn read_u24_be(&mut self) -> Result<u32, Amf0ReadError> {
        let bytes = self.read_bytes(3)?;
        Ok(u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]))
    }

    /// Read all the encoded values from the decoder.
    /// Returns both successfully decoded values and any error that occurred.
    pub fn decode_all(&mut self) -> (Vec<Amf0Value<'a>>, Option<Amf0ReadError>) {
        let mut results = vec![];

        while !self.is_empty() {
            match self.decode() {
                Ok(value) => results.push(value),
                Err(err) => return (results, Some(err)),
            }
        }

        (results, None)
    }

    /// Read all encoded values, skipping over invalid bytes when possible.
    ///
    /// Unlike [`decode_all`](Self::decode_all), this method attempts to recover
    /// from recoverable errors (unknown or unsupported marker bytes) by skipping
    /// past them and retrying. It gives up after `max_consecutive_skip`
    /// consecutive bad bytes without a successful decode.
    ///
    /// # Arguments
    ///
    /// * `max_consecutive_skip` - Maximum number of consecutive bytes to skip
    ///   before giving up. The counter resets after each successful decode.
    pub fn decode_all_lossy(&mut self, max_consecutive_skip: usize) -> LossyDecodeResult<'a> {
        let mut values = vec![];
        let mut bytes_skipped: usize = 0;
        let mut consecutive_skips: usize = 0;

        while !self.is_empty() {
            let saved_pos = self.pos;

            match self.decode() {
                Ok(value) => {
                    values.push(value);
                    consecutive_skips = 0;
                }
                Err(err) if err.is_recoverable() => {
                    if consecutive_skips >= max_consecutive_skip {
                        return LossyDecodeResult {
                            values,
                            bytes_skipped,
                            error: Some(err),
                        };
                    }
                    // Skip 1 byte past where this decode attempt started
                    self.pos = saved_pos + 1;
                    bytes_skipped += 1;
                    consecutive_skips += 1;
                }
                Err(err) => {
                    return LossyDecodeResult {
                        values,
                        bytes_skipped,
                        error: Some(err),
                    };
                }
            }
        }

        LossyDecodeResult {
            values,
            bytes_skipped,
            error: None,
        }
    }

    /// Read the next encoded value from the decoder.
    pub fn decode(&mut self) -> Result<Amf0Value<'a>, Amf0ReadError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(Amf0ReadError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "maximum nesting depth exceeded",
            )));
        }

        let result = self.decode_value();
        self.depth -= 1;
        result
    }

    fn decode_value(&mut self) -> Result<Amf0Value<'a>, Amf0ReadError> {
        let marker_byte = self.read_u8()?;
        let marker = Amf0Marker::try_from(marker_byte).map_err(Amf0ReadError::UnknownMarker)?;

        match marker {
            Amf0Marker::Number => Ok(Amf0Value::Number(self.read_number()?)),
            Amf0Marker::Boolean => Ok(Amf0Value::Boolean(self.read_bool()?)),
            Amf0Marker::String => Ok(Amf0Value::String(self.read_string()?)),
            Amf0Marker::Object => Ok(Amf0Value::Object(self.read_object()?.into())),
            Amf0Marker::Null => Ok(Amf0Value::Null),
            Amf0Marker::Undefined => Ok(Amf0Value::Undefined),
            Amf0Marker::EcmaArray => Ok(Amf0Value::EcmaArray(self.read_ecma_array()?.into())),
            Amf0Marker::LongString => Ok(Amf0Value::LongString(self.read_long_string()?)),
            Amf0Marker::StrictArray => Ok(Amf0Value::StrictArray(self.read_strict_array()?.into())),
            Amf0Marker::Date => self.read_date(),
            _ => Err(Amf0ReadError::UnsupportedType(marker)),
        }
    }

    /// Read the next encoded value from the decoder and check if it matches the
    /// specified marker.
    pub fn decode_with_type(
        &mut self,
        specified_marker: Amf0Marker,
    ) -> Result<Amf0Value<'a>, Amf0ReadError> {
        // Peek at the next byte without advancing
        if self.pos >= self.data.len() {
            return Err(Amf0ReadError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not enough data",
            )));
        }

        let marker_byte = self.data[self.pos];
        let marker = Amf0Marker::try_from(marker_byte).map_err(Amf0ReadError::UnknownMarker)?;

        if marker != specified_marker {
            return Err(Amf0ReadError::WrongType {
                expected: specified_marker,
                got: marker,
            });
        }

        self.decode()
    }

    fn read_number(&mut self) -> Result<f64, Amf0ReadError> {
        self.read_f64_be()
    }

    fn read_bool(&mut self) -> Result<bool, Amf0ReadError> {
        Ok(self.read_u8()? > 0)
    }

    fn read_string(&mut self) -> Result<Cow<'a, str>, Amf0ReadError> {
        let len = self.read_u16_be()? as usize;
        let bytes = self.read_bytes(len)?;
        Ok(Cow::Borrowed(std::str::from_utf8(bytes)?))
    }

    fn is_read_object_eof(&mut self) -> Result<bool, Amf0ReadError> {
        // Need at least 3 bytes to check
        if self.pos + 3 > self.data.len() {
            return Ok(false);
        }

        let saved_pos = self.pos;
        let value = self.read_u24_be()?;

        if Amf0Marker::is_object_end_u24(value) {
            Ok(true)
        } else {
            self.pos = saved_pos;
            Ok(false)
        }
    }

    fn read_object(&mut self) -> Result<Vec<(Cow<'a, str>, Amf0Value<'a>)>, Amf0ReadError> {
        let mut properties = Vec::new();

        loop {
            if self.is_read_object_eof()? {
                break;
            }

            let key = self.read_string()?;
            let val = self.decode()?;

            properties.push((key, val));
        }

        Ok(properties)
    }

    fn read_ecma_array(&mut self) -> Result<Vec<(Cow<'a, str>, Amf0Value<'a>)>, Amf0ReadError> {
        let len = self.read_u32_be()?;

        let mut properties = Vec::new();

        for _ in 0..len {
            let key = self.read_string()?;
            let val = self.decode()?;
            properties.push((key, val));
        }

        // Sometimes the object end marker is present and sometimes it is not.
        // If it is there just consume it, if not then we are done.
        let _ = self.is_read_object_eof()?;

        Ok(properties)
    }

    fn read_long_string(&mut self) -> Result<Cow<'a, str>, Amf0ReadError> {
        let len = self.read_u32_be()? as usize;
        let bytes = self.read_bytes(len)?;
        let val = std::str::from_utf8(bytes)?;
        Ok(Cow::Borrowed(val))
    }

    fn read_strict_array(&mut self) -> Result<Vec<Amf0Value<'a>>, Amf0ReadError> {
        let len = self.read_u32_be()?;

        // Every element needs at least a marker byte, so the remaining input is
        // a safe upper bound for eager allocation.
        let remaining = self.data.len() - self.pos;
        let mut values = Vec::with_capacity((len as usize).min(remaining));

        for _ in 0..len {
            let val = self.decode()?;
            values.push(val);
        }

        Ok(values)
    }

    fn read_date(&mut self) -> Result<Amf0Value<'a>, Amf0ReadError> {
        let timestamp = self.read_f64_be()?;
        let timezone = self.read_i16_be()?;
        Ok(Amf0Value::Date {
            timestamp,
            timezone,
        })
    }
}

impl<'a> Iterator for Amf0Decoder<'a> {
    type Item = Result<Amf0Value<'a>, Amf0ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.is_empty() {
            return None;
        }

        Some(self.decode())
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_reader_bool() {
        let amf0_bool = vec![0x01, 0x01]; // true
        let mut amf_reader = Amf0Decoder::new(&amf0_bool);
        let value = amf_reader.decode_with_type(Amf0Marker::Boolean).unwrap();
        assert_eq!(value, Amf0Value::Boolean(true));
    }

    #[test]
    fn test_reader_number() {
        let mut amf0_number = vec![0x00];
        amf0_number.extend_from_slice(&772.161_f64.to_be_bytes());

        let mut amf_reader = Amf0Decoder::new(&amf0_number);
        let value = amf_reader.decode_with_type(Amf0Marker::Number).unwrap();
        assert_eq!(value, Amf0Value::Number(772.161));
    }

    #[test]
    fn test_reader_string() {
        let mut amf0_string = vec![0x02, 0x00, 0x0b]; // 11 bytes
        amf0_string.extend_from_slice(b"Hello World");

        let mut amf_reader = Amf0Decoder::new(&amf0_string);
        let value = amf_reader.decode_with_type(Amf0Marker::String).unwrap();
        assert_eq!(value, Amf0Value::String(Cow::Borrowed("Hello World")));
    }

    #[test]
    fn test_reader_long_string() {
        let mut amf0_string = vec![0x0c, 0x00, 0x00, 0x00, 0x0b]; // 11 bytes
        amf0_string.extend_from_slice(b"Hello World");

        let mut amf_reader = Amf0Decoder::new(&amf0_string);
        let value = amf_reader.decode_with_type(Amf0Marker::LongString).unwrap();
        assert_eq!(value, Amf0Value::LongString(Cow::Borrowed("Hello World")));
    }

    #[test]
    fn test_reader_object() {
        let mut amf0_object = vec![0x03, 0x00, 0x04]; // 1 property with 4 bytes
        amf0_object.extend_from_slice(b"test");
        amf0_object.extend_from_slice(&[0x05]); // null
        amf0_object.extend_from_slice(&[0x00, 0x00, 0x09]); // object end (0x00 0x00 0x09)

        let mut amf_reader = Amf0Decoder::new(&amf0_object);
        let value = amf_reader.decode_with_type(Amf0Marker::Object).unwrap();

        assert_eq!(
            value,
            Amf0Value::Object(vec![("test".into(), Amf0Value::Null)].into())
        );
    }

    #[test]
    fn test_reader_ecma_array() {
        let mut amf0_object = vec![0x08, 0x00, 0x00, 0x00, 0x01]; // 1 property
        amf0_object.extend_from_slice(&[0x00, 0x04]); // 4 bytes
        amf0_object.extend_from_slice(b"test");
        amf0_object.extend_from_slice(&[0x05]); // null

        let mut amf_reader = Amf0Decoder::new(&amf0_object);
        let value = amf_reader.decode_with_type(Amf0Marker::EcmaArray).unwrap();

        assert_eq!(
            value,
            Amf0Value::EcmaArray(vec![("test".into(), Amf0Value::Null)].into())
        );
    }

    #[test]
    fn test_reader_ecma_array_with_object_end() {
        let mut amf0_object = vec![0x08, 0x00, 0x00, 0x00, 0x01]; // 1 property
        amf0_object.extend_from_slice(&[0x00, 0x04]); // 4 bytes
        amf0_object.extend_from_slice(b"test");
        amf0_object.extend_from_slice(&[0x05]); // null
        amf0_object.extend_from_slice(&[0x00, 0x00, 0x09]); // object end

        let mut amf_reader = Amf0Decoder::new(&amf0_object);
        let value = amf_reader.decode_with_type(Amf0Marker::EcmaArray).unwrap();

        assert_eq!(
            value,
            Amf0Value::EcmaArray(vec![("test".into(), Amf0Value::Null)].into())
        );
        assert!(amf_reader.is_empty());
    }

    #[test]
    fn test_reader_strict_array() {
        let mut amf0_array = vec![0x0a, 0x00, 0x00, 0x00, 0x03]; // StrictArray marker with 3 elements
        amf0_array.extend_from_slice(&[0x00]); // Number marker
        amf0_array.extend_from_slice(&1.0_f64.to_be_bytes());
        amf0_array.extend_from_slice(&[0x01, 0x01]); // Boolean true
        amf0_array.extend_from_slice(&[0x02, 0x00, 0x04]); // String with 4 bytes
        amf0_array.extend_from_slice(b"test");

        let mut amf_reader = Amf0Decoder::new(&amf0_array);
        let value = amf_reader
            .decode_with_type(Amf0Marker::StrictArray)
            .unwrap();

        let expected = Amf0Value::StrictArray(
            vec![
                Amf0Value::Number(1.0),
                Amf0Value::Boolean(true),
                Amf0Value::String(Cow::Borrowed("test")),
            ]
            .into(),
        );

        assert_eq!(value, expected);
    }

    #[test]
    fn strict_array_count_is_bounded_by_remaining_input() {
        let data = [Amf0Marker::StrictArray as u8, 0xff, 0xff, 0xff, 0xff];
        let mut decoder = Amf0Decoder::new(&data);

        assert!(matches!(decoder.decode(), Err(Amf0ReadError::Io(_))));
    }

    #[test]
    fn deeply_nested_arrays_return_an_error() {
        let mut data = Vec::with_capacity((MAX_DEPTH + 1) * 5 + 1);
        for _ in 0..=MAX_DEPTH {
            data.push(Amf0Marker::StrictArray as u8);
            data.extend_from_slice(&1_u32.to_be_bytes());
        }
        data.push(Amf0Marker::Null as u8);

        let mut decoder = Amf0Decoder::new(&data);
        let error = decoder.decode().unwrap_err();
        assert!(matches!(
            error,
            Amf0ReadError::Io(ref source) if source.kind() == io::ErrorKind::InvalidData
        ));
    }

    #[test]
    fn test_reader_date() {
        let mut amf0_date = vec![0x0b]; // Date marker
        amf0_date.extend_from_slice(&1234567890.0_f64.to_be_bytes());
        amf0_date.extend_from_slice(&120_i16.to_be_bytes());

        let mut amf_reader = Amf0Decoder::new(&amf0_date);
        let value = amf_reader.decode_with_type(Amf0Marker::Date).unwrap();

        assert_eq!(
            value,
            Amf0Value::Date {
                timestamp: 1234567890.0,
                timezone: 120
            }
        );
    }

    #[test]
    fn test_reader_undefined() {
        let amf0_undefined = vec![0x06]; // Undefined marker
        let mut amf_reader = Amf0Decoder::new(&amf0_undefined);
        let value = amf_reader.decode_with_type(Amf0Marker::Undefined).unwrap();
        assert_eq!(value, Amf0Value::Undefined);
    }

    #[test]
    fn test_reader_multi_value() {
        let mut amf0_multi = vec![0x00];
        amf0_multi.extend_from_slice(&772.161_f64.to_be_bytes());
        amf0_multi.extend_from_slice(&[0x01, 0x01]); // true
        amf0_multi.extend_from_slice(&[0x02, 0x00, 0x0b]); // 11 bytes
        amf0_multi.extend_from_slice(b"Hello World");
        amf0_multi.extend_from_slice(&[0x03, 0x00, 0x04]); // 1 property with 4 bytes
        amf0_multi.extend_from_slice(b"test");
        amf0_multi.extend_from_slice(&[0x05]); // null
        amf0_multi.extend_from_slice(&[0x00, 0x00, 0x09]); // object end (0x00 0x00 0x09)

        let mut amf_reader = Amf0Decoder::new(&amf0_multi);
        let (values, error) = amf_reader.decode_all();

        assert_eq!(values.len(), 4);
        assert!(error.is_none());

        assert_eq!(values[0], Amf0Value::Number(772.161));
        assert_eq!(values[1], Amf0Value::Boolean(true));
        assert_eq!(values[2], Amf0Value::String(Cow::Borrowed("Hello World")));
        assert_eq!(
            values[3],
            Amf0Value::Object(vec![("test".into(), Amf0Value::Null)].into())
        );
    }

    #[test]
    fn test_decode_all_with_error() {
        let mut amf0_data = vec![0x00]; // Number marker
        amf0_data.extend_from_slice(&772.161_f64.to_be_bytes());
        amf0_data.extend_from_slice(&[0x01, 0x01]); // Boolean true
        amf0_data.push(0xFF); // Invalid marker

        let mut amf_reader = Amf0Decoder::new(&amf0_data);
        let (values, error) = amf_reader.decode_all();

        assert_eq!(values.len(), 2);
        assert!(error.is_some());
        assert!(matches!(error, Some(Amf0ReadError::UnknownMarker(0xFF))));

        assert_eq!(values[0], Amf0Value::Number(772.161));
        assert_eq!(values[1], Amf0Value::Boolean(true));
    }

    #[test]
    fn test_reader_iterator() {
        let mut amf0_multi = vec![0x00];
        amf0_multi.extend_from_slice(&772.161_f64.to_be_bytes());
        amf0_multi.extend_from_slice(&[0x01, 0x01]); // true
        amf0_multi.extend_from_slice(&[0x02, 0x00, 0x0b]); // 11 bytes
        amf0_multi.extend_from_slice(b"Hello World");

        let amf_reader = Amf0Decoder::new(&amf0_multi);
        let values = amf_reader.collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(values.len(), 3);

        assert_eq!(values[0], Amf0Value::Number(772.161));
        assert_eq!(values[1], Amf0Value::Boolean(true));
        assert_eq!(values[2], Amf0Value::String(Cow::Borrowed("Hello World")));
    }

    #[test]
    fn test_reader_invalid_marker() {
        let amf0_unsupported_marker = vec![Amf0Marker::Unsupported as u8];
        let mut amf_reader = Amf0Decoder::new(&amf0_unsupported_marker);
        let result = amf_reader.decode();

        assert!(matches!(
            result,
            Err(Amf0ReadError::UnsupportedType(Amf0Marker::Unsupported))
        ));
    }

    #[test]
    fn test_truncated_input_returns_error() {
        // Truncated number (marker + only 3 bytes of 8-byte f64)
        let truncated = vec![0x00, 0x40, 0x59, 0x00];
        let mut reader = Amf0Decoder::new(&truncated);
        let result = reader.decode();
        assert!(matches!(result, Err(Amf0ReadError::Io(_))));

        // Empty input
        let empty: Vec<u8> = vec![];
        let reader = Amf0Decoder::new(&empty);
        assert!(reader.is_empty());

        // Truncated string (claims 11 bytes but only has 3)
        let truncated_str = vec![0x02, 0x00, 0x0b, b'H', b'e', b'l'];
        let mut reader = Amf0Decoder::new(&truncated_str);
        let result = reader.decode();
        assert!(matches!(result, Err(Amf0ReadError::Io(_))));
    }

    #[test]
    fn test_date_round_trip() {
        use crate::Amf0Encoder;

        let mut buf = Vec::new();
        Amf0Encoder::encode_date(&mut buf, 1234567890.0, -300).unwrap();

        let mut decoder = Amf0Decoder::new(&buf);
        let value = decoder.decode().unwrap();
        assert_eq!(
            value,
            Amf0Value::Date {
                timestamp: 1234567890.0,
                timezone: -300,
            }
        );
        assert!(decoder.is_empty());
    }

    #[test]
    fn test_ecma_array_round_trip() {
        use crate::Amf0Encoder;

        let props: Vec<(Cow<str>, Amf0Value)> = vec![
            ("duration".into(), Amf0Value::Number(120.5)),
            ("width".into(), Amf0Value::Number(1920.0)),
        ];

        let mut buf = Vec::new();
        Amf0Encoder::encode_ecma_array(&mut buf, &props).unwrap();

        let mut decoder = Amf0Decoder::new(&buf);
        let value = decoder.decode().unwrap();

        assert_eq!(
            value,
            Amf0Value::EcmaArray(
                vec![
                    ("duration".into(), Amf0Value::Number(120.5)),
                    ("width".into(), Amf0Value::Number(1920.0)),
                ]
                .into()
            )
        );
        assert!(decoder.is_empty());
    }

    #[test]
    fn test_decode_all_lossy_skips_unknown_marker() {
        // Number(1.0), invalid byte, Boolean(true)
        let mut data = vec![0x00]; // Number marker
        data.extend_from_slice(&1.0_f64.to_be_bytes());
        data.push(0xFF); // Invalid marker
        data.extend_from_slice(&[0x01, 0x01]); // Boolean true

        let mut decoder = Amf0Decoder::new(&data);
        let result = decoder.decode_all_lossy(64);

        assert_eq!(result.values.len(), 2);
        assert_eq!(result.values[0], Amf0Value::Number(1.0));
        assert_eq!(result.values[1], Amf0Value::Boolean(true));
        assert_eq!(result.bytes_skipped, 1);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_decode_all_lossy_skips_multiple_bad_bytes() {
        // Number(1.0), 3 invalid bytes, String("hi")
        let mut data = vec![0x00]; // Number marker
        data.extend_from_slice(&1.0_f64.to_be_bytes());
        data.extend_from_slice(&[0xFF, 0xFE, 0xFD]); // 3 invalid markers
        data.extend_from_slice(&[0x02, 0x00, 0x02]); // String marker, length 2
        data.extend_from_slice(b"hi");

        let mut decoder = Amf0Decoder::new(&data);
        let result = decoder.decode_all_lossy(64);

        assert_eq!(result.values.len(), 2);
        assert_eq!(result.values[0], Amf0Value::Number(1.0));
        assert_eq!(result.values[1], Amf0Value::String(Cow::Borrowed("hi")));
        assert_eq!(result.bytes_skipped, 3);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_decode_all_lossy_all_garbage() {
        let data = vec![0xFF, 0xFE, 0xFD];

        let mut decoder = Amf0Decoder::new(&data);
        let result = decoder.decode_all_lossy(64);

        assert!(result.values.is_empty());
        assert_eq!(result.bytes_skipped, 3);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_decode_all_lossy_exceeds_skip_budget() {
        // 5 invalid bytes with budget of 2
        let data = vec![0xFF, 0xFE, 0xFD, 0xFC, 0xFB];

        let mut decoder = Amf0Decoder::new(&data);
        let result = decoder.decode_all_lossy(2);

        assert!(result.values.is_empty());
        assert_eq!(result.bytes_skipped, 2);
        assert!(result.error.is_some());
        assert!(matches!(
            result.error,
            Some(Amf0ReadError::UnknownMarker(_))
        ));
    }

    #[test]
    fn test_decode_all_lossy_resets_skip_counter() {
        // bad byte, Boolean(true), 2 bad bytes, Number(42.0)
        // with budget of 2 — should succeed because counter resets after Boolean
        let mut data = vec![0xFF]; // 1 bad byte
        data.extend_from_slice(&[0x01, 0x01]); // Boolean true
        data.extend_from_slice(&[0xFE, 0xFD]); // 2 bad bytes
        data.push(0x00); // Number marker
        data.extend_from_slice(&42.0_f64.to_be_bytes());

        let mut decoder = Amf0Decoder::new(&data);
        let result = decoder.decode_all_lossy(2);

        assert_eq!(result.values.len(), 2);
        assert_eq!(result.values[0], Amf0Value::Boolean(true));
        assert_eq!(result.values[1], Amf0Value::Number(42.0));
        assert_eq!(result.bytes_skipped, 3);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_decode_all_lossy_skips_unsupported_type() {
        // UnsupportedType is a valid marker (0x0d) but not handled by decode()
        // The lossy decoder should skip past it
        let mut data = vec![0x01, 0x01]; // Boolean true
        data.push(0x0d); // Unsupported marker (valid AMF0 marker, but decode returns UnsupportedType)
        data.push(0x05); // Null

        let mut decoder = Amf0Decoder::new(&data);
        let result = decoder.decode_all_lossy(64);

        assert_eq!(result.values.len(), 2);
        assert_eq!(result.values[0], Amf0Value::Boolean(true));
        assert_eq!(result.values[1], Amf0Value::Null);
        assert_eq!(result.bytes_skipped, 1);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_decode_all_lossy_stops_on_io_error() {
        // Truncated number: marker present but not enough bytes for f64 body
        // This is a non-recoverable IO error — lossy decoder should stop
        let mut data = vec![0x01, 0x01]; // Boolean true
        data.push(0x00); // Number marker
        data.extend_from_slice(&[0x40, 0x59]); // Only 2 of 8 bytes for f64

        let mut decoder = Amf0Decoder::new(&data);
        let result = decoder.decode_all_lossy(64);

        assert_eq!(result.values.len(), 1);
        assert_eq!(result.values[0], Amf0Value::Boolean(true));
        assert_eq!(result.bytes_skipped, 0);
        assert!(result.error.is_some());
        assert!(matches!(result.error, Some(Amf0ReadError::Io(_))));
    }
}
