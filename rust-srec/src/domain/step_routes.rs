//! Stored pipeline documents, step references, and upload-step proxy routes.
//!
//! Only the places the runtime reads pipelines from are walked, so a value
//! shaped like a pipeline elsewhere is never taken for one. Every search for
//! step users, proxy references, route validation and backup rewriting
//! walks them here.

use serde_json::Value;

/// The keys a streamer-specific config or a template platform override
/// keeps its pipelines under: each segment, after each session, and after
/// each paired segment.
pub(crate) const PIPELINE_FIELD: &str = "pipeline";
pub(crate) const SESSION_COMPLETE_PIPELINE_FIELD: &str = "session_complete_pipeline";
pub(crate) const PAIRED_SEGMENT_PIPELINE_FIELD: &str = "paired_segment_pipeline";
pub(crate) const PIPELINE_FIELDS: [&str; 3] = [
    PIPELINE_FIELD,
    SESSION_COMPLETE_PIPELINE_FIELD,
    PAIRED_SEGMENT_PIPELINE_FIELD,
];

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

/// A pipeline step kind that stored pipelines reference by name.
#[derive(Clone, Copy, Debug)]
pub enum StepRef {
    JobPreset,
    Workflow,
}

impl StepRef {
    /// The `type` tag of a stored `PipelineStep`.
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Self::JobPreset => "preset",
            Self::Workflow => "workflow",
        }
    }
}

impl std::fmt::Display for StepRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.tag())
    }
}

/// A change to a job preset or workflow that stored pipelines can block.
#[derive(Clone, Copy, Debug)]
pub enum PresetChange {
    Delete,
    Rename,
}

impl std::fmt::Display for PresetChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Delete => "delete",
            Self::Rename => "rename",
        })
    }
}

/// What kind of configuration owns a pipeline that names a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepUserKind {
    Global,
    Platform,
    Template,
    Streamer,
    Workflow,
}

impl StepUserKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Platform => "platform",
            Self::Template => "template",
            Self::Streamer => "streamer",
            Self::Workflow => "workflow",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "global" => Some(Self::Global),
            "platform" => Some(Self::Platform),
            "template" => Some(Self::Template),
            "streamer" => Some(Self::Streamer),
            "workflow" => Some(Self::Workflow),
            _ => None,
        }
    }
}

/// A stored pipeline owner that names a step definition.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct StepUser {
    pub kind: StepUserKind,
    /// The owner's id; empty for the global config.
    pub id: String,
    /// The owner's name; empty for the global config.
    pub name: String,
    /// The owner is being removed but still runs the pipelines owed to its
    /// last session.
    pub removing: bool,
}

impl std::fmt::Display for StepUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            StepUserKind::Global => return f.write_str("the global config"),
            StepUserKind::Platform => "platform",
            StepUserKind::Template => "template",
            StepUserKind::Streamer => "streamer",
            StepUserKind::Workflow => "workflow",
        };
        write!(f, "{kind} '{}'", self.name)?;
        if self.removing {
            f.write_str(" (being removed)")?;
        }
        Ok(())
    }
}

/// How many users a message names before summarising the rest as a count.
const LISTED_USERS: usize = 5;

/// `users` as a comma-separated list of the first [`LISTED_USERS`], followed
/// by a count of the rest, so a widely used preset still yields a readable
/// message.
pub(crate) fn summarize_users(users: &[StepUser]) -> String {
    let mut labels: Vec<String> = users
        .iter()
        .take(LISTED_USERS)
        .map(ToString::to_string)
        .collect();
    if users.len() > LISTED_USERS {
        labels.push(format!("{} more", users.len() - LISTED_USERS));
    }
    labels.join(", ")
}
