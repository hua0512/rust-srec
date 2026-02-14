use std::borrow::Cow;

/// AMF0 marker types.
/// Defined in amf0_spec_121207.pdf section 2.1
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[repr(u8)]
pub enum Amf0Marker {
    /// number-marker
    Number = 0x00,
    /// boolean-marker
    Boolean = 0x01,
    /// string-marker
    String = 0x02,
    /// object-marker
    Object = 0x03,
    /// movieclip-marker
    ///
    /// reserved, not supported
    MovieClipMarker = 0x04,
    /// null-marker
    Null = 0x05,
    /// undefined-marker
    Undefined = 0x06,
    /// reference-marker
    Reference = 0x07,
    /// ecma-array-marker
    EcmaArray = 0x08,
    /// object-end-marker
    ObjectEnd = 0x09,
    /// strict-array-marker
    StrictArray = 0x0a,
    /// date-marker
    Date = 0x0b,
    /// long-string-marker
    LongString = 0x0c,
    /// unsupported-marker
    Unsupported = 0x0d,
    /// recordset-marker
    ///
    /// reserved, not supported
    Recordset = 0x0e,
    /// xml-document-marker
    XmlDocument = 0x0f,
    /// typed-object-marker
    TypedObject = 0x10,
    /// avmplus-object-marker
    ///
    /// AMF3 marker
    AVMPlusObject = 0x11,
}

impl TryFrom<u8> for Amf0Marker {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, u8> {
        match value {
            0x00 => Ok(Self::Number),
            0x01 => Ok(Self::Boolean),
            0x02 => Ok(Self::String),
            0x03 => Ok(Self::Object),
            0x04 => Ok(Self::MovieClipMarker),
            0x05 => Ok(Self::Null),
            0x06 => Ok(Self::Undefined),
            0x07 => Ok(Self::Reference),
            0x08 => Ok(Self::EcmaArray),
            0x09 => Ok(Self::ObjectEnd),
            0x0a => Ok(Self::StrictArray),
            0x0b => Ok(Self::Date),
            0x0c => Ok(Self::LongString),
            0x0d => Ok(Self::Unsupported),
            0x0e => Ok(Self::Recordset),
            0x0f => Ok(Self::XmlDocument),
            0x10 => Ok(Self::TypedObject),
            0x11 => Ok(Self::AVMPlusObject),
            other => Err(other),
        }
    }
}

impl Amf0Marker {
    /// Check if a u24 value represents the object-end marker (0x000009).
    pub fn is_object_end_u24(value: u32) -> bool {
        value == 0x000009
    }
}

/// AMF0 value types.
/// Defined in amf0_spec_121207.pdf section 2.2-2.14
#[derive(PartialEq, Clone, Debug)]
pub enum Amf0Value<'a> {
    /// Number Type defined section 2.2
    Number(f64),
    /// Boolean Type defined section 2.3
    Boolean(bool),
    /// String Type defined section 2.4
    String(Cow<'a, str>),
    /// Object Type defined section 2.5
    Object(Cow<'a, [(Cow<'a, str>, Amf0Value<'a>)]>),
    /// Null Type defined section 2.7
    Null,
    /// Undefined Type defined section 2.8
    Undefined,
    /// EcmaArray Type defined section 2.10
    EcmaArray(Cow<'a, [(Cow<'a, str>, Amf0Value<'a>)]>),
    /// StrictArray Type defined section 2.12
    StrictArray(Cow<'a, [Amf0Value<'a>]>),
    /// Date Type defined section 2.13
    Date {
        /// Timestamp in milliseconds since Unix epoch
        timestamp: f64,
        /// Timezone offset in minutes
        timezone: i16,
    },
    /// LongString Type defined section 2.14
    LongString(Cow<'a, str>),
}

impl<'a> Amf0Value<'a> {
    /// Get the marker of the value.
    #[inline]
    pub fn marker(&self) -> Amf0Marker {
        match self {
            Self::Number(_) => Amf0Marker::Number,
            Self::Boolean(_) => Amf0Marker::Boolean,
            Self::String(_) => Amf0Marker::String,
            Self::Object(_) => Amf0Marker::Object,
            Self::Null => Amf0Marker::Null,
            Self::Undefined => Amf0Marker::Undefined,
            Self::EcmaArray(_) => Amf0Marker::EcmaArray,
            Self::StrictArray(_) => Amf0Marker::StrictArray,
            Self::Date { .. } => Amf0Marker::Date,
            Self::LongString(_) => Amf0Marker::LongString,
        }
    }

    /// Convert borrowed value to an owned value with `'static` lifetime.
    ///
    /// Named `into_owned` to be consistent with [`Cow::into_owned`] and
    /// to avoid shadowing [`std::borrow::ToOwned`].
    #[inline]
    pub fn into_owned(&self) -> Amf0Value<'static> {
        match self {
            Self::Number(n) => Amf0Value::Number(*n),
            Self::Boolean(b) => Amf0Value::Boolean(*b),
            Self::String(s) => Amf0Value::String(Cow::Owned(s.to_string())),
            Self::LongString(s) => Amf0Value::LongString(Cow::Owned(s.to_string())),
            Self::Object(o) => Amf0Value::Object(
                o.iter()
                    .map(|(k, v)| (Cow::Owned(k.to_string()), v.into_owned()))
                    .collect(),
            ),
            Self::EcmaArray(o) => Amf0Value::EcmaArray(
                o.iter()
                    .map(|(k, v)| (Cow::Owned(k.to_string()), v.into_owned()))
                    .collect(),
            ),
            Self::StrictArray(a) => {
                Amf0Value::StrictArray(a.iter().map(|v| v.into_owned()).collect())
            }
            Self::Date {
                timestamp,
                timezone,
            } => Amf0Value::Date {
                timestamp: *timestamp,
                timezone: *timezone,
            },
            Self::Null => Amf0Value::Null,
            Self::Undefined => Amf0Value::Undefined,
        }
    }

    /// Returns the inner `f64` if this is a `Number`, or `None` otherwise.
    #[inline]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Returns the inner `bool` if this is a `Boolean`, or `None` otherwise.
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns the inner string slice if this is a `String` or `LongString`,
    /// or `None` otherwise.
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) | Self::LongString(s) => Some(s),
            _ => None,
        }
    }

    /// Returns the inner property slice if this is an `Object` or `EcmaArray`,
    /// or `None` otherwise.
    ///
    /// This is useful for consumers that don't need to distinguish between
    /// the two wire formats.
    #[inline]
    pub fn as_object_properties(&self) -> Option<&[(Cow<'a, str>, Amf0Value<'a>)]> {
        match self {
            Self::Object(o) | Self::EcmaArray(o) => Some(o),
            _ => None,
        }
    }

    /// Returns the inner value slice if this is a `StrictArray`,
    /// or `None` otherwise.
    #[inline]
    pub fn as_array(&self) -> Option<&[Amf0Value<'a>]> {
        match self {
            Self::StrictArray(a) => Some(a),
            _ => None,
        }
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_marker() {
        let cases = [
            (Amf0Value::Number(1.0), Amf0Marker::Number),
            (Amf0Value::Boolean(true), Amf0Marker::Boolean),
            (Amf0Value::String(Cow::Borrowed("test")), Amf0Marker::String),
            (
                Amf0Value::Object(Cow::Borrowed(&[(
                    Cow::Borrowed("test"),
                    Amf0Value::Number(1.0),
                )])),
                Amf0Marker::Object,
            ),
            (Amf0Value::Null, Amf0Marker::Null),
            (Amf0Value::Undefined, Amf0Marker::Undefined),
            (
                Amf0Value::EcmaArray(Cow::Borrowed(&[(Cow::Borrowed("key"), Amf0Value::Null)])),
                Amf0Marker::EcmaArray,
            ),
            (
                Amf0Value::LongString(Cow::Borrowed("test")),
                Amf0Marker::LongString,
            ),
            (
                Amf0Value::StrictArray(Cow::Borrowed(&[Amf0Value::Number(1.0)])),
                Amf0Marker::StrictArray,
            ),
            (
                Amf0Value::Date {
                    timestamp: 1000.0,
                    timezone: 0,
                },
                Amf0Marker::Date,
            ),
        ];

        for (value, marker) in cases {
            assert_eq!(value.marker(), marker);
        }
    }

    #[test]
    fn test_into_owned() {
        let value = Amf0Value::Object(Cow::Borrowed(&[(
            Cow::Borrowed("test"),
            Amf0Value::LongString(Cow::Borrowed("test")),
        )]));
        let owned = value.into_owned();
        assert_eq!(
            owned,
            Amf0Value::Object(Cow::Owned(vec![(
                "test".to_string().into(),
                Amf0Value::LongString(Cow::Owned("test".to_string()))
            )]))
        );

        let value = Amf0Value::String(Cow::Borrowed("test"));
        let owned = value.into_owned();
        assert_eq!(owned, Amf0Value::String(Cow::Owned("test".to_string())));

        let value = Amf0Value::Number(1.0);
        let owned = value.into_owned();
        assert_eq!(owned, Amf0Value::Number(1.0));

        let value = Amf0Value::Boolean(true);
        let owned = value.into_owned();
        assert_eq!(owned, Amf0Value::Boolean(true));

        let value = Amf0Value::Null;
        let owned = value.into_owned();
        assert_eq!(owned, Amf0Value::Null);

        let value = Amf0Value::Undefined;
        let owned = value.into_owned();
        assert_eq!(owned, Amf0Value::Undefined);

        let value = Amf0Value::StrictArray(Cow::Borrowed(&[
            Amf0Value::Number(1.0),
            Amf0Value::String(Cow::Borrowed("test")),
        ]));
        let owned = value.into_owned();
        assert_eq!(
            owned,
            Amf0Value::StrictArray(Cow::Owned(vec![
                Amf0Value::Number(1.0),
                Amf0Value::String(Cow::Owned("test".to_string()))
            ]))
        );

        let value = Amf0Value::EcmaArray(Cow::Borrowed(&[(
            Cow::Borrowed("key"),
            Amf0Value::Number(42.0),
        )]));
        let owned = value.into_owned();
        assert_eq!(
            owned,
            Amf0Value::EcmaArray(Cow::Owned(vec![(
                Cow::Owned("key".to_string()),
                Amf0Value::Number(42.0),
            )]))
        );

        let value = Amf0Value::Date {
            timestamp: 1234567890.0,
            timezone: -120,
        };
        let owned = value.into_owned();
        assert_eq!(
            owned,
            Amf0Value::Date {
                timestamp: 1234567890.0,
                timezone: -120,
            }
        );
    }

    #[test]
    fn test_marker_try_from() {
        let cases = [
            (Amf0Marker::Number, 0x00),
            (Amf0Marker::Boolean, 0x01),
            (Amf0Marker::String, 0x02),
            (Amf0Marker::Object, 0x03),
            (Amf0Marker::MovieClipMarker, 0x04),
            (Amf0Marker::Null, 0x05),
            (Amf0Marker::Undefined, 0x06),
            (Amf0Marker::Reference, 0x07),
            (Amf0Marker::EcmaArray, 0x08),
            (Amf0Marker::ObjectEnd, 0x09),
            (Amf0Marker::StrictArray, 0x0a),
            (Amf0Marker::Date, 0x0b),
            (Amf0Marker::LongString, 0x0c),
            (Amf0Marker::Unsupported, 0x0d),
            (Amf0Marker::Recordset, 0x0e),
            (Amf0Marker::XmlDocument, 0x0f),
            (Amf0Marker::TypedObject, 0x10),
            (Amf0Marker::AVMPlusObject, 0x11),
        ];

        for (marker, value) in cases {
            assert_eq!(marker as u8, value);
            assert_eq!(Amf0Marker::try_from(value), Ok(marker));
        }

        assert_eq!(Amf0Marker::try_from(0x12), Err(0x12));
        assert_eq!(Amf0Marker::try_from(0xFF), Err(0xFF));
    }

    #[test]
    fn test_accessors() {
        assert_eq!(Amf0Value::Number(42.0).as_number(), Some(42.0));
        assert_eq!(Amf0Value::Boolean(true).as_number(), None);

        assert_eq!(Amf0Value::Boolean(true).as_bool(), Some(true));
        assert_eq!(Amf0Value::Number(1.0).as_bool(), None);

        assert_eq!(
            Amf0Value::String(Cow::Borrowed("hello")).as_str(),
            Some("hello")
        );
        assert_eq!(
            Amf0Value::LongString(Cow::Borrowed("world")).as_str(),
            Some("world")
        );
        assert_eq!(Amf0Value::Number(1.0).as_str(), None);

        let obj = Amf0Value::Object(Cow::Borrowed(&[(Cow::Borrowed("k"), Amf0Value::Null)]));
        assert!(obj.as_object_properties().is_some());

        let ecma = Amf0Value::EcmaArray(Cow::Borrowed(&[(Cow::Borrowed("k"), Amf0Value::Null)]));
        assert!(ecma.as_object_properties().is_some());
        assert!(Amf0Value::Null.as_object_properties().is_none());

        let arr = Amf0Value::StrictArray(Cow::Borrowed(&[Amf0Value::Number(1.0)]));
        assert!(arr.as_array().is_some());
        assert!(Amf0Value::Null.as_array().is_none());
    }

    #[test]
    fn test_is_object_end_u24() {
        assert!(Amf0Marker::is_object_end_u24(0x000009));
        assert!(!Amf0Marker::is_object_end_u24(0x000000));
        assert!(!Amf0Marker::is_object_end_u24(0x000109));
    }
}
