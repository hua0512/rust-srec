//! The proxy routes of upload steps inside stored configuration: the
//! `proxy_route` of an inline step's config in a pipeline, and of a job
//! preset's config.
//!
//! Pipelines keep their steps at `steps[N].step`, at the top of the document
//! or under any key ending in `pipeline` nested in it (template platform
//! overrides and streamer-specific configs). The proxy references query
//! (`preset::names_step`) searches the same places.

use serde_json::Value;

/// The route values of the inline steps in a pipeline document.
pub(crate) fn pipeline_routes_mut(document: &mut Value) -> Vec<&mut Value> {
    let mut routes = Vec::new();
    visit(document, true, &mut routes);
    routes
}

/// The route value of a job preset's config, if it sets one.
pub(crate) fn preset_route_mut(config: &mut Value) -> Option<&mut Value> {
    config.get_mut(ROUTE_KEY)
}

const ROUTE_KEY: &str = "proxy_route";

fn visit<'a>(value: &'a mut Value, pipeline: bool, routes: &mut Vec<&'a mut Value>) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields.iter_mut() {
                if pipeline && key == "steps" && child.is_array() {
                    if let Value::Array(steps) = child {
                        routes.extend(steps.iter_mut().filter_map(inline_route));
                    }
                } else {
                    visit(child, key.ends_with("pipeline"), routes);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                visit(item, false, routes);
            }
        }
        _ => {}
    }
}

fn inline_route(entry: &mut Value) -> Option<&mut Value> {
    let step = entry.get_mut("step")?;
    if step.get("type").and_then(Value::as_str) != Some("inline") {
        return None;
    }
    step.get_mut("config")?.get_mut(ROUTE_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_found_where_pipelines_keep_inline_steps() {
        let route = |id: &str| serde_json::json!({"kind": "proxy", "id": id});
        let step = |id: &str| serde_json::json!({"id": id, "step": {"type": "inline", "processor": "rclone", "config": {"proxy_route": route(id)}}});
        let mut document = serde_json::json!({
            "steps": [step("top"), {"id": "p", "step": {"type": "preset", "name": "x", "config": {"proxy_route": route("named")}}}],
            "huya": {"session_complete_pipeline": {"steps": [step("nested")]}},
            "other": {"steps": [step("not-a-pipeline")]},
        });
        let mut found: Vec<String> = pipeline_routes_mut(&mut document)
            .into_iter()
            .map(|route| route["id"].as_str().unwrap().to_owned())
            .collect();
        found.sort();
        assert_eq!(found, ["nested", "top"]);
    }
}
