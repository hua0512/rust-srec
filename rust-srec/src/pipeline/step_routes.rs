//! The pipeline steps inside stored configuration, and the proxy routes of
//! the upload steps among them: the `proxy_route` of an inline rclone or
//! BaiduPCS-Go step's config, and of a job preset's config for those
//! processors.
//!
//! Pipelines keep their steps at `steps[N].step`, at the top of the document
//! or under any key ending in `pipeline` nested in it (template platform
//! overrides and streamer-specific configs). Every search for step users,
//! proxy references, route validation and backup rewriting walks them here.

use serde_json::Value;

/// The job types of the processors that take a proxy route.
const UPLOAD_PROCESSORS: [&str; 3] = ["rclone", "upload", "baidupcs"];

const ROUTE_KEY: &str = "proxy_route";

/// The steps (`steps[N].step`) of the pipelines in `document`.
pub(crate) fn pipeline_steps_mut(document: &mut Value) -> Vec<&mut Value> {
    let mut steps = Vec::new();
    visit(document, true, &mut steps);
    steps
}

/// The route values of the inline upload steps in a pipeline document.
pub(crate) fn pipeline_routes_mut(document: &mut Value) -> Vec<&mut Value> {
    pipeline_steps_mut(document)
        .into_iter()
        .filter_map(|step| {
            if step.get("type").and_then(Value::as_str) != Some("inline") {
                return None;
            }
            let upload = step
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

fn visit<'a>(value: &'a mut Value, pipeline: bool, steps: &mut Vec<&'a mut Value>) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields.iter_mut() {
                if pipeline && key == "steps" && child.is_array() {
                    if let Value::Array(entries) = child {
                        steps.extend(entries.iter_mut().filter_map(|entry| {
                            entry.get_mut("step").filter(|step| step.is_object())
                        }));
                    }
                } else {
                    visit(child, key.ends_with("pipeline"), steps);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                visit(item, false, steps);
            }
        }
        _ => {}
    }
}
