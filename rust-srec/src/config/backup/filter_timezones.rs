use serde_json::Value;

use super::{schema_version_at_least, unwrap_json_value};

/// Backup version where omitted time-based zones mean UTC rather than local.
pub(crate) const EXPORT_SCHEMA_VERSION: &str = "0.1.8";
const UTC_DEFAULT_SCHEMA: (u32, u32, u32) = (0, 1, 8);

fn materialize_timezone(mut config: Value, timezone: &str) -> Value {
    if let Value::Object(object) = &mut config
        && object.get("timezone").is_none_or(Value::is_null)
    {
        object.insert("timezone".into(), Value::String(timezone.to_owned()));
    }
    config
}

/// Legacy backup omissions retain their original server-local semantics.
/// Change only the timezone member, retaining extensions and invalid shapes for
/// the existing validation path to report rather than inventing a configuration.
pub(crate) fn import_filter_config(version: &str, filter_type: &str, config: Value) -> Value {
    let config = unwrap_json_value(config);
    if filter_type == "TIME_BASED" && !schema_version_at_least(version, UTC_DEFAULT_SCHEMA) {
        materialize_timezone(config, "local")
    } else {
        config
    }
}

/// Explicit export defaults make bundles independent of the reader's default.
pub(crate) fn export_filter_config(filter_type: &str, config: Value) -> Value {
    let config = unwrap_json_value(config);
    if matches!(filter_type, "TIME_BASED" | "CRON") {
        materialize_timezone(config, "UTC")
    } else {
        config
    }
}

#[cfg(test)]
mod tests;
