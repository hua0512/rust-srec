//! CRC32 helpers shared by the FLV and HLS repair pipelines.
//!
//! Both back onto `zlib_rs` so the values match zlib/flate2 semantics:
//! `crc32("") == 0`, and feeding a stream through [`crc32_update`] yields the
//! same digest as one [`crc32`] call over the concatenation. `flv-fix`'s
//! sequence-header change detection relies on that equivalence — it digests a
//! tag in pieces (codec id, then payload) rather than copying it.

/// One-shot CRC32 of `data`, starting from the zlib initial state.
pub fn crc32(data: &[u8]) -> u32 {
    zlib_rs::crc32::crc32(0, data)
}

/// Folds `data` into an in-progress CRC32 `state`. Start from `0` and chain
/// calls to digest a value that isn't contiguous in memory.
pub fn crc32_update(state: u32, data: &[u8]) -> u32 {
    zlib_rs::crc32::crc32(state, data)
}
