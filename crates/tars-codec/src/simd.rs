use bytes::Bytes;

/// SIMD-optimized UTF-8 validation and string operations
/// Falls back to standard library implementations on unsupported architectures
pub mod utf8_simd {
    use super::*;

    /// Fast UTF-8 validation using SIMD when available
    #[inline]
    pub fn validate_utf8(bytes: &[u8]) -> Result<(), std::str::Utf8Error> {
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("sse2") {
                // SAFETY: SSE2 support was checked at runtime. The helper only
                // performs unaligned loads within the bounds of `bytes`.
                unsafe { validate_utf8_sse2(bytes) }
            } else {
                std::str::from_utf8(bytes).map(|_| ())
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            std::str::from_utf8(bytes).map(|_| ())
        }
    }

    /// Validate and create a ValidatedBytes with potential SIMD acceleration
    #[inline]
    pub fn validated_bytes_fast(
        bytes: Bytes,
    ) -> Result<crate::ValidatedBytes, std::str::Utf8Error> {
        validate_utf8(&bytes)?;
        // SAFETY: We just validated the UTF-8
        Ok(unsafe { crate::ValidatedBytes::new_unchecked(bytes) })
    }

    #[cfg(target_arch = "x86_64")]
    unsafe fn validate_utf8_sse2(bytes: &[u8]) -> Result<(), std::str::Utf8Error> {
        use std::arch::x86_64::*;

        let len = bytes.len();
        let mut pos = 0;

        // Process 16-byte chunks with SSE2
        while pos + 16 <= len {
            let non_ascii_bits = unsafe {
                // SAFETY: `pos + 16 <= len`, so the unaligned 16-byte load is
                // in bounds. This function is called only after checking SSE2.
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(pos) as *const __m128i);
                let non_ascii_mask = _mm_cmplt_epi8(chunk, _mm_set1_epi8(0));
                _mm_movemask_epi8(non_ascii_mask) as u16
            };

            if non_ascii_bits == 0 {
                // All bytes are ASCII, so this chunk is valid UTF-8.
                pos += 16;
                continue;
            }

            // Non-ASCII found: validate remaining bytes from this position using std
            // This correctly reports error position since we validate from `pos` forward
            return std::str::from_utf8(&bytes[pos..]).map(|_| ());
        }

        // Validate remaining bytes with standard library
        if pos < len {
            std::str::from_utf8(&bytes[pos..]).map(|_| ())?;
        }

        Ok(())
    }

    /// SIMD-accelerated byte comparison for large strings
    #[inline]
    pub fn bytes_equal_simd(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }

        #[cfg(target_arch = "x86_64")]
        {
            if a.len() >= 16 && is_x86_feature_detected!("sse2") {
                // SAFETY: SSE2 support was checked at runtime. The helper only
                // performs unaligned loads within the bounds of both slices.
                unsafe { bytes_equal_sse2(a, b) }
            } else {
                a == b
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            a == b
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe fn bytes_equal_sse2(a: &[u8], b: &[u8]) -> bool {
        use std::arch::x86_64::*;

        let len = a.len();
        let mut pos = 0;

        // Compare 16-byte chunks
        while pos + 16 <= len {
            let mask = unsafe {
                // SAFETY: `pos + 16 <= len` for equal-length slices, so both
                // unaligned loads are in bounds. SSE2 was checked by the caller.
                let chunk_a = _mm_loadu_si128(a.as_ptr().add(pos) as *const __m128i);
                let chunk_b = _mm_loadu_si128(b.as_ptr().add(pos) as *const __m128i);
                let cmp = _mm_cmpeq_epi8(chunk_a, chunk_b);
                _mm_movemask_epi8(cmp) as u16
            };

            if mask != 0xFFFF {
                return false; // Found difference
            }

            pos += 16;
        }

        // Compare remaining bytes
        if pos < len {
            return a[pos..] == b[pos..];
        }

        true
    }
}

/// Bulk string operations optimized for TARS codec usage patterns
pub mod bulk_ops {
    use super::*;
    use crate::TarsValue;

    /// Optimized bulk string validation for collections of TarsValues
    pub fn validate_string_collection(values: &[TarsValue]) -> Result<(), std::str::Utf8Error> {
        for value in values {
            match value {
                TarsValue::StringRef(bytes) => {
                    utf8_simd::validate_utf8(bytes)?;
                }
                TarsValue::Struct(map) => {
                    for value in map.values() {
                        validate_string_collection(std::slice::from_ref(value))?;
                    }
                }
                TarsValue::Map(map) => {
                    for value in map.values() {
                        validate_string_collection(std::slice::from_ref(value))?;
                    }
                }
                TarsValue::List(list) => {
                    for boxed_value in list.iter() {
                        validate_string_collection(&[boxed_value.as_ref().clone()])?;
                    }
                }
                _ => {} // Other types don't need validation
            }
        }
        Ok(())
    }

    /// Bulk hash computation for collections
    pub fn bulk_hash_values(values: &[TarsValue]) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();

        // Hash all values in sequence
        for value in values {
            value.hash(&mut hasher);
        }

        hasher.finish()
    }
}

// Extension trait for adding SIMD capabilities to existing types
impl crate::ValidatedBytes {
    /// Create ValidatedBytes without validation (unsafe)
    /// Used internally after SIMD validation
    ///
    /// # Safety
    ///
    /// The caller must ensure that `bytes` contains valid UTF-8 data.
    /// This function bypasses UTF-8 validation for performance reasons.
    /// Calling this with invalid UTF-8 data may lead to undefined behavior
    /// when the contained data is later used as a string.
    #[inline]
    pub unsafe fn new_unchecked(bytes: Bytes) -> Self {
        Self(bytes)
    }

    /// Fast equality comparison using SIMD when available
    #[inline]
    pub fn equals_fast(&self, other: &Self) -> bool {
        utf8_simd::bytes_equal_simd(&self.0, &other.0)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::utf8_simd;

    #[test]
    fn rejects_full_non_ascii_invalid_chunk() {
        let invalid = [0xff; 16];

        assert!(utf8_simd::validate_utf8(&invalid).is_err());
        assert!(utf8_simd::validated_bytes_fast(Bytes::copy_from_slice(&invalid)).is_err());
    }

    #[test]
    fn rejects_invalid_data_after_ascii_chunk() {
        let mut invalid = vec![b'a'; 32];
        invalid[23] = 0xff;

        assert!(utf8_simd::validate_utf8(&invalid).is_err());
    }

    #[test]
    fn accepts_ascii_and_multibyte_utf8() {
        let ascii = [b'a'; 32];
        let multibyte = "ASCII prefix long enough: 你好, мир";

        assert!(utf8_simd::validate_utf8(&ascii).is_ok());
        assert!(utf8_simd::validate_utf8(multibyte.as_bytes()).is_ok());
    }
}
