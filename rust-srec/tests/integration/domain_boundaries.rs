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
