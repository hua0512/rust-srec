use std::fs;
use std::path::{Path, PathBuf};

const OUTER_LAYER_PREFIXES: &[&str] = &[
    "crate::api::",
    "crate::config::",
    "crate::credentials::",
    "crate::danmu::",
    "crate::database::",
    "crate::downloader::",
    "crate::metrics::",
    "crate::monitor::",
    "crate::notification::",
    "crate::pipeline::",
    "crate::scheduler::",
    "crate::services::",
    "crate::session::",
    "crate::streamer::",
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
            for forbidden in OUTER_LAYER_PREFIXES {
                if line.contains(forbidden) {
                    violations.push(format!(
                        "{}:{} references {forbidden}",
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
