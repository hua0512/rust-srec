use md5::{Digest, Md5};

const XBOGUS_ALPHABET: &[u8; 64] =
    b"Dkdpgh4ZKsQB80/Mfvw36XI1R25+WUAlEi7NLboqYTOPuzmFjJnryx9HVGcaStCe";
const STANDARD_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

// Precomputed: md5(decode(md5(''))) last 2 bytes = 0x45, 0x3f
const EMPTY_MD5_BYTES: [u8; 2] = [0x45, 0x3f];

// Lookup table for standard -> xbogus alphabet
const fn build_lookup() -> [u8; 128] {
    let mut table = [0u8; 128];
    let mut i = 0;
    while i < 64 {
        table[STANDARD_ALPHABET[i] as usize] = XBOGUS_ALPHABET[i];
        i += 1;
    }
    table
}
const ALPHABET_LOOKUP: [u8; 128] = build_lookup();

/// RC4 encryption (in-place capable)
#[inline]
fn rc4_encrypt(key: u8, data: &mut [u8]) {
    let mut s: [u8; 256] = core::array::from_fn(|i| i as u8);
    let mut j: usize = 0;

    for i in 0..256 {
        j = (j + s[i] as usize + key as usize) % 256;
        s.swap(i, j);
    }

    let mut ii: usize = 0;
    j = 0;
    for byte in data.iter_mut() {
        ii = (ii + 1) % 256;
        j = (j + s[ii] as usize) % 256;
        s.swap(ii, j);
        *byte ^= s[(s[ii] as usize + s[j] as usize) % 256];
    }
}

/// Custom base64 encode into pre-allocated buffer
#[inline]
fn encode_base64(data: &[u8; 12], out: &mut [u8; 16]) {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut i = 0;
    let mut o = 0;
    while i < 12 {
        let b0 = data[i] as usize;
        let b1 = data[i + 1] as usize;
        let b2 = data[i + 2] as usize;

        out[o] = ALPHABET_LOOKUP[B64[(b0 >> 2) & 0x3f] as usize];
        out[o + 1] = ALPHABET_LOOKUP[B64[((b0 << 4) | (b1 >> 4)) & 0x3f] as usize];
        out[o + 2] = ALPHABET_LOOKUP[B64[((b1 << 2) | (b2 >> 6)) & 0x3f] as usize];
        out[o + 3] = ALPHABET_LOOKUP[B64[b2 & 0x3f] as usize];

        i += 3;
        o += 4;
    }
}

/// Parse 2 hex chars to byte
#[inline]
fn hex_byte(h: u8, l: u8) -> u8 {
    let hi = if h >= b'a' { h - b'a' + 10 } else { h - b'0' };
    let lo = if l >= b'a' { l - b'a' + 10 } else { l - b'0' };
    (hi << 4) | lo
}

/// Get last 2 bytes of md5(decode(hex_str))
#[inline]
fn md5_last2(hex_str: &[u8; 32]) -> [u8; 2] {
    let mut bytes = [0u8; 16];
    for i in 0..16 {
        bytes[i] = hex_byte(hex_str[i * 2], hex_str[i * 2 + 1]);
    }
    let hash = Md5::digest(bytes);
    [hash[14], hash[15]]
}

/// Generate X-Bogus signature
///
/// Returns 16-byte ASCII string
#[inline]
pub fn generate_xbogus(ms_stub: &[u8; 32], counter: u8) -> [u8; 16] {
    let random1 = rand::random::<u8>();
    let random2 = (rand::random::<u8>() as u16 * 255 / 256) as u8;

    // Header byte: version(1)<<6 | initialized(0)<<5 | (random1 & 0x1f)
    let header = 0x40 | (random1 & 0x1f);

    // Build payload
    let md5_bytes = md5_last2(ms_stub);
    let mut payload: [u8; 10] = [
        counter & 0x3f,     // platform(0)<<6 | counter
        0,                  // envcode >> 8
        1,                  // envcode & 0xff
        0x0e,               // ubcode
        EMPTY_MD5_BYTES[0], // from empty md5
        EMPTY_MD5_BYTES[1],
        md5_bytes[0], // from input md5
        md5_bytes[1],
        random2, // random
        0,       // checksum placeholder
    ];

    // XOR checksum
    payload[9] = payload[..9].iter().fold(0, |a, &x| a ^ x);

    // RC4 encrypt in place
    rc4_encrypt(random2, &mut payload);

    // Build final 12 bytes
    let mut final_data: [u8; 12] = [0; 12];
    final_data[0] = header;
    final_data[1] = random2;
    final_data[2..].copy_from_slice(&payload);

    // Encode
    let mut result = [0u8; 16];
    encode_base64(&final_data, &mut result);
    result
}

// /// Convenience: generate from &str, returns String
// pub fn generate_xbogus_string(ms_stub: &str, counter: u8) -> String {
//     let mut stub = [0u8; 32];
//     stub.copy_from_slice(ms_stub.as_bytes());
//     let result = generate_xbogus(&stub, counter);
//     // SAFETY: result contains only ASCII from XBOGUS_ALPHABET
//     unsafe { String::from_utf8_unchecked(result.to_vec()) }
// }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_md5_last2() {
        let input = b"56a634b4228ef02b53388ada4e6f76c7";
        let result = md5_last2(input);
        assert_eq!(result, [0x26, 0x54]);
    }

    #[test]
    fn test_empty_md5_constant() {
        let empty_md5 = format!("{:x}", Md5::digest(b""));
        let mut stub = [0u8; 32];
        stub.copy_from_slice(empty_md5.as_bytes());
        let result = md5_last2(&stub);
        assert_eq!(result, EMPTY_MD5_BYTES);
    }
}
