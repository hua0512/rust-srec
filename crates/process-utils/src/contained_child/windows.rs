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
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
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

    pub(super) fn terminate_tree(
        &self,
        _direct_exit_observed: bool,
    ) -> Result<(), ContainmentError> {
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
