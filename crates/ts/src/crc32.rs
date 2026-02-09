/// MPEG-2 CRC-32 (ITU-T H.222.0 / ISO 13818-1)
///
/// Polynomial: 0x04C11DB7, init: 0xFFFFFFFF, no bit reflection, no final XOR.
/// This is NOT the same as the zlib/ISO 3309 CRC-32.
///
/// Compile-time generated 256-entry lookup table for MPEG-2 CRC-32.
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i << 24;
        let mut j = 0;
        while j < 8 {
            if crc & 0x8000_0000 != 0 {
                crc = (crc << 1) ^ 0x04C1_1DB7;
            } else {
                crc <<= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

/// Compute MPEG-2 CRC-32 over a byte slice.
pub fn mpeg2_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc = (crc << 8) ^ CRC32_TABLE[((crc >> 24) ^ byte as u32) as usize];
    }
    crc
}

/// Validate that the MPEG-2 CRC-32 over the full PSI section (including the
/// stored 4-byte CRC at the end) equals zero.
pub fn validate_section_crc32(section_data: &[u8]) -> bool {
    mpeg2_crc32(section_data) == 0x0000_0000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_vector() {
        // CRC-32/MPEG-2 of "123456789" is 0x0376E6E7
        assert_eq!(mpeg2_crc32(b"123456789"), 0x0376_E6E7);
    }

    #[test]
    fn test_empty_data() {
        // CRC-32/MPEG-2 of empty data is 0xFFFFFFFF (init value, no processing)
        assert_eq!(mpeg2_crc32(b""), 0xFFFF_FFFF);
    }

    #[test]
    fn test_section_validation() {
        // Create a section where we manually append the correct CRC
        let data = b"test section data";
        let crc = mpeg2_crc32(data);
        let mut section = data.to_vec();
        section.extend_from_slice(&crc.to_be_bytes());
        assert!(validate_section_crc32(&section));
    }

    #[test]
    fn test_section_validation_corrupt() {
        let data = b"test section data";
        let crc = mpeg2_crc32(data);
        let mut section = data.to_vec();
        section.extend_from_slice(&crc.to_be_bytes());
        // Corrupt one byte
        section[0] ^= 0xFF;
        assert!(!validate_section_crc32(&section));
    }
}
