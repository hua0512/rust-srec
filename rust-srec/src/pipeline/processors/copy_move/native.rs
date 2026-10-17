//! Whole-file copies that stop between transfers when cancelled.
//!
//! Linux and Windows check for cancellation between chunks; a blocked transfer
//! still has to return before the next check. macOS and other platforms keep
//! std's native copy, with its cloning and metadata, which cannot stop
//! mid-file.

use std::io;
use std::path::Path;

/// Copy `source` over `destination` and return the default stream's byte
/// count. Once `is_cancelled` returns `true` the copy stops with a
/// "Copy cancelled" error and may leave a partial `destination` for the caller
/// to remove.
pub(super) fn copy_file(
    source: &Path,
    destination: &Path,
    is_cancelled: impl FnMut() -> bool,
) -> io::Result<u64> {
    #[cfg(target_os = "linux")]
    {
        copy_linux(source, destination, is_cancelled)
    }
    #[cfg(windows)]
    {
        copy_windows(source, destination, is_cancelled)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = is_cancelled;
        std::fs::copy(source, destination)
    }
}

/// Retain Linux's kernel-assisted copy path while checking cancellation
/// between bounded transfers.
#[cfg(target_os = "linux")]
fn copy_linux(
    source: &Path,
    destination: &Path,
    mut is_cancelled: impl FnMut() -> bool,
) -> io::Result<u64> {
    use std::fs::{File, OpenOptions};
    use std::io::Read;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

    let mut reader = File::open(source)?;
    let metadata = reader.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the source path is neither a regular file nor a symlink to a regular file",
        ));
    }
    let permissions = metadata.permissions();
    let mut writer = OpenOptions::new()
        .mode(permissions.mode())
        .write(true)
        .create(true)
        .truncate(true)
        .open(destination)?;
    if writer.metadata()?.is_file() {
        writer.set_permissions(permissions)?;
    }

    let mut copied = 0;
    loop {
        if is_cancelled() {
            return Err(io::Error::other("Copy cancelled"));
        }
        // Take<File> retains std::io::copy's Linux copy_file_range /
        // sendfile / splice specialization and fallback handling.
        let bytes = io::copy(&mut (&mut reader).take(CHUNK_SIZE), &mut writer)?;
        copied += bytes;
        if bytes < CHUNK_SIZE {
            return Ok(copied);
        }
    }
}

/// Copy through `CopyFileExW`, the call `std::fs::copy` makes on Windows, so
/// attributes, alternate data streams and metadata are copied the same way and
/// the result is the default stream's byte count. Windows invokes the progress
/// routine on this thread before the first transfer and after each chunk;
/// returning `PROGRESS_CANCEL` stops the copy, and Windows then deletes the
/// partial destination itself.
#[cfg(windows)]
fn copy_windows<F: FnMut() -> bool>(
    source: &Path,
    destination: &Path,
    is_cancelled: F,
) -> io::Result<u64> {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{ERROR_REQUEST_ABORTED, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        COPYPROGRESSROUTINE_PROGRESS, CopyFileExW, LPPROGRESS_ROUTINE_CALLBACK_REASON,
        PROGRESS_CANCEL, PROGRESS_CONTINUE,
    };

    struct Progress<F> {
        is_cancelled: F,
        copied: i64,
    }

    // The routine must not unwind across the FFI boundary; the production
    // closure only reads a cancellation token.
    unsafe extern "system" fn progress_routine<F: FnMut() -> bool>(
        _total_file_size: i64,
        _total_bytes_transferred: i64,
        _stream_size: i64,
        stream_bytes_transferred: i64,
        stream_number: u32,
        _reason: LPPROGRESS_ROUTINE_CALLBACK_REASON,
        _source_file: HANDLE,
        _destination_file: HANDLE,
        data: *const c_void,
    ) -> COPYPROGRESSROUTINE_PROGRESS {
        // SAFETY: `data` points to the `Progress<F>` that `copy_windows`
        // exclusively borrows for the whole `CopyFileExW` call, which runs this
        // routine synchronously on the calling thread and never after it
        // returns.
        let progress = unsafe { &mut *data.cast_mut().cast::<Progress<F>>() };
        if stream_number == 1 {
            progress.copied = stream_bytes_transferred;
        }
        if (progress.is_cancelled)() {
            PROGRESS_CANCEL
        } else {
            PROGRESS_CONTINUE
        }
    }

    let source = windows_long_path(source)?;
    let destination = windows_long_path(destination)?;
    let mut progress = Progress {
        is_cancelled,
        copied: 0,
    };
    // SAFETY: both paths are NUL-terminated UTF-16 buffers that outlive the
    // call, and `progress` matches the routine's type parameter.
    let copied = unsafe {
        CopyFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            Some(progress_routine::<F>),
            (&raw mut progress).cast::<c_void>().cast_const(),
            std::ptr::null_mut(),
            0,
        )
    };
    if copied == 0 {
        let error = io::Error::last_os_error();
        return Err(
            if error.raw_os_error() == Some(ERROR_REQUEST_ABORTED as i32) {
                io::Error::other("Copy cancelled")
            } else {
                error
            },
        );
    }
    Ok(progress.copied as u64)
}

/// Convert a path for a direct Win32 call the way std does for its own file
/// APIs. The path is made absolute, which also normalizes it; one at or past
/// the legacy length limit then gets the `\\?\` or `\\?\UNC\` prefix, which
/// lifts that limit but disables Win32's own normalization. The result is
/// NUL-terminated UTF-16.
#[cfg(windows)]
fn windows_long_path(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;

    const LEGACY_MAX_PATH: usize = 248;
    const SEP: u16 = b'\\' as u16;
    const QUERY: u16 = b'?' as u16;
    const VERBATIM: [u16; 4] = [SEP, SEP, QUERY, SEP];
    const NT: [u16; 4] = [SEP, QUERY, QUERY, SEP];
    const DEVICE: [u16; 4] = [SEP, SEP, b'.' as u16, SEP];

    let absolute = std::path::absolute(path)?;
    let wide: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains an interior NUL",
        ));
    }
    let mut converted: Vec<u16> =
        if wide.len() < LEGACY_MAX_PATH || wide.starts_with(&VERBATIM) || wide.starts_with(&NT) {
            wide
        } else if let Some(device) = wide.strip_prefix(&DEVICE[..]) {
            VERBATIM.iter().chain(device).copied().collect()
        } else if let Some(share) = wide.strip_prefix(&[SEP, SEP][..]) {
            r"\\?\UNC\"
                .encode_utf16()
                .chain(share.iter().copied())
                .collect()
        } else {
            VERBATIM.iter().chain(&wide).copied().collect()
        };
    converted.push(0);
    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::processors::outputs::TempOutputGuard;
    use tempfile::TempDir;

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_copy_cancels_after_a_chunk_and_cleans_the_unpublished_output() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        let chunk_size = 8 * 1024 * 1024;
        let contents = vec![b'x'; 2 * chunk_size + 1];
        std::fs::write(&source, &contents).unwrap();
        std::fs::write(&destination, b"original").unwrap();

        let staged_path;
        {
            let mut staged = TempOutputGuard::new(&destination);
            staged.cleanup_on_current_thread = true;
            staged_path = staged.path().to_path_buf();
            // Cancel as soon as a complete chunk is visible. This executes in
            // the copy thread and does not depend on another task's timing.
            let result = copy_file(&source, &staged_path, || {
                std::fs::metadata(&staged_path)
                    .is_ok_and(|metadata| metadata.len() >= chunk_size as u64)
            });

            assert_eq!(result.unwrap_err().to_string(), "Copy cancelled");
            assert_eq!(
                std::fs::metadata(&staged_path).unwrap().len(),
                chunk_size as u64
            );
        }

        assert!(!staged_path.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        assert_eq!(std::fs::read(&source).unwrap(), contents);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_copy_preserves_content_permissions_and_truncation_through_source_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let link = directory.path().join("source-link");
        let destination = directory.path().join("destination");
        symlink(&source, &link).unwrap();

        // Empty, exact-chunk and partial final transfers must all reach EOF.
        for size in [0, 8 * 1024 * 1024, 8 * 1024 * 1024 + 1] {
            let contents = vec![b'x'; size];
            std::fs::write(&source, &contents).unwrap();
            std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o440)).unwrap();
            std::fs::write(&destination, vec![b'y'; size + 1]).unwrap();

            let copied = copy_file(&link, &destination, || false).unwrap();

            assert_eq!(copied, size as u64);
            assert_eq!(std::fs::read(&destination).unwrap(), contents);
            assert_eq!(
                std::fs::metadata(&destination)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o440
            );
            std::fs::remove_file(&source).unwrap();
            std::fs::remove_file(&destination).unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_copy_reads_to_eof_if_the_source_grows_between_chunks() {
        use std::io::Write;

        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        let chunk_size = 8 * 1024 * 1024;
        std::fs::write(&source, vec![b'x'; chunk_size]).unwrap();
        let mut appended = false;

        let copied = copy_file(&source, &destination, || {
            if !appended
                && std::fs::metadata(&destination)
                    .is_ok_and(|metadata| metadata.len() >= chunk_size as u64)
            {
                let mut growing_source = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&source)
                    .unwrap();
                growing_source.write_all(b"appended").unwrap();
                appended = true;
            }
            false
        })
        .unwrap();

        assert!(appended);
        assert_eq!(copied, chunk_size as u64 + 8);
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            std::fs::read(&source).unwrap()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_copy_rejects_a_non_file_source_before_truncating_destination() {
        let directory = TempDir::new().unwrap();
        let destination = directory.path().join("destination");
        std::fs::write(&destination, b"original").unwrap();

        let error = copy_file(directory.path(), &destination, || false).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    }

    #[cfg(windows)]
    #[test]
    fn windows_copy_cancels_after_a_chunk_and_leaves_no_staged_output() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        let contents = vec![b'x'; 64 * 1024 * 1024 + 1];
        std::fs::write(&source, &contents).unwrap();
        std::fs::write(&destination, b"original").unwrap();

        // The routine runs once before the first transfer and after every
        // chunk, so a complete copy of this file sees it many times.
        let mut complete_calls = 0;
        let copied = copy_file(&source, &directory.path().join("complete"), || {
            complete_calls += 1;
            false
        })
        .unwrap();
        assert_eq!(copied, contents.len() as u64);
        assert!(complete_calls > 2, "{complete_calls}");

        let staged_path;
        let mut calls = 0;
        {
            let mut staged = TempOutputGuard::new(&destination);
            staged.cleanup_on_current_thread = true;
            staged_path = staged.path().to_path_buf();
            // Cancel after the first chunk. This executes in the copy thread
            // and does not depend on another task's timing.
            let result = copy_file(&source, &staged_path, || {
                calls += 1;
                calls == 2
            });

            assert_eq!(result.unwrap_err().to_string(), "Copy cancelled");
            // Windows removes the partial destination itself; the guard's
            // cleanup must tolerate that.
            assert!(!staged_path.exists());
        }

        assert_eq!(calls, 2);
        assert!(!staged_path.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        assert_eq!(std::fs::read(&source).unwrap(), contents);
    }

    #[cfg(windows)]
    #[test]
    fn windows_copy_returns_the_byte_count_and_identical_content() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");

        for size in [0, 1, 4 * 1024 * 1024 + 7] {
            let contents = vec![b'x'; size];
            std::fs::write(&source, &contents).unwrap();
            std::fs::write(&destination, vec![b'y'; size + 1]).unwrap();

            let copied = copy_file(&source, &destination, || false).unwrap();

            assert_eq!(copied, size as u64);
            assert_eq!(std::fs::read(&destination).unwrap(), contents);
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_copy_reaches_destinations_past_the_legacy_path_limit() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        std::fs::write(&source, b"contents").unwrap();
        let nested = directory.path().join("a".repeat(120)).join("b".repeat(120));
        std::fs::create_dir_all(&nested).unwrap();
        let destination = nested.join(format!("{}.mp4", "c".repeat(60)));
        assert!(destination.as_os_str().len() > 260);

        let copied = copy_file(&source, &destination, || false).unwrap();

        assert_eq!(copied, 8);
        assert_eq!(std::fs::read(&destination).unwrap(), b"contents");
    }

    #[cfg(windows)]
    #[test]
    fn windows_long_paths_get_the_verbatim_prefix_after_normalization() {
        let convert = |path: &str| {
            let wide = windows_long_path(Path::new(path)).unwrap();
            assert_eq!(wide.last(), Some(&0));
            String::from_utf16(&wide[..wide.len() - 1]).unwrap()
        };
        let long = "d".repeat(260);

        assert_eq!(convert(r"C:\short\file.mp4"), r"C:\short\file.mp4");
        assert_eq!(
            convert(&format!(r"C:\{long}\file.mp4")),
            format!(r"\\?\C:\{long}\file.mp4")
        );
        assert_eq!(
            convert(&format!(r"C:/{long}/./x/../file.mp4")),
            format!(r"\\?\C:\{long}\file.mp4")
        );
        assert_eq!(
            convert(&format!(r"\\server\share\{long}")),
            format!(r"\\?\UNC\server\share\{long}")
        );
        assert_eq!(
            convert(&format!(r"\\?\C:\{long}")),
            format!(r"\\?\C:\{long}")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_copy_of_a_read_only_source_is_still_cleaned_up() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        std::fs::write(&source, b"contents").unwrap();
        let mut permissions = std::fs::metadata(&source).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&source, permissions).unwrap();

        let staged_path;
        {
            let mut staged = TempOutputGuard::new(&destination);
            staged.cleanup_on_current_thread = true;
            staged_path = staged.path().to_path_buf();
            copy_file(&source, &staged_path, || false).unwrap();
            // CopyFileExW carries the source's read-only attribute over.
            assert!(
                std::fs::metadata(&staged_path)
                    .unwrap()
                    .permissions()
                    .readonly()
            );
        }

        assert!(!staged_path.exists());
        assert!(!destination.exists());
        let mut permissions = std::fs::metadata(&source).unwrap().permissions();
        assert!(permissions.readonly());
        #[allow(
            clippy::permissions_set_readonly_false,
            reason = "Windows-only cleanup of a test-owned file"
        )]
        permissions.set_readonly(false);
        std::fs::set_permissions(&source, permissions).unwrap();
    }
}
