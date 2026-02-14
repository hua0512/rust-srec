use bytes::Bytes;

/// Program Clock Reference (PCR) — 33-bit base @ 90kHz + 9-bit extension @ 27MHz
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pcr {
    /// 33-bit base value at 90 kHz
    pub base: u64,
    /// 9-bit extension value at 27 MHz
    pub extension: u16,
}

impl Pcr {
    /// Parse PCR from exactly 6 bytes.
    ///
    /// Layout: `[base32..25][base24..17][base16..9][base8..1][base0 | reserved(6) | ext_high][ext_low]`
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 6 {
            return None;
        }
        let base = ((data[0] as u64) << 25)
            | ((data[1] as u64) << 17)
            | ((data[2] as u64) << 9)
            | ((data[3] as u64) << 1)
            | ((data[4] as u64) >> 7);
        let extension = (((data[4] & 0x01) as u16) << 8) | data[5] as u16;
        Some(Pcr { base, extension })
    }

    /// Full PCR value at 27 MHz resolution.
    pub fn as_27mhz(&self) -> u64 {
        self.base * 300 + self.extension as u64
    }

    /// PCR as seconds (floating point).
    pub fn as_seconds(&self) -> f64 {
        self.as_27mhz() as f64 / 27_000_000.0
    }
}

/// Owned adaptation field with all parsed fields.
#[derive(Debug, Clone)]
pub struct AdaptationField {
    pub discontinuity_indicator: bool,
    pub random_access_indicator: bool,
    pub elementary_stream_priority_indicator: bool,
    pub pcr: Option<Pcr>,
    pub opcr: Option<Pcr>,
    pub splice_countdown: Option<i8>,
    pub transport_private_data: Option<Vec<u8>>,
}

impl AdaptationField {
    /// Parse an adaptation field from its data bytes (after the length byte).
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }

        let flags = data[0];
        let discontinuity_indicator = (flags & 0x80) != 0;
        let random_access_indicator = (flags & 0x40) != 0;
        let elementary_stream_priority_indicator = (flags & 0x20) != 0;
        let pcr_flag = (flags & 0x10) != 0;
        let opcr_flag = (flags & 0x08) != 0;
        let splicing_point_flag = (flags & 0x04) != 0;
        let transport_private_data_flag = (flags & 0x02) != 0;

        let mut offset = 1;

        let pcr = if pcr_flag && offset + 6 <= data.len() {
            let pcr = Pcr::parse(&data[offset..]);
            offset += 6;
            pcr
        } else {
            if pcr_flag {
                offset += 6;
            }
            None
        };

        let opcr = if opcr_flag && offset + 6 <= data.len() {
            let opcr = Pcr::parse(&data[offset..]);
            offset += 6;
            opcr
        } else {
            if opcr_flag {
                offset += 6;
            }
            None
        };

        let splice_countdown = if splicing_point_flag && offset < data.len() {
            let val = data[offset] as i8;
            offset += 1;
            Some(val)
        } else {
            if splicing_point_flag {
                offset += 1;
            }
            None
        };

        let transport_private_data = if transport_private_data_flag && offset < data.len() {
            let length = data[offset] as usize;
            offset += 1;
            if offset + length <= data.len() {
                let private_data = data[offset..offset + length].to_vec();
                Some(private_data)
            } else {
                None
            }
        } else {
            None
        };

        Some(AdaptationField {
            discontinuity_indicator,
            random_access_indicator,
            elementary_stream_priority_indicator,
            pcr,
            opcr,
            splice_countdown,
            transport_private_data,
        })
    }
}

/// Zero-copy adaptation field reference.
///
/// Flags are parsed eagerly; optional fields (PCR, OPCR, etc.) are parsed lazily.
#[derive(Debug, Clone)]
pub struct AdaptationFieldRef {
    data: Bytes,
    pub discontinuity_indicator: bool,
    pub random_access_indicator: bool,
    pub elementary_stream_priority_indicator: bool,
    pcr_flag: bool,
    opcr_flag: bool,
    splicing_point_flag: bool,
    transport_private_data_flag: bool,
}

impl AdaptationFieldRef {
    /// Parse an adaptation field reference from its data bytes (after the length byte).
    pub fn parse(data: Bytes) -> Option<Self> {
        if data.is_empty() {
            return None;
        }

        let flags = data[0];
        Some(AdaptationFieldRef {
            data,
            discontinuity_indicator: (flags & 0x80) != 0,
            random_access_indicator: (flags & 0x40) != 0,
            elementary_stream_priority_indicator: (flags & 0x20) != 0,
            pcr_flag: (flags & 0x10) != 0,
            opcr_flag: (flags & 0x08) != 0,
            splicing_point_flag: (flags & 0x04) != 0,
            transport_private_data_flag: (flags & 0x02) != 0,
        })
    }

    /// Get PCR if present. Parsed lazily from known offset.
    pub fn pcr(&self) -> Option<Pcr> {
        if !self.pcr_flag {
            return None;
        }
        // PCR starts at offset 1 (after flags byte)
        if self.data.len() < 7 {
            return None;
        }
        Pcr::parse(&self.data[1..])
    }

    /// Get OPCR if present.
    pub fn opcr(&self) -> Option<Pcr> {
        if !self.opcr_flag {
            return None;
        }
        let offset = 1 + if self.pcr_flag { 6 } else { 0 };
        if offset + 6 > self.data.len() {
            return None;
        }
        Pcr::parse(&self.data[offset..])
    }

    /// Get splice countdown if present.
    pub fn splice_countdown(&self) -> Option<i8> {
        if !self.splicing_point_flag {
            return None;
        }
        let offset = 1 + if self.pcr_flag { 6 } else { 0 } + if self.opcr_flag { 6 } else { 0 };
        if offset >= self.data.len() {
            return None;
        }
        Some(self.data[offset] as i8)
    }

    /// Get transport private data if present.
    pub fn transport_private_data(&self) -> Option<Bytes> {
        if !self.transport_private_data_flag {
            return None;
        }
        let offset = 1
            + if self.pcr_flag { 6 } else { 0 }
            + if self.opcr_flag { 6 } else { 0 }
            + if self.splicing_point_flag { 1 } else { 0 };
        if offset >= self.data.len() {
            return None;
        }
        let length = self.data[offset] as usize;
        let data_offset = offset + 1;
        if data_offset + length > self.data.len() {
            return None;
        }
        Some(self.data.slice(data_offset..data_offset + length))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pcr_parse() {
        // PCR base = 0, extension = 0
        let data = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let pcr = Pcr::parse(&data).unwrap();
        assert_eq!(pcr.base, 0);
        assert_eq!(pcr.extension, 0);
        assert_eq!(pcr.as_27mhz(), 0);
        assert_eq!(pcr.as_seconds(), 0.0);
    }

    #[test]
    fn test_pcr_parse_max() {
        // PCR base = max 33-bit (0x1FFFFFFFF), extension = max 9-bit (0x1FF)
        let data = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        let pcr = Pcr::parse(&data).unwrap();
        assert_eq!(pcr.base, 0x1_FFFF_FFFF);
        assert_eq!(pcr.extension, 0x1FF);
    }

    #[test]
    fn test_pcr_as_seconds() {
        // 90000 base ticks = 1 second (at 90kHz, with extension=0)
        let pcr = Pcr {
            base: 90_000,
            extension: 0,
        };
        let seconds = pcr.as_seconds();
        assert!((seconds - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_adaptation_field_flags_only() {
        // Flags byte with only RAI set, no optional fields
        let data = vec![0x40]; // random_access_indicator = true
        let af = AdaptationField::parse(&data).unwrap();
        assert!(!af.discontinuity_indicator);
        assert!(af.random_access_indicator);
        assert!(!af.elementary_stream_priority_indicator);
        assert!(af.pcr.is_none());
        assert!(af.opcr.is_none());
        assert!(af.splice_countdown.is_none());
        assert!(af.transport_private_data.is_none());
    }

    #[test]
    fn test_adaptation_field_with_pcr() {
        let mut data = vec![0x10]; // pcr_flag set
        // PCR: base=90000 (1 second), extension=0
        // 90000 in binary (33 bits): 0_0000_0000_0000_0001_0101_1111_1001_0000_0
        // Byte[0]: bits[32..25] = 0x00
        // Byte[1]: bits[24..17] = 0x00
        // Byte[2]: bits[16..9]  = 0xAF
        // Byte[3]: bits[8..1]   = 0xC8
        // Byte[4]: bit[0]<<7 | reserved(6) | ext_high(1) = 0x7E
        // Byte[5]: ext_low = 0x00
        data.extend_from_slice(&[0x00, 0x00, 0xAF, 0xC8, 0x7E, 0x00]);
        let af = AdaptationField::parse(&data).unwrap();
        let pcr = af.pcr.unwrap();
        assert_eq!(pcr.base, 90000);
        assert_eq!(pcr.extension, 0);
    }

    #[test]
    fn test_adaptation_field_ref_with_pcr() {
        let mut data = vec![0x10u8]; // pcr_flag set
        data.extend_from_slice(&[0x00, 0x00, 0xAF, 0xC8, 0x7E, 0x00]);
        let af = AdaptationFieldRef::parse(Bytes::from(data)).unwrap();
        assert!(af.pcr_flag);
        let pcr = af.pcr().unwrap();
        assert_eq!(pcr.base, 90000);
        assert_eq!(pcr.extension, 0);
    }

    #[test]
    fn test_adaptation_field_with_splice_countdown() {
        let data = vec![0x04, 0xFE]; // splicing_point_flag set, countdown = -2
        let af = AdaptationField::parse(&data).unwrap();
        assert_eq!(af.splice_countdown, Some(-2));
    }

    #[test]
    fn test_adaptation_field_with_private_data() {
        let mut data = vec![0x02]; // transport_private_data_flag set
        data.push(3); // length
        data.extend_from_slice(&[0xDE, 0xAD, 0xBE]); // private data
        let af = AdaptationField::parse(&data).unwrap();
        assert_eq!(
            af.transport_private_data.as_deref(),
            Some(&[0xDE, 0xAD, 0xBE][..])
        );
    }

    #[test]
    fn test_adaptation_field_empty() {
        let data: Vec<u8> = vec![];
        assert!(AdaptationField::parse(&data).is_none());
    }
}
