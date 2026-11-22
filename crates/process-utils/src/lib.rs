//! Small process-related helpers shared across the workspace.

use std::ffi::OsStr;

#[cfg(all(
    feature = "tokio",
    any(
        windows,
        target_os = "aix",
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "haiku",
        target_os = "hurd",
        target_os = "illumos",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "nto",
        target_os = "openbsd",
        target_os = "solaris",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos"
    )
))]
mod contained_child;

#[cfg(all(
    feature = "tokio",
    any(
        windows,
        target_os = "aix",
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "haiku",
        target_os = "hurd",
        target_os = "illumos",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "nto",
        target_os = "openbsd",
        target_os = "solaris",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos"
    )
))]
pub use contained_child::{ContainedChild, ContainmentError, TreeTerminator};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Set by [`confine_thp_opt_out`] when transparent hugepages are disabled for
/// this process.
#[cfg(target_os = "linux")]
static THP_OPT_OUT_CONFINED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Keep this process's transparent hugepage opt-out away from the processes it
/// starts.
///
/// Linux hands `PR_SET_THP_DISABLE` down to forked children and keeps it across
/// `execve`, so a process that opts out to keep its own heap small would also
/// switch hugepages off for every tool it launches. Once this has been called
/// in a process whose hugepages are disabled, commands spawned through
/// [`ContainedChild`] or prepared with [`use_system_thp`] clear the flag before
/// they exec. Does nothing when hugepages are not disabled for this process,
/// and on other platforms.
pub fn confine_thp_opt_out() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `PR_GET_THP_DISABLE` takes no pointers and only reads a flag
        // of the calling process.
        let disabled = unsafe { libc::prctl(libc::PR_GET_THP_DISABLE, 0, 0, 0, 0) } == 1;
        THP_OPT_OUT_CONFINED.store(disabled, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Start `command` under the system's transparent hugepage policy while
/// [`confine_thp_opt_out`] is in effect; a no-op otherwise.
///
/// The hook makes the spawn fork instead of using `posix_spawn`, so it is only
/// installed when there is a flag to clear.
#[cfg(feature = "tokio")]
pub fn use_system_thp(command: &mut tokio::process::Command) {
    #[cfg(target_os = "linux")]
    if THP_OPT_OUT_CONFINED.load(std::sync::atomic::Ordering::Relaxed) {
        // SAFETY: the hook runs in the forked child before exec and makes a
        // single raw `prctl` system call, which is async-signal-safe: it
        // allocates nothing and takes no locks.
        unsafe {
            command.pre_exec(|| {
                // A failure leaves the child with hugepages off, which is no
                // reason to refuse to start it.
                libc::prctl(libc::PR_SET_THP_DISABLE, 0, 0, 0, 0);
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = command;
}

/// Apply the Windows `CREATE_NO_WINDOW` flag to child processes.
///
/// On non-Windows targets this is a no-op.
pub trait NoWindowExt {
    fn no_window(&mut self);
}

impl NoWindowExt for std::process::Command {
    fn no_window(&mut self) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
    }
}

/// Create a `std::process::Command` with `CREATE_NO_WINDOW` applied on Windows.
pub fn std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.no_window();
    cmd
}

#[cfg(feature = "tokio")]
impl NoWindowExt for tokio::process::Command {
    fn no_window(&mut self) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.as_std_mut().creation_flags(CREATE_NO_WINDOW);
        }
    }
}

/// Create a `tokio::process::Command` with `CREATE_NO_WINDOW` applied on Windows.
#[cfg(feature = "tokio")]
pub fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.no_window();
    cmd
}
