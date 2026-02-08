use rand::Rng;
use std::time::{SystemTime, UNIX_EPOCH};

/// Base character set for generating random strings (alphanumeric).
const BASE_CHARS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Base36 character set for timestamp encoding.
const BASE36_CHARS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Fixed character positions in the UUID-like part.
const FIXED_UNDERSCORE_POSITIONS: [usize; 4] = [8, 13, 18, 23];
const FIXED_4_POSITION: usize = 14;
const VARIANT_POSITION: usize = 19;

/// UUID-like part length.
const UUID_PART_LEN: usize = 36;

/// Maximum length needed for base36 representation of u64::MAX.
/// log36(2^64) ≈ 12.38, so 13 characters is sufficient.
const MAX_BASE36_LEN: usize = 13;

/// Generates a `verify_fp` token used for Douyin API requests.
///
/// The format is: `verify_<base36_timestamp>_<uuid-like-string>`
///
/// # Example
/// ```ignore
/// let fp = gen_verify_fp();
/// // Returns something like: "verify_lz5x7q_8fK3jR2m_1aB4_4cD5_eF6G_7hI8jK9LmN0P"
/// ```
pub fn gen_verify_fp() -> String {
    let mut rng = rand::rng();

    // Get current time in milliseconds
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    // Pre-calculate the base36 timestamp
    let mut base36_buf = [0u8; MAX_BASE36_LEN];
    let base36_len = write_base36(milliseconds, &mut base36_buf);

    // Build UUID-like part directly into a buffer
    let mut uuid_buf = [0u8; UUID_PART_LEN];
    let base_len = BASE_CHARS.len();

    for (i, byte) in uuid_buf.iter_mut().enumerate() {
        *byte = if FIXED_UNDERSCORE_POSITIONS.contains(&i) {
            b'_'
        } else if i == FIXED_4_POSITION {
            b'4'
        } else {
            let n = rng.random_range(0..base_len);
            let char_idx = if i == VARIANT_POSITION {
                (3 & n) | 8
            } else {
                n
            };
            BASE_CHARS[char_idx]
        };
    }

    // Calculate final string capacity: "verify_" (7) + base36 + "_" (1) + uuid (36)
    let capacity = 7 + base36_len + 1 + UUID_PART_LEN;
    let mut result = String::with_capacity(capacity);

    result.push_str("verify_");
    // SAFETY: base36_buf contains only ASCII alphanumeric characters
    result.push_str(unsafe {
        std::str::from_utf8_unchecked(&base36_buf[MAX_BASE36_LEN - base36_len..])
    });
    result.push('_');
    // SAFETY: uuid_buf contains only ASCII alphanumeric characters and underscores
    result.push_str(unsafe { std::str::from_utf8_unchecked(&uuid_buf) });

    result
}

/// Generates an `s_v_web_id` token (alias for `gen_verify_fp`).
#[inline]
#[allow(dead_code)]
pub fn gen_s_v_web_id() -> String {
    gen_verify_fp()
}

/// Writes a number in base36 representation to a buffer (right-aligned).
///
/// Returns the number of characters written.
#[inline]
fn write_base36(mut num: u64, buf: &mut [u8; MAX_BASE36_LEN]) -> usize {
    if num == 0 {
        buf[MAX_BASE36_LEN - 1] = b'0';
        return 1;
    }

    let mut pos = MAX_BASE36_LEN;
    while num > 0 {
        pos -= 1;
        buf[pos] = BASE36_CHARS[(num % 36) as usize];
        num /= 36;
    }

    MAX_BASE36_LEN - pos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gen_verify_fp_format() {
        let fp = gen_verify_fp();

        // Should start with "verify_"
        assert!(fp.starts_with("verify_"));

        // Should have the expected structure: verify_<base36>_<uuid>
        let parts: Vec<&str> = fp.splitn(3, '_').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "verify");

        // Base36 part should only contain valid base36 characters
        assert!(
            parts[1]
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );

        // The UUID part should be 36 characters
        let uuid_part = parts[2];
        assert_eq!(uuid_part.len(), UUID_PART_LEN);

        // Verify fixed positions
        let uuid_bytes = uuid_part.as_bytes();
        assert_eq!(uuid_bytes[8], b'_');
        assert_eq!(uuid_bytes[13], b'_');
        assert_eq!(uuid_bytes[18], b'_');
        assert_eq!(uuid_bytes[23], b'_');
        assert_eq!(uuid_bytes[14], b'4');
    }

    #[test]
    fn test_write_base36() {
        let mut buf = [0u8; MAX_BASE36_LEN];

        let len = write_base36(0, &mut buf);
        assert_eq!(len, 1);
        assert_eq!(&buf[MAX_BASE36_LEN - len..], b"0");

        let len = write_base36(10, &mut buf);
        assert_eq!(&buf[MAX_BASE36_LEN - len..], b"a");

        let len = write_base36(35, &mut buf);
        assert_eq!(&buf[MAX_BASE36_LEN - len..], b"z");

        let len = write_base36(36, &mut buf);
        assert_eq!(&buf[MAX_BASE36_LEN - len..], b"10");

        let len = write_base36(1000, &mut buf);
        assert_eq!(&buf[MAX_BASE36_LEN - len..], b"rs");
    }

    #[test]
    fn test_gen_s_v_web_id() {
        let web_id = gen_s_v_web_id();
        assert!(web_id.starts_with("verify_"));
    }

    #[test]
    fn test_verify_fp_uniqueness() {
        // Generate multiple tokens and ensure they're different
        let fp1 = gen_verify_fp();
        let fp2 = gen_verify_fp();
        // While timestamps might be same, UUID parts should differ
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn test_verify_fp_capacity() {
        let fp = gen_verify_fp();
        // Ensure we're not over-allocating (capacity should equal length for well-sized allocation)
        assert!(fp.capacity() <= fp.len() + 16); // Allow small overhead
    }
}
