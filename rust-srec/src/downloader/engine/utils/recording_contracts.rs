//! Native recording fixtures re-enter the existing test executable. All input,
//! file writes and synchronization are local; installed media tools are unnecessary.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::task::AbortOnDropHandle;

use crate::downloader::EngineEndSignal;
use crate::downloader::engine::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, IoErrorKindSer,
    SegmentEvent,
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum ContractCase {
    Rotation,
    Exit228,
    RepeatedEnospc228,
    Permission228,
    LateWrite,
    WaitFailure,
    SingleOutput,
}

impl ContractCase {
    pub(crate) const ALL: [Self; 7] = [
        Self::Rotation,
        Self::Exit228,
        Self::RepeatedEnospc228,
        Self::Permission228,
        Self::LateWrite,
        Self::WaitFailure,
        Self::SingleOutput,
    ];
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Rotation => "rotation",
            Self::Exit228 => "exit228",
            Self::RepeatedEnospc228 => "enospc228",
            Self::Permission228 => "permission228",
            Self::LateWrite => "late-write",
            Self::WaitFailure => "wait-failure",
            Self::SingleOutput => "single",
        }
    }
}

pub(crate) async fn wait_for_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture handshake must arrive");
}

fn wait_for_release(path: &Path) {
    let start = std::time::Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "fixture release must arrive"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn close_stderr() {
    std::io::stderr().flush().unwrap();
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn close(fd: i32) -> i32;
        }
        // SAFETY: only this fixture subprocess owns descriptor 2. It emits no
        // further stderr records after closing the inherited pipe.
        assert_eq!(unsafe { close(2) }, 0);
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(which: u32) -> *mut std::ffi::c_void;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        // SAFETY: close only this subprocess's inherited standard-error handle,
        // after flushing it; no further stderr use occurs before process exit.
        assert_ne!(unsafe { CloseHandle(GetStdHandle(-12i32 as u32)) }, 0);
    }
}

pub(crate) fn recording_contract_child(path: &Path, role: &str, case: &str) -> ! {
    let pipeline = std::env::var("SREC_RECORDING_PIPELINE").unwrap() == "1";
    if role == "source" {
        if case == "wait-failure" {
            std::thread::sleep(Duration::from_secs(30));
        }
        std::process::exit(0);
    }
    if pipeline && case != "wait-failure" {
        // The remux fixture waits for the source pipe's EOF so exit-228 tests
        // cannot accidentally turn into broken-pipe/source-settlement tests.
        std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
    }
    std::fs::write(path, b"prefix").unwrap();
    if case != "single" {
        eprintln!(
            "[segment @ fixture] Opening '{}' for writing",
            path.display()
        );
    }
    eprint!("frame=1 size=1kB time=00:00:01.25\r");
    std::io::stderr().flush().unwrap();
    match case {
        "rotation" => {
            let second = path.with_file_name("second.ts");
            std::fs::write(&second, b"second-segment").unwrap();
            eprintln!(
                "[segment @ fixture] Opening '{}' for writing",
                second.display()
            );
            eprint!("frame=2 size=2kB time=00:00:00.75\r");
        }
        "enospc228" => {
            eprintln!("Error writing trailer: No space left on device");
            eprintln!("Error closing output file: No space left on device");
        }
        "permission228" => eprintln!("Error opening output file: Permission denied"),
        "late-write" => {
            close_stderr();
            std::fs::write(path.with_file_name("stderr-closed"), b"closed").unwrap();
            wait_for_release(&path.with_file_name("release-final-write"));
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .unwrap()
                .write_all(b"-late-trailer")
                .unwrap();
        }
        "wait-failure" => {
            std::fs::write(path.with_file_name("wait-ready"), b"ready").unwrap();
            if pipeline {
                let mut buffer = Vec::new();
                std::io::stdin().read_to_end(&mut buffer).unwrap();
            } else {
                std::thread::sleep(Duration::from_secs(30));
            }
        }
        _ => {}
    }
    if case != "late-write" {
        std::io::stderr().flush().unwrap();
    }
    std::process::exit(
        if matches!(case, "exit228" | "enospc228" | "permission228") {
            228
        } else {
            0
        },
    );
}

pub(crate) async fn assert_recording_contract(
    engine: impl DownloadEngine + 'static,
    directory: &Path,
    case: ContractCase,
) {
    let config = DownloadConfig::new(
        "https://invalid.test/live",
        directory,
        "streamer",
        "Streamer",
        "session",
    )
    .with_filename_template("final")
    .with_output_format("ts")
    .with_max_segment_duration(if matches!(case, ContractCase::SingleOutput) {
        0
    } else {
        10
    });
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let handle = Arc::new(DownloadHandle::new(
        "contract",
        engine.engine_type(),
        config,
        tx,
    ));
    let task = AbortOnDropHandle::new(tokio::spawn(async move { engine.run(handle).await }));
    let mut events = Vec::new();
    if matches!(case, ContractCase::LateWrite) {
        wait_for_file(&directory.join("stderr-closed")).await;
        let premature = tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                let event = rx.recv().await.expect("writer is still alive");
                let terminal = matches!(
                    event,
                    SegmentEvent::SegmentCompleted(_)
                        | SegmentEvent::DownloadCompleted { .. }
                        | SegmentEvent::DownloadFailed { .. }
                );
                events.push(event);
                if terminal {
                    return;
                }
            }
        })
        .await;
        assert!(
            premature.is_err(),
            "stderr EOF must not publish a mutable final path"
        );
        std::fs::write(directory.join("release-final-write"), b"release").unwrap();
    }
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("recording must settle")
        .unwrap()
        .unwrap();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    let completed: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let SegmentEvent::SegmentCompleted(segment) = event {
                Some(segment)
            } else {
                None
            }
        })
        .collect();
    let terminal: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            matches!(
                event,
                SegmentEvent::DownloadCompleted { .. } | SegmentEvent::DownloadFailed { .. }
            )
        })
        .collect();
    assert_eq!(terminal.len(), 1, "{case:?}: {events:?}");
    assert_eq!(
        terminal[0].0,
        events.len() - 1,
        "terminal must follow every segment/output event"
    );
    assert_eq!(
        completed.len(),
        if matches!(case, ContractCase::Rotation) {
            2
        } else {
            1
        },
        "{case:?}: {events:?}"
    );
    for (index, segment) in completed.iter().enumerate() {
        assert_eq!(segment.index, index as u32);
        assert_eq!(
            segment.path,
            directory.join(if index == 0 { "final.ts" } else { "second.ts" })
        );
        assert_eq!(
            segment.size_bytes,
            std::fs::metadata(&segment.path).unwrap().len()
        );
    }
    if matches!(case, ContractCase::LateWrite) {
        assert_eq!(completed[0].size_bytes, 19);
    }
    let errors: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let SegmentEvent::OutputIoError { io_kind, .. } = event {
                Some(io_kind)
            } else {
                None
            }
        })
        .collect();
    match case {
        ContractCase::Exit228 | ContractCase::RepeatedEnospc228 | ContractCase::Permission228 => {
            let kind = if matches!(case, ContractCase::Permission228) {
                IoErrorKindSer::PermissionDenied
            } else {
                IoErrorKindSer::StorageFull
            };
            assert_eq!(
                errors,
                vec![&kind],
                "output failures are deduplicated and retain stderr classification"
            );
            assert!(
                matches!(terminal[0].1, SegmentEvent::DownloadFailed { kind: DownloadFailureKind::OutputRootUnavailable { io_kind }, .. } if *io_kind == kind)
            );
        }
        ContractCase::WaitFailure => {
            assert!(errors.is_empty());
            assert!(
                matches!(terminal[0].1, SegmentEvent::DownloadFailed { kind: DownloadFailureKind::ProcessExit { code: None }, message } if message.contains("injected recording wait failure"))
            );
        }
        _ => {
            assert!(errors.is_empty());
            let bytes: u64 = completed.iter().map(|segment| segment.size_bytes).sum();
            let duration = if matches!(case, ContractCase::Rotation) {
                2.0
            } else {
                1.25
            };
            assert!(
                matches!(terminal[0].1, SegmentEvent::DownloadCompleted { total_bytes, total_duration_secs, total_segments, engine_signal: EngineEndSignal::SubprocessExitZero } if *total_bytes == bytes && *total_duration_secs == duration && *total_segments as usize == completed.len())
            );
        }
    }
    if matches!(case, ContractCase::Rotation) {
        assert_eq!(completed[0].duration_secs, 1.25);
        assert_eq!(completed[1].duration_secs, 0.75);
        let starts: Vec<_> = events
            .iter()
            .filter_map(|event| {
                if let SegmentEvent::SegmentStarted { sequence, .. } = event {
                    Some(*sequence)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(starts, vec![0, 1]);
        let first_complete = events
            .iter()
            .position(|event| matches!(event, SegmentEvent::SegmentCompleted(_)))
            .unwrap();
        assert!(matches!(
            &events[first_complete + 1],
            SegmentEvent::SegmentStarted { sequence: 1, .. }
        ));
    }
}
