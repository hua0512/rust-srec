pub mod danmaku;
pub mod extractor;
#[cfg(feature = "rquickjs")]
pub mod js_engine;
pub mod media;

/// Format a digest hash as a lowercase hex string.
pub fn digest_to_hex(hash: &[u8]) -> String {
    use std::fmt::Write;
    let mut hex = String::with_capacity(hash.len() * 2);
    for b in hash {
        write!(hex, "{b:02x}").unwrap();
    }
    hex
}
