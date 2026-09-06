use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::downloader::engine::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, IoErrorKindSer,
    SegmentEvent,
};

pub(crate) fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path.to_str().unwrap().to_owned()
}

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
