use super::*;
use axum::body::to_bytes;
use futures::StreamExt;
use rand::{Rng, SeedableRng};
use std::io::Cursor;
use std::time::Duration;
use tempfile::TempDir;
use zip::{HasZipMetadata, ZipArchive};

fn log_file(dir: &TempDir, contents: &[u8]) -> LogFileInternal {
    let path = dir.path().join("rust-srec.log.2026-09-07");
    std::fs::write(&path, contents).unwrap();
    LogFileInternal {
        date: chrono::NaiveDate::from_ymd_opt(2026, 9, 7).unwrap(),
        filename: path.file_name().unwrap().to_str().unwrap().to_owned(),
        path,
        size_bytes: contents.len() as u64,
    }
}

#[test]
fn numbered_log_segments_keep_names_and_scanned_lengths_in_zip() {
    let dir = TempDir::new().unwrap();
    let name = "rust-srec.log.2001-01-01.00000000000000000001";
    let path = dir.path().join(name);
    std::fs::write(&path, b"first\n").unwrap();
    let files = super::super::scan_log_files_matching(dir.path(), |_| true, 10).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"later\n")
        .unwrap();
    let mut bytes = Vec::new();
    build_archive_zip(&files, &mut bytes, &CancellationToken::new()).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut entry = archive.by_name(name).unwrap();
    let mut text = String::new();
    entry.read_to_string(&mut text).unwrap();
    assert_eq!(text, "first\n");
}

#[test]
fn streaming_zip_is_readable_and_uses_zip64_local_headers() {
    let dir = TempDir::new().unwrap();
    let file = log_file(&dir, b"first line\nsecond line\n");
    let mut bytes = Vec::new();
    build_archive_zip(
        std::slice::from_ref(&file),
        &mut bytes,
        &CancellationToken::new(),
    )
    .unwrap();
    let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let mut entry = archive.by_index(0).unwrap();
    assert_eq!(entry.name(), file.filename);
    assert!(entry.get_metadata().using_data_descriptor);
    assert_eq!(entry.size(), file.size_bytes);
    let mut content = String::new();
    entry.read_to_string(&mut content).unwrap();
    assert_eq!(content, "first line\nsecond line\n");
    // ZIP64 is reserved in the local header even when the bounded fixture is small.
    let header = entry.header_start() as usize;
    assert_eq!(
        u16::from_le_bytes(bytes[header + 4..header + 6].try_into().unwrap()),
        45
    );
    let name_len = u16::from_le_bytes(bytes[header + 26..header + 28].try_into().unwrap()) as usize;
    let extra = header + 30 + name_len;
    assert_eq!(&bytes[extra..extra + 4], &[1, 0, 16, 0]);
}

#[test]
fn growing_log_is_limited_to_its_snapshot_and_truncation_fails() {
    let dir = TempDir::new().unwrap();
    let file = log_file(&dir, b"before\n");
    std::fs::write(&file.path, b"before\nafter\n").unwrap();
    let mut bytes = Vec::new();
    build_archive_zip(
        std::slice::from_ref(&file),
        &mut bytes,
        &CancellationToken::new(),
    )
    .unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut content = String::new();
    archive
        .by_index(0)
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    assert_eq!(content, "before\n");
    std::fs::write(&file.path, b"b").unwrap();
    assert!(build_archive_zip(&[file], io::sink(), &CancellationToken::new()).is_err());
}

#[test]
fn output_failure_and_pre_cancelled_generation_return_errors() {
    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("failed output"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let dir = TempDir::new().unwrap();
    let file = log_file(&dir, b"log");
    assert!(
        build_archive_zip(
            std::slice::from_ref(&file),
            FailingWriter,
            &CancellationToken::new()
        )
        .is_err()
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(build_archive_zip(&[file], io::sink(), &cancelled).is_err());
}

#[tokio::test]
async fn download_capacity_is_held_until_bodies_finish_or_are_dropped() {
    let service = LogArchiveService::new();
    let dir = TempDir::new().unwrap();
    log_file(&dir, b"log");
    let first = service
        .download(dir.path().to_owned(), None, None)
        .await
        .unwrap();
    let second = service
        .download(dir.path().to_owned(), None, None)
        .await
        .unwrap();
    let error = service
        .download(dir.path().to_owned(), None, None)
        .await
        .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error.retry_after_secs, Some(5));
    to_bytes(first, 4096).await.unwrap();
    drop(second);
    let permits = tokio::time::timeout(
        Duration::from_secs(2),
        service
            .slots
            .clone()
            .acquire_many_owned(MAX_CONCURRENT_ARCHIVES as u32),
    )
    .await
    .unwrap()
    .unwrap();
    drop(permits);
    let body = service
        .download(dir.path().to_owned(), None, None)
        .await
        .unwrap();
    assert!(!to_bytes(body, 4096).await.unwrap().is_empty());
}

#[tokio::test]
async fn slow_client_bounds_buffering_and_disconnect_releases_producer() {
    let service = LogArchiveService::new();
    let dir = TempDir::new().unwrap();
    let mut file = log_file(&dir, b"");
    let mut input = std::fs::File::create(&file.path).unwrap();
    let mut rng = rand::rngs::StdRng::seed_from_u64(7);
    let mut buffer = [0u8; CHUNK_SIZE];
    for _ in 0..64 {
        rng.fill_bytes(&mut buffer);
        input.write_all(&buffer).unwrap();
    }
    file.size_bytes = input.metadata().unwrap().len();
    drop(input);
    let stream = ArchiveStream::new(
        vec![file],
        service.slots.clone().try_acquire_owned().unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while stream.receiver.len() < BUFFERED_CHUNKS {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(!stream.task.is_finished());
    assert_eq!(stream.receiver.max_capacity(), BUFFERED_CHUNKS);
    drop(stream);
    let permits = tokio::time::timeout(
        Duration::from_secs(2),
        service
            .slots
            .clone()
            .acquire_many_owned(MAX_CONCURRENT_ARCHIVES as u32),
    )
    .await
    .unwrap()
    .unwrap();
    drop(permits);
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "streaming creates no staging file"
    );
}

#[tokio::test]
async fn removed_input_aborts_the_body_and_returns_capacity() {
    let service = LogArchiveService::new();
    let dir = TempDir::new().unwrap();
    let file = log_file(&dir, b"log");
    std::fs::remove_file(&file.path).unwrap();
    let stream = ArchiveStream::new(
        vec![file],
        service.slots.clone().try_acquire_owned().unwrap(),
    );
    assert!(to_bytes(Body::from_stream(stream), 4096).await.is_err());
    assert_eq!(service.slots.available_permits(), MAX_CONCURRENT_ARCHIVES);
}

#[tokio::test]
async fn producer_panic_is_an_error_instead_of_successful_eof() {
    let (sender, receiver) = mpsc::channel(1);
    let service = LogArchiveService::new();
    let mut stream = ArchiveStream {
        receiver,
        task: tokio::task::spawn_blocking(move || {
            let _sender = sender;
            // Exercise task failure without timing global panic-hook diagnostics.
            std::panic::resume_unwind(Box::new("archive test panic"));
        }),
        _cancel_on_drop: CancellationToken::new().drop_guard(),
        permit: Some(Arc::new(service.slots.clone().try_acquire_owned().unwrap())),
        finished: false,
    };
    assert!(
        tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(stream.next().await.is_none());
    assert_eq!(service.slots.available_permits(), MAX_CONCURRENT_ARCHIVES);
}

#[tokio::test]
async fn scan_failure_returns_capacity() {
    let dir = TempDir::new().unwrap();
    let service = LogArchiveService::new();
    assert!(
        service
            .download(dir.path().join("missing"), None, None)
            .await
            .is_err()
    );
    assert_eq!(service.slots.available_permits(), MAX_CONCURRENT_ARCHIVES);
}
