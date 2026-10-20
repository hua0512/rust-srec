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
