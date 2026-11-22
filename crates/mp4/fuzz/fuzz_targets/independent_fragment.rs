#![no_main]

use std::sync::LazyLock;

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use mp4::independent::IndependentFragmentCheck;
use mp4::merge::merge_init;

static CHECK: LazyLock<IndependentFragmentCheck> = LazyLock::new(|| {
    let video = Bytes::from_static(include_bytes!("../../tests/fixtures/dash/video-init.mp4"));
    let audio = Bytes::from_static(include_bytes!("../../tests/fixtures/dash/audio-init.mp4"));
    let merged = merge_init(&[video, audio]).expect("fixture inits merge");
    IndependentFragmentCheck::from_init(merged.data()).expect("merged init has tracks")
});

fuzz_target!(|data: &[u8]| {
    let data = Bytes::copy_from_slice(data);
    let _ = IndependentFragmentCheck::from_init(&data);
    let _ = CHECK.is_independent(&data);
});
