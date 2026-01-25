//! Small process-related helpers shared across the workspace.

use std::ffi::OsStr;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
