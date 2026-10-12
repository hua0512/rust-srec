//! Representation selection within a Period.

use crate::dash::config::{AudioSelection, DashSelectionConfig, VideoSelection};
use crate::dash::manifest::{ContentType, Period, Representation};
use crate::segment_engine::descriptor::MediaTrack;

/// Codec families (RFC 6381 `codecs` prefixes) that carry audio. A video
/// Representation listing one of them is multiplexed and already contains
/// the audio track.
const AUDIO_CODECS: &[&str] = &[
    "mp4a", "opus", "ac-3", "ec-3", "ac-4", "flac", "mhm1", "mhm2", "dtsc", "dtse", "dtsh", "dtsl",
    "dtsx", ".mp3",
];

/// The Representation `track` records in `period`, or `None` when the
/// Period has no Representation of that kind.
pub(crate) fn select<'a>(
    period: &'a Period,
    track: MediaTrack,
    config: &DashSelectionConfig,
) -> Option<&'a Representation> {
    match track {
        MediaTrack::Video => select_video(period, config.video),
        MediaTrack::Audio => select_audio(period, config.audio, config.audio_language.as_deref()),
    }
}

/// The Representation with `id` and the track's content type in `period`.
pub(crate) fn find<'a>(
    period: &'a Period,
    track: MediaTrack,
    id: &str,
) -> Option<&'a Representation> {
    period
        .adaptation_sets
        .iter()
        .flat_map(|set| &set.representations)
        .find(|representation| {
            representation.id == id && content_type(track) == representation.content_type
        })
}

/// Whether a Representation's codecs list audio next to video: one
/// Representation then carries both tracks in one initialization segment.
pub(crate) fn is_multiplexed(representation: &Representation) -> bool {
    representation.content_type == ContentType::Video
        && representation.codecs.as_deref().is_some_and(|codecs| {
            codecs.split(',').map(str::trim).any(|codec| {
                AUDIO_CODECS
                    .iter()
                    .any(|family| codec.to_ascii_lowercase().starts_with(family))
            })
        })
}

fn content_type(track: MediaTrack) -> ContentType {
    match track {
        MediaTrack::Video => ContentType::Video,
        MediaTrack::Audio => ContentType::Audio,
    }
}

fn select_video(period: &Period, policy: VideoSelection) -> Option<&Representation> {
    let candidates: Vec<&Representation> = period
        .adaptation_sets
        .iter()
        .flat_map(|set| &set.representations)
        .filter(|representation| representation.content_type == ContentType::Video)
        .collect();
    match policy {
        VideoSelection::Highest => highest(&candidates),
        VideoSelection::Lowest => lowest(&candidates),
        VideoSelection::ClosestBandwidth(target) => closest(&candidates, target),
        VideoSelection::MaxHeight(max) => {
            // An unknown height is assumed to fit: the MPD did not say it is
            // too large, and excluding it could leave nothing to record.
            let fitting: Vec<&Representation> = candidates
                .iter()
                .copied()
                .filter(|representation| representation.height.is_none_or(|h| h <= max))
                .collect();
            if fitting.is_empty() {
                candidates.iter().copied().min_by_key(|representation| {
                    (
                        representation.height.unwrap_or(u32::MAX),
                        representation.bandwidth,
                    )
                })
            } else {
                highest(&fitting)
            }
        }
    }
}

fn select_audio<'a>(
    period: &'a Period,
    policy: AudioSelection,
    language: Option<&str>,
) -> Option<&'a Representation> {
    let audio_sets = || {
        period.adaptation_sets.iter().filter(|set| {
            set.representations
                .iter()
                .any(|representation| representation.content_type == ContentType::Audio)
        })
    };
    let collect = |matches: &dyn Fn(Option<&str>) -> bool| -> Vec<&'a Representation> {
        audio_sets()
            .filter(|set| matches(set.lang.as_deref()))
            .flat_map(|set| &set.representations)
            .filter(|representation| representation.content_type == ContentType::Audio)
            .collect()
    };
    let mut candidates = Vec::new();
    if let Some(wanted) = language {
        candidates = collect(&|lang| lang.is_some_and(|lang| lang.eq_ignore_ascii_case(wanted)));
        if candidates.is_empty() {
            let primary = primary_subtag(wanted);
            candidates = collect(&|lang| {
                lang.is_some_and(|lang| primary_subtag(lang).eq_ignore_ascii_case(primary))
            });
        }
    }
    if candidates.is_empty() {
        candidates = collect(&|_| true);
    }
    match policy {
        AudioSelection::Highest => highest(&candidates),
        AudioSelection::Lowest => lowest(&candidates),
        AudioSelection::ClosestBandwidth(target) => closest(&candidates, target),
    }
}

fn primary_subtag(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or(tag)
}

/// Ties keep the first Representation in document order.
fn highest<'a>(candidates: &[&'a Representation]) -> Option<&'a Representation> {
    candidates
        .iter()
        .copied()
        .rev()
        .max_by_key(|representation| representation.bandwidth)
}

fn lowest<'a>(candidates: &[&'a Representation]) -> Option<&'a Representation> {
    candidates
        .iter()
        .copied()
        .min_by_key(|representation| representation.bandwidth)
}

fn closest<'a>(candidates: &[&'a Representation], target: u64) -> Option<&'a Representation> {
    candidates
        .iter()
        .copied()
        .min_by_key(|representation| representation.bandwidth.abs_diff(target))
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;
    use crate::dash::manifest::Manifest;

    const MPD: &str = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT10S"
     profiles="urn:mpeg:dash:profile:isoff-live:2011">
  <Period id="p0">
    <AdaptationSet mimeType="video/mp4" segmentAlignment="true">
      <SegmentTemplate media="$RepresentationID$/$Number$.m4s" initialization="$RepresentationID$/init.mp4" duration="2" timescale="1"/>
      <Representation id="v360" bandwidth="800000" width="640" height="360" codecs="avc1.4d401e"/>
      <Representation id="v1080" bandwidth="5000000" width="1920" height="1080" codecs="avc1.640028"/>
      <Representation id="v720" bandwidth="2500000" width="1280" height="720" codecs="avc1.4d401f"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="en">
      <SegmentTemplate media="$RepresentationID$/$Number$.m4s" initialization="$RepresentationID$/init.mp4" duration="2" timescale="1"/>
      <Representation id="a-en-64" bandwidth="64000" codecs="mp4a.40.2"/>
      <Representation id="a-en-128" bandwidth="128000" codecs="mp4a.40.2"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="pt-BR">
      <SegmentTemplate media="$RepresentationID$/$Number$.m4s" initialization="$RepresentationID$/init.mp4" duration="2" timescale="1"/>
      <Representation id="a-pt" bandwidth="96000" codecs="mp4a.40.2"/>
    </AdaptationSet>
  </Period>
</MPD>"#;

    fn period() -> Period {
        let url = Url::parse("https://example.com/live/manifest.mpd").unwrap();
        let manifest = Manifest::parse(MPD.as_bytes(), &url).unwrap();
        manifest.periods.into_iter().next().unwrap()
    }

    fn video(policy: VideoSelection) -> String {
        let config = DashSelectionConfig {
            video: policy,
            ..DashSelectionConfig::default()
        };
        select(&period(), MediaTrack::Video, &config)
            .unwrap()
            .id
            .clone()
    }

    fn audio(policy: AudioSelection, language: Option<&str>) -> String {
        let config = DashSelectionConfig {
            audio: policy,
            audio_language: language.map(str::to_owned),
            ..DashSelectionConfig::default()
        };
        select(&period(), MediaTrack::Audio, &config)
            .unwrap()
            .id
            .clone()
    }

    #[test]
    fn video_policies() {
        assert_eq!(video(VideoSelection::Highest), "v1080");
        assert_eq!(video(VideoSelection::Lowest), "v360");
        assert_eq!(video(VideoSelection::MaxHeight(720)), "v720");
        assert_eq!(video(VideoSelection::MaxHeight(100)), "v360");
        assert_eq!(video(VideoSelection::ClosestBandwidth(2_000_000)), "v720");
    }

    #[test]
    fn audio_policies_and_languages() {
        assert_eq!(audio(AudioSelection::Highest, None), "a-en-128");
        assert_eq!(audio(AudioSelection::Lowest, None), "a-en-64");
        assert_eq!(audio(AudioSelection::Highest, Some("pt-BR")), "a-pt");
        assert_eq!(audio(AudioSelection::Highest, Some("pt")), "a-pt");
        assert_eq!(audio(AudioSelection::Lowest, Some("EN-us")), "a-en-64");
        assert_eq!(audio(AudioSelection::Highest, Some("de")), "a-en-128");
    }

    #[test]
    fn finds_representations_by_id_and_track() {
        let period = period();
        assert!(find(&period, MediaTrack::Video, "v720").is_some());
        assert!(find(&period, MediaTrack::Audio, "v720").is_none());
        assert!(find(&period, MediaTrack::Audio, "missing").is_none());
    }

    #[test]
    fn multiplexed_representations_list_an_audio_codec() {
        let mut representation = find(&period(), MediaTrack::Video, "v720").unwrap().clone();
        assert!(!is_multiplexed(&representation));
        representation.codecs = Some("avc1.4d401f, mp4a.40.2".to_owned());
        assert!(is_multiplexed(&representation));
    }
}
