//! Shared RFC 6381 / ISO BMFF codec prefix classification for HLS and DASH.

/// Extracts the leading sample-entry prefix from a single RFC 6381 codec token
/// (for example `"avc1.640028"` -> `"avc1"`), preserving `".mp3"` as `"mp3"`.
pub(crate) fn codec_prefix(codec: &str) -> &str {
    let trimmed = codec.trim();
    if let Some(rest) = trimmed.strip_prefix('.') {
        return rest.split('.').next().unwrap_or(rest);
    }
    trimmed.split('.').next().unwrap_or(trimmed)
}

/// Whether `codec` names a video sample entry in RFC 6381 / ISO BMFF notation.
pub(crate) fn is_video_codec(codec: &str) -> bool {
    let prefix = codec_prefix(codec);
    [
        "avc1", "avc3", "hvc1", "hev1", "av01", "vp08", "vp09", "vp8", "vp9", "vvc1", "vvi1",
        "mp4v", "dva1", "dvav", "dvh1", "dvhe", "dav1",
    ]
    .iter()
    .any(|known| prefix.eq_ignore_ascii_case(known))
}

/// Whether `codec` names an audio sample entry in RFC 6381 / ISO BMFF notation.
pub(crate) fn is_audio_codec(codec: &str) -> bool {
    let prefix = codec_prefix(codec);
    [
        "mp4a", "ac-3", "ec-3", "ac-4", "opus", "flac", "alac", "mhm1", "mhm2", "mha1", "dtsc",
        "dtse", "dtsh", "dtsl", "dtsx", "mp3",
    ]
    .iter()
    .any(|known| prefix.eq_ignore_ascii_case(known))
}

/// Whether `codec` names a subtitle/timed-text sample entry in ISO BMFF.
pub(crate) fn is_text_codec(codec: &str) -> bool {
    let prefix = codec_prefix(codec);
    ["wvtt", "stpp", "tx3g", "c608", "c708", "evte"]
        .iter()
        .any(|known| prefix.eq_ignore_ascii_case(known))
}

/// Scans a comma-separated RFC 6381 `CODECS` attribute and returns
/// `(has_video, has_audio)`.
pub(crate) fn has_video_and_audio(codecs: &str) -> (bool, bool) {
    let mut has_video = false;
    let mut has_audio = false;
    for item in codecs.split(',') {
        if is_video_codec(item) {
            has_video = true;
        } else if is_audio_codec(item) {
            has_audio = true;
        }
    }
    (has_video, has_audio)
}
