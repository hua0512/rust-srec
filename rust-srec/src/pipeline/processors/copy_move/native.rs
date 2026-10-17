//! Whole-file copies that stop between transfers when cancelled.
//!
//! Linux, Windows and macOS check for cancellation between chunks; a blocked
//! transfer still has to return before the next check. A macOS copy within one
//! APFS volume is a clone, which finishes at once. Other platforms keep std's
//! native copy, which cannot stop mid-file.

use std::io;
use std::path::Path;

/// Copy `source` over `destination` and return the default stream's byte
/// count. Once `is_cancelled` returns `true` the copy stops with a
/// "Copy cancelled" error and may leave a partial `destination` for the caller
/// to remove. `on_progress` receives the bytes copied so far, ending with the
/// total; a clone, or a platform without cancellation, reports only the total.
pub(super) fn copy_file(
    source: &Path,
    destination: &Path,
    is_cancelled: impl FnMut() -> bool,
    on_progress: impl FnMut(u64),
) -> io::Result<u64> {
    #[cfg(target_os = "linux")]
    {
        copy_linux(source, destination, is_cancelled, on_progress)
    }
    #[cfg(windows)]
    {
        copy_windows(source, destination, is_cancelled, on_progress)
    }
    #[cfg(target_os = "macos")]
    {
        copy_macos(source, destination, is_cancelled, on_progress)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = is_cancelled;
        let mut on_progress = on_progress;
        let copied = std::fs::copy(source, destination)?;
        on_progress(copied);
        Ok(copied)
    }
}

/// Open `source` for a copy, accepting only a regular file or a symlink to
/// one, before anything touches the destination.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_source(source: &Path) -> io::Result<(std::fs::File, std::fs::Metadata)> {
    let reader = std::fs::File::open(source)?;
    let metadata = reader.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the source path is neither a regular file nor a symlink to a regular file",
        ));
    }
    Ok((reader, metadata))
}

/// Create or truncate `destination` with `permissions`, as std does: an
/// existing regular file gets them too, while a FIFO or device keeps its own.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_destination(
    destination: &Path,
    permissions: std::fs::Permissions,
) -> io::Result<(std::fs::File, std::fs::Metadata)> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let writer = std::fs::OpenOptions::new()
        .mode(permissions.mode())
        .write(true)
        .create(true)
        .truncate(true)
        .open(destination)?;
    let metadata = writer.metadata()?;
    if metadata.is_file() {
        writer.set_permissions(permissions)?;
    }
    Ok((writer, metadata))
}

/// Retain Linux's kernel-assisted copy path while checking cancellation
/// between bounded transfers.
#[cfg(target_os = "linux")]
fn copy_linux(
    source: &Path,
    destination: &Path,
    mut is_cancelled: impl FnMut() -> bool,
    mut on_progress: impl FnMut(u64),
) -> io::Result<u64> {
    use std::io::Read;

    const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

    let (mut reader, metadata) = open_source(source)?;
    let (mut writer, _) = open_destination(destination, metadata.permissions())?;

    let mut copied = 0;
    loop {
        if is_cancelled() {
            return Err(io::Error::other("Copy cancelled"));
        }
        // Take<File> retains std::io::copy's Linux copy_file_range /
        // sendfile / splice specialization and fallback handling.
        let bytes = io::copy(&mut (&mut reader).take(CHUNK_SIZE), &mut writer)?;
        copied += bytes;
        on_progress(copied);
        if bytes < CHUNK_SIZE {
            return Ok(copied);
        }
    }
}

/// Copy through `CopyFileExW`, the call `std::fs::copy` makes on Windows, so
/// attributes, alternate data streams and metadata are copied the same way and
/// the result is the default stream's byte count. Windows invokes the progress
/// routine on this thread before the first transfer and after each chunk, which
/// also reports the default stream's progress; returning `PROGRESS_CANCEL`
/// stops the copy, and Windows then deletes the partial destination itself.
#[cfg(windows)]
fn copy_windows<F: FnMut() -> bool, P: FnMut(u64)>(
    source: &Path,
    destination: &Path,
    is_cancelled: F,
    on_progress: P,
) -> io::Result<u64> {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{ERROR_REQUEST_ABORTED, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        COPYPROGRESSROUTINE_PROGRESS, CopyFileExW, LPPROGRESS_ROUTINE_CALLBACK_REASON,
        PROGRESS_CANCEL, PROGRESS_CONTINUE,
    };

    struct Progress<F, P> {
        is_cancelled: F,
        on_progress: P,
        copied: i64,
    }

    // The routine must not unwind across the FFI boundary; the production
    // closures only read a cancellation token and send a progress report.
    unsafe extern "system" fn progress_routine<F: FnMut() -> bool, P: FnMut(u64)>(
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
        // SAFETY: `data` points to the `Progress<F, P>` that `copy_windows`
        // exclusively borrows for the whole `CopyFileExW` call, which runs this
        // routine synchronously on the calling thread and never after it
        // returns.
        let progress = unsafe { &mut *data.cast_mut().cast::<Progress<F, P>>() };
        if stream_number == 1 {
            progress.copied = stream_bytes_transferred;
            (progress.on_progress)(stream_bytes_transferred as u64);
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
        on_progress,
        copied: 0,
    };
    // SAFETY: both paths are NUL-terminated UTF-16 buffers that outlive the
    // call, and `progress` matches the routine's type parameter.
    let copied = unsafe {
        CopyFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            Some(progress_routine::<F, P>),
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

/// Copy the way `std::fs::copy` does on macOS: a clone within one APFS
/// volume, otherwise `fcopyfile`, which reports progress and can be
/// cancelled.
#[cfg(target_os = "macos")]
fn copy_macos(
    source: &Path,
    destination: &Path,
    is_cancelled: impl FnMut() -> bool,
    mut on_progress: impl FnMut(u64),
) -> io::Result<u64> {
    let (reader, metadata) = open_source(source)?;
    // A clone shares the source's blocks and finishes at once, so there is
    // nothing to cancel or report before it is done.
    if clone_macos(&reader, destination)? {
        on_progress(metadata.len());
        return Ok(metadata.len());
    }
    fcopyfile_macos(&reader, &metadata, destination, is_cancelled, on_progress)
}

/// Clone `reader` to `destination`, or return `Ok(false)` where cloning is
/// impossible and std falls back to a copy: a volume other than APFS, an
/// existing destination, or another volume.
#[cfg(target_os = "macos")]
fn clone_macos(reader: &std::fs::File, destination: &Path) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "path contains an interior NUL")
    })?;
    // SAFETY: the descriptor stays open for the call and the path is
    // NUL-terminated.
    let cloned =
        unsafe { libc::fclonefileat(reader.as_raw_fd(), libc::AT_FDCWD, destination.as_ptr(), 0) };
    if cloned == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOTSUP | libc::EEXIST | libc::EXDEV) => Ok(false),
        _ => Err(error),
    }
}

/// Copy `reader` into `destination` with `fcopyfile` and the data and
/// metadata flags std uses, creating the destination with the source's
/// permissions as std does. macOS calls `status` on this thread at each stage
/// and after every chunk of data; it reports progress and returns
/// `COPYFILE_QUIT` once the copy is cancelled, which leaves the partial
/// destination for the caller to remove.
#[cfg(target_os = "macos")]
fn fcopyfile_macos<F: FnMut() -> bool, P: FnMut(u64)>(
    reader: &std::fs::File,
    metadata: &std::fs::Metadata,
    destination: &Path,
    is_cancelled: F,
    on_progress: P,
) -> io::Result<u64> {
    use std::ffi::{c_char, c_int, c_void};
    use std::os::fd::AsRawFd;

    struct State(libc::copyfile_state_t);

    impl Drop for State {
        fn drop(&mut self) {
            // SAFETY: the state came from `copyfile_state_alloc` and is freed
            // only here. A failure to close descriptors it opened is not an
            // error for `fcopyfile`, which opens none.
            unsafe { libc::copyfile_state_free(self.0) };
        }
    }

    struct Progress<F, P> {
        is_cancelled: F,
        on_progress: P,
        cancelled: bool,
    }

    fn check(result: c_int) -> io::Result<()> {
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    // The callback must not unwind across the FFI boundary; the production
    // closures only read a cancellation token and send a progress report.
    extern "C" fn status<F: FnMut() -> bool, P: FnMut(u64)>(
        what: c_int,
        stage: c_int,
        state: libc::copyfile_state_t,
        _source: *const c_char,
        _destination: *const c_char,
        context: *mut c_void,
    ) -> c_int {
        // SAFETY: `context` points to the `Progress<F, P>` that
        // `fcopyfile_macos` exclusively borrows for the whole `fcopyfile`
        // call, which runs this callback synchronously on the calling thread
        // and never after it returns.
        let progress = unsafe { &mut *context.cast::<Progress<F, P>>() };
        // Without a callback any failed step ends the copy with its error.
        // Quitting keeps that, where `COPYFILE_CONTINUE` would skip the step.
        if stage == libc::COPYFILE_ERR {
            return libc::COPYFILE_QUIT;
        }
        if what == libc::COPYFILE_COPY_DATA && stage == libc::COPYFILE_PROGRESS {
            let mut copied: libc::off_t = 0;
            // SAFETY: `state` is this copy's live state, and
            // `COPYFILE_STATE_COPIED` writes one `off_t`.
            let read = unsafe {
                libc::copyfile_state_get(
                    state,
                    libc::COPYFILE_STATE_COPIED as u32,
                    (&raw mut copied).cast(),
                )
            };
            if read == 0 {
                (progress.on_progress)(copied as u64);
            }
        }
        if (progress.is_cancelled)() {
            progress.cancelled = true;
            libc::COPYFILE_QUIT
        } else {
            libc::COPYFILE_CONTINUE
        }
    }

    let (writer, writer_metadata) = open_destination(destination, metadata.permissions())?;
    // SAFETY: allocation has no preconditions; a null state is an error.
    let state = unsafe { libc::copyfile_state_alloc() };
    if state.is_null() {
        return Err(io::Error::last_os_error());
    }
    let state = State(state);
    let mut progress = Progress {
        is_cancelled,
        on_progress,
        cancelled: false,
    };
    // SAFETY: `state` is live, `copyfile_state_set` stores the callback and
    // context pointers as given, and `progress` outlives the `fcopyfile` call
    // that reads it.
    unsafe {
        check(libc::copyfile_state_set(
            state.0,
            libc::COPYFILE_STATE_STATUS_CB as u32,
            status::<F, P> as *const c_void,
        ))?;
        check(libc::copyfile_state_set(
            state.0,
            libc::COPYFILE_STATE_STATUS_CTX as u32,
            (&raw mut progress).cast::<c_void>().cast_const(),
        ))?;
    }
    // Like std, copy metadata only onto a regular file.
    let flags = if writer_metadata.is_file() {
        libc::COPYFILE_METADATA | libc::COPYFILE_DATA
    } else {
        libc::COPYFILE_DATA
    };
    // SAFETY: both descriptors stay open for the call and `state` is live.
    let result = unsafe { libc::fcopyfile(reader.as_raw_fd(), writer.as_raw_fd(), state.0, flags) };
    if result != 0 {
        // A quit leaves errno as it was, so the flag tells cancellation apart.
        return Err(if progress.cancelled {
            io::Error::other("Copy cancelled")
        } else {
            io::Error::last_os_error()
        });
    }
    let mut copied: libc::off_t = 0;
    // SAFETY: `state` is live, and `COPYFILE_STATE_COPIED` writes one `off_t`.
    check(unsafe {
        libc::copyfile_state_get(
            state.0,
            libc::COPYFILE_STATE_COPIED as u32,
            (&raw mut copied).cast(),
        )
    })?;
    let copied = copied as u64;
    (progress.on_progress)(copied);
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::processors::outputs::TempOutputGuard;
    use tempfile::TempDir;

    /// The copy each platform falls back to when it cannot clone: all of
    /// `copy_file` on Linux and Windows, and the `fcopyfile` path on macOS,
    /// whose test directories share one APFS volume and would clone.
    #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
    fn transfer(
        source: &Path,
        destination: &Path,
        is_cancelled: impl FnMut() -> bool,
        on_progress: impl FnMut(u64),
    ) -> io::Result<u64> {
        #[cfg(target_os = "macos")]
        {
            let (reader, metadata) = open_source(source)?;
            fcopyfile_macos(&reader, &metadata, destination, is_cancelled, on_progress)
        }
        #[cfg(not(target_os = "macos"))]
        {
            copy_file(source, destination, is_cancelled, on_progress)
        }
    }

    /// The cancellable copies report the default stream's cumulative bytes as
    /// they go, ending exactly at the file size.
    #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
    #[test]
    fn copy_reports_increasing_bytes_ending_at_the_total() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        let contents = vec![b'x'; 20 * 1024 * 1024 + 3];
        std::fs::write(&source, &contents).unwrap();
        let mut reported = Vec::new();

        let copied = transfer(
            &source,
            &destination,
            || false,
            |copied| reported.push(copied),
        )
        .unwrap();

        assert_eq!(copied, contents.len() as u64);
        assert!(reported.len() > 2, "{reported:?}");
        assert!(
            reported.windows(2).all(|pair| pair[0] <= pair[1]),
            "{reported:?}"
        );
        assert!(
            reported.iter().any(|&bytes| bytes > 0 && bytes < copied),
            "{reported:?}"
        );
        assert_eq!(reported.last(), Some(&copied));
    }

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
            let result = copy_file(
                &source,
                &staged_path,
                || {
                    std::fs::metadata(&staged_path)
                        .is_ok_and(|metadata| metadata.len() >= chunk_size as u64)
                },
                |_| {},
            );

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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn copy_preserves_content_permissions_and_truncation_through_source_symlinks() {
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

            let copied = transfer(&link, &destination, || false, |_| {}).unwrap();

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

        let copied = copy_file(
            &source,
            &destination,
            || {
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
            },
            |_| {},
        )
        .unwrap();

        assert!(appended);
        assert_eq!(copied, chunk_size as u64 + 8);
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            std::fs::read(&source).unwrap()
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn copy_rejects_a_non_file_source_before_truncating_destination() {
        let directory = TempDir::new().unwrap();
        let destination = directory.path().join("destination");
        std::fs::write(&destination, b"original").unwrap();

        let error = copy_file(directory.path(), &destination, || false, |_| {}).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_copy_stops_mid_file_when_cancelled_and_leaves_the_rest_to_the_guard() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        let contents = vec![b'x'; 64 * 1024 * 1024 + 1];
        std::fs::write(&source, &contents).unwrap();
        std::fs::write(&destination, b"original").unwrap();

        let staged_path;
        {
            let mut staged = TempOutputGuard::new(&destination);
            staged.cleanup_on_current_thread = true;
            staged_path = staged.path().to_path_buf();
            // Cancel once the first chunk of data has been reported. This runs
            // in the copy thread and does not depend on another task's timing.
            let reported = std::cell::Cell::new(0);
            let result = transfer(
                &source,
                &staged_path,
                || reported.get() > 0,
                |copied| reported.set(copied),
            );

            assert_eq!(result.unwrap_err().to_string(), "Copy cancelled");
            let partial = std::fs::metadata(&staged_path).unwrap().len();
            assert!(partial < contents.len() as u64, "{partial}");
        }

        assert!(!staged_path.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        assert_eq!(std::fs::read(&source).unwrap(), contents);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_copy_and_clone_return_the_byte_count_and_identical_content() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");

        for size in [0, 1, 4 * 1024 * 1024 + 7] {
            let contents = vec![b'x'; size];
            std::fs::write(&source, &contents).unwrap();
            std::fs::write(&destination, vec![b'y'; size + 1]).unwrap();

            let copied = transfer(&source, &destination, || false, |_| {}).unwrap();
            assert_eq!(copied, size as u64);
            assert_eq!(std::fs::read(&destination).unwrap(), contents);

            // A fresh destination on the same volume takes the clone path.
            let clone = directory.path().join(format!("clone-{size}"));
            let mut reported = Vec::new();
            let cloned =
                copy_file(&source, &clone, || false, |bytes| reported.push(bytes)).unwrap();
            assert_eq!(cloned, size as u64);
            assert_eq!(reported.last(), Some(&(size as u64)));
            assert_eq!(std::fs::read(&clone).unwrap(), contents);
        }
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
        let copied = copy_file(
            &source,
            &directory.path().join("complete"),
            || {
                complete_calls += 1;
                false
            },
            |_| {},
        )
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
            let result = copy_file(
                &source,
                &staged_path,
                || {
                    calls += 1;
                    calls == 2
                },
                |_| {},
            );

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

            let copied = copy_file(&source, &destination, || false, |_| {}).unwrap();

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

        let copied = copy_file(&source, &destination, || false, |_| {}).unwrap();

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
            copy_file(&source, &staged_path, || false, |_| {}).unwrap();
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
