pub mod danmaku;
pub mod extractor;
// Disabled with the `rquickjs` feature in Cargo.toml: no extractor currently
// needs a JS runtime. The js_engine module source is retained for reference.
// #[cfg(feature = "rquickjs")]
// pub mod js_engine;
pub mod media;
pub mod proxy;

/// Format a digest hash as a lowercase hex string.
pub fn digest_to_hex(hash: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(hash.len() * 2);
    for &b in hash {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
