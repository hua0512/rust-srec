use std::fs;
use std::path::{Path, PathBuf};

const OUTER_LAYER_MODULES: &[&str] = &[
    "api",
    "config",
    "credentials",
    "danmu",
    "database",
    "downloader",
    "metrics",
    "monitor",
    "notification",
    "pipeline",
    "scheduler",
    "services",
    "session",
    "streamer",
];

fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read domain directory") {
        let path = entry.expect("read domain entry").path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// Whether `line` mentions `crate::<module>` as a full path segment.
///
/// Requires the character after the module name to not be part of an
/// identifier, so `use crate::api;`, `use crate::api::x`, and
/// `crate::api as y` all count while `crate::api_types` does not.
fn references_module(line: &str, module: &str) -> bool {
    let needle = format!("crate::{module}");
    let mut search_start = 0;
    while let Some(position) = line[search_start..].find(&needle) {
        let end = search_start + position + needle.len();
        let boundary = line[end..].chars().next();
        if boundary.is_none_or(|character| !(character.is_ascii_alphanumeric() || character == '_'))
        {
            return true;
        }
        search_start = end;
    }
    false
}

#[test]
fn domain_does_not_depend_on_outer_application_layers() {
    let domain = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/domain");
    let mut files = Vec::new();
    rust_files(&domain, &mut files);
    files.sort();

    let mut violations = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file).expect("read domain source");
        for (index, line) in source.lines().enumerate() {
            for module in OUTER_LAYER_MODULES {
                if references_module(line, module) {
                    violations.push(format!(
                        "{}:{} references crate::{module}",
                        file.display(),
                        index + 1
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "domain must remain inward-facing:\n{}",
        violations.join("\n")
    );
}

fn is_test_source(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "tests.rs" || name.ends_with("_tests.rs") || name == "test_support.rs" {
        return true;
    }
    path.components()
        .any(|component| component.as_os_str() == "tests")
}

/// Collect production lines of `source`, skipping `#[cfg(test)]` items/blocks.
fn production_lines(source: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut skip_depth: Option<i32> = None;
    let mut pending_cfg_test = false;

    for (idx, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(depth) = skip_depth.as_mut() {
            *depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
            if *depth <= 0 && (line.contains('}') || (*depth == 0 && trimmed.ends_with(';'))) {
                skip_depth = None;
            }
            continue;
        }
        if trimmed.starts_with("#[cfg(test)]") {
            pending_cfg_test = true;
            continue;
        }
        if pending_cfg_test {
            if trimmed.starts_with("#[") {
                continue;
            }
            if line.contains('{') {
                let depth = line.matches('{').count() as i32 - line.matches('}').count() as i32;
                if depth > 0 {
                    skip_depth = Some(depth);
                }
                pending_cfg_test = false;
                continue;
            }
            if trimmed.ends_with(';') {
                pending_cfg_test = false;
                continue;
            }
            continue;
        }
        out.push((idx + 1, line));
    }

    out
}

fn references_identifier(line: &str, ident: &str) -> bool {
    let mut search_start = 0;
    while let Some(pos) = line[search_start..].find(ident) {
        let start = search_start + pos;
        let end = start + ident.len();
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        let before_ok = before.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        let after_ok = after.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if before_ok && after_ok {
            return true;
        }
        search_start = end;
    }
    false
}

fn collect_production_violations(
    relative_root: &str,
    forbidden_modules: &[&str],
    forbidden_identifiers: &[&str],
) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_root);
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    files.sort();

    let mut violations = Vec::new();
    for file in files {
        if is_test_source(&file) {
            continue;
        }
        let source = fs::read_to_string(&file).expect("read source file");
        for (line_no, line) in production_lines(&source) {
            for module in forbidden_modules {
                if references_module(line, module) {
                    violations.push(format!(
                        "{}:{} references crate::{module}",
                        file.display(),
                        line_no
                    ));
                }
            }
            for ident in forbidden_identifiers {
                if references_identifier(line, ident) {
                    violations.push(format!(
                        "{}:{} references `{ident}`",
                        file.display(),
                        line_no
                    ));
                }
            }
        }
    }
    violations
}

#[test]
fn database_production_code_has_no_outer_module_dependencies() {
    const FORBIDDEN: &[&str] = &[
        "api",
        "baidupcs",
        "config",
        "credentials",
        "danmu",
        "downloader",
        "monitor",
        "notification",
        "pipeline",
        "proxies",
        "recorder",
        "services",
        "session",
        "streamer",
    ];
    let violations = collect_production_violations("src/database", FORBIDDEN, &[]);
    assert!(
        violations.is_empty(),
        "database production code must not depend on outer modules:\n{}",
        violations.join("\n")
    );
}

#[test]
fn pipeline_production_code_does_not_depend_on_outer_runtime_modules() {
    const FORBIDDEN: &[&str] = &[
        "api",
        "config",
        "credentials",
        "danmu",
        "downloader",
        "monitor",
        "notification",
        "proxies",
        "recorder",
        "services",
        "session",
        "streamer",
    ];
    let violations = collect_production_violations("src/pipeline", FORBIDDEN, &[]);
    assert!(
        violations.is_empty(),
        "pipeline production code must depend only on ports and domain contracts:\n{}",
        violations.join("\n")
    );
}

#[test]
fn pipeline_processors_have_no_sqlx_database_notification_or_credential_deps() {
    const FORBIDDEN_MODULES: &[&str] = &[
        "config",
        "credentials",
        "database",
        "downloader",
        "notification",
        "proxies",
        "recorder",
        "services",
        "streamer",
    ];
    let violations =
        collect_production_violations("src/pipeline/processors", FORBIDDEN_MODULES, &["sqlx"]);
    assert!(
        violations.is_empty(),
        "pipeline processors must stay decoupled from sqlx, database, notification, credentials, and proxies:\n{}",
        violations.join("\n")
    );
}
