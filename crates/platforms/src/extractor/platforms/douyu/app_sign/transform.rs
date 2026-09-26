//! The native transform is a 32-round inverse Skipjack-style permutation.
//! Evaluate keyed S-box entries on demand: signing uses only 256 entries,
//! so materializing all 2,560 entries per request is unnecessary.

use super::data::SBOX;

pub(super) fn transform_block(block: &[u8; 8], key: &[u8; 10]) -> [u8; 8] {
    let mut words: [u16; 4] =
        std::array::from_fn(|i| u16::from_be_bytes([block[i * 2], block[i * 2 + 1]]));
    for counter in (1..=32_u16).rev() {
        let key_start = (4 * usize::from(counter - 1)) % 10;
        let mut value = words[1];
        // Reverse the four byte substitutions, walking the ten key bytes
        // cyclically. High/low halves alternate at each substitution.
        for offset in (0..4).rev() {
            let key_byte = usize::from(key[(key_start + offset) % 10]);
            if offset % 2 == 1 {
                value ^= u16::from(SBOX[key_byte ^ usize::from(value >> 8)] ^ 0xc3);
            } else {
                value ^= u16::from(SBOX[key_byte ^ usize::from(value & 255)] ^ 0xc3) << 8;
            }
        }
        // Invert rule A for rounds 1..8 and 17..24, rule B otherwise.
        words = if matches!((counter - 1) / 8, 0 | 2) {
            [value, words[2], words[3], words[0] ^ words[1] ^ counter]
        } else {
            [value, words[2] ^ value ^ counter, words[3], words[0]]
        };
    }
    let mut out = [0; 8];
    for (dest, word) in out.as_chunks_mut::<2>().0.iter_mut().zip(words) {
        dest.copy_from_slice(&word.to_be_bytes());
    }
    out
}
