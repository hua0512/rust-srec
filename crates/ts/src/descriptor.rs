use bytes::{Buf, Bytes};

/// Registration descriptor (tag 0x05)
pub const TAG_REGISTRATION: u8 = 0x05;
/// ISO 639 language descriptor (tag 0x0A)
pub const TAG_ISO_639_LANGUAGE: u8 = 0x0A;
/// AC-3 audio descriptor (tag 0x6A)
pub const TAG_AC3: u8 = 0x6A;
/// Enhanced AC-3 audio descriptor (tag 0x7A)
pub const TAG_EAC3: u8 = 0x7A;
/// DTS audio descriptor (tag 0x7B)
pub const TAG_DTS: u8 = 0x7B;
/// AAC audio descriptor (tag 0x7C)
pub const TAG_AAC: u8 = 0x7C;
/// Subtitling descriptor (tag 0x59)
pub const TAG_SUBTITLING: u8 = 0x59;

/// Zero-copy descriptor reference.
#[derive(Debug, Clone)]
pub struct DescriptorRef {
    pub tag: u8,
    pub data: Bytes,
}

/// Iterator over descriptors in a TLV descriptor loop.
///
/// Each descriptor is `[tag: u8][length: u8][data: length bytes]`.
#[derive(Debug, Clone)]
pub struct DescriptorIterator {
    data: Bytes,
}

impl DescriptorIterator {
    /// Create a new descriptor iterator from a descriptor loop byte sequence.
    pub fn new(data: Bytes) -> Self {
        DescriptorIterator { data }
    }
}

impl Iterator for DescriptorIterator {
    type Item = DescriptorRef;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.remaining() < 2 {
            return None;
        }
        let tag = self.data[0];
        let length = self.data[1] as usize;
        self.data.advance(2);

        if self.data.remaining() < length {
            // Malformed descriptor — consume remaining and stop
            self.data.advance(self.data.remaining());
            return None;
        }

        let data = self.data.split_to(length);
        Some(DescriptorRef { tag, data })
    }
}

/// Parse a registration descriptor (tag 0x05).
///
/// Returns the 4-byte format_identifier if the descriptor data is at least 4 bytes.
pub fn parse_registration_descriptor(data: &[u8]) -> Option<[u8; 4]> {
    if data.len() < 4 {
        return None;
    }
    Some([data[0], data[1], data[2], data[3]])
}

/// A single ISO 639 language entry.
#[derive(Debug, Clone)]
pub struct LanguageEntry {
    /// 3-character ISO 639-2/T language code (e.g., b"eng", b"fra")
    pub language_code: [u8; 3],
    /// Audio type: 0=undefined, 1=clean effects, 2=hearing impaired, 3=visual impaired commentary
    pub audio_type: u8,
}

/// Parse ISO 639 language descriptor (tag 0x0A).
///
/// Returns a list of (language_code, audio_type) entries.
pub fn parse_iso639_language(data: &[u8]) -> Vec<LanguageEntry> {
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset + 4 <= data.len() {
        entries.push(LanguageEntry {
            language_code: [data[offset], data[offset + 1], data[offset + 2]],
            audio_type: data[offset + 3],
        });
        offset += 4;
    }
    entries
}

/// Parsed AC-3 audio descriptor.
#[derive(Debug, Clone)]
pub struct Ac3Descriptor {
    pub component_type_flag: bool,
    pub bsid_flag: bool,
    pub mainid_flag: bool,
    pub asvc_flag: bool,
    pub component_type: Option<u8>,
    pub bsid: Option<u8>,
    pub mainid: Option<u8>,
    pub asvc: Option<u8>,
}

/// Parse AC-3 descriptor (tag 0x6A).
pub fn parse_ac3_descriptor(data: &[u8]) -> Option<Ac3Descriptor> {
    if data.is_empty() {
        return None;
    }

    let flags = data[0];
    let component_type_flag = (flags & 0x80) != 0;
    let bsid_flag = (flags & 0x40) != 0;
    let mainid_flag = (flags & 0x20) != 0;
    let asvc_flag = (flags & 0x10) != 0;

    let mut offset = 1;

    let component_type = if component_type_flag && offset < data.len() {
        let val = data[offset];
        offset += 1;
        Some(val)
    } else {
        None
    };

    let bsid = if bsid_flag && offset < data.len() {
        let val = data[offset];
        offset += 1;
        Some(val)
    } else {
        None
    };

    let mainid = if mainid_flag && offset < data.len() {
        let val = data[offset];
        offset += 1;
        Some(val)
    } else {
        None
    };

    let asvc = if asvc_flag && offset < data.len() {
        let val = data[offset];
        Some(val)
    } else {
        None
    };

    Some(Ac3Descriptor {
        component_type_flag,
        bsid_flag,
        mainid_flag,
        asvc_flag,
        component_type,
        bsid,
        mainid,
        asvc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_descriptor_iterator_empty() {
        let iter = DescriptorIterator::new(Bytes::new());
        assert_eq!(iter.count(), 0);
    }

    #[test]
    fn test_descriptor_iterator_single() {
        // One descriptor: tag=0x05, length=4, data="CUEI"
        let data = Bytes::from_static(&[0x05, 0x04, b'C', b'U', b'E', b'I']);
        let descriptors: Vec<_> = DescriptorIterator::new(data).collect();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].tag, TAG_REGISTRATION);
        assert_eq!(&descriptors[0].data[..], b"CUEI");
    }

    #[test]
    fn test_descriptor_iterator_multiple() {
        let mut data = Vec::new();
        // Registration descriptor
        data.extend_from_slice(&[0x05, 0x04, b'C', b'U', b'E', b'I']);
        // ISO 639 language descriptor
        data.extend_from_slice(&[0x0A, 0x04, b'e', b'n', b'g', 0x00]);
        let descriptors: Vec<_> = DescriptorIterator::new(Bytes::from(data)).collect();
        assert_eq!(descriptors.len(), 2);
        assert_eq!(descriptors[0].tag, TAG_REGISTRATION);
        assert_eq!(descriptors[1].tag, TAG_ISO_639_LANGUAGE);
    }

    #[test]
    fn test_descriptor_iterator_malformed() {
        // Tag + length that exceeds remaining data
        let data = Bytes::from_static(&[0x05, 0xFF]);
        let descriptors: Vec<_> = DescriptorIterator::new(data).collect();
        assert_eq!(descriptors.len(), 0);
    }

    #[test]
    fn test_parse_registration_descriptor() {
        let data = b"CUEI";
        let id = parse_registration_descriptor(data).unwrap();
        assert_eq!(&id, b"CUEI");
    }

    #[test]
    fn test_parse_registration_descriptor_too_short() {
        assert!(parse_registration_descriptor(&[0x01, 0x02]).is_none());
    }

    #[test]
    fn test_parse_iso639_language() {
        let data = [b'e', b'n', b'g', 0x00, b'f', b'r', b'a', 0x01];
        let entries = parse_iso639_language(&data);
        assert_eq!(entries.len(), 2);
        assert_eq!(&entries[0].language_code, b"eng");
        assert_eq!(entries[0].audio_type, 0);
        assert_eq!(&entries[1].language_code, b"fra");
        assert_eq!(entries[1].audio_type, 1);
    }

    #[test]
    fn test_parse_iso639_language_empty() {
        let entries = parse_iso639_language(&[]);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_ac3_descriptor() {
        // flags: component_type=1, bsid=1, mainid=0, asvc=0
        // component_type=0x48, bsid=0x08
        let data = [0xC0, 0x48, 0x08];
        let desc = parse_ac3_descriptor(&data).unwrap();
        assert!(desc.component_type_flag);
        assert!(desc.bsid_flag);
        assert!(!desc.mainid_flag);
        assert!(!desc.asvc_flag);
        assert_eq!(desc.component_type, Some(0x48));
        assert_eq!(desc.bsid, Some(0x08));
        assert!(desc.mainid.is_none());
        assert!(desc.asvc.is_none());
    }

    #[test]
    fn test_parse_ac3_descriptor_empty() {
        assert!(parse_ac3_descriptor(&[]).is_none());
    }
}
