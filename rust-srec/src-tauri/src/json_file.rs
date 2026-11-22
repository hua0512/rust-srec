//! The small JSON files the desktop shell keeps in the app data directory.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

use serde::Serialize;

/// Replace `path` with `value` as pretty-printed JSON.
///
/// The JSON is written and synced to `<path>.tmp` and then renamed over
/// `path`, which replaces an existing file on every platform, so a crash or a
/// concurrent reader sees either the old file or the new one. The temporary
/// name is fixed: callers must not write the same `path` from two threads at
/// once. On Unix the file is readable by its owner only.
pub(crate) fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = path.with_file_name(tmp_name);

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let written = options
        .open(&tmp_path)
        .and_then(|mut file| {
            file.write_all(&json)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp_path, path));
    if written.is_err()
        && let Err(error) = std::fs::remove_file(&tmp_path)
        && error.kind() != io::ErrorKind::NotFound
    {
        log::debug!(
            "Failed to remove temporary file '{}': {}",
            tmp_path.display(),
            error
        );
    }
    written
}
