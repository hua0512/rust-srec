use std::borrow::Cow;
use std::io;

use byteorder::{BigEndian, WriteBytesExt};

use super::define::Amf0Marker;
use super::{Amf0Value, Amf0WriteError};

/// A macro to encode an AMF property key into a buffer
#[macro_export]
macro_rules! write_amf_property_key {
    ($buffer:expr, $key:expr) => {
        // write key length (u16 in big endian)
        $buffer.write_u16::<BigEndian>($key.len() as u16)?;
        // write key string bytes
        $buffer.write_all($key.as_bytes())?;
    };
}

/// AMF0 encoder.
///
/// Allows for encoding an AMF0 to some writer.
pub struct Amf0Encoder;

impl Amf0Encoder {
    /// Encode a generic AMF0 value
    pub fn encode(writer: &mut impl io::Write, value: &Amf0Value) -> Result<(), Amf0WriteError> {
        match value {
            Amf0Value::Boolean(val) => Self::encode_bool(writer, *val),
            Amf0Value::Null => Self::encode_null(writer),
            Amf0Value::Number(val) => Self::encode_number(writer, *val),
            Amf0Value::String(val) => Self::encode_string(writer, val),
            Amf0Value::Object(val) => Self::encode_object(writer, val),
            Amf0Value::StrictArray(val) => Self::encode_strict_array(writer, val),
            _ => Err(Amf0WriteError::UnsupportedType(value.marker())),
        }
    }

    /// Write object end marker to signify the end of an AMF0 object
    pub fn object_eof(writer: &mut impl io::Write) -> Result<(), Amf0WriteError> {
        writer.write_u24::<BigEndian>(Amf0Marker::ObjectEnd as u32)?;
        Ok(())
    }

    /// Encode an AMF0 number
    pub fn encode_number(writer: &mut impl io::Write, value: f64) -> Result<(), Amf0WriteError> {
        writer.write_u8(Amf0Marker::Number as u8)?;
        writer.write_f64::<BigEndian>(value)?;
        Ok(())
    }

    /// Encode an AMF0 boolean
    pub fn encode_bool(writer: &mut impl io::Write, value: bool) -> Result<(), Amf0WriteError> {
        writer.write_u8(Amf0Marker::Boolean as u8)?;
        writer.write_u8(value as u8)?;
        Ok(())
    }

    /// Encode an AMF0 string
    pub fn encode_string(writer: &mut impl io::Write, value: &str) -> Result<(), Amf0WriteError> {
        if value.len() > (u16::MAX as usize) {
            return Err(Amf0WriteError::NormalStringTooLong);
        }

        writer.write_u8(Amf0Marker::String as u8)?;
        write_amf_property_key!(writer, value);
        Ok(())
    }

    /// Encode an AMF0 null
    pub fn encode_null(writer: &mut impl io::Write) -> Result<(), Amf0WriteError> {
        writer.write_u8(Amf0Marker::Null as u8)?;
        Ok(())
    }

    /// Encode an AMF0 object
    pub fn encode_object(
        writer: &mut impl io::Write,
        properties: &[(Cow<'_, str>, Amf0Value<'_>)],
    ) -> Result<(), Amf0WriteError> {
        writer.write_u8(Amf0Marker::Object as u8)?;
        for (key, value) in properties {
            write_amf_property_key!(writer, key);
            Self::encode(writer, value)?;
        }

        Self::object_eof(writer)?;
        Ok(())
    }

    /// Encode an AMF0 strict array
    pub fn encode_strict_array(
        writer: &mut impl io::Write,
        values: &[Amf0Value<'_>],
    ) -> Result<(), Amf0WriteError> {
        writer.write_u8(Amf0Marker::StrictArray as u8)?;
        writer.write_u32::<BigEndian>(values.len() as u32)?;
        for value in values {
            Self::encode(writer, value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_write_number() {
        let mut amf0_number = vec![0x00];
        amf0_number.extend_from_slice(&772.161_f64.to_be_bytes());

        let mut vec = Vec::<u8>::new();

        Amf0Encoder::encode_number(&mut vec, 772.161).unwrap();

        assert_eq!(vec, amf0_number);
    }

    #[test]
    fn test_write_boolean() {
        let amf0_boolean = vec![0x01, 0x01];

        let mut vec = Vec::<u8>::new();

        Amf0Encoder::encode_bool(&mut vec, true).unwrap();

        assert_eq!(vec, amf0_boolean);
    }

    #[test]
    fn test_write_string() {
        let mut amf0_string = vec![0x02, 0x00, 0x0b];
        amf0_string.extend_from_slice(b"Hello World");

        let mut vec = Vec::<u8>::new();

        Amf0Encoder::encode_string(&mut vec, "Hello World").unwrap();

        assert_eq!(vec, amf0_string);
    }

    #[test]
    fn test_write_null() {
        let amf0_null = vec![0x05];

        let mut vec = Vec::<u8>::new();

        Amf0Encoder::encode_null(&mut vec).unwrap();

        assert_eq!(vec, amf0_null);
    }

    #[test]
    fn test_write_object() {
        let mut amf0_object = vec![0x03, 0x00, 0x04];
        amf0_object.extend_from_slice(b"test");
        amf0_object.extend_from_slice(&[0x05]);
        amf0_object.extend_from_slice(&[0x00, 0x00, 0x09]);

        let mut vec = Vec::<u8>::new();

        Amf0Encoder::encode_object(&mut vec, &[("test".into(), Amf0Value::Null)]).unwrap();

        assert_eq!(vec, amf0_object);
    }

    #[test]
    fn test_encode_boolean() {
        let amf0_boolean_true = vec![Amf0Marker::Boolean as u8, 0x01];
        let amf0_boolean_false = vec![Amf0Marker::Boolean as u8, 0x00];

        let mut vec_true = Vec::<u8>::new();
        let mut vec_false = Vec::<u8>::new();

        Amf0Encoder::encode(&mut vec_true, &Amf0Value::Boolean(true)).unwrap();
        assert_eq!(vec_true, amf0_boolean_true);
        Amf0Encoder::encode(&mut vec_false, &Amf0Value::Boolean(false)).unwrap();
        assert_eq!(vec_false, amf0_boolean_false);
    }

    #[test]
    fn test_encode_object() {
        let mut amf0_object = vec![Amf0Marker::Object as u8, 0x00, 0x04];
        amf0_object.extend_from_slice(b"test");
        amf0_object.push(Amf0Marker::Null as u8);
        amf0_object.extend_from_slice(&[0x00, 0x00, 0x09]);
        let mut vec = Vec::<u8>::new();

        Amf0Encoder::encode(
            &mut vec,
            &Amf0Value::Object(vec![("test".into(), Amf0Value::Null)].into()),
        )
        .unwrap();
        assert_eq!(vec, amf0_object);
    }

    #[test]
    fn test_encode_generic_error_unsupported_type() {
        let mut writer = Vec::<u8>::new();
        let unsupported_marker = Amf0Value::ObjectEnd;
        let result = Amf0Encoder::encode(&mut writer, &unsupported_marker);
        assert!(matches!(result, Err(Amf0WriteError::UnsupportedType(_))));
    }

    #[test]
    fn test_encode_string_too_long() {
        let long_string = "a".repeat(u16::MAX as usize + 1);
        let mut writer = Vec::<u8>::new();
        let result = Amf0Encoder::encode_string(&mut writer, &long_string);
        assert!(matches!(result, Err(Amf0WriteError::NormalStringTooLong)));
    }

    #[test]
    fn test_encode_strict_array() {
        let mut amf0_array = vec![Amf0Marker::StrictArray as u8, 0x00, 0x00, 0x00, 0x03]; // StrictArray marker with 3 elements
        amf0_array.extend_from_slice(&[0x00]); // Number marker
        amf0_array.extend_from_slice(&1.0_f64.to_be_bytes());
        amf0_array.extend_from_slice(&[0x01, 0x01]); // Boolean true
        amf0_array.extend_from_slice(&[0x02, 0x00, 0x04]); // String with 4 bytes
        amf0_array.extend_from_slice(b"test");

        let mut vec = Vec::<u8>::new();

        Amf0Encoder::encode_strict_array(
            &mut vec,
            &[
                Amf0Value::Number(1.0),
                Amf0Value::Boolean(true),
                Amf0Value::String(Cow::Borrowed("test")),
            ],
        )
        .unwrap();

        assert_eq!(vec, amf0_array);
    }

    #[test]
    fn test_encode_generic_strict_array() {
        let mut amf0_array = vec![Amf0Marker::StrictArray as u8, 0x00, 0x00, 0x00, 0x03]; // StrictArray marker with 3 elements
        amf0_array.extend_from_slice(&[0x00]); // Number marker
        amf0_array.extend_from_slice(&1.0_f64.to_be_bytes());
        amf0_array.extend_from_slice(&[0x01, 0x01]); // Boolean true
        amf0_array.extend_from_slice(&[0x02, 0x00, 0x04]); // String with 4 bytes
        amf0_array.extend_from_slice(b"test");

        let mut vec = Vec::<u8>::new();

        Amf0Encoder::encode(
            &mut vec,
            &Amf0Value::StrictArray(
                vec![
                    Amf0Value::Number(1.0),
                    Amf0Value::Boolean(true),
                    Amf0Value::String(Cow::Borrowed("test")),
                ]
                .into(),
            ),
        )
        .unwrap();

        assert_eq!(vec, amf0_array);
    }
}
