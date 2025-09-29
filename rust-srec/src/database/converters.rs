use crate::database::repositories::errors::{RepositoryError, RepositoryResult};
use chrono::{DateTime, Utc};
use serde_json::Value;

/// Helper functions for converting between database string representations and domain types

pub fn datetime_to_string(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

pub fn string_to_datetime(s: &str) -> RepositoryResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| RepositoryError::Validation(format!("Invalid datetime format: {}", s)))
}

pub fn optional_datetime_to_string(dt: &Option<DateTime<Utc>>) -> Option<String> {
    dt.as_ref().map(datetime_to_string)
}

pub fn optional_string_to_datetime(s: &Option<String>) -> RepositoryResult<Option<DateTime<Utc>>> {
    match s {
        Some(s) => Ok(Some(string_to_datetime(s)?)),
        None => Ok(None),
    }
}

pub fn json_to_string<T: serde::Serialize>(value: &T) -> RepositoryResult<String> {
    serde_json::to_string(value).map_err(RepositoryError::from)
}

pub fn string_to_json<T: for<'de> serde::Deserialize<'de>>(s: &str) -> RepositoryResult<T> {
    serde_json::from_str(s).map_err(RepositoryError::from)
}

pub fn optional_json_to_string<T: serde::Serialize>(
    value: &Option<T>,
) -> RepositoryResult<Option<String>> {
    match value {
        Some(v) => Ok(Some(json_to_string(v)?)),
        None => Ok(None),
    }
}

pub fn optional_string_to_json<T: for<'de> serde::Deserialize<'de>>(
    s: &Option<String>,
) -> RepositoryResult<Option<T>> {
    match s {
        Some(s) => Ok(Some(string_to_json(s)?)),
        None => Ok(None),
    }
}

pub fn value_to_string(value: &Value) -> RepositoryResult<String> {
    serde_json::to_string(value).map_err(RepositoryError::from)
}

pub fn string_to_value(s: &str) -> RepositoryResult<Value> {
    serde_json::from_str(s).map_err(RepositoryError::from)
}

pub fn optional_value_to_string(value: &Option<Value>) -> RepositoryResult<Option<String>> {
    match value {
        Some(v) => Ok(Some(value_to_string(v)?)),
        None => Ok(None),
    }
}

pub fn optional_string_to_value(s: &Option<String>) -> RepositoryResult<Option<Value>> {
    match s {
        Some(s) => Ok(Some(string_to_value(s)?)),
        None => Ok(None),
    }
}
