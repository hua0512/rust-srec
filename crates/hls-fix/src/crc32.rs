pub(crate) fn crc32(data: &[u8]) -> u32 {
    // Matches zlib/flate2 semantics: CRC32("") == 0 and incremental updates.
    zlib_rs::crc32::crc32(0, data)
}

#[cfg(test)]
pub(crate) fn crc32_update(state: u32, data: &[u8]) -> u32 {
    zlib_rs::crc32::crc32(state, data)
}

#[cfg(test)]
mod tests {
    use super::{crc32, crc32_update};

    fn crc32_streaming(chunks: &[&[u8]]) -> u32 {
        let mut state = 0u32;
        for chunk in chunks {
            state = crc32_update(state, chunk);
        }
        state
    }

    #[test]
    fn known_vectors_match_zlib() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"hello"), 0x3610_A686);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn streaming_update_matches_one_shot() {
        let one_shot = crc32(b"hello world");
        let streaming = crc32_streaming(&[b"hello", b" ", b"world"]);
        assert_eq!(streaming, one_shot);
    }
}
