//! Resolve HLS variables before resource URLs leave the server's playback context.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use crate::api::error::{ApiError, ApiResult};
use crate::services::playback_context::{PlaybackVariables, validate_playback_variables};

const MAX_EXPANDED_VALUE: usize = 16_384;
const MAX_VARIABLE_NAME: usize = 128;

fn invalid() -> ApiError {
    ApiError::bad_request("Invalid HLS variable definition or reference")
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_VARIABLE_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn attributes(input: &str) -> ApiResult<BTreeMap<&str, &str>> {
    let mut fields = BTreeMap::new();
    let mut input = input.trim();
    while !input.is_empty() {
        let (key, remainder) = input.split_once('=').ok_or_else(invalid)?;
        let key = key.trim();
        if !matches!(key, "NAME" | "VALUE" | "IMPORT" | "QUERYPARAM") {
            return Err(invalid());
        }
        let remainder = remainder
            .trim_start()
            .strip_prefix('"')
            .ok_or_else(invalid)?;
        let end = remainder.find('"').ok_or_else(invalid)?;
        let value = &remainder[..end];
        if value.len() > MAX_EXPANDED_VALUE
            || value.chars().any(char::is_control)
            || fields.insert(key, value).is_some()
        {
            return Err(invalid());
        }
        input = remainder[end + 1..].trim_start();
        if !input.is_empty() {
            input = input.strip_prefix(',').ok_or_else(invalid)?.trim_start();
            if input.is_empty() {
                return Err(invalid());
            }
        }
    }
    Ok(fields)
}

fn expand(input: &str, mut lookup: impl FnMut(&str) -> ApiResult<String>) -> ApiResult<String> {
    let mut output = String::new();
    let mut remaining = input;
    while let Some(start) = remaining.find("{$") {
        let rest = &remaining[start + 2..];
        let end = rest.find('}').ok_or_else(invalid)?;
        let name = &rest[..end];
        if !valid_name(name) {
            return Err(invalid());
        }
        output.push_str(&remaining[..start]);
        let value = lookup(name)?;
        if output.len().saturating_add(value.len()) > MAX_EXPANDED_VALUE {
            return Err(invalid());
        }
        output.push_str(&value);
        remaining = &rest[end + 1..];
    }
    if output.len().saturating_add(remaining.len()) > MAX_EXPANDED_VALUE {
        return Err(invalid());
    }
    output.push_str(remaining);
    Ok(output)
}

fn resolve(
    name: &str,
    raw: &PlaybackVariables,
    resolved: &mut PlaybackVariables,
    visiting: &mut HashSet<String>,
) -> ApiResult<String> {
    if let Some(value) = resolved.get(name) {
        return Ok(value.clone());
    }
    if !visiting.insert(name.to_owned()) {
        return Err(invalid());
    }
    let value = raw.get(name).ok_or_else(invalid)?;
    let value = expand(value, |name| resolve(name, raw, resolved, visiting))?;
    visiting.remove(name);
    resolved.insert(name.to_owned(), value.clone());
    validate_playback_variables(resolved)?;
    Ok(value)
}

pub(super) fn definitions(
    manifest: &str,
    final_url: &url::Url,
    parent: &PlaybackVariables,
) -> ApiResult<Arc<PlaybackVariables>> {
    validate_playback_variables(parent)?;
    let mut raw = PlaybackVariables::new();
    for line in manifest.lines().map(str::trim) {
        if !line.starts_with("#EXT-X-DEFINE") {
            continue;
        }
        let fields = attributes(line.strip_prefix("#EXT-X-DEFINE:").ok_or_else(invalid)?)?;
        let (name, value) =
            if fields.len() == 2 && fields.contains_key("NAME") && fields.contains_key("VALUE") {
                (fields["NAME"].to_owned(), fields["VALUE"].to_owned())
            } else if fields.len() == 1 && fields.contains_key("IMPORT") {
                let name = fields["IMPORT"];
                (
                    name.to_owned(),
                    parent.get(name).ok_or_else(invalid)?.clone(),
                )
            } else if fields.len() == 1 && fields.contains_key("QUERYPARAM") {
                let name = fields["QUERYPARAM"];
                let mut matches = final_url.query_pairs().filter(|(key, _)| key == name);
                let value = matches.next().ok_or_else(invalid)?.1.into_owned();
                if matches.next().is_some() {
                    return Err(invalid());
                }
                (name.to_owned(), value)
            } else {
                return Err(invalid());
            };
        if !valid_name(&name)
            || value.len() > MAX_EXPANDED_VALUE
            || raw.insert(name, value).is_some()
        {
            return Err(invalid());
        }
        validate_playback_variables(&raw)?;
    }
    let mut resolved = PlaybackVariables::new();
    for name in raw.keys() {
        resolve(name, &raw, &mut resolved, &mut HashSet::new())?;
    }
    Ok(Arc::new(resolved))
}

pub(super) fn expand_uri(uri: &str, variables: &PlaybackVariables) -> ApiResult<String> {
    expand(uri, |name| variables.get(name).cloned().ok_or_else(invalid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_expand_query_parameters_and_parent_imports_without_serialization() {
        let base = url::Url::parse("https://cdn.test/master.m3u8?auth=secret%2Btoken").unwrap();
        let parent = definitions("#EXT-X-DEFINE:QUERYPARAM=\"auth\"\n#EXT-X-DEFINE:NAME=\"path\",VALUE=\"video?sig={$auth}\"", &base, &PlaybackVariables::new()).unwrap();
        assert_eq!(
            expand_uri("{$path}", &parent).unwrap(),
            "video?sig=secret+token"
        );
        let child = definitions("#EXT-X-DEFINE:IMPORT=\"auth\"", &base, &parent).unwrap();
        assert_eq!(
            expand_uri("key?sig={$auth}", &child).unwrap(),
            "key?sig=secret+token"
        );
        assert!(
            expand_uri("{$path}", &child).is_err(),
            "imports must be explicit"
        );
    }

    #[test]
    fn malformed_missing_cyclic_and_oversized_definitions_fail_closed() {
        let base = url::Url::parse("https://cdn.test/master").unwrap();
        for manifest in [
            "#EXT-X-DEFINE:NAME=\"a\",VALUE=\"{$b}\"\n#EXT-X-DEFINE:NAME=\"b\",VALUE=\"{$a}\"",
            "#EXT-X-DEFINE:NAME=\"a\",VALUE=\"{$missing}\"",
            "#EXT-X-DEFINE:NAME=\"a\",VALUE=\"ok\"\n#EXT-X-DEFINE:NAME=\"a\",VALUE=\"duplicate\"",
            "#EXT-X-DEFINE:IMPORT=\"absent\"",
            "#EXT-X-DEFINE:QUERYPARAM=\"absent\"",
            "#EXT-X-DEFINE:NAME=unquoted,VALUE=\"value\"",
        ] {
            assert!(definitions(manifest, &base, &PlaybackVariables::new()).is_err());
        }
        let too_many = (0..=64)
            .map(|index| format!("#EXT-X-DEFINE:NAME=\"v{index}\",VALUE=\"value\"\n"))
            .collect::<String>();
        assert!(definitions(&too_many, &base, &PlaybackVariables::new()).is_err());
        let oversized = format!(
            "#EXT-X-DEFINE:NAME=\"a\",VALUE=\"{}\"",
            "x".repeat(MAX_EXPANDED_VALUE + 1)
        );
        assert!(definitions(&oversized, &base, &PlaybackVariables::new()).is_err());
    }
}
