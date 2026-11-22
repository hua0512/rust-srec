use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{Instant, sleep, timeout};

use super::ContainedChild;

const HELPER_MODE: &str = "PROCESS_UTILS_CONTAINED_CHILD_HELPER";
const HEARTBEAT_PATH: &str = "PROCESS_UTILS_CONTAINED_CHILD_HEARTBEAT";
const HELPER_LIFETIME: Duration = Duration::from_secs(15);
static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn false_exit_observation_waits_for_confirmation_before_retry() {
    let (exit, exited) = tokio::sync::oneshot::channel();
    let retried = AtomicBool::new(false);
    let mut recovery = Box::pin(async {
        super::confirm_direct_exit_until(
            false,
            async {
                exited.await.map_err(|error| {
                    super::ContainmentError::io(
                        "observe fixture exit",
                        std::io::Error::other(error),
                    )
                })
            },
            Instant::now() + Duration::from_secs(2),
            1,
        )
        .await?;
        retried.store(true, Ordering::SeqCst);
        Ok::<(), super::ContainmentError>(())
    });
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(recovery.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(!retried.load(Ordering::SeqCst));
    exit.send(()).expect("release nonreaping exit observation");
    timeout(Duration::from_secs(2), recovery)
        .await
        .expect("recovery remains bounded")
        .expect("exit becomes confirmed");
    assert!(retried.load(Ordering::SeqCst));
}

#[tokio::test]
async fn failed_or_expired_exit_confirmation_never_authorizes_retry() {
    for expires in [false, true] {
        let retried = AtomicBool::new(false);
        let recovery = async {
            super::confirm_direct_exit_until(
                false,
                async {
                    if expires {
                        std::future::pending::<()>().await;
                    }
                    Err(super::ContainmentError::io(
                        "fixture exit observation",
                        std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                    ))
                },
                if expires {
                    Instant::now()
                } else {
                    Instant::now() + Duration::from_secs(2)
                },
                123,
            )
            .await?;
            retried.store(true, Ordering::SeqCst);
            Ok::<(), super::ContainmentError>(())
        };
        let result = timeout(Duration::from_secs(2), recovery)
            .await
            .expect("failed recovery remains bounded");
        if expires {
            assert!(matches!(
                result,
                Err(super::ContainmentError::DeadlineExceeded { pid: 123 })
            ));
        } else {
            assert!(
                matches!(result, Err(super::ContainmentError::Io { source, .. }) if source.kind() == std::io::ErrorKind::PermissionDenied)
            );
        }
        assert!(!retried.load(Ordering::SeqCst));
    }
}

#[tokio::test]
#[cfg(target_os = "macos")]
async fn delayed_native_exit_is_confirmed_before_group_retry_and_reap() {
    let mut command = helper_command("stdin");
    command.stdin(Stdio::piped());
    let mut child = ContainedChild::spawn(&mut command).expect("spawn native exit fixture");
    let mut stdin = child.take_stdin().expect("fixture stdin");
    let observed = child
        .platform
        .direct_exit_observed_without_reap(&mut child.child)
        .expect("initial exit observation");
    assert!(!observed, "stdin-gated child has not exited");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut confirmation = Box::pin(super::confirm_direct_exit_until(
        observed,
        child
            .platform
            .wait_for_direct_exit_without_reap(&mut child.child),
        deadline,
        child.original_pid,
    ));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(confirmation.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    stdin
        .write_all(b"hello\n")
        .await
        .expect("allow native child to exit");
    drop(stdin);
    confirmation
        .await
        .expect("native leader exit becomes observable");
    assert!(
        child.id().is_some(),
        "exit observation must not reap the leader"
    );
    child
        .request_tree_termination_after_direct_exit()
        .expect("retry guarded group termination");
    let status = child
        .terminate_tree_until(deadline)
        .await
        .expect("reap using remaining original deadline");
    assert!(status.success());
    assert!(child.id().is_none());
}

#[test]
#[cfg(any(unix, windows))]
fn contained_child_helper() {
    let Ok(mode) = std::env::var(HELPER_MODE) else {
        return;
    };

    match mode.as_str() {
        "exit-7" => std::process::exit(7),
        "stdin" => {
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .expect("helper reads stdin");
            if line.trim_end() != "hello" {
                std::process::exit(9);
            }
        }
        "hold" => std::thread::sleep(HELPER_LIFETIME),
        "tree-parent" => {
            let executable = std::env::current_exe().expect("helper executable");
            let heartbeat = std::env::var_os(HEARTBEAT_PATH).expect("heartbeat path");
            let mut command = crate::std_command(executable);
            command
                .arg("contained_child_helper")
                .arg("--nocapture")
                .env(HELPER_MODE, "tree-leaf")
                .env(HEARTBEAT_PATH, heartbeat)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut leaf = command.spawn().expect("spawn tree leaf");
            let deadline = std::time::Instant::now() + HELPER_LIFETIME;
            while std::time::Instant::now() < deadline {
                if let Some(status) = leaf.try_wait().expect("query tree leaf") {
                    panic!("tree leaf exited unexpectedly: {status}");
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            leaf.kill().expect("terminate expired tree leaf");
            leaf.wait().expect("reap expired tree leaf");
        }
        "tree-leaf" => {
            let heartbeat = std::env::var_os(HEARTBEAT_PATH).expect("heartbeat path");
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(heartbeat)
                .expect("open heartbeat");
            let deadline = std::time::Instant::now() + HELPER_LIFETIME;
            while std::time::Instant::now() < deadline {
                file.write_all(b"x").expect("write heartbeat");
                file.flush().expect("flush heartbeat");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        other => panic!("unknown helper mode: {other}"),
    }
}

#[tokio::test]
#[cfg(any(unix, windows))]
async fn waits_for_direct_child() {
    let mut command = helper_command("exit-7");
    let mut child = ContainedChild::spawn(&mut command).expect("spawn contained child");
    assert!(child.id().is_some());
    let _initial_status = child.try_wait().expect("query child status");

    let status = timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("child wait stays bounded")
        .expect("wait for child");
    assert_eq!(status.code(), Some(7));
    assert!(
        !child
            .request_shutdown()
            .expect("reaped child is not signalled")
    );
    assert!(child.id().is_none());
}

#[tokio::test]
#[cfg(any(unix, windows))]
async fn exposes_configured_stdin() {
    let mut command = helper_command("stdin");
    command.stdin(Stdio::piped());
    let mut child = ContainedChild::spawn(&mut command).expect("spawn contained child");
    let mut stdin = child.take_stdin().expect("configured stdin pipe");
    stdin
        .write_all(b"hello\n")
        .await
        .expect("write child stdin");
    drop(stdin);

    let status = timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("child wait stays bounded")
        .expect("wait for child");
    assert!(status.success());
}

#[tokio::test]
#[cfg(any(unix, windows))]
async fn cooperative_shutdown_respects_the_platform_signal_contract() {
    let mut command = helper_command("hold");
    let mut child = ContainedChild::spawn(&mut command).expect("spawn contained child");
    assert_eq!(
        child
            .request_shutdown()
            .expect("request cooperative shutdown"),
        cfg!(unix)
    );
    child
        .terminate_tree_until(Instant::now() + Duration::from_secs(5))
        .await
        .expect("cleanup platform-signal fixture");
}

#[tokio::test]
#[cfg(target_os = "macos")]
async fn forced_cleanup_reaps_a_cooperatively_exited_unreaped_leader() {
    let mut command = helper_command("hold");
    let mut child = ContainedChild::spawn(&mut command).expect("spawn contained child");
    assert!(
        child
            .request_shutdown()
            .expect("request cooperative shutdown")
    );
    // Pin the race's post-SIGTERM state: the leader has exited, but its PID
    // and process-group identity remain owned until forced cleanup reaps it.
    timeout(Duration::from_secs(5), async {
        while !child
            .platform
            .direct_exit_observed_without_reap(&mut child.child)
            .expect("observe cooperative exit without reaping")
        {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("cooperative exit remains bounded");
    assert!(child.id().is_some(), "leader must remain unreaped");
    child
        .terminate_tree_until(Instant::now() + Duration::from_secs(2))
        .await
        .expect("confirmed exit permits contained cleanup after Darwin EPERM");
    assert!(child.id().is_none(), "forced cleanup reaps the leader");
}

#[tokio::test]
#[cfg(any(unix, windows))]
async fn expired_deadline_never_adds_an_unbounded_wait() {
    let mut command = helper_command("hold");
    let mut child = ContainedChild::spawn(&mut command).expect("spawn contained child");

    let started = Instant::now();
    let result = child.terminate_tree_until(started).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "an expired deadline must return promptly"
    );
    assert!(
        result.is_ok()
            || matches!(
                &result,
                Err(super::ContainmentError::DeadlineExceeded { .. })
            ),
        "force termination returned an unexpected error: {result:?}"
    );

    if child.id().is_some() {
        timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("force-terminated child eventually exits")
            .expect("reap force-terminated child");
    }
}

#[test]
#[cfg(unix)]
fn cancelled_wait_does_not_leave_blocking_runtime_work() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let mut child = runtime.block_on(async {
        let mut command = helper_command("hold");
        ContainedChild::spawn(&mut command).expect("spawn contained child")
    });
    let terminator = child.tree_terminator();

    let timed_out = runtime.block_on(async {
        timeout(Duration::from_millis(50), child.wait())
            .await
            .is_err()
    });
    assert!(timed_out, "the live child wait should be cancelled");

    let started = std::time::Instant::now();
    drop(runtime);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "cancelling wait must not strand blocking runtime work"
    );

    terminator
        .terminate_tree()
        .expect("terminate cancelled-wait helper");
    let cleanup_deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if child
            .try_wait()
            .expect("query cancelled-wait helper during cleanup")
            .is_some()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < cleanup_deadline,
            "cancelled-wait helper did not exit during cleanup"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[tokio::test]
#[cfg(any(unix, windows))]
async fn synchronous_terminator_can_run_on_a_watchdog_thread() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<super::TreeTerminator>();

    let mut command = helper_command("hold");
    let mut child = ContainedChild::spawn(&mut command).expect("spawn contained child");
    let terminator = child.tree_terminator();
    std::thread::spawn(move || terminator.terminate_tree())
        .join()
        .expect("watchdog thread does not panic")
        .expect("watchdog terminates contained tree");

    timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("terminated child wait stays bounded")
        .expect("reap watchdog-terminated child");
}

#[tokio::test]
#[cfg(any(unix, windows))]
async fn force_termination_targets_descendants() {
    let heartbeat = unique_heartbeat_path();
    remove_heartbeat_if_present(&heartbeat);

    let mut command = helper_command("tree-parent");
    command.env(HEARTBEAT_PATH, &heartbeat);
    let mut child = ContainedChild::spawn(&mut command).expect("spawn contained process tree");
    wait_for_heartbeat(&heartbeat).await;

    child
        .terminate_tree_until(Instant::now() + Duration::from_secs(5))
        .await
        .expect("terminate and reap contained process tree");

    sleep(Duration::from_millis(150)).await;
    let settled_len = fs::metadata(&heartbeat)
        .expect("heartbeat metadata after termination")
        .len();
    sleep(Duration::from_millis(250)).await;
    let final_len = fs::metadata(&heartbeat)
        .expect("heartbeat metadata remains available")
        .len();
    assert_eq!(
        settled_len, final_len,
        "descendant kept writing after tree termination"
    );

    fs::remove_file(&heartbeat).expect("remove heartbeat file");
}

#[cfg(any(unix, windows))]
fn helper_command(mode: &str) -> Command {
    let executable = std::env::current_exe().expect("test executable");
    let mut command = crate::tokio_command(executable);
    command
        .arg("contained_child_helper")
        .arg("--nocapture")
        .env(HELPER_MODE, mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(any(unix, windows))]
async fn wait_for_heartbeat(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::metadata(path).is_ok_and(|metadata| metadata.len() >= 3) {
            return;
        }
        assert!(Instant::now() < deadline, "heartbeat helper did not start");
        sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(any(unix, windows))]
fn unique_heartbeat_path() -> PathBuf {
    let test_id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "process-utils-contained-child-{}-{test_id}.heartbeat",
        std::process::id()
    ))
}

#[cfg(any(unix, windows))]
fn remove_heartbeat_if_present(path: &std::path::Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("failed to clear heartbeat file {}: {error}", path.display()),
    }
}
