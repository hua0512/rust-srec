//! Per-segment pairing of the video and danmu inputs handed to a paired-segment
//! or session-complete pipeline.
//!
//! The manifest is stored on the DAG row and copied into every step job's state,
//! so processors that pair a video with its subtitles read it from
//! `ProcessorInput::manifest` instead of the job's input list. Pairing never
//! crosses a segment: a segment that produced no danmu file leaves its videos
//! unpaired rather than shifting later segments' pairs.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Which dispatch produced the manifest.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManifestScope {
    /// Session-complete pipeline: every segment of the session.
    Session,
    /// Paired-segment pipeline for one segment.
    Segment { index: u32 },
}

/// Inputs one segment contributed, in the order the coordinator collected them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSegment {
    pub segment_index: u32,
    pub video: Vec<String>,
    pub danmu: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineInputManifest {
    /// Schema version of the stored JSON; readers reject nothing today.
    pub version: u32,
    pub session_id: String,
    pub streamer_id: String,
    pub scope: ManifestScope,
    /// Ascending by `segment_index`.
    pub segments: Vec<ManifestSegment>,
}

impl PipelineInputManifest {
    pub const VERSION: u32 = 1;

    /// Segments are sorted by index so flattened views follow recording order
    /// regardless of the order the caller collected them.
    pub fn new(
        session_id: impl Into<String>,
        streamer_id: impl Into<String>,
        scope: ManifestScope,
        mut segments: Vec<ManifestSegment>,
    ) -> Self {
        segments.sort_by_key(|segment| segment.segment_index);
        Self {
            version: Self::VERSION,
            session_id: session_id.into(),
            streamer_id: streamer_id.into(),
            scope,
            segments,
        }
    }

    /// Every video path in segment order.
    pub fn video_inputs(&self) -> impl Iterator<Item = &str> {
        self.segments
            .iter()
            .flat_map(|segment| segment.video.iter().map(String::as_str))
    }

    /// Every danmu path in segment order.
    pub fn danmu_inputs(&self) -> impl Iterator<Item = &str> {
        self.segments
            .iter()
            .flat_map(|segment| segment.danmu.iter().map(String::as_str))
    }

    /// Danmu paths recorded for the segment that produced `video`.
    ///
    /// The segment is found by exact spelling first, then by file stem so a
    /// relocated or remuxed copy of the video still resolves. Returns an empty
    /// slice when no segment lists the video.
    pub fn danmu_for_video(&self, video: &str) -> &[String] {
        if let Some(segment) = self
            .segments
            .iter()
            .find(|segment| segment.video.iter().any(|candidate| candidate == video))
        {
            return &segment.danmu;
        }
        let Some(stem) = file_stem(video) else {
            return &[];
        };
        self.segments
            .iter()
            .find(|segment| {
                segment
                    .video
                    .iter()
                    .any(|candidate| file_stem(candidate).is_some_and(|other| other == stem))
            })
            .map(|segment| segment.danmu.as_slice())
            .unwrap_or(&[])
    }

    /// Every `(video, danmu)` combination within a segment, in segment order.
    /// A segment with videos but no danmu yields nothing.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.segments.iter().flat_map(|segment| {
            segment.video.iter().flat_map(move |video| {
                segment
                    .danmu
                    .iter()
                    .map(move |danmu| (video.as_str(), danmu.as_str()))
            })
        })
    }

    pub fn is_empty(&self) -> bool {
        self.segments
            .iter()
            .all(|segment| segment.video.is_empty() && segment.danmu.is_empty())
    }
}

/// Stem comparison folds ASCII case because the same recording can be listed
/// with different case on Windows.
fn file_stem(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PipelineInputManifest {
        PipelineInputManifest::new(
            "session",
            "streamer",
            ManifestScope::Session,
            vec![
                ManifestSegment {
                    segment_index: 2,
                    video: vec!["/rec/part2.mp4".to_owned(), "/rec/part2b.mp4".to_owned()],
                    danmu: vec!["/rec/part2.xml".to_owned()],
                },
                ManifestSegment {
                    segment_index: 0,
                    video: vec!["/rec/part0.mp4".to_owned()],
                    danmu: vec!["/rec/part0.xml".to_owned()],
                },
                ManifestSegment {
                    segment_index: 1,
                    video: vec!["/rec/part1.mp4".to_owned()],
                    danmu: Vec::new(),
                },
            ],
        )
    }

    #[test]
    fn serde_round_trip_keeps_scope_and_segment_order() {
        let manifest = manifest();
        assert_eq!(
            manifest
                .segments
                .iter()
                .map(|segment| segment.segment_index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        let json = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<PipelineInputManifest>(&json).unwrap(),
            manifest
        );

        let segment =
            PipelineInputManifest::new("s", "t", ManifestScope::Segment { index: 7 }, Vec::new());
        let value = serde_json::to_value(&segment).unwrap();
        assert_eq!(
            value["scope"],
            serde_json::json!({ "segment": { "index": 7 } })
        );
        assert_eq!(value["version"], PipelineInputManifest::VERSION);
        assert!(segment.is_empty());
        assert!(!manifest.is_empty());
    }

    #[test]
    fn flattened_views_follow_segment_order() {
        let manifest = manifest();
        assert_eq!(
            manifest.video_inputs().collect::<Vec<_>>(),
            vec![
                "/rec/part0.mp4",
                "/rec/part1.mp4",
                "/rec/part2.mp4",
                "/rec/part2b.mp4"
            ]
        );
        assert_eq!(
            manifest.danmu_inputs().collect::<Vec<_>>(),
            vec!["/rec/part0.xml", "/rec/part2.xml"]
        );
    }

    #[test]
    fn danmu_for_video_matches_by_spelling_then_stem() {
        let manifest = manifest();
        assert_eq!(
            manifest.danmu_for_video("/rec/part2b.mp4"),
            &["/rec/part2.xml".to_owned()]
        );
        // A relocated remux of the same recording resolves through its stem.
        assert_eq!(
            manifest.danmu_for_video("/archive/PART0.flv"),
            &["/rec/part0.xml".to_owned()]
        );
        assert!(manifest.danmu_for_video("/rec/part1.mp4").is_empty());
        assert!(manifest.danmu_for_video("/rec/unknown.mp4").is_empty());
    }

    #[test]
    fn pairs_never_cross_segments() {
        let manifest = manifest();
        assert_eq!(
            manifest.pairs().collect::<Vec<_>>(),
            vec![
                ("/rec/part0.mp4", "/rec/part0.xml"),
                ("/rec/part2.mp4", "/rec/part2.xml"),
                ("/rec/part2b.mp4", "/rec/part2.xml"),
            ]
        );
    }
}
