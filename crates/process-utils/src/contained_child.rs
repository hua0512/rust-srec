use std::io;
use std::process::ExitStatus;
use std::sync::Arc;

use thiserror::Error;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::time::{Instant, timeout_at};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(test)]
mod tests;

/// A Tokio child whose descendants share one operating-system containment.
///
/// Unix children start in a dedicated process group. Windows children start
/// suspended, are enrolled in a kill-on-close Job Object, and resume only
/// after enrollment is confirmed. Dropping this value requests best-effort
/// termination of the whole tree and also applies Tokio's direct-child
/// kill-on-drop behavior.
///
/// Containment assumes descendants do not deliberately escape their inherited
/// process group or Job Object. This interface is available on Windows and on
/// Unix targets whose libc exposes POSIX `waitid` with `WNOWAIT`.
#[must_use = "dropping the contained child terminates its process tree"]
pub struct ContainedChild {
    child: Child,
    platform: Arc<platform::PlatformContainment>,
    original_pid: u32,
    tree_termination_requested: bool,
}

impl ContainedChild {
    /// Spawn `command` under a new process-tree containment.
    ///
    /// On Windows this method owns the command's creation flags so it can
    /// guarantee suspended enrollment before any child code runs.
    pub fn spawn(command: &mut Command) -> Result<Self, ContainmentError> {
        command.kill_on_drop(true);
        platform::prepare_command(command)?;

        let mut child = command
            .spawn()
            .map_err(|source| ContainmentError::io("spawn contained child", source))?;
        let original_pid = child.id().ok_or(ContainmentError::MissingProcessId)?;

        let platform = match platform::PlatformContainment::enroll(&mut child, original_pid) {
            Ok(platform) => Arc::new(platform),
            Err(error) => {
                if let Err(source) = child.start_kill() {
                    tracing::warn!(
                        error = %source,
                        pid = original_pid,
                        "Failed to terminate child after containment setup failed"
                    );
                }
                return Err(error);
            }
        };

        Ok(Self {
            child,
            platform,
            original_pid,
            tree_termination_requested: false,
        })
    }

    /// Take the child's configured stdin pipe, if one exists.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    /// Take the child's configured stdout pipe, if one exists.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// Take the child's configured stderr pipe, if one exists.
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    /// Return the direct child's process identifier while Tokio still owns it.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Request cooperative termination of the direct child on Unix via SIGTERM.
    /// Descendants remain running until the leader exits or tree termination is
    /// requested. Returns false for an already-reaped child or on Windows, where
    /// a hidden contained process has no console control-signal contract.
    pub fn request_shutdown(&mut self) -> Result<bool, ContainmentError> {
        #[cfg(unix)]
        {
            let Some(pid) = self.child.id() else {
                return Ok(false);
            };
            let pid = libc::pid_t::try_from(pid).map_err(|error| {
                ContainmentError::io(
                    "convert child PID",
                    io::Error::new(io::ErrorKind::InvalidInput, error),
                )
            })?;
            // SAFETY: Tokio still owns this unreaped child, so its PID cannot
            // have been reused. Signal only its leader for cooperative cleanup.
            if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
                return Ok(true);
            }
            let source = io::Error::last_os_error();
            if source.raw_os_error() == Some(libc::ESRCH) {
                return Ok(false);
            }
            Err(ContainmentError::io("request child shutdown", source))
        }
        #[cfg(windows)]
        {
            Ok(false)
        }
    }

    /// Clone a synchronous whole-tree terminator for deadline watchdogs.
    ///
    /// The returned handle is independent of Tokio. It may be moved to a
    /// dedicated thread and invoked immediately before a fail-stop exit.
    pub fn tree_terminator(&self) -> TreeTerminator {
        TreeTerminator {
            platform: Arc::clone(&self.platform),
        }
    }

    /// Check whether the direct child has exited without waiting.
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, ContainmentError> {
        if self.child.id().is_none() {
            if !self.tree_termination_requested {
                self.request_tree_termination_after_direct_exit()?;
            }
            return self
                .child
                .try_wait()
                .map_err(|source| ContainmentError::io("query contained child status", source));
        }
        if !self
            .platform
            .direct_exit_observed_without_reap(&mut self.child)?
        {
            return Ok(None);
        }
        if !self.tree_termination_requested {
            self.request_tree_termination_after_direct_exit()?;
        }
        self.platform.retire_tree_target();
        let status = self
            .child
            .try_wait()
            .map_err(|source| ContainmentError::io("query contained child status", source))?;
        Ok(status)
    }

    /// Wait for and reap the direct child.
    ///
    /// Once the direct child exits, any descendants are force-terminated before
    /// this method returns. On Unix the leader remains waitable until the process
    /// group is killed, preventing its group identifier from being reused first.
    /// Unix callers must enable Tokio's time driver.
    pub async fn wait(&mut self) -> Result<ExitStatus, ContainmentError> {
        if self.child.id().is_none() {
            if !self.tree_termination_requested {
                self.request_tree_termination_after_direct_exit()?;
            }
            return self
                .child
                .wait()
                .await
                .map_err(|source| ContainmentError::io("wait for contained child", source));
        }
        self.platform
            .wait_for_direct_exit_without_reap(&mut self.child)
            .await?;
        if !self.tree_termination_requested {
            self.request_tree_termination_after_direct_exit()?;
        }
        self.platform.retire_tree_target();
        self.child
            .wait()
            .await
            .map_err(|source| ContainmentError::io("wait for contained child", source))
    }

    /// Force-terminate the contained tree and reap the direct child by `deadline`.
    ///
    /// The whole-tree termination request is issued synchronously before the
    /// deadline-bounded wait begins. A successful return proves the direct child
    /// was reaped. Descendants receive the same hard termination request, but are
    /// not individually enumerated or awaited.
    pub async fn terminate_tree_until(
        &mut self,
        deadline: Instant,
    ) -> Result<ExitStatus, ContainmentError> {
        if self.child.id().is_none() {
            if !self.tree_termination_requested {
                self.request_tree_termination_after_direct_exit()?;
            }
            if Instant::now() >= deadline {
                return Err(ContainmentError::DeadlineExceeded {
                    pid: self.original_pid,
                });
            }
            // A completed reap is a completed reap: the tree is contained no
            // matter which side of `deadline` the clock landed on when the wait
            // resolved. Only an unfinished wait is a deadline violation.
            return match timeout_at(deadline, self.child.wait()).await {
                Ok(Ok(status)) => Ok(status),
                Ok(Err(source)) => Err(ContainmentError::io(
                    "reap force-terminated contained child",
                    source,
                )),
                Err(_) => Err(ContainmentError::DeadlineExceeded {
                    pid: self.original_pid,
                }),
            };
        }
        let tree_result = self.request_tree_termination();
        if let Err(source) = self.child.start_kill() {
            tracing::warn!(
                error = %source,
                pid = self.original_pid,
                "Direct-child termination request failed after whole-tree termination"
            );
        }
        #[cfg(target_os = "macos")]
        let tree_result = match tree_result {
            Err(error) if matches!(&error, ContainmentError::Io { source, .. } if source.raw_os_error() == Some(libc::EPERM)) =>
            {
                // A cooperative signal can exit the leader just before force
                // cleanup. Darwin excludes zombies from killpg's recipients and
                // returns EPERM for that now-unsignalable group. Exit may not
                // be waitable yet, so use the remaining cleanup deadline after
                // requesting direct-child kill. Never reap or accept EPERM before
                // confirming exit and retrying the guarded group termination.
                let observed = self
                    .platform
                    .direct_exit_observed_without_reap(&mut self.child)?;
                confirm_direct_exit_until(
                    observed,
                    self.platform
                        .wait_for_direct_exit_without_reap(&mut self.child),
                    deadline,
                    self.original_pid,
                )
                .await?;
                self.request_tree_termination_after_direct_exit()
            }
            result => result,
        };
        tree_result?;
        if Instant::now() >= deadline {
            return Err(ContainmentError::DeadlineExceeded {
                pid: self.original_pid,
            });
        }

        match timeout_at(
            deadline,
            self.platform
                .wait_for_direct_exit_without_reap(&mut self.child),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                return Err(ContainmentError::DeadlineExceeded {
                    pid: self.original_pid,
                });
            }
        }
        if Instant::now() >= deadline {
            return Err(ContainmentError::DeadlineExceeded {
                pid: self.original_pid,
            });
        }
        self.platform.retire_tree_target();

        match timeout_at(deadline, self.child.wait()).await {
            Ok(Ok(status)) => Ok(status),
            Ok(Err(source)) => Err(ContainmentError::io(
                "reap force-terminated contained child",
                source,
            )),
            Err(_) => Err(ContainmentError::DeadlineExceeded {
                pid: self.original_pid,
            }),
        }
    }

    fn request_tree_termination(&mut self) -> Result<(), ContainmentError> {
        if self.tree_termination_requested {
            return Ok(());
        }
        self.platform.terminate_tree(false)?;
        self.tree_termination_requested = true;
        Ok(())
    }

    fn request_tree_termination_after_direct_exit(&mut self) -> Result<(), ContainmentError> {
        if self.tree_termination_requested {
            return Ok(());
        }
        self.platform.terminate_tree(true)?;
        self.tree_termination_requested = true;
        Ok(())
    }
}

/// A false immediate observation must not discard the remaining cleanup budget.
/// The supplied waiter observes exit without reaping, preserving the tree identity
/// until the caller retries group termination.
#[cfg(any(target_os = "macos", test))]
async fn confirm_direct_exit_until(
    already_observed: bool,
    wait_for_exit: impl std::future::Future<Output = Result<(), ContainmentError>>,
    deadline: Instant,
    pid: u32,
) -> Result<(), ContainmentError> {
    if already_observed {
        return Ok(());
    }
    match timeout_at(deadline, wait_for_exit).await {
        Ok(result) => result,
        Err(_) => Err(ContainmentError::DeadlineExceeded { pid }),
    }
}

/// A cloneable synchronous termination handle for one contained process tree.
///
/// On Windows every clone retains the same kill-on-close Job Object. On Unix a
/// clone retains the dedicated process-group identifier. Concurrent and repeated
/// calls to [`Self::terminate_tree`] are supported; the operating-system
/// termination operation is idempotent for the retained containment identity.
#[derive(Clone)]
#[must_use = "retain this handle until its deadline watchdog is disarmed"]
pub struct TreeTerminator {
    platform: Arc<platform::PlatformContainment>,
}

impl TreeTerminator {
    /// Request immediate force termination of the whole contained tree.
    pub fn terminate_tree(&self) -> Result<(), ContainmentError> {
        self.platform.terminate_tree(false)
    }
}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        if let Err(error) = self.request_tree_termination() {
            tracing::warn!(
                %error,
                pid = self.original_pid,
                "Best-effort contained process-tree termination failed during drop"
            );
        }
        self.platform.retire_tree_target();
        if let Err(error) = self.child.start_kill() {
            tracing::warn!(
                %error,
                pid = self.original_pid,
                "Best-effort direct-child termination failed during drop"
            );
        }
    }
}

/// Failures at the process-containment seam.
#[derive(Debug, Error)]
pub enum ContainmentError {
    #[error("spawned child has no process identifier")]
    MissingProcessId,

    #[error("process identifier {pid} cannot be represented by this platform")]
    InvalidProcessId { pid: u32 },

    #[error("deadline elapsed while reaping force-terminated child {pid}")]
    DeadlineExceeded { pid: u32 },

    #[error("{operation}: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
}

impl ContainmentError {
    fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }
}
