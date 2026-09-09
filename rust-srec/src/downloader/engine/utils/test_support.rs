#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::downloader::engine::{DownloadConfig, DownloadEngine, DownloadHandle, SegmentEvent};
#[cfg(unix)]
use crate::downloader::engine::{DownloadFailureKind, IoErrorKindSer};

#[cfg(unix)]
pub(crate) fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path.to_str().unwrap().to_owned()
}

#[cfg(unix)]
pub(crate) async fn assert_output_failure(engine: &dyn DownloadEngine, dir: &Path) {
    let config = DownloadConfig::new(
        "https://invalid.test/live",
        dir.join("recording"),
        "streamer",
        "Streamer",
        "session",
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let handle = Arc::new(DownloadHandle::new(
        "test",
        engine.engine_type(),
        config,
        tx,
    ));
    tokio::time::timeout(Duration::from_secs(10), engine.run(handle))
        .await
        .unwrap()
        .unwrap();
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    let output_error = events
        .iter()
        .position(|event| {
            matches!(
                event,
                SegmentEvent::OutputIoError {
                    io_kind: IoErrorKindSer::PermissionDenied,
                    ..
                }
            )
        })
        .expect("output error must be classified");
    let terminal = events
        .iter()
        .position(|event| {
            matches!(
                event,
                SegmentEvent::DownloadFailed {
                    kind: DownloadFailureKind::OutputRootUnavailable {
                        io_kind: IoErrorKindSer::PermissionDenied
                    },
                    ..
                }
            )
        })
        .expect("terminal must retain output classification");
    assert!(output_error < terminal);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SegmentEvent::DownloadCompleted { .. }))
    );
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum StopCase {
    Graceful,
    Expired,
    ExpiresWhileWaiting,
    Unconfirmed,
}

impl StopCase {
    pub(crate) const ALL: [Self; 4] = [
        Self::Graceful,
        Self::Expired,
        Self::ExpiresWhileWaiting,
        Self::Unconfirmed,
    ];
}

#[derive(Clone)]
pub(crate) struct RecordingFixture {
    path: std::path::PathBuf,
    graceful: bool,
    pub(crate) unconfirmed: bool,
    contract: Option<super::recording_contracts::ContractCase>,
    pipeline: bool,
}

impl RecordingFixture {
    pub(crate) fn new(dir: &Path, case: StopCase) -> Self {
        Self {
            path: dir.join("final.ts"),
            graceful: matches!(case, StopCase::Graceful),
            unconfirmed: matches!(case, StopCase::Unconfirmed),
            contract: None,
            pipeline: false,
        }
    }

    pub(crate) fn contract(
        dir: &Path,
        case: super::recording_contracts::ContractCase,
        pipeline: bool,
    ) -> Self {
        Self {
            path: dir.join("final.ts"),
            graceful: false,
            unconfirmed: false,
            contract: Some(case),
            pipeline,
        }
    }

    pub(crate) async fn inject_wait_failure(&self) -> std::io::Result<()> {
        if matches!(
            self.contract,
            Some(super::recording_contracts::ContractCase::WaitFailure)
        ) {
            super::recording_contracts::wait_for_file(&self.path.with_file_name("wait-ready"))
                .await;
            return Err(std::io::Error::other("injected recording wait failure"));
        }
        Ok(())
    }

    pub(crate) fn command(&self, streamlink: bool) -> tokio::process::Command {
        let mut command = process_utils::tokio_command(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "downloader::engine::utils::test_support::recording_child",
                "--nocapture",
            ])
            .env("SREC_RECORDING_TEST_PATH", &self.path)
            .env(
                "SREC_RECORDING_TEST_MODE",
                if streamlink {
                    "source"
                } else if self.graceful {
                    "graceful"
                } else {
                    "stubborn"
                },
            );
        if let Some(case) = self.contract {
            command.env("SREC_RECORDING_CONTRACT", case.name()).env(
                "SREC_RECORDING_PIPELINE",
                if self.pipeline { "1" } else { "0" },
            );
        }
        command
    }
}

// Re-enter the native Rust test executable so the pipe and process behavior is
// exercised identically on Windows and Unix, without a shell or installed FFmpeg.
#[test]
fn recording_child() {
    use std::io::{Read, Write};
    let Some(path) = std::env::var_os("SREC_RECORDING_TEST_PATH") else {
        return;
    };
    let mode = std::env::var("SREC_RECORDING_TEST_MODE").unwrap();
    if let Ok(case) = std::env::var("SREC_RECORDING_CONTRACT") {
        super::recording_contracts::recording_contract_child(Path::new(&path), &mode, &case);
    }
    if mode != "source" {
        // This must finish before announcing the segment: an unread stdout pipe
        // would fill here and make the parent fail its bounded startup wait.
        std::io::stdout()
            .write_all(&vec![b'x'; 2 * 1024 * 1024])
            .unwrap();
        std::io::stdout().flush().unwrap();
        std::fs::write(&path, b"settled recording").unwrap();
        eprintln!(
            "[segment @ fixture] Opening '{}' for writing",
            Path::new(&path).display()
        );
        if mode == "graceful" {
            let mut byte = [0];
            loop {
                match std::io::stdin().read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if byte[0] == b'q' => break,
                    Ok(_) => {}
                }
            }
            return;
        }
    }
    std::thread::sleep(Duration::from_secs(30));
}

pub(crate) async fn assert_recording_stop(
    engine: impl DownloadEngine + 'static,
    fixture: RecordingFixture,
    case: StopCase,
) {
    let config = DownloadConfig::new(
        "https://invalid.test/live",
        fixture.path.parent().unwrap(),
        "streamer",
        "Streamer",
        "session",
    )
    .with_max_segment_duration(10);
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let handle = Arc::new(DownloadHandle::new(
        "test",
        engine.engine_type(),
        config,
        tx,
    ));
    let engine_handle = handle.clone();
    let task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        engine.run(engine_handle).await
    }));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if matches!(
                rx.recv().await.expect("engine must announce segment"),
                SegmentEvent::SegmentStarted { .. }
            ) {
                break;
            }
        }
    })
    .await
    .expect("stdout flood and segment startup must finish");
    match case {
        StopCase::Graceful => {}
        StopCase::Expired | StopCase::Unconfirmed => {
            handle.set_stop_deadline(tokio::time::Instant::now())
        }
        StopCase::ExpiresWhileWaiting => {
            handle.set_stop_deadline(tokio::time::Instant::now() + Duration::from_millis(100))
        }
    }
    handle.cancellation_token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .expect("stop remains bounded")
        .unwrap();
    assert_eq!(result.is_err(), fixture.unconfirmed, "{case:?}: {result:?}");
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
    if fixture.unconfirmed {
        assert!(
            completed.is_empty(),
            "unconfirmed cleanup must not publish completion"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SegmentEvent::DownloadFailed { .. }))
        );
        return;
    }
    assert_eq!(completed.len(), 1, "{case:?}: {events:?}");
    let terminal = events
        .iter()
        .position(|event| {
            matches!(
                event,
                SegmentEvent::DownloadCompleted { .. } | SegmentEvent::DownloadFailed { .. }
            )
        })
        .expect("terminal event");
    assert!(completed[0] < terminal, "final segment precedes terminal");
    if let SegmentEvent::SegmentCompleted(segment) = &events[completed[0]] {
        assert_eq!(segment.path, fixture.path);
        assert_eq!(segment.size_bytes, 17);
    }
}
