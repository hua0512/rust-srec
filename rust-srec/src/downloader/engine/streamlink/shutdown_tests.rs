use std::io::{Read, Write};
use std::path::Path;

use tokio::sync::Notify;

use super::*;
use crate::downloader::engine::SegmentEvent;

const TAIL_BYTES: usize = 2 * 1024 * 1024;
const SHORT_STOP_BUDGET: Duration = Duration::from_millis(100);
// Allow native process reaping and event delivery to be scheduled after the
// deadline, while still detecting the old three-/five-second settlement waits.
const SETTLEMENT_SLACK: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub(super) struct Fixture {
    directory: PathBuf,
    mode: &'static str,
}

impl Fixture {
    pub(super) fn settling_after_source_exit(&self) {
        std::fs::write(self.path("settling-after-source-exit"), b"waiting").unwrap();
    }

    pub(super) fn settling_after_ffmpeg_exit(&self) {
        std::fs::write(self.path("settling-after-ffmpeg-exit"), b"waiting").unwrap();
    }

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

fn wait_for_child_release(path: &Path) {
    let started = std::time::Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "fixture release was not published: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
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
        std::fs::write(directory.join("ffmpeg-ready"), b"ready").unwrap();
        if mode == "ffmpeg-exit-late-stop" {
            let mut prefix = [0; 8];
            std::io::stdin().read_exact(&mut prefix).unwrap();
            file.write_all(&prefix).unwrap();
            file.flush().unwrap();
            wait_for_child_release(&directory.join("release-leading-child"));
            std::process::exit(0);
        }
        std::io::copy(&mut std::io::stdin(), &mut file).unwrap();
        file.flush().unwrap();
        if mode == "source-exit-late-stop" {
            std::fs::write(directory.join("peer-waiting"), b"finalizing").unwrap();
            std::thread::sleep(Duration::from_secs(30));
        }
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
        // This tail is already accepted before the fixture advertises readiness.
        let accepted_tail = vec![b'x'; TAIL_BYTES];
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
        } else if matches!(
            mode.as_str(),
            "stubborn" | "forced-source" | "ffmpeg-exit-late-stop"
        ) {
            tokio::time::sleep(Duration::from_secs(30)).await;
            leaf.kill().unwrap();
            leaf.wait().unwrap();
            std::process::exit(1);
        } else if mode == "source-exit-late-stop" {
            wait_for_child_release(&directory.join("release-leading-child"));
        }
        std::io::stdout().write_all(&accepted_tail).unwrap();
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

fn collect_events(
    mut receiver: tokio::sync::mpsc::Receiver<SegmentEvent>,
) -> (AbortOnDropHandle<Vec<SegmentEvent>>, Arc<Notify>) {
    let started = Arc::new(Notify::new());
    let notify_started = started.clone();
    let task = AbortOnDropHandle::new(tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            if matches!(event, SegmentEvent::SegmentStarted { .. }) {
                notify_started.notify_one();
            }
            events.push(event);
        }
        events
    }));
    (task, started)
}

fn deadline_cleanup_failure<'a>(mode: &str, error: &'a EngineStartError) -> Option<&'a str> {
    if error.kind != DownloadFailureKind::Other
        || !matches!(
            mode,
            "stubborn" | "source-exit-late-stop" | "ffmpeg-exit-late-stop"
        )
    {
        return None;
    }
    let failure = error
        .message
        .strip_prefix("Streamlink task settlement failed: process cleanup was not confirmed: ")?;
    // Consume the whole message: an auxiliary timeout/panic or unrelated process
    // error must still fail the test.
    let cleanup = if mode == "ffmpeg-exit-late-stop" {
        failure
            .strip_prefix("FFmpeg exited before Streamlink completed; process cleanup error: ")?
    } else {
        failure
    };
    // A forced FFmpeg exit leads the message when its own containment succeeded.
    let cleanup = cleanup
        .strip_prefix("ffmpeg exceeded the stop deadline; ")
        .or_else(|| cleanup.strip_prefix("process cleanup error: "))
        .unwrap_or(cleanup);
    let prefixes = [
        "failed to contain streamlink before the stop deadline: deadline elapsed while reaping force-terminated child ",
        "failed to contain ffmpeg before the stop deadline: deadline elapsed while reaping force-terminated child ",
    ];
    let mut seen = [false; 2];
    for reason in cleanup.split("; process cleanup error: ") {
        let (index, pid) = prefixes
            .iter()
            .enumerate()
            .find_map(|(index, prefix)| reason.strip_prefix(*prefix).map(|pid| (index, pid)))?;
        if seen[index]
            || !pid.bytes().all(|byte| byte.is_ascii_digit())
            || !pid.parse::<u32>().is_ok_and(|pid| pid > 0)
        {
            return None;
        }
        seen[index] = true;
    }
    Some(failure)
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
        version: EngineVersion::settled(None),
        fixture: None,
        shutdown_fixture: Some(fixture.clone()),
    };
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let (event_task, segment_started) = collect_events(rx);
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
    tokio::time::timeout(Duration::from_secs(5), segment_started.notified())
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
    let mut stop_deadline = None;
    if mode != "natural" {
        if mode == "stubborn" {
            let deadline = Instant::now() + SHORT_STOP_BUDGET;
            handle.set_stop_deadline(deadline);
            stop_deadline = Some(deadline);
        } else if mode == "forced-source" {
            // Long enough that FFmpeg finishes on its own once the ignored
            // shutdown request ends in Streamlink's forced termination.
            let deadline = Instant::now() + Duration::from_secs(2);
            handle.set_stop_deadline(deadline);
            stop_deadline = Some(deadline);
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
    let completion_deadline = stop_deadline
        .map(|deadline| deadline + SETTLEMENT_SLACK)
        .unwrap_or_else(|| Instant::now() + Duration::from_secs(8));
    let result = tokio::time::timeout_at(completion_deadline, task)
        .await
        .expect("pipeline shutdown remains bounded")
        .unwrap();
    // A short hard deadline can expire before the OS acknowledges forced reap.
    // That is an explicitly unconfirmed failure, never confirmed completion.
    let unconfirmed_failure = result.as_ref().err().map(|error| {
        deadline_cleanup_failure(mode, error)
            .unwrap_or_else(|| panic!("unexpected shutdown failure for {mode}: {error:?}"))
    });
    drop(handle);
    let events = tokio::time::timeout(Duration::from_secs(2), event_task)
        .await
        .unwrap()
        .unwrap();
    let completed: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(event, SegmentEvent::SegmentCompleted(_)).then_some(index)
        })
        .collect();
    if let Some(expected_failure) = unconfirmed_failure {
        assert!(
            completed.is_empty(),
            "unconfirmed cleanup must not publish a final segment: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SegmentEvent::DownloadCompleted { .. })),
            "{events:?}"
        );
        let failures: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, SegmentEvent::DownloadFailed { .. }))
            .collect();
        assert_eq!(failures.len(), 1, "{events:?}");
        assert!(
            matches!(failures[0], SegmentEvent::DownloadFailed {
            kind: DownloadFailureKind::ProcessExit { code: None }, message,
        } if message == expected_failure),
            "the terminal failure must preserve the exact cleanup cause: {events:?}"
        );
        assert_descendants_stopped(&fixture).await;
        return;
    }
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
    // The short `stubborn` budget may or may not leave FFmpeg time to finish on
    // its own, so that mode's outcome is not pinned.
    if mode == "forced-source" {
        // Streamlink is forced, but FFmpeg exits on its own: an ordinary stop
        // that keeps every byte forwarded before the termination.
        let output = tokio::fs::read(fixture.path("final.ts")).await.unwrap();
        // `<prefix>` is the source's last write before it hangs; the bytes ahead
        // of it are the test harness's own stdout.
        assert!(output.ends_with(b"<prefix>"), "{output:?}");
        assert!(
            matches!(events[terminal], SegmentEvent::DownloadCompleted { .. }),
            "{events:?}"
        );
    } else if mode != "stubborn" {
        let output = tokio::fs::read(fixture.path("final.ts")).await.unwrap();
        assert!(output.ends_with(b"<tail>"));
        assert_eq!(
            &output[output.len() - TAIL_BYTES - 6..output.len() - 6],
            vec![b'x'; TAIL_BYTES]
        );
        assert!(
            matches!(events[terminal], SegmentEvent::DownloadCompleted { .. }),
            "{events:?}"
        );
    }
    assert_descendants_stopped(&fixture).await;
}

#[tokio::test]
async fn graceful_shutdown_drains_emitted_tail_before_final_segment() {
    assert_shutdown("graceful", false).await;
}

async fn assert_late_stop(mode: &'static str, branch: &str) {
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
        version: EngineVersion::settled(None),
        fixture: None,
        shutdown_fixture: Some(fixture.clone()),
    };
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let (event_task, segment_started) = collect_events(rx);
    let handle = Arc::new(DownloadHandle::new(
        "late-stop-fixture",
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
    wait_for_file(&fixture.path("ffmpeg-ready")).await;
    tokio::time::timeout(Duration::from_secs(5), segment_started.notified())
        .await
        .unwrap();
    tokio::fs::write(fixture.path("release-leading-child"), b"exit")
        .await
        .unwrap();
    wait_for_file(&fixture.path(branch)).await;
    // These tests use one runtime thread. The synchronous production hook is
    // immediately followed by timeout capture and the peer wait's first poll;
    // the parent cannot observe this marker until that poll returns Pending.
    if mode == "source-exit-late-stop" {
        wait_for_file(&fixture.path("peer-waiting")).await;
    }
    assert!(
        !task.is_finished(),
        "the peer must still be settling before the stop request"
    );

    let deadline = Instant::now() + SHORT_STOP_BUDGET;
    handle.set_stop_deadline(deadline);
    handle.cancellation_token.cancel();
    let result = tokio::time::timeout_at(deadline + SETTLEMENT_SLACK, task)
        .await
        .expect("an already-active settlement must observe the later attempt deadline")
        .unwrap();
    // A late stop can exhaust the same hard deadline while the OS acknowledges
    // forced reap. Keep checking the deadline, events, bytes, and descendants.
    let unconfirmed_failure = result.as_ref().err().map(|error| {
        deadline_cleanup_failure(mode, error)
            .unwrap_or_else(|| panic!("unexpected late-stop failure for {mode}: {error:?}"))
    });
    drop(handle);
    let events = tokio::time::timeout(Duration::from_secs(2), event_task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SegmentEvent::DownloadFailed { .. }))
            .count(),
        1,
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SegmentEvent::DownloadCompleted { .. })),
        "a forced late stop is incomplete: {events:?}"
    );
    let terminal = events
        .iter()
        .position(|event| matches!(event, SegmentEvent::DownloadFailed { .. }))
        .unwrap();
    if let Some(expected_failure) = unconfirmed_failure {
        let expected_kind = if mode == "ffmpeg-exit-late-stop" {
            DownloadFailureKind::Other
        } else {
            DownloadFailureKind::ProcessExit { code: None }
        };
        assert!(
            matches!(&events[terminal], SegmentEvent::DownloadFailed { kind, message }
                if kind == &expected_kind && message == expected_failure),
            "the terminal failure must preserve the exact cleanup cause: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SegmentEvent::SegmentCompleted(_))),
            "unconfirmed cleanup must not publish a final segment: {events:?}"
        );
    }
    for (index, event) in events.iter().enumerate() {
        if matches!(event, SegmentEvent::SegmentCompleted(_)) {
            assert!(index < terminal);
        }
    }
    if mode == "source-exit-late-stop" {
        let output = tokio::fs::read(fixture.path("final.ts")).await.unwrap();
        assert!(
            output.ends_with(b"<tail>"),
            "accepted bytes were already flushed before finalization stalled"
        );
        assert!(output.len() >= TAIL_BYTES + 6);
        assert_eq!(
            &output[output.len() - TAIL_BYTES - 6..output.len() - 6],
            vec![b'x'; TAIL_BYTES]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SegmentEvent::SegmentCompleted(_)))
                .count(),
            usize::from(unconfirmed_failure.is_none()),
            "only confirmed cleanup may publish the accepted final segment: {events:?}"
        );
    }
    assert_descendants_stopped(&fixture).await;
}

#[tokio::test(flavor = "current_thread")]
async fn late_stop_shortens_ffmpeg_settlement_after_source_exit() {
    assert_late_stop("source-exit-late-stop", "settling-after-source-exit").await;
}

#[tokio::test(flavor = "current_thread")]
async fn late_stop_shortens_source_settlement_after_ffmpeg_exit() {
    assert_late_stop("ffmpeg-exit-late-stop", "settling-after-ffmpeg-exit").await;
}

#[tokio::test]
async fn expired_grace_force_contains_both_process_trees() {
    assert_shutdown("stubborn", false).await;
}

#[tokio::test]
async fn forced_streamlink_exit_completes_with_the_forwarded_output() {
    assert_shutdown("forced-source", false).await;
}

#[tokio::test]
async fn early_leader_exit_closes_descendant_held_pipes() {
    assert_shutdown("natural", false).await;
}

#[tokio::test]
async fn dropping_engine_future_contains_spawned_process_trees() {
    assert_shutdown("stubborn", true).await;
}
