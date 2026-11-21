//! The pipeline steps inside stored configuration, and the proxy routes of
//! the upload steps among them: the `proxy_route` of an inline rclone or
//! BaiduPCS-Go step's config, and of a job preset's config for those
//! processors.
//!
//! Only the places the runtime reads pipelines from are walked, so a value
//! shaped like a pipeline elsewhere is never taken for one. Every search for
//! step users, proxy references, route validation and backup rewriting
//! walks them here.

use serde_json::Value;

use crate::config::PIPELINE_FIELDS;

/// The job types of the processors that take a proxy route.
const UPLOAD_PROCESSORS: [&str; 3] = ["rclone", "upload", "baidupcs"];

const ROUTE_KEY: &str = "proxy_route";

/// Where a stored document keeps its pipelines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// The document is a pipeline: a pipeline column or a workflow
    /// definition.
    Pipeline,
    /// A streamer-specific config, with pipelines under its
    /// [`PIPELINE_FIELDS`].
    Config,
    /// Template platform overrides: per platform, pipelines under its
    /// [`PIPELINE_FIELDS`].
    Overrides,
}

/// The steps (`steps[N].step`) of the pipelines in `document`.
pub(crate) fn pipeline_steps_mut(document: &mut Value, layout: Layout) -> Vec<&mut Value> {
    let pipelines: Vec<&mut Value> = match layout {
        Layout::Pipeline => vec![document],
        Layout::Config => config_pipelines(document),
        Layout::Overrides => match document {
            Value::Object(platforms) => platforms.values_mut().flat_map(config_pipelines).collect(),
            _ => Vec::new(),
        },
    };
    pipelines
        .into_iter()
        .filter_map(|pipeline| pipeline.get_mut("steps")?.as_array_mut())
        .flatten()
        .filter_map(|entry| entry.get_mut("step").filter(|step| step.is_object()))
        .collect()
}

/// The route values of the inline upload steps in `document`.
pub(crate) fn pipeline_routes_mut(document: &mut Value, layout: Layout) -> Vec<&mut Value> {
    pipeline_steps_mut(document, layout)
        .into_iter()
        .filter_map(|step| {
            let upload = step.get("type").and_then(Value::as_str) == Some("inline")
                && step
                    .get("processor")
                    .and_then(Value::as_str)
                    .is_some_and(is_upload);
            upload.then(|| step.get_mut("config")?.get_mut(ROUTE_KEY))?
        })
        .collect()
}

/// The route value of a job preset's config, if it is an upload preset
/// that sets one.
pub(crate) fn preset_route_mut<'a>(
    processor: &str,
    config: &'a mut Value,
) -> Option<&'a mut Value> {
    is_upload(processor).then(|| config.get_mut(ROUTE_KEY))?
}

fn is_upload(processor: &str) -> bool {
    UPLOAD_PROCESSORS.contains(&processor)
}

fn config_pipelines(config: &mut Value) -> Vec<&mut Value> {
    match config {
        Value::Object(fields) => fields
            .iter_mut()
            .filter(|(key, _)| PIPELINE_FIELDS.contains(&key.as_str()))
            .map(|(_, pipeline)| pipeline)
            .collect(),
        _ => Vec::new(),
    }
}
