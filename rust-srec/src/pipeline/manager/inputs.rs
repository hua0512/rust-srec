use std::path::{Path, PathBuf};

use serde::Serialize;
use tracing::warn;

use crate::utils::filename::sanitize_filename;

#[derive(Serialize)]
struct InputManifest<'a> {
    session_id: &'a str,
    streamer_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    segment_index: Option<u32>,
    video_inputs: &'a [String],
    danmu_inputs: &'a [String],
}

/// Preserve the caller's artifact order and prepend a manifest only after its
/// write succeeds. A failed manifest leaves the source inputs usable, but the
/// false completion flag tells recovery that this dispatch was incomplete.
pub(super) async fn prepare_pipeline_inputs(
    session_id: &str,
    streamer_id: &str,
    segment_index: Option<u32>,
    video: Vec<PathBuf>,
    danmu: Vec<PathBuf>,
) -> (Vec<String>, bool) {
    let video_inputs: Vec<_> = video
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let danmu_inputs: Vec<_> = danmu
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let mut inputs = Vec::with_capacity(video_inputs.len() + danmu_inputs.len() + 1);
    let mut complete = true;
    if let Some(directory) = video
        .first()
        .or_else(|| danmu.first())
        .and_then(|path| path.parent())
    {
        let name = match segment_index {
            Some(index) => format!(
                "segment_{}_{index}_inputs.json",
                sanitize_filename(session_id)
            ),
            None => format!("session_{}_inputs.json", sanitize_filename(session_id)),
        };
        let path = directory.join(name);
        let manifest = InputManifest {
            session_id,
            streamer_id,
            segment_index,
            video_inputs: &video_inputs,
            danmu_inputs: &danmu_inputs,
        };
        match write_manifest(&path, &manifest).await {
            Ok(()) => inputs.push(path.to_string_lossy().into_owned()),
            Err(error) => {
                complete = false;
                warn!(session_id, ?segment_index, path = %path.display(), %error,
                    "Failed to write pipeline input manifest (continuing without manifest)");
            }
        }
    }
    inputs.extend(video_inputs);
    inputs.extend(danmu_inputs);
    (inputs, complete)
}

async fn write_manifest(path: &Path, manifest: &InputManifest<'_>) -> crate::Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|error| crate::Error::io_path("writing pipeline input manifest", path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_manifest_preserves_scope_schema_and_video_then_danmu_order() {
        let directory = tempfile::tempdir().unwrap();
        let video = vec![
            directory.path().join("second.mp4"),
            directory.path().join("first.mp4"),
        ];
        let danmu = vec![directory.path().join("chat.xml")];
        for segment_index in [None, Some(7)] {
            let (inputs, complete) = prepare_pipeline_inputs(
                "session",
                "streamer",
                segment_index,
                video.clone(),
                danmu.clone(),
            )
            .await;
            assert!(complete);
            assert_eq!(inputs.len(), 4);
            let expected_name = if segment_index.is_some() {
                "segment_session_7_inputs.json"
            } else {
                "session_session_inputs.json"
            };
            assert_eq!(Path::new(&inputs[0]).file_name().unwrap(), expected_name);
            let manifest: serde_json::Value =
                serde_json::from_slice(&tokio::fs::read(&inputs[0]).await.unwrap()).unwrap();
            assert_eq!(manifest["session_id"], "session");
            assert_eq!(manifest["streamer_id"], "streamer");
            assert_eq!(
                manifest.get("segment_index"),
                segment_index.map(serde_json::Value::from).as_ref()
            );
            assert_eq!(manifest["video_inputs"], serde_json::json!(&inputs[1..3]));
            assert_eq!(manifest["danmu_inputs"], serde_json::json!(&inputs[3..]));
        }
    }

    #[tokio::test]
    async fn manifest_failure_and_absent_inputs_keep_the_dispatch_contract() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing").join("input.xml");
        for segment_index in [None, Some(2)] {
            let (inputs, complete) = prepare_pipeline_inputs(
                "session",
                "streamer",
                segment_index,
                Vec::new(),
                vec![missing.clone()],
            )
            .await;
            assert!(!complete);
            assert_eq!(inputs, vec![missing.to_string_lossy().into_owned()]);
            let (inputs, complete) = prepare_pipeline_inputs(
                "session",
                "streamer",
                segment_index,
                Vec::new(),
                Vec::new(),
            )
            .await;
            assert!(complete);
            assert!(inputs.is_empty());
        }
    }
}
