#![no_main]

use std::sync::LazyLock;

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use mp4::merge::{FragmentWriter, MergedInit, merge_init};

static MERGED: LazyLock<MergedInit> = LazyLock::new(|| {
    let video = Bytes::from_static(include_bytes!("../../tests/fixtures/dash/video-init.mp4"));
    let audio = Bytes::from_static(include_bytes!("../../tests/fixtures/dash/audio-init.mp4"));
    merge_init(&[video, audio]).expect("fixture inits merge")
});

fuzz_target!(|data: &[u8]| {
    let segment = Bytes::copy_from_slice(data);
    for track in 0..MERGED.tracks().len() {
        let Ok(fragments) = MERGED.parse_media_segment(track, &segment, 0) else {
            continue;
        };
        let mut writer = FragmentWriter::new();
        for fragment in fragments {
            let (decode_time, duration, samples, media) = (
                fragment.decode_time(),
                fragment.duration(),
                fragment.sample_count(),
                fragment.media_data_len(),
            );
            let mut out = Vec::new();
            if writer.write(fragment, &mut out).is_err() {
                assert!(out.is_empty(), "a failed write leaves the output untouched");
                continue;
            }
            // Both fixture sources use track ID 1, which is also the first
            // merged track ID, so track 0's output is valid track 0 input:
            // rewriting must be idempotent.
            if track == 0 {
                let again = MERGED
                    .parse_media_segment(0, &Bytes::from(out), 0)
                    .expect("written fragment parses again");
                assert_eq!(again.len(), 1);
                assert_eq!(again[0].decode_time(), decode_time);
                assert_eq!(again[0].duration(), duration);
                assert_eq!(again[0].sample_count(), samples);
                assert_eq!(again[0].media_data_len(), media);
            }
        }
    }
});
