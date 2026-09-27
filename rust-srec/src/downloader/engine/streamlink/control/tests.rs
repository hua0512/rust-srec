use super::*;
use tokio::net::TcpStream;

fn message(token: &str, event: &str) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&serde_json::json!({
        "token": token, "protocol": 1, "profile": PROFILE, "event": event,
    }))
    .unwrap();
    bytes.push(b'\n');
    bytes
}

#[tokio::test]
async fn startup_stop_is_latched_and_requires_authenticated_drain_acknowledgement() {
    let control = Companion::prepare().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut stopping = std::pin::pin!(control.request_stop(deadline));
    assert!(futures::poll!(&mut stopping).is_pending());
    let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, control.port))
        .await
        .unwrap();
    socket
        .write_all(&message(&control.token, "ready"))
        .await
        .unwrap();
    assert!(stopping.await);
    let mut reader = BufReader::new(socket);
    let mut command = String::new();
    timeout_at(deadline, reader.read_line(&mut command))
        .await
        .unwrap()
        .unwrap();
    let command: serde_json::Value = serde_json::from_str(&command).unwrap();
    assert_eq!(command["command"], "stop");
    assert_eq!(command["token"], control.token);
    assert!(command["budget_ms"].as_u64().unwrap() <= 2000);
    reader
        .get_mut()
        .write_all(&message(&control.token, "drained"))
        .await
        .unwrap();
    assert!(control.drain_result(deadline).await.is_ok());
}

#[tokio::test]
async fn unauthenticated_connection_and_unsolicited_drained_frame_cannot_claim_success() {
    let control = Companion::prepare().await.unwrap();
    let mut rogue = TcpStream::connect((Ipv4Addr::LOCALHOST, control.port))
        .await
        .unwrap();
    rogue
        .write_all(&message(&"0".repeat(64), "ready"))
        .await
        .unwrap();
    drop(rogue);
    let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, control.port))
        .await
        .unwrap();
    socket
        .write_all(&message(&control.token, "drained"))
        .await
        .unwrap();
    let error = control
        .drain_result(Instant::now() + Duration::from_secs(2))
        .await
        .unwrap_err();
    assert!(error.contains("unexpected control state"));
}

#[tokio::test]
async fn partial_control_frame_survives_a_concurrent_stop_request() {
    let control = Companion::prepare().await.unwrap();
    let socket = TcpStream::connect((Ipv4Addr::LOCALHOST, control.port))
        .await
        .unwrap();
    let (read, mut write) = socket.into_split();
    write
        .write_all(&message(&control.token, "ready"))
        .await
        .unwrap();
    let mut state = control.state.clone();
    timeout_at(Instant::now() + Duration::from_secs(2), async {
        while !state.borrow_and_update().ready {
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let incoming = message(&control.token, "drained");
    write.write_all(&incoming[..10]).await.unwrap();
    assert!(
        control
            .request_stop(Instant::now() + Duration::from_secs(2))
            .await
    );
    let mut command = String::new();
    timeout_at(
        Instant::now() + Duration::from_secs(2),
        BufReader::new(read).read_line(&mut command),
    )
    .await
    .unwrap()
    .unwrap();
    write.write_all(&incoming[10..]).await.unwrap();
    assert!(
        control
            .drain_result(Instant::now() + Duration::from_secs(2))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn unavailable_companion_obeys_the_existing_deadline() {
    let control = Companion::prepare().await.unwrap();
    assert!(
        !timeout_at(
            Instant::now() + Duration::from_secs(1),
            control.request_stop(Instant::now() + Duration::from_millis(10))
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn oversized_authenticated_frames_are_bounded() {
    let control = Companion::prepare().await.unwrap();
    let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, control.port))
        .await
        .unwrap();
    socket
        .write_all(&message(&control.token, "ready"))
        .await
        .unwrap();
    socket
        .write_all(&vec![b'x'; MAX_FRAME as usize + 1])
        .await
        .unwrap();
    let error = control
        .drain_result(Instant::now() + Duration::from_secs(2))
        .await
        .unwrap_err();
    assert!(error.contains("frame length"));
}

#[test]
fn opaque_versions_keep_the_configured_cli_without_injecting_new_options() {
    for version in [
        None,
        Some("custom executable"),
        Some("streamlink 7.6.0"),
        Some("streamlink 8.5.01"),
    ] {
        assert!(!Companion::supports_version(version));
    }
    assert!(Companion::supports_version(Some("streamlink 8.5.0\n")));
}

#[tokio::test]
async fn actual_backend_preserves_recording_after_rejected_or_ignored_plugin_capability() {
    use crate::database::models::engine::StreamlinkEngineConfig;
    use crate::downloader::engine::streamlink::StreamlinkEngine;
    use crate::downloader::engine::{
        DownloadConfig, DownloadEngine, DownloadHandle, EngineType, SegmentEvent,
    };
    let directory = tempfile::tempdir().unwrap();
    let reject = directory.path().join("opaque_reject.exe");
    let ignore = directory.path().join("opaque_ignore.exe");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/streamlink_control/opaque_cli.rs");
    let mut command =
        process_utils::tokio_command(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
    command
        .arg("--edition=2021")
        .arg(source)
        .arg("-o")
        .arg(&reject)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut compiler = process_utils::ContainedChild::spawn(&mut command).unwrap();
    assert!(
        timeout_at(Instant::now() + Duration::from_secs(20), compiler.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    tokio::fs::copy(&reject, &ignore).await.unwrap();
    for executable in [reject, ignore] {
        let binary = executable.to_str().unwrap();
        let version = crate::downloader::engine::utils::probe_version(binary, "--version").await;
        assert!(Companion::supports_version(version.as_deref()));
        assert!(!Companion::cached_capability(binary).await);
        let engine = StreamlinkEngine::with_version(
            StreamlinkEngineConfig {
                binary_path: binary.to_owned(),
                ffmpeg_path: Some(binary.to_owned()),
                graceful_stop_timeout_secs: 5,
                ..Default::default()
            },
            version,
        );
        let mut config = DownloadConfig::new(
            "https://invalid.test/opaque",
            directory.path(),
            "streamer",
            "Streamer",
            "session",
        )
        .with_max_segment_duration(10);
        let filename = format!(
            "{}-recording",
            executable.file_stem().unwrap().to_str().unwrap()
        );
        let output = directory.path().join(format!("{filename}.ts"));
        config.filename_template = filename;
        config.output_format = "ts".into();
        let (events, mut receiver) = tokio::sync::mpsc::channel(64);
        let handle = Arc::new(DownloadHandle::new(
            "opaque",
            EngineType::Streamlink,
            config,
            events,
        ));
        let collector = tokio::spawn(async move {
            let mut events = Vec::new();
            while let Some(event) = receiver.recv().await {
                events.push(event);
            }
            events
        });
        assert!(!tokio::fs::try_exists(&output).await.unwrap());
        timeout_at(
            Instant::now() + Duration::from_secs(10),
            engine.run(handle.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        drop(handle);
        let events = timeout_at(Instant::now() + Duration::from_secs(2), collector)
            .await
            .unwrap()
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SegmentEvent::DownloadCompleted { .. })),
            "{events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SegmentEvent::DownloadFailed { .. })),
            "{events:?}"
        );
        assert_eq!(
            tokio::fs::read(output).await.unwrap(),
            b"opaque recording remains available\n"
        );
        assert_eq!(
            tokio::fs::read(executable.with_extension("probe-count"))
                .await
                .unwrap(),
            b"x",
            "the cached negative probe must not inject control options into the recording command"
        );
    }
}
