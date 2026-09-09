use super::*;
use crate::downloader::engine::SegmentEvent;
use std::io::Write;
use std::path::Path;

const TAIL_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Fixture {
    directory: PathBuf,
    mode: &'static str,
}

impl Fixture {
    pub(super) fn command(&self, source: bool) -> tokio::process::Command {
        let mut command = process_utils::tokio_command(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "downloader::engine::streamlink::shutdown_tests::shutdown_child",
                "--nocapture",
            ])
            .env("SREC_SHUTDOWN_FIXTURE_DIR", &self.directory)
            .env("SREC_SHUTDOWN_FIXTURE_MODE", self.mode)
            .env(
                "SREC_SHUTDOWN_FIXTURE_ROLE",
                if source { "source" } else { "ffmpeg" },
            );
        command
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }
}

#[test]
fn shutdown_child() {
    let Some(directory) = std::env::var_os("SREC_SHUTDOWN_FIXTURE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let role = std::env::var("SREC_SHUTDOWN_FIXTURE_ROLE").unwrap();
    let mode = std::env::var("SREC_SHUTDOWN_FIXTURE_MODE").unwrap();
    if role.ends_with("-leaf") {
        let mut file = std::fs::File::create(directory.join(&role)).unwrap();
        for _ in 0..1500 {
            file.write_all(b".").unwrap();
            file.flush().unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        std::process::exit(0);
    }

    // Both descendants retain stdout/stderr. Parent exit must contain them
    // before pipe EOF can let the recording's auxiliary tasks settle.
    let mut command = process_utils::std_command(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "downloader::engine::streamlink::shutdown_tests::shutdown_child",
            "--nocapture",
        ])
        .env("SREC_SHUTDOWN_FIXTURE_ROLE", format!("{role}-leaf"))
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut leaf = command.spawn().unwrap();
    let started = std::time::Instant::now();
    while !std::fs::metadata(directory.join(format!("{role}-leaf"))).is_ok_and(|m| m.len() > 0) {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    if role == "ffmpeg" {
        let output = directory.join("final.ts");
        let mut file = std::fs::File::create(&output).unwrap();
        eprintln!(
            "[segment @ fixture] Opening '{}' for writing",
            output.display()
        );
        std::io::copy(&mut std::io::stdin(), &mut file).unwrap();
        file.flush().unwrap();
        // Exiting deliberately leaves the descendant alive for containment.
        std::process::exit(0);
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        #[cfg(unix)]
        let mut signal =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        std::io::stdout().write_all(b"<prefix>").unwrap();
        std::io::stdout().flush().unwrap();
        std::fs::write(directory.join("source-ready"), b"ready").unwrap();
        if mode == "graceful" {
            #[cfg(unix)]
            tokio::time::timeout(Duration::from_secs(10), signal.recv())
                .await
                .unwrap();
            #[cfg(windows)]
            tokio::time::timeout(Duration::from_secs(10), async {
                while !directory.join("release-source").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        } else if mode == "stubborn" {
            tokio::time::sleep(Duration::from_secs(30)).await;
            leaf.kill().unwrap();
            leaf.wait().unwrap();
            std::process::exit(1);
        }
        std::io::stdout()
            .write_all(&vec![b'x'; TAIL_BYTES])
            .unwrap();
        std::io::stdout().write_all(b"<tail>").unwrap();
        std::io::stdout().flush().unwrap();
    });
    std::process::exit(0);
}

async fn wait_for_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !tokio::fs::try_exists(path).await.unwrap() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture must start");
}

async fn assert_descendants_stopped(fixture: &Fixture) {
    // Containment requests termination synchronously; allow the OS to finish
    // scheduling it before measuring a stable heartbeat interval.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let paths = [fixture.path("source-leaf"), fixture.path("ffmpeg-leaf")];
    let sizes: Vec<_> = paths
        .iter()
        .map(|path| std::fs::metadata(path).unwrap().len())
        .collect();
    tokio::time::sleep(Duration::from_millis(200)).await;
    for (path, size) in paths.iter().zip(sizes) {
        assert_eq!(
            std::fs::metadata(path).unwrap().len(),
            size,
            "descendant remains active: {}",
            path.display()
        );
    }
}

async fn assert_shutdown(mode: &'static str, abort: bool) {
    let directory = tempfile::tempdir().unwrap();
    let fixture = Fixture {
        directory: directory.path().to_path_buf(),
        mode,
    };
    let engine = StreamlinkEngine {
        config: StreamlinkEngineConfig {
            graceful_stop_timeout_secs: 5,
            ..Default::default()
        },
        ffmpeg_path: String::new(),
        version: None,
        fixture: None,
        shutdown_fixture: Some(fixture.clone()),
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let handle = Arc::new(DownloadHandle::new(
        "shutdown-fixture",
        EngineType::Streamlink,
        DownloadConfig::new(
            "https://invalid.test/live",
            directory.path(),
            "streamer",
            "Streamer",
            "session",
        )
        .with_max_segment_duration(10),
        tx,
    ));
    let owned_handle = handle.clone();
    let task = AbortOnDropHandle::new(tokio::spawn(async move { engine.run(owned_handle).await }));
    wait_for_file(&fixture.path("source-ready")).await;
    wait_for_file(&fixture.path("ffmpeg-leaf")).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                rx.recv().await.expect("recording must start"),
                SegmentEvent::SegmentStarted { .. }
            ) {
                break;
            }
        }
    })
    .await
    .expect("FFmpeg must announce its output before stop");
    if abort {
        task.abort();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap_err()
                .is_cancelled()
        );
        assert_descendants_stopped(&fixture).await;
        return;
    }
    if mode != "natural" {
        if mode == "stubborn" {
            handle.set_stop_deadline(Instant::now() + Duration::from_millis(100));
        }
        handle.cancellation_token.cancel();
        // Hidden Windows children have no console-signal contract. This fixture
        // exits naturally during the same bounded grace period instead.
        #[cfg(windows)]
        if mode == "graceful" {
            tokio::fs::write(fixture.path("release-source"), b"release")
                .await
                .unwrap();
        }
    }
    tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .expect("pipeline shutdown remains bounded")
        .unwrap()
        .expect("contained cleanup is confirmed");
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    let completed: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(event, SegmentEvent::SegmentCompleted(_)).then_some(index)
        })
        .collect();
    assert_eq!(completed.len(), 1, "{events:?}");
    let terminal = events
        .iter()
        .position(|event| {
            matches!(
                event,
                SegmentEvent::DownloadCompleted { .. } | SegmentEvent::DownloadFailed { .. }
            )
        })
        .unwrap();
    assert!(completed[0] < terminal);
    if mode != "stubborn" {
        let output = tokio::fs::read(fixture.path("final.ts")).await.unwrap();
        assert!(output.ends_with(b"<tail>"));
        assert_eq!(
            &output[output.len() - TAIL_BYTES - 6..output.len() - 6],
            vec![b'x'; TAIL_BYTES]
        );
        assert!(matches!(
            events[terminal],
            SegmentEvent::DownloadCompleted { .. }
        ));
    }
    assert_descendants_stopped(&fixture).await;
}

#[tokio::test]
async fn graceful_shutdown_drains_emitted_tail_before_final_segment() {
    assert_shutdown("graceful", false).await;
}

#[tokio::test]
async fn expired_grace_force_contains_both_process_trees() {
    assert_shutdown("stubborn", false).await;
}

#[tokio::test]
async fn early_leader_exit_closes_descendant_held_pipes() {
    assert_shutdown("natural", false).await;
}

#[tokio::test]
async fn dropping_engine_future_contains_spawned_process_trees() {
    assert_shutdown("stubborn", true).await;
}
