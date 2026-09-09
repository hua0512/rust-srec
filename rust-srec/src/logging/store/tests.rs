use std::collections::HashSet;
use std::time::Duration;

use super::*;

fn instant(day: u32) -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(2026, 9, day)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_utc()
}

fn store(dir: &tempfile::TempDir) -> LogStore {
    LogStore::new(
        dir.path().to_owned(),
        LogPolicy {
            max_file_bytes: 128,
            max_files: 3,
        },
    )
}

fn assert_bounds(store: &LogStore) {
    let files = store.inventory().unwrap();
    assert!(files.len() <= store.policy.max_files);
    for file in files {
        assert!(
            file.bytes <= store.policy.max_file_bytes,
            "{}",
            file.path.display()
        );
    }
}

#[test]
fn policy_rejects_unbounded_invalid_and_extreme_settings() {
    for (bytes, files) in [
        (Some("0"), None),
        (Some("1023"), None),
        (Some("18446744073709551615"), None),
        (Some("abc"), None),
        (None, Some("1")),
        (None, Some("1025")),
        (None, Some("-1")),
    ] {
        assert!(LogPolicy::parse(bytes, files).is_err());
    }
    assert_eq!(
        LogPolicy::parse(None, None).unwrap().max_file_bytes,
        16 * 1024 * 1024
    );
    assert_eq!(LogPolicy::parse(None, None).unwrap().max_files, 16);
    assert!(LogPolicy::parse(Some("1024"), Some("2")).is_ok());
}

#[test]
fn filename_grammar_excludes_unrelated_suffixes() {
    let date = instant(8).date_naive();
    assert_eq!(managed_log_date("rust-srec.log.2026-09-08"), Some(date));
    assert_eq!(parse_name(&filename(date, 42)), Some((date, 42)));
    for name in [
        "rust-srec.log.2026-09-08.backup",
        "rust-srec.log.2026-09-08.1",
        "rust-srec.log.2026-09-08.00000000000000000000",
        "rust-srec.log.2026-02-30",
        "rust-srec.log.2026-9-08",
        "other.log.2026-09-08",
    ] {
        assert!(managed_log_date(name).is_none(), "{name}");
    }
}

#[test]
fn exact_cap_rotates_without_truncating_the_previous_segment() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let legacy = dir.path().join("rust-srec.log.2026-09-08");
    std::fs::write(&legacy, [b'a'; 127]).unwrap();
    store.append(b"\n", instant(8), false).unwrap();
    assert_eq!(std::fs::metadata(&legacy).unwrap().len(), 128);
    store.append(b"next\n", instant(8), false).unwrap();
    assert_eq!(
        std::fs::read(&legacy).unwrap(),
        [vec![b'a'; 127], vec![b'\n']].concat()
    );
    assert_eq!(store.inventory().unwrap().len(), 2);
    assert_bounds(&store);
}

#[test]
fn count_pressure_restart_and_clock_rollback_do_not_reuse_deleted_names() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let mut seen = HashSet::new();
    for day in [8, 8, 9, 9, 8, 8, 10] {
        store.append(&[b'x'; 128], instant(day), false).unwrap();
        let current = store
            .inventory()
            .unwrap()
            .into_iter()
            .filter(|file| file.date == instant(day).date_naive())
            .max_by_key(|file| file.index)
            .unwrap();
        assert!(seen.insert(current.path));
        assert_bounds(&store);
    }
    let restarted = LogStore::new(dir.path().to_owned(), store.policy);
    restarted.append(&[b'y'; 128], instant(8), false).unwrap();
    let file = restarted
        .inventory()
        .unwrap()
        .into_iter()
        .filter(|file| file.date == instant(8).date_naive())
        .max_by_key(|file| file.index)
        .unwrap();
    assert!(seen.insert(file.path));
    assert_bounds(&restarted);
}

#[test]
fn reconciliation_removes_oversized_and_expired_logs_but_preserves_unrelated_paths() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let old = dir.path().join("rust-srec.log.2000-01-01");
    let oversized = dir.path().join("rust-srec.log.2026-09-08");
    let unrelated = dir.path().join("rust-srec.log.2000-01-01.backup");
    let directory = dir.path().join(filename(instant(8).date_naive(), 1));
    std::fs::write(&old, b"old").unwrap();
    std::fs::write(&oversized, [b'x'; 256]).unwrap();
    std::fs::write(&unrelated, b"keep").unwrap();
    std::fs::create_dir(&directory).unwrap();
    store.append(b"new\n", instant(8), false).unwrap();
    assert!(!old.exists());
    assert!(!oversized.exists());
    assert_eq!(std::fs::read(unrelated).unwrap(), b"keep");
    assert!(directory.is_dir());
    assert_bounds(&store);
}

#[cfg(unix)]
#[test]
fn managed_looking_symlinks_are_never_followed_or_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let target = dir.path().join("private");
    let link = dir.path().join(filename(instant(8).date_naive(), 1));
    std::fs::write(&target, b"unrelated").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    store.append(b"log\n", instant(8), false).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"unrelated");
    assert!(
        std::fs::symlink_metadata(link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_bounds(&store);
}

#[test]
fn failed_eviction_stops_rotation_until_capacity_can_be_reclaimed() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    for _ in 0..3 {
        store.append(&[b'x'; 128], instant(8), false).unwrap();
    }
    let before: Vec<_> = store
        .inventory()
        .unwrap()
        .into_iter()
        .map(|file| file.path)
        .collect();
    store.fail_removal.store(true, Ordering::Relaxed);
    assert_eq!(
        store
            .append(b"blocked", instant(8), false)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        store
            .inventory()
            .unwrap()
            .into_iter()
            .map(|file| file.path)
            .collect::<Vec<_>>(),
        before
    );
    assert_bounds(&store);
    store.fail_removal.store(false, Ordering::Relaxed);
    store.append(b"recovered", instant(8), false).unwrap();
    assert_bounds(&store);
}

#[test]
fn corrupt_rotation_counter_fails_without_reusing_or_growing_a_full_segment() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    store.append(&[b'x'; 128], instant(8), false).unwrap();
    let original = store.inventory().unwrap().remove(0).path;
    std::fs::write(dir.path().join(LOCK_NAME), b"corrupt").unwrap();
    assert!(store.append(b"next", instant(8), false).is_err());
    assert_eq!(std::fs::read(&original).unwrap(), [b'x'; 128]);
    assert_eq!(store.inventory().unwrap().len(), 1);
    assert_bounds(&store);
}

#[test]
fn reconciling_imported_oversized_segments_does_not_reuse_their_names() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let imported = dir.path().join(filename(instant(8).date_naive(), 42));
    std::fs::write(&imported, [b'x'; 256]).unwrap();
    store.reconcile(instant(8)).unwrap();
    store.append(b"fresh", instant(8), false).unwrap();
    assert!(!imported.exists());
    assert_eq!(store.inventory().unwrap()[0].index, 43);
    assert_bounds(&store);
}

#[test]
fn oversized_unicode_emergency_records_are_bounded_and_contention_never_waits() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let lock = store.lock(false).unwrap();
    assert_eq!(
        store.try_append_emergency("panic").unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(lock);
    store.try_append_emergency(&"大😀".repeat(100)).unwrap();
    let files = store.inventory().unwrap();
    let bytes = std::fs::read(&files[0].path).unwrap();
    assert!(std::str::from_utf8(&bytes).is_ok());
    assert!(bytes.ends_with(TRUNCATED));
    assert_eq!(store.truncated_records.load(Ordering::Relaxed), 1);
    assert_bounds(&store);
}

#[test]
fn nonblocking_queue_accepts_while_another_writer_holds_ownership_and_guard_flushes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(store(&dir));
    let lock = store.lock(false).unwrap();
    let (mut writer, guard) = tracing_appender::non_blocking(LogWriter(store.clone()));
    let (sent, received) = std::sync::mpsc::channel();
    let producer = std::thread::spawn(move || {
        writer.write_all(b"queued record\n").unwrap();
        sent.send(()).unwrap();
    });
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(lock);
    producer.join().unwrap();
    drop(guard);
    let files = store.inventory().unwrap();
    assert_eq!(std::fs::read(&files[0].path).unwrap(), b"queued record\n");
}

#[test]
fn concurrent_writer_child() {
    let Ok(directory) = std::env::var("SREC_LOG_STORE_TEST_DIRECTORY") else {
        return;
    };
    let identity = std::env::var("SREC_LOG_STORE_TEST_ID").unwrap();
    let store = LogStore::new(
        directory.into(),
        LogPolicy {
            max_file_bytes: 128,
            max_files: 3,
        },
    );
    for sequence in 0..40 {
        store
            .append(
                format!("writer-{identity}:{sequence:02}\n").as_bytes(),
                instant(8),
                false,
            )
            .unwrap();
    }
}

#[tokio::test]
async fn independent_processes_share_rotation_without_interleaved_records() {
    let dir = tempfile::tempdir().unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut children = tokio::task::JoinSet::new();
        for identity in 0..4 {
            let mut command = process_utils::tokio_command(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "logging::store::tests::concurrent_writer_child",
                    "--nocapture",
                ])
                .env("SREC_LOG_STORE_TEST_DIRECTORY", dir.path())
                .env("SREC_LOG_STORE_TEST_ID", identity.to_string())
                .kill_on_drop(true);
            children.spawn(async move {
                let output = command.output().await.unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            });
        }
        while let Some(result) = children.join_next().await {
            result.unwrap();
        }
    })
    .await
    .expect("concurrent log writers must finish");
    let store = store(&dir);
    assert_bounds(&store);
    for file in store.inventory().unwrap() {
        for line in std::fs::read_to_string(file.path).unwrap().lines() {
            assert_eq!(line.len(), 11);
            assert!(line.starts_with("writer-"));
            assert_eq!(&line[8..9], ":");
            assert!(line[9..].bytes().all(|byte| byte.is_ascii_digit()));
        }
    }
}

#[tokio::test]
async fn cancellation_joins_reconciliation_waiting_for_file_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _layer) = crate::logging::LoggingConfig::for_route_tests(dir.path().to_owned());
    let config = Arc::new(config);
    let lock = config.store.lock(false).unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let task_config = config.clone();
    let task_cancel = cancel.clone();
    let mut cleanup = tokio::spawn(async move {
        task_config.run_retention_cleanup(task_cancel).await;
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !config.store.reconcile_started.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("blocking reconciliation must be admitted");
    cancel.cancel();
    tokio::select! {
        result = &mut cleanup => panic!("cleanup abandoned an admitted reconciliation: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(20)) => {}
    }
    drop(lock);
    tokio::time::timeout(Duration::from_secs(2), cleanup)
        .await
        .expect("cleanup must finish after ownership is released")
        .unwrap();
}
