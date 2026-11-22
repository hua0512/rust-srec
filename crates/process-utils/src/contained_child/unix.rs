use std::mem::MaybeUninit;
use std::os::unix::process::CommandExt;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::signal::unix::{SignalKind, signal};

use super::ContainmentError;

/// Upper bound on how long `wait_for_direct_exit_without_reap` sleeps
/// between `waitid` probes when no SIGCHLD arrives. Only a backstop: a
/// signal delivered while this task was not parked would otherwise be
/// missed, and a Tokio signal stream coalesces repeated deliveries.
const SIGCHLD_POLL_BACKSTOP: Duration = Duration::from_secs(1);

pub(super) struct PlatformContainment {
    target: Mutex<ProcessGroupTarget>,
}

struct ProcessGroupTarget {
    process_group_id: libc::pid_t,
    termination_requested: bool,
    retired: bool,
}

pub(super) fn prepare_command(command: &mut Command) -> Result<(), ContainmentError> {
    command.as_std_mut().process_group(0);
    Ok(())
}

impl PlatformContainment {
    pub(super) fn enroll(_child: &mut Child, pid: u32) -> Result<Self, ContainmentError> {
        let process_group_id =
            libc::pid_t::try_from(pid).map_err(|_| ContainmentError::InvalidProcessId { pid })?;
        if process_group_id <= 0 {
            return Err(ContainmentError::InvalidProcessId { pid });
        }
        Ok(Self {
            target: Mutex::new(ProcessGroupTarget {
                process_group_id,
                termination_requested: false,
                retired: false,
            }),
        })
    }

    pub(super) fn direct_exit_observed_without_reap(
        &self,
        _child: &mut Child,
    ) -> Result<bool, ContainmentError> {
        let process_group_id = self.target().process_group_id;
        loop {
            let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
            // SAFETY: `info` points to enough writable storage for siginfo_t;
            // WNOWAIT leaves the observed child waitable so its PGID cannot be
            // recycled before whole-tree termination is requested.
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    process_group_id as libc::id_t,
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                )
            };
            if result == 0 {
                // SAFETY: the storage was fully zero-initialized before waitid
                // wrote its siginfo_t result. si_signo stays zero when WNOHANG
                // observes no state change and is SIGCHLD for a reported child
                // transition.
                return Ok(unsafe { info.assume_init() }.si_signo != 0);
            }

            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                // A signal delivered to this process interrupted the call and
                // says nothing about the child. Ask again.
                Some(libc::EINTR) => continue,
                // The child is gone from this process's perspective, so its
                // exit is as observed as it can be. Callers proceed to
                // whole-tree termination and then reap through Tokio, which
                // reports the status it cached when it reaped the child.
                Some(libc::ECHILD) => return Ok(true),
                _ => {
                    return Err(ContainmentError::io(
                        "observe Unix child exit without reaping",
                        error,
                    ));
                }
            }
        }
    }

    /// Park until `waitid` reports the direct child's exit.
    ///
    /// The wakeup is a SIGCHLD stream rather than a fixed poll interval: a
    /// supervisor parks here for the entire lifetime of its worker, so a
    /// short interval would keep the process off idle for hours to observe
    /// a single event. `SIGCHLD_POLL_BACKSTOP` covers a signal delivered
    /// between the `waitid` above and the `recv` below, and any exit that
    /// was already pending when this loop was entered.
    pub(super) async fn wait_for_direct_exit_without_reap(
        &self,
        child: &mut Child,
    ) -> Result<(), ContainmentError> {
        let mut sigchld = signal(SignalKind::child()).map_err(|source| {
            ContainmentError::io("register SIGCHLD listener for contained child", source)
        })?;
        loop {
            if self.direct_exit_observed_without_reap(child)? {
                return Ok(());
            }
            let _ = tokio::time::timeout(SIGCHLD_POLL_BACKSTOP, sigchld.recv()).await;
        }
    }

    pub(super) fn terminate_tree(
        &self,
        direct_exit_observed: bool,
    ) -> Result<(), ContainmentError> {
        let mut target = self.target();
        if target.retired || target.termination_requested {
            return Ok(());
        }
        // SAFETY: `process_group_id` is a positive PID captured from the child;
        // negating it asks the kernel to signal that dedicated process group.
        let result = unsafe { libc::kill(-target.process_group_id, libc::SIGKILL) };
        if result == 0 {
            target.termination_requested = true;
            return Ok(());
        }

        let source = std::io::Error::last_os_error();
        let settled = source.raw_os_error() == Some(libc::ESRCH)
            || (cfg!(target_os = "macos")
                && direct_exit_observed
                && source.raw_os_error() == Some(libc::EPERM));
        if settled {
            // Darwin reports EPERM when killpg targets an unreaped process
            // group whose leader has exited and no signalable descendants
            // remain. Accept it only after waitid proved that leader exit;
            // explicit watchdog termination still surfaces EPERM.
            target.termination_requested = true;
            Ok(())
        } else {
            Err(ContainmentError::io("terminate Unix process group", source))
        }
    }

    pub(super) fn retire_tree_target(&self) {
        self.target().retired = true;
    }

    fn target(&self) -> MutexGuard<'_, ProcessGroupTarget> {
        self.target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
