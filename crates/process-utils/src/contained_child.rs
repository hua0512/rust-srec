use std::io;
use std::process::ExitStatus;
use std::sync::Arc;

use thiserror::Error;
use tokio::process::{Child, ChildStdin, Command};
use tokio::time::{Instant, timeout_at};

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

    /// Return the direct child's process identifier while Tokio still owns it.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
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
                self.request_tree_termination()?;
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
            self.request_tree_termination()?;
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
                self.request_tree_termination()?;
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
            self.request_tree_termination()?;
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
                self.request_tree_termination()?;
            }
            if Instant::now() >= deadline {
                return Err(ContainmentError::DeadlineExceeded {
                    pid: self.original_pid,
                });
            }
            return match timeout_at(deadline, self.child.wait()).await {
                Ok(Ok(status)) if Instant::now() < deadline => Ok(status),
                Ok(Ok(_)) => Err(ContainmentError::DeadlineExceeded {
                    pid: self.original_pid,
                }),
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
            Ok(Ok(status)) if Instant::now() < deadline => Ok(status),
            Ok(Ok(_)) => Err(ContainmentError::DeadlineExceeded {
                pid: self.original_pid,
            }),
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
        self.platform.terminate_tree()?;
        self.tree_termination_requested = true;
        Ok(())
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
        self.platform.terminate_tree()
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

#[cfg(unix)]
mod platform {
    use std::mem::MaybeUninit;
    use std::os::unix::process::CommandExt;
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration;

    use tokio::process::{Child, Command};

    use super::ContainmentError;

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
            let process_group_id = libc::pid_t::try_from(pid)
                .map_err(|_| ContainmentError::InvalidProcessId { pid })?;
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
            if result != 0 {
                return Err(ContainmentError::io(
                    "observe Unix child exit without reaping",
                    std::io::Error::last_os_error(),
                ));
            }

            // SAFETY: the storage was fully zero-initialized before waitid wrote
            // its siginfo_t result. si_signo stays zero when WNOHANG observes no
            // state change and is SIGCHLD for a reported child transition.
            Ok(unsafe { info.assume_init() }.si_signo != 0)
        }

        pub(super) async fn wait_for_direct_exit_without_reap(
            &self,
            child: &mut Child,
        ) -> Result<(), ContainmentError> {
            loop {
                if self.direct_exit_observed_without_reap(child)? {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        pub(super) fn terminate_tree(&self) -> Result<(), ContainmentError> {
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
            if source.raw_os_error() == Some(libc::ESRCH) {
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
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::io;
    use std::mem::size_of;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::ptr;

    use tokio::process::{Child, Command};
    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    use super::ContainmentError;

    const FORCE_EXIT_CODE: u32 = 1;
    const RESUME_THREAD_FAILED: u32 = u32::MAX;

    pub(super) struct PlatformContainment {
        job: OwnedHandle,
    }

    pub(super) fn prepare_command(command: &mut Command) -> Result<(), ContainmentError> {
        command
            .as_std_mut()
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        Ok(())
    }

    impl PlatformContainment {
        pub(super) fn enroll(child: &mut Child, pid: u32) -> Result<Self, ContainmentError> {
            let process = child
                .raw_handle()
                .ok_or(ContainmentError::MissingProcessId)?;
            let job = create_kill_on_close_job()?;

            // SAFETY: both handles are live and owned for the duration of this call.
            if unsafe { AssignProcessToJobObject(job_handle(&job), process) } == 0 {
                return Err(ContainmentError::io(
                    "assign child to Windows Job Object",
                    io::Error::last_os_error(),
                ));
            }

            let mut enrolled = 0;
            // SAFETY: both handles are live and `enrolled` is valid writable storage.
            if unsafe { IsProcessInJob(process, job_handle(&job), &mut enrolled) } == 0 {
                return Err(ContainmentError::io(
                    "confirm child Job Object enrollment",
                    io::Error::last_os_error(),
                ));
            }
            if enrolled == 0 {
                return Err(ContainmentError::io(
                    "confirm child Job Object enrollment",
                    io::Error::other("Windows reported that the child is outside its Job Object"),
                ));
            }

            resume_primary_thread(pid)?;
            Ok(Self { job })
        }

        pub(super) fn terminate_tree(&self) -> Result<(), ContainmentError> {
            // SAFETY: the Job Object handle remains owned by `self` during the call.
            if unsafe { TerminateJobObject(job_handle(&self.job), FORCE_EXIT_CODE) } != 0 {
                Ok(())
            } else {
                Err(ContainmentError::io(
                    "terminate Windows Job Object",
                    io::Error::last_os_error(),
                ))
            }
        }

        pub(super) fn retire_tree_target(&self) {}

        pub(super) fn direct_exit_observed_without_reap(
            &self,
            child: &mut Child,
        ) -> Result<bool, ContainmentError> {
            child
                .try_wait()
                .map(|status| status.is_some())
                .map_err(|source| ContainmentError::io("query Windows child status", source))
        }

        pub(super) async fn wait_for_direct_exit_without_reap(
            &self,
            child: &mut Child,
        ) -> Result<(), ContainmentError> {
            child
                .wait()
                .await
                .map(|_status| ())
                .map_err(|source| ContainmentError::io("wait for Windows child", source))
        }
    }

    fn create_kill_on_close_job() -> Result<OwnedHandle, ContainmentError> {
        // SAFETY: null security attributes and name request an unnamed Job Object
        // with default security, as documented by CreateJobObjectW.
        let raw_job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw_job.is_null() {
            return Err(ContainmentError::io(
                "create Windows Job Object",
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: CreateJobObjectW returned a fresh non-null owned handle.
        let job = unsafe { OwnedHandle::from_raw_handle(raw_job) };

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` has the exact layout and byte length named by the
        // information class, and the Job Object handle is live.
        let configured = unsafe {
            SetInformationJobObject(
                job_handle(&job),
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limits).cast::<c_void>(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(ContainmentError::io(
                "configure kill-on-close Windows Job Object",
                io::Error::last_os_error(),
            ));
        }

        Ok(job)
    }

    fn resume_primary_thread(pid: u32) -> Result<(), ContainmentError> {
        // SAFETY: the flags and process ID follow CreateToolhelp32Snapshot's
        // contract; the returned handle is validated before ownership is taken.
        let raw_snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if raw_snapshot == INVALID_HANDLE_VALUE {
            return Err(ContainmentError::io(
                "snapshot Windows process threads",
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: the snapshot call returned a valid, newly owned handle.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw_snapshot) };

        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..THREADENTRY32::default()
        };
        // SAFETY: the snapshot is live and `entry` has the required size and
        // writable storage for THREADENTRY32.
        if unsafe { Thread32First(job_handle(&snapshot), &mut entry) } == 0 {
            return Err(ContainmentError::io(
                "enumerate Windows process threads",
                io::Error::last_os_error(),
            ));
        }

        loop {
            if entry.th32OwnerProcessID == pid {
                return resume_thread(entry.th32ThreadID);
            }

            // SAFETY: the same live snapshot and initialized writable entry are
            // retained across the documented enumeration sequence.
            if unsafe { Thread32Next(job_handle(&snapshot), &mut entry) } == 0 {
                let source = io::Error::last_os_error();
                if source.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                    break;
                }
                return Err(ContainmentError::io(
                    "enumerate Windows process threads",
                    source,
                ));
            }
        }

        Err(ContainmentError::io(
            "locate suspended Windows child thread",
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no thread belongs to child process {pid}"),
            ),
        ))
    }

    fn resume_thread(thread_id: u32) -> Result<(), ContainmentError> {
        // SAFETY: OpenThread validates the OS-assigned thread ID and returns a
        // handle with precisely the resume access requested here.
        let raw_thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
        if raw_thread.is_null() {
            return Err(ContainmentError::io(
                "open suspended Windows child thread",
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: OpenThread returned a fresh non-null owned handle.
        let thread = unsafe { OwnedHandle::from_raw_handle(raw_thread) };

        // SAFETY: the owned thread handle has THREAD_SUSPEND_RESUME access.
        let previous_suspend_count = unsafe { ResumeThread(job_handle(&thread)) };
        if previous_suspend_count == RESUME_THREAD_FAILED {
            return Err(ContainmentError::io(
                "resume Windows child thread",
                io::Error::last_os_error(),
            ));
        }
        if previous_suspend_count != 1 {
            return Err(ContainmentError::io(
                "resume Windows child thread",
                io::Error::other(format!(
                    "expected this module's single suspend count, observed {previous_suspend_count}"
                )),
            ));
        }

        Ok(())
    }

    fn job_handle(handle: &OwnedHandle) -> HANDLE {
        handle.as_raw_handle()
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{BufRead, Write};
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;
    use tokio::time::{Instant, sleep, timeout};

    use super::ContainedChild;

    const HELPER_MODE: &str = "PROCESS_UTILS_CONTAINED_CHILD_HELPER";
    const HEARTBEAT_PATH: &str = "PROCESS_UTILS_CONTAINED_CHILD_HEARTBEAT";
    const HELPER_LIFETIME: Duration = Duration::from_secs(15);
    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

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
}
