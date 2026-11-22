use std::io;

use bytes_util::BitReader;

/// Read a little-endian variable-length integer.
/// AV1-Spec-2 - 4.10.5
///
/// Per the spec, conforming bitstreams produce values `<= (1 << 32) - 1`.
/// This function rejects values exceeding that limit.
pub fn read_leb128<T: io::Read>(reader: &mut BitReader<T>) -> io::Result<u64> {
    let mut result = 0;
    for i in 0..8 {
        let byte = reader.read_bits(8)?;
        result |= (byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            break;
        }
    }
    if result > u32::MAX as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LEB128 value exceeds u32::MAX",
        ));
    }
    Ok(result)
}

/// Write a little-endian variable-length integer.
/// AV1-Spec-2 - 4.10.5
///
/// Returns the number of bytes written (1-8).
pub fn write_leb128<W: io::Write>(writer: &mut W, mut value: u64) -> io::Result<usize> {
    let mut bytes_written = 0;
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        writer.write_all(&[byte])?;
        bytes_written += 1;
        if value == 0 {
            break;
        }
    }
    Ok(bytes_written)
}

/// Returns the number of bytes needed to encode `value` as LEB128.
pub fn leb128_size(mut value: u64) -> usize {
    let mut size = 1;
    while value >= 0x80 {
        value >>= 7;
        size += 1;
    }
    size
}

/// Read a variable-length unsigned integer.
/// AV1-Spec-2 - 4.10.3
pub fn read_uvlc<T: io::Read>(reader: &mut BitReader<T>) -> io::Result<u64> {
    let mut leading_zeros: u32 = 0;
    while !reader.read_bit()? {
        leading_zeros = leading_zeros.saturating_add(1);
    }

    if leading_zeros >= 32 {
        return Ok(u64::from(u32::MAX));
    }

    let value = reader.read_bits(leading_zeros as u8)?;
    Ok(value + (1u64 << leading_zeros) - 1)
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_read_leb128() {
        let mut cursor = std::io::Cursor::new([0b11010101, 0b00101010]);
        let mut reader = BitReader::new(&mut cursor);
        assert_eq!(read_leb128(&mut reader).unwrap(), 0b1010101010101);

        // u32::MAX should be accepted
        let mut buf = Vec::new();
        write_leb128(&mut buf, u32::MAX as u64).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let mut reader = BitReader::new(&mut cursor);
        assert_eq!(read_leb128(&mut reader).unwrap(), u32::MAX as u64);
    }

    #[test]
    fn test_read_leb128_overflow() {
        // Value exceeding u32::MAX must be rejected
        let mut buf = Vec::new();
        // Manually write a 5-byte LEB128 encoding of u32::MAX + 1 = 0x1_0000_0000
        // 0x1_0000_0000 = 0b1_00000000_00000000_00000000_00000000
        // LEB128: [0x80, 0x80, 0x80, 0x80, 0x10]
        buf.extend_from_slice(&[0x80, 0x80, 0x80, 0x80, 0x10]);
        let mut cursor = std::io::Cursor::new(buf);
        let mut reader = BitReader::new(&mut cursor);
        let err = read_leb128(&mut reader).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_write_leb128() {
        let mut buf = Vec::new();
        assert_eq!(write_leb128(&mut buf, 0).unwrap(), 1);
        assert_eq!(buf, [0x00]);

        let mut buf = Vec::new();
        assert_eq!(write_leb128(&mut buf, 1).unwrap(), 1);
        assert_eq!(buf, [0x01]);

        let mut buf = Vec::new();
        assert_eq!(write_leb128(&mut buf, 127).unwrap(), 1);
        assert_eq!(buf, [0x7f]);

        let mut buf = Vec::new();
        assert_eq!(write_leb128(&mut buf, 128).unwrap(), 2);
        assert_eq!(buf, [0x80, 0x01]);

        let mut buf = Vec::new();
        assert_eq!(write_leb128(&mut buf, 16383).unwrap(), 2);
        assert_eq!(buf, [0xff, 0x7f]);

        let mut buf = Vec::new();
        assert_eq!(write_leb128(&mut buf, 16384).unwrap(), 3);
        assert_eq!(buf, [0x80, 0x80, 0x01]);
    }

    #[test]
    fn test_write_leb128_round_trip() {
        let values = [0, 1, 127, 128, 255, 256, 16383, 16384, u32::MAX as u64];
        for value in values {
            let mut buf = Vec::new();
            let written = write_leb128(&mut buf, value).unwrap();
            assert_eq!(
                written,
                leb128_size(value),
                "leb128_size mismatch for {value}"
            );

            let mut cursor = std::io::Cursor::new(buf);
            let mut reader = BitReader::new(&mut cursor);
            let decoded = read_leb128(&mut reader).unwrap();
            assert_eq!(decoded, value, "round-trip failed for {value}");
        }
    }

    #[test]
    fn test_read_uvlc() {
        let mut cursor = std::io::Cursor::new([0x01, 0xff]);
        let mut reader = BitReader::new(&mut cursor);
        assert_eq!(read_uvlc(&mut reader).unwrap(), 0xfe);

        let mut cursor = std::io::Cursor::new([0x00, 0x00, 0x00, 0x00, 0x01]);
        let mut reader = BitReader::new(&mut cursor);
        assert_eq!(read_uvlc(&mut reader).unwrap(), (1 << 32) - 1);

        // >= 256 leading zero bits must saturate to u32::MAX rather than wrapping u8 to 0.
        let mut over_256_zeros = vec![0x00; 33];
        over_256_zeros.push(0x80);
        let mut cursor = std::io::Cursor::new(over_256_zeros);
        let mut reader = BitReader::new(&mut cursor);
        assert_eq!(read_uvlc(&mut reader).unwrap(), u64::from(u32::MAX));
    }
}
