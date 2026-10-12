#![no_main]

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Parse as a single source, and as both sources of a two-track merge.
    let source = Bytes::copy_from_slice(data);
    let _ = mp4::merge::merge_init(std::slice::from_ref(&source));
    let _ = mp4::merge::merge_init(&[source.clone(), source]);
});
