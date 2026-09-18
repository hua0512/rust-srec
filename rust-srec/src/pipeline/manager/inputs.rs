use crate::pipeline::manifest::{ManifestScope, ManifestSegment, PipelineInputManifest};

/// Flatten `segments` into the DAG's input list and build the manifest that
/// records which danmu file belongs to which video.
///
/// Inputs are every video path in segment order followed by every danmu path,
/// so `{input}` in an execute step and the first input of any processor is the
/// first recording. Pairing is delivered through the manifest, which is stored
/// on the DAG row; nothing is written next to the recordings.
pub(super) fn build_pipeline_inputs(
    session_id: &str,
    streamer_id: &str,
    scope: ManifestScope,
    segments: Vec<ManifestSegment>,
) -> (Vec<String>, PipelineInputManifest) {
    let manifest = PipelineInputManifest::new(session_id, streamer_id, scope, segments);
    let inputs = manifest
        .video_inputs()
        .chain(manifest.danmu_inputs())
        .map(str::to_owned)
        .collect();
    (inputs, manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(index: u32, video: &[&str], danmu: &[&str]) -> ManifestSegment {
        ManifestSegment {
            segment_index: index,
            video: video.iter().map(|path| path.to_string()).collect(),
            danmu: danmu.iter().map(|path| path.to_string()).collect(),
        }
    }

    #[test]
    fn inputs_list_videos_in_segment_order_then_danmu() {
        let (inputs, manifest) = build_pipeline_inputs(
            "session",
            "streamer",
            ManifestScope::Session,
            vec![
                segment(2, &["/rec/2.mp4"], &["/rec/2.xml"]),
                segment(0, &["/rec/0.mp4"], &["/rec/0.xml"]),
                segment(1, &["/rec/1.mp4"], &[]),
            ],
        );
        assert_eq!(
            inputs,
            vec![
                "/rec/0.mp4",
                "/rec/1.mp4",
                "/rec/2.mp4",
                "/rec/0.xml",
                "/rec/2.xml"
            ]
        );
        assert_eq!(manifest.session_id, "session");
        assert_eq!(manifest.streamer_id, "streamer");
        assert_eq!(manifest.scope, ManifestScope::Session);
        assert!(manifest.danmu_for_video("/rec/1.mp4").is_empty());
        assert_eq!(
            manifest.danmu_for_video("/rec/2.mp4"),
            &["/rec/2.xml".to_owned()]
        );
    }

    #[test]
    fn paired_segment_keeps_collected_order_and_scope() {
        let (inputs, manifest) = build_pipeline_inputs(
            "session",
            "streamer",
            ManifestScope::Segment { index: 7 },
            vec![segment(
                7,
                &["/rec/second.mp4", "/rec/first.mp4"],
                &["/rec/chat.xml"],
            )],
        );
        assert_eq!(
            inputs,
            vec!["/rec/second.mp4", "/rec/first.mp4", "/rec/chat.xml"]
        );
        assert_eq!(manifest.scope, ManifestScope::Segment { index: 7 });
        assert_eq!(manifest.segments.len(), 1);

        let (inputs, manifest) =
            build_pipeline_inputs("session", "streamer", ManifestScope::Session, Vec::new());
        assert!(inputs.is_empty());
        assert!(manifest.is_empty());
    }
}
