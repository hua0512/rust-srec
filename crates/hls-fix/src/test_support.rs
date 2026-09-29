use bytes::Bytes;
use hls::HlsData;
use m3u8_rs::MediaSegment;

pub(crate) const INIT: &[u8] = include_bytes!("../tests/fixtures/init.mp4");
pub(crate) const OTHER_INIT: &[u8] = include_bytes!("../tests/fixtures/init-64x64.mp4");
pub(crate) const OTHER_MEDIA: &[u8] = include_bytes!("../tests/fixtures/media-64x64.m4s");
pub(crate) const MEDIA0: &[u8] = include_bytes!("../tests/fixtures/media00.m4s");
pub(crate) const MEDIA1: &[u8] = include_bytes!("../tests/fixtures/media01.m4s");

pub(crate) fn init(data: &'static [u8]) -> HlsData {
    HlsData::mp4_init(MediaSegment::empty(), Bytes::from_static(data))
}

pub(crate) fn media(data: &'static [u8]) -> HlsData {
    HlsData::mp4_segment(
        MediaSegment {
            duration: 1.0,
            ..MediaSegment::empty()
        },
        Bytes::from_static(data),
    )
}
