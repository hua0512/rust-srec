//! JSON parsing/serialization helpers with consistent warning logs.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::warn;

#[derive(Debug, Clone, Copy)]
pub enum JsonContext<'a> {
    StreamerConfig {
        streamer_id: &'a str,
        scope: &'static str,
        scope_id: Option<&'a str>,
        field: &'static str,
    },
    StreamerField {
        streamer_id: &'a str,
        field: &'static str,
    },
    TemplateField {
        template_id: &'a str,
        field: &'static str,
    },
    JobField {
        job_id: &'a str,
        field: &'static str,
    },
    DagExecutionField {
        dag_execution_id: &'a str,
        field: &'static str,
    },
    DagStepExecutionField {
        dag_step_execution_id: &'a str,
        dag_execution_id: &'a str,
        step_id: &'a str,
        field: &'static str,
    },
    PipelinePresetField {
        pipeline_preset_id: &'a str,
        field: &'static str,
    },
    UserField {
        user_id: &'a str,
        field: &'static str,
    },
}

fn warn_parse_error(
    raw_len: usize,
    error: serde_json::Error,
    ctx: JsonContext<'_>,
    msg: &'static str,
) {
    match ctx {
        JsonContext::StreamerConfig {
            streamer_id,
            scope,
            scope_id,
            field,
        } => {
            warn!(
                streamer_id = %streamer_id,
                scope,
                scope_id,
                field,
                raw_len,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::StreamerField { streamer_id, field } => {
            warn!(
                streamer_id = %streamer_id,
                field,
                raw_len,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::TemplateField { template_id, field } => {
            warn!(
                template_id = %template_id,
                field,
                raw_len,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::JobField { job_id, field } => {
            warn!(
                job_id = %job_id,
                field,
                raw_len,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::DagExecutionField {
            dag_execution_id,
            field,
        } => {
            warn!(
                dag_execution_id = %dag_execution_id,
                field,
                raw_len,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::DagStepExecutionField {
            dag_step_execution_id,
            dag_execution_id,
            step_id,
            field,
        } => {
            warn!(
                dag_step_execution_id = %dag_step_execution_id,
                dag_execution_id = %dag_execution_id,
                step_id = %step_id,
                field,
                raw_len,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::PipelinePresetField {
            pipeline_preset_id,
            field,
        } => {
            warn!(
                pipeline_preset_id = %pipeline_preset_id,
                field,
                raw_len,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::UserField { user_id, field } => {
            warn!(
                user_id = %user_id,
                field,
                raw_len,
                error = %error,
                "{msg}"
            );
        }
    }
}

fn warn_serialize_error(error: serde_json::Error, ctx: JsonContext<'_>, msg: &'static str) {
    match ctx {
        JsonContext::StreamerConfig {
            streamer_id,
            scope,
            scope_id,
            field,
        } => {
            warn!(
                streamer_id = %streamer_id,
                scope,
                scope_id,
                field,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::StreamerField { streamer_id, field } => {
            warn!(
                streamer_id = %streamer_id,
                field,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::TemplateField { template_id, field } => {
            warn!(
                template_id = %template_id,
                field,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::JobField { job_id, field } => {
            warn!(
                job_id = %job_id,
                field,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::DagExecutionField {
            dag_execution_id,
            field,
        } => {
            warn!(
                dag_execution_id = %dag_execution_id,
                field,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::DagStepExecutionField {
            dag_step_execution_id,
            dag_execution_id,
            step_id,
            field,
        } => {
            warn!(
                dag_step_execution_id = %dag_step_execution_id,
                dag_execution_id = %dag_execution_id,
                step_id = %step_id,
                field,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::PipelinePresetField {
            pipeline_preset_id,
            field,
        } => {
            warn!(
                pipeline_preset_id = %pipeline_preset_id,
                field,
                error = %error,
                "{msg}"
            );
        }
        JsonContext::UserField { user_id, field } => {
            warn!(
                user_id = %user_id,
                field,
                error = %error,
                "{msg}"
            );
        }
    }
}

pub fn parse_optional<T: DeserializeOwned>(
    raw: Option<&str>,
    ctx: JsonContext<'_>,
    msg: &'static str,
) -> Option<T> {
    let raw = raw?;
    match serde_json::from_str(raw) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            warn_parse_error(raw.len(), error, ctx, msg);
            None
        }
    }
}

pub fn parse_optional_or_default<T: DeserializeOwned + Default>(
    raw: Option<&str>,
    ctx: JsonContext<'_>,
    msg: &'static str,
) -> T {
    parse_optional(raw, ctx, msg).unwrap_or_default()
}

pub fn parse_or_default<T: DeserializeOwned + Default>(
    raw: &str,
    ctx: JsonContext<'_>,
    msg: &'static str,
) -> T {
    // Treat empty string as "no value" - return default without warning
    if raw.is_empty() {
        return T::default();
    }
    match serde_json::from_str(raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            warn_parse_error(raw.len(), error, ctx, msg);
            T::default()
        }
    }
}

pub fn parse_optional_value_non_null(
    raw: Option<&str>,
    ctx: JsonContext<'_>,
    msg: &'static str,
) -> Option<serde_json::Value> {
    let raw = raw?;
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) if value.is_null() => None,
        Ok(value) => Some(value),
        Err(error) => {
            warn_parse_error(raw.len(), error, ctx, msg);
            None
        }
    }
}

pub fn to_string_or_fallback<T: Serialize + ?Sized>(
    value: &T,
    fallback: &'static str,
    ctx: JsonContext<'_>,
    msg: &'static str,
) -> String {
    match serde_json::to_string(value) {
        Ok(json) => json,
        Err(error) => {
            warn_serialize_error(error, ctx, msg);
            fallback.to_string()
        }
    }
}

pub fn to_string_option_or_warn<T: Serialize + ?Sized>(
    value: &T,
    ctx: JsonContext<'_>,
    msg: &'static str,
) -> Option<String> {
    match serde_json::to_string(value) {
        Ok(json) => Some(json),
        Err(error) => {
            warn_serialize_error(error, ctx, msg);
            None
        }
    }
}
