use std::process::Stdio;

use process_utils::ContainedChild;
use tokio::io::AsyncReadExt;
use tokio::time::{Duration, Instant, timeout};
use tracing::warn;

use super::PROCESS_CLEANUP_TIMEOUT;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_VERSION_BYTES: usize = 64 * 1024;

pub(crate) async fn probe_version(path: &str, argument: &str) -> Option<String> {
    let mut command = process_utils::tokio_command(path);
    command.arg(argument);
    run_probe(command, PROBE_TIMEOUT).await
}

async fn run_probe(mut command: tokio::process::Command, budget: Duration) -> Option<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match ContainedChild::spawn(&mut command) {
        Ok(child) => child,
        Err(error) => {
            warn!(%error, "Could not start engine version probe");
            return None;
        }
    };
    let Some(mut stdout) = child.take_stdout() else {
        if let Err(error) = child
            .terminate_tree_until(Instant::now() + PROCESS_CLEANUP_TIMEOUT)
            .await
        {
            warn!(%error, "Failed to clean up engine version probe without stdout");
        }
        return None;
    };
    let result = timeout(budget, async {
        let output = async {
            let mut output = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stdout.read(&mut buffer).await?;
                if count == 0 {
                    break;
                }
                let keep = count.min(MAX_VERSION_BYTES.saturating_sub(output.len()));
                output.extend_from_slice(&buffer[..keep]);
            }
            Ok::<_, std::io::Error>(output)
        };
        let (output, status) = tokio::join!(output, child.wait());
        status.map_err(|error| error.to_string())?;
        output.map_err(|error| error.to_string())
    })
    .await;
    match result {
        Ok(Ok(output)) => return String::from_utf8(output).ok(),
        Ok(Err(error)) => warn!(%error, "Engine version probe failed"),
        Err(_) => warn!("Engine version probe timed out"),
    }
    if let Err(error) = child
        .terminate_tree_until(Instant::now() + PROCESS_CLEANUP_TIMEOUT)
        .await
    {
        warn!(%error, "Failed to clean up engine version probe");
    }
    None
}

/// Compatibility for synchronous constructors. Runtime callers should use the
/// async constructor; this bridge blocks its caller, but the child is bounded.
pub(crate) fn probe_version_sync(path: &str, argument: &'static str) -> Option<String> {
    let path = path.to_owned();
    let worker = std::thread::Builder::new()
        .name("engine-version-probe".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            Ok::<_, std::io::Error>(runtime.block_on(probe_version(&path, argument)))
        });
    match worker {
        Ok(worker) => match worker.join() {
            Ok(Ok(version)) => version,
            Ok(Err(error)) => {
                warn!(%error, "Could not create engine probe runtime");
                None
            }
            Err(_) => {
                warn!("Engine probe worker panicked");
                None
            }
        },
        Err(error) => {
            warn!(%error, "Could not create engine probe worker");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(mode: &str) -> tokio::process::Command {
        let mut command = process_utils::tokio_command(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "downloader::engine::utils::version_probe::tests::version_child",
                "--nocapture",
            ])
            .env("SREC_VERSION_TEST_MODE", mode);
        command
    }

    #[test]
    fn version_child() {
        use std::io::Write;
        let Ok(mode) = std::env::var("SREC_VERSION_TEST_MODE") else {
            return;
        };
        if mode == "hung" {
            let path = std::env::var_os("SREC_VERSION_TEST_HEARTBEAT").unwrap();
            let mut heartbeat = std::fs::File::create(path).unwrap();
            for _ in 0..1500 {
                heartbeat.write_all(b".").unwrap();
                heartbeat.flush().unwrap();
                std::thread::sleep(Duration::from_millis(20));
            }
        } else {
            println!("{mode}");
            std::io::stdout()
                .write_all(&vec![b'x'; 2 * 1024 * 1024])
                .unwrap();
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hung_probe_yields_and_reaps_its_child() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat = dir.path().join("heartbeat");
        let mut command = command("hung");
        command.env("SREC_VERSION_TEST_HEARTBEAT", &heartbeat);
        let probe = run_probe(command, Duration::from_secs(2));
        tokio::pin!(probe);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(50)) => {},
            result = &mut probe => panic!("probe finished before independent timer: {result:?}"),
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(5), probe)
                .await
                .unwrap()
                .is_none()
        );
        let bytes = std::fs::metadata(&heartbeat)
            .expect("native child started")
            .len();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            std::fs::metadata(&heartbeat).unwrap().len(),
            bytes,
            "timed-out child was reaped"
        );
    }

    #[tokio::test]
    async fn probes_drain_large_output_and_do_not_cache_another_configuration() {
        for version in ["fixture-version-one", "fixture-version-two"] {
            let output = run_probe(command(version), Duration::from_secs(5))
                .await
                .unwrap();
            assert!(output.contains(version));
            assert_eq!(output.len(), MAX_VERSION_BYTES);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn synchronous_compatibility_constructor_can_run_inside_a_runtime() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            probe_version_sync(
                dir.path().join("missing-engine").to_str().unwrap(),
                "--version"
            )
            .is_none()
        );
    }
}
