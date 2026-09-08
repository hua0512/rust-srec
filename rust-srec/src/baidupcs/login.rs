//! BaiduPCS-Go's no-argument REPL accepts commands on stdin, but normally writes
//! every command to history. Stage its standard config privately and block that
//! history path; import only the successful account after the child is reaped.
//!
//! Protocol: qjfoidnh/BaiduPCS-Go v4.0.1 main.go, pcsliner/args/args.go,
//! pcsliner/linehistory.go and internal/pcsconfig/pcsconfig.go.

#[cfg(windows)]
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use process_utils::{ContainedChild, NoWindowExt};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{OwnedRwLockWriteGuard, oneshot};
use tokio::time::Instant;

use super::{
    CONFIG_DIR_ENV, CliOutput, LOGIN_SUCCESS_MARKER, LoginMaterial, LoginOutcome, base_command,
    scrub,
};
use crate::{Error, Result};

const CONFIG_FILE: &str = "pcs_config.json";
const HISTORY_FILE: &str = "pcs_command_history.txt";
const OUTPUT_LIMIT: usize = 1024 * 1024;
// Losing proof of child termination must not reopen account operations. A backend
// restart is the explicit recovery boundary for this otherwise unreachable latch.
static UNCONFIRMED_CLEANUP_LEASE: OnceLock<Arc<OwnedRwLockWriteGuard<()>>> = OnceLock::new();

pub(super) async fn run(
    binary: &str,
    config_dir: Option<&str>,
    material: &LoginMaterial,
    timeout: Duration,
    lease: Arc<OwnedRwLockWriteGuard<()>>,
) -> Result<LoginOutcome> {
    let input = material.login_input()?;
    let binary = binary.to_owned();
    let config_dir = config_dir.map(str::to_owned);
    let material = material.clone();
    let (mut response, receiver) = oneshot::channel();
    let deadline = Instant::now() + timeout;
    // The cleanup task retains both the private directory and the account lock.
    // Dropping the request closes its receiver, which cancels/reaps the child.
    tokio::spawn(async move {
        let _lease = lease;
        let mut cleanup_uncertain = false;
        let result = perform(
            &binary,
            config_dir.as_deref(),
            &input,
            deadline,
            &mut response,
            &mut cleanup_uncertain,
        )
        .await;
        if cleanup_uncertain {
            retain_cleanup_lease(&UNCONFIRMED_CLEANUP_LEASE, _lease.clone());
            tracing::error!(
                "BaiduPCS-Go account access remains locked after unconfirmed child cleanup; backend restart is required"
            );
        }
        let mut secrets: Vec<String> = material.secrets().into_iter().map(str::to_owned).collect();
        if let Some(cookies) = material.cookies.as_deref() {
            secrets.extend(
                cookies
                    .split(';')
                    .filter_map(|cookie| {
                        cookie
                            .split_once('=')
                            .map(|(_, value)| value.trim().to_owned())
                    })
                    .filter(|value| !value.is_empty()),
            );
        }
        let escaped: Vec<_> = secrets
            .iter()
            .map(|secret| secret.replace('\\', "\\\\").replace('"', "\\\""))
            .collect();
        secrets.extend(escaped);
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        let secrets: Vec<_> = secrets.iter().map(String::as_str).collect();
        let result = result
            .map(|mut outcome| {
                outcome.message = scrub(&outcome.message, &secrets);
                outcome
            })
            .map_err(|error| Error::Other(scrub(&error.to_string(), &secrets)));
        // A closed receiver means its request was cancelled; cleanup is already complete.
        if response.send(result).is_err() {
            tracing::debug!("BaiduPCS-Go login request ended before cleanup completed");
        }
    });
    // Filesystem calls may remain blocked in the OS. Bound the caller without
    // pretending those calls were cancelled; the cleanup owner keeps its lease.
    tokio::time::timeout_at(deadline + Duration::from_secs(5), receiver)
        .await
        .map_err(|_| {
            Error::Other(
                "BaiduPCS-Go login timed out; account cleanup may still be running".to_owned(),
            )
        })?
        .map_err(|error| Error::Other(format!("BaiduPCS-Go login task failed: {error}")))?
}

fn retain_cleanup_lease(
    slot: &OnceLock<Arc<OwnedRwLockWriteGuard<()>>>,
    lease: Arc<OwnedRwLockWriteGuard<()>>,
) {
    if slot.set(lease).is_err() {
        tracing::debug!("BaiduPCS-Go account cleanup lock was already retained");
    }
}

async fn perform(
    binary: &str,
    config_dir: Option<&str>,
    input: &[u8],
    deadline: Instant,
    response: &mut oneshot::Sender<Result<LoginOutcome>>,
    preserve_stage: &mut bool,
) -> Result<LoginOutcome> {
    let mut probe = base_command(binary, config_dir);
    probe.arg("env");
    let output = capture(probe, &[], deadline, response, preserve_stage).await?;
    if !output.status.success() {
        return Err(Error::Other(
            "BaiduPCS-Go could not report its config directory".to_owned(),
        ));
    }
    let binary_for_resolution = binary.to_owned();
    let target_dir = tokio::task::spawn_blocking(move || {
        config_directory(&binary_for_resolution, &output.stdout)
    })
    .await
    .map_err(|error| {
        Error::Other(format!(
            "BaiduPCS-Go config resolution task failed: {error}"
        ))
    })??;
    let staged = tokio::task::spawn_blocking(move || Stage::prepare(&target_dir))
        .await
        .map_err(|error| Error::Other(format!("BaiduPCS-Go staging task failed: {error}")))??;
    let mut command = base_command(binary, None);
    command.env(CONFIG_DIR_ENV, staged.directory.path());
    let result = capture(command, input, deadline, response, preserve_stage).await;
    if *preserve_stage {
        let path = staged.directory.keep();
        tracing::error!(path = %path.display(), "Retained private BaiduPCS-Go staging directory because child cleanup was not confirmed");
        return result.map(|_| LoginOutcome {
            success: false,
            message: "BaiduPCS-Go child cleanup was not confirmed".to_owned(),
        });
    }
    let output = result?;
    let message = output.combined();
    if !output.status.success() || !has_success_line(&message) {
        return Ok(LoginOutcome {
            success: false,
            message,
        });
    }
    if response.is_closed() {
        return Err(Error::Other(
            "BaiduPCS-Go login request was cancelled".to_owned(),
        ));
    }
    tokio::task::spawn_blocking(move || staged.commit())
        .await
        .map_err(|error| {
            Error::Other(format!("BaiduPCS-Go account commit task failed: {error}"))
        })??;
    Ok(LoginOutcome {
        success: true,
        message,
    })
}

fn has_success_line(output: &str) -> bool {
    output.lines().any(|line| {
        let line = line.trim();
        let line = line
            .strip_prefix("BaiduPCS-Go > ")
            .or_else(|| {
                line.strip_prefix("BaiduPCS-Go:")
                    .and_then(|prompt| prompt.rsplit_once("$ ").map(|(_, rest)| rest))
            })
            .unwrap_or(line);
        line.strip_prefix(LOGIN_SUCCESS_MARKER)
            .is_some_and(|rest| rest.starts_with(':'))
    })
}

async fn drain(mut reader: impl AsyncRead + Unpin) -> io::Result<String> {
    let mut output = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok(if truncated {
                "BaiduPCS-Go output exceeded the capture limit and was omitted".to_owned()
            } else {
                String::from_utf8_lossy(&output).into_owned()
            });
        }
        if count > OUTPUT_LIMIT.saturating_sub(output.len()) {
            // A retained prefix could end inside an echoed secret and evade full-value
            // redaction. Discard the whole stream but continue draining the child pipe.
            truncated = true;
            output.clear();
        }
        if !truncated {
            output.extend_from_slice(&buffer[..count]);
        }
    }
}

async fn capture(
    mut command: tokio::process::Command,
    input: &[u8],
    deadline: Instant,
    response: &mut oneshot::Sender<Result<LoginOutcome>>,
    preserve_stage: &mut bool,
) -> Result<CliOutput> {
    if response.is_closed() || Instant::now() >= deadline {
        return Err(Error::Other(
            "BaiduPCS-Go login cancelled or timed out".to_owned(),
        ));
    }
    command.no_window();
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ContainedChild::spawn(&mut command)
        .map_err(|error| Error::Other(format!("Failed to spawn BaiduPCS-Go: {error}")))?;
    let mut stdin = child
        .take_stdin()
        .ok_or_else(|| Error::Other("Missing BaiduPCS-Go stdin pipe".to_owned()))?;
    let stdout = child
        .take_stdout()
        .ok_or_else(|| Error::Other("Missing BaiduPCS-Go stdout pipe".to_owned()))?;
    let stderr = child
        .take_stderr()
        .ok_or_else(|| Error::Other("Missing BaiduPCS-Go stderr pipe".to_owned()))?;
    let operation = async {
        let write = async move {
            // Unsupported binaries may exit before reading. Their status/output still
            // determines failure, but other write errors are reported explicitly.
            if let Err(error) = stdin.write_all(input).await
                && error.kind() != io::ErrorKind::BrokenPipe
            {
                return Err(error);
            }
            if let Err(error) = stdin.shutdown().await
                && error.kind() != io::ErrorKind::BrokenPipe
            {
                return Err(error);
            }
            // Deliver EOF before waiting for the child and its output pipes.
            drop(stdin);
            Ok(())
        };
        let (write, stdout, stderr, status) =
            tokio::join!(write, drain(stdout), drain(stderr), child.wait());
        write.map_err(|error| Error::Other(format!("BaiduPCS-Go input failed: {error}")))?;
        Ok(CliOutput {
            status: status
                .map_err(|error| Error::Other(format!("BaiduPCS-Go wait failed: {error}")))?,
            stdout: stdout
                .map_err(|error| Error::Other(format!("BaiduPCS-Go stdout failed: {error}")))?,
            stderr: stderr
                .map_err(|error| Error::Other(format!("BaiduPCS-Go stderr failed: {error}")))?,
        })
    };
    let result = tokio::select! {
        biased;
        _ = response.closed() => Err(Error::Other("BaiduPCS-Go login request was cancelled".to_owned())),
        _ = tokio::time::sleep_until(deadline) => Err(Error::Other("BaiduPCS-Go login timed out".to_owned())),
        result = operation => result,
    };
    if result.is_err()
        && let Err(error) = child
            .terminate_tree_until(Instant::now() + Duration::from_secs(5))
            .await
    {
        *preserve_stage = true;
        return Err(Error::Other(format!(
            "BaiduPCS-Go child cleanup could not be confirmed: {error}"
        )));
    }
    result
}

fn config_directory(binary: &str, output: &str) -> Result<PathBuf> {
    let prefix = format!("{CONFIG_DIR_ENV}=\"");
    let directory = output
        .lines()
        .find_map(|line| {
            line.strip_prefix(&prefix)
                .and_then(|path| path.strip_suffix('"'))
        })
        .ok_or_else(|| {
            Error::Other(
                "Unsupported BaiduPCS-Go env response: config directory missing".to_owned(),
            )
        })?;
    let path = PathBuf::from(directory);
    if path.is_absolute() {
        return Ok(path);
    }
    // Upstream resolves relative config overrides against the real executable's
    // directory, including symlinks, rather than the recorder's working directory.
    let binary = resolve_executable(binary)?;
    let parent = binary
        .parent()
        .ok_or_else(|| Error::Other("BaiduPCS-Go executable directory unavailable".to_owned()))?;
    Ok(parent.join(path))
}

fn resolve_executable(binary: &str) -> Result<PathBuf> {
    let path = Path::new(binary);
    let candidates = if path.components().count() > 1 || path.is_absolute() {
        vec![path.to_path_buf()]
    } else {
        let mut directories = Vec::new();
        #[cfg(windows)]
        directories
            .push(std::env::current_dir().map_err(|error| {
                Error::Other(format!("Current directory unavailable: {error}"))
            })?);
        directories.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        directories
            .into_iter()
            .map(|directory| directory.join(path))
            .collect()
    };
    for candidate in candidates {
        let alternatives = vec![candidate.clone()];
        #[cfg(windows)]
        let alternatives = {
            let mut alternatives = alternatives;
            if candidate.extension().is_none() {
                alternatives.push(candidate.with_extension("exe"));
            }
            alternatives
        };
        for candidate in alternatives {
            if candidate.is_file() {
                return std::fs::canonicalize(&candidate).map_err(|error| {
                    Error::io_path("resolving BaiduPCS-Go executable", &candidate, error)
                });
            }
        }
    }
    Err(Error::Other(
        "Cannot resolve BaiduPCS-Go executable for its relative config directory".to_owned(),
    ))
}

struct Stage {
    directory: PrivateDirectory,
    target: PathBuf,
    original: Vec<u8>,
    existed: bool,
}

impl Stage {
    fn prepare(directory: &Path) -> Result<Self> {
        let target = directory.join(CONFIG_FILE);
        let target = if target.exists() {
            std::fs::canonicalize(&target)
                .map_err(|error| Error::io_path("resolving BaiduPCS-Go config", &target, error))?
        } else {
            target
        };
        let (original, existed) = match std::fs::read(&target) {
            Ok(bytes) => (bytes, true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (b"{}".to_vec(), false),
            Err(error) => return Err(Error::io_path("reading BaiduPCS-Go config", &target, error)),
        };
        let value: Value = serde_json::from_slice(&original)
            .map_err(|_| Error::validation("BaiduPCS-Go config is not valid JSON"))?;
        if !value.is_object() {
            return Err(Error::validation(
                "BaiduPCS-Go config must be a JSON object",
            ));
        }
        let directory = PrivateDirectory::new_in(&std::env::temp_dir()).map_err(|error| {
            Error::Other(format!(
                "Could not create private BaiduPCS-Go staging directory: {error}"
            ))
        })?;
        let config = directory.path().join(CONFIG_FILE);
        std::fs::write(&config, &original)
            .map_err(|error| Error::io_path("staging BaiduPCS-Go config", &config, error))?;
        make_private(&config, false).map_err(|error| {
            Error::io_path("protecting BaiduPCS-Go staged config", &config, error)
        })?;
        // OpenFile(O_RDWR|O_CREATE) rejects a directory on both supported platforms.
        // Upstream sets History=nil and never writes command history in this case.
        let history = directory.path().join(HISTORY_FILE);
        std::fs::create_dir(&history).map_err(|error| {
            Error::io_path("blocking BaiduPCS-Go command history", &history, error)
        })?;
        Ok(Self {
            directory,
            target,
            original,
            existed,
        })
    }

    fn commit(self) -> Result<()> {
        let staged_path = self.directory.path().join(CONFIG_FILE);
        let bytes = std::fs::read(&staged_path).map_err(|error| {
            Error::io_path("reading BaiduPCS-Go login result", &staged_path, error)
        })?;
        let staged: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Error::validation("Unsupported BaiduPCS-Go login result config"))?;
        let mut original: Value = serde_json::from_slice(&self.original)
            .map_err(|_| Error::validation("BaiduPCS-Go original config is not valid JSON"))?;
        merge_account(&mut original, &staged)?;
        let current = match std::fs::read(&self.target) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound && !self.existed => {
                b"{}".to_vec()
            }
            Err(error) => {
                return Err(Error::io_path(
                    "checking BaiduPCS-Go config before commit",
                    &self.target,
                    error,
                ));
            }
        };
        if current != self.original {
            return Err(Error::validation(
                "BaiduPCS-Go config changed during login; retry after other CLI operations finish",
            ));
        }
        let parent = self
            .target
            .parent()
            .ok_or_else(|| Error::validation("BaiduPCS-Go config parent is missing"))?;
        // Create the replacement inside an atomically private sibling directory so
        // no other user can pre-open its handle before its ACL is protected.
        let replacement_dir = PrivateDirectory::new_in(parent).map_err(|error| {
            Error::io_path(
                "creating private BaiduPCS-Go replacement directory",
                parent,
                error,
            )
        })?;
        let mut file =
            tempfile::NamedTempFile::new_in(replacement_dir.path()).map_err(|error| {
                Error::io_path(
                    "creating BaiduPCS-Go config replacement",
                    replacement_dir.path(),
                    error,
                )
            })?;
        make_private(file.path(), false).map_err(|error| {
            Error::io_path(
                "protecting BaiduPCS-Go config replacement",
                file.path(),
                error,
            )
        })?;
        let bytes = serde_json::to_vec_pretty(&original).map_err(|error| {
            Error::Other(format!(
                "Could not serialize BaiduPCS-Go account config: {error}"
            ))
        })?;
        file.write_all(&bytes).map_err(|error| {
            Error::io_path("writing BaiduPCS-Go account config", file.path(), error)
        })?;
        file.as_file().sync_all().map_err(|error| {
            Error::io_path("syncing BaiduPCS-Go account config", file.path(), error)
        })?;
        file.persist(&self.target).map_err(|error| {
            Error::io_path(
                "replacing BaiduPCS-Go account config",
                &self.target,
                error.error,
            )
        })?;
        Ok(())
    }
}

fn merge_account(original: &mut Value, staged: &Value) -> Result<()> {
    let uid = staged
        .get("baidu_active_uid")
        .and_then(Value::as_u64)
        .filter(|uid| *uid != 0)
        .ok_or_else(|| Error::validation("BaiduPCS-Go login did not persist an active account"))?;
    let account = staged
        .get("baidu_user_list")
        .and_then(Value::as_array)
        .and_then(|users| {
            users
                .iter()
                .find(|user| user.get("uid").and_then(Value::as_u64) == Some(uid))
        })
        .ok_or_else(|| {
            Error::validation("BaiduPCS-Go login did not persist the active account details")
        })?
        .clone();
    let object = original
        .as_object_mut()
        .ok_or_else(|| Error::validation("BaiduPCS-Go config must be a JSON object"))?;
    let users = object
        .entry("baidu_user_list")
        .or_insert_with(|| Value::Array(Vec::new()));
    if users.is_null() {
        *users = Value::Array(Vec::new());
    }
    let users = users
        .as_array_mut()
        .ok_or_else(|| Error::validation("Unsupported BaiduPCS-Go account-list config"))?;
    if let Some(existing) = users
        .iter_mut()
        .find(|user| user.get("uid").and_then(Value::as_u64) == Some(uid))
    {
        *existing = account;
    } else {
        users.push(account);
    }
    object.insert("baidu_active_uid".to_owned(), Value::from(uid));
    Ok(())
}

#[cfg(unix)]
fn make_private(path: &Path, directory: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
    )
}

#[cfg(windows)]
fn make_private(path: &Path, directory: bool) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    with_private_descriptor(directory, |descriptor| {
        // SAFETY: The path is NUL-terminated; the descriptor outlives this call.
        if unsafe {
            SetFileSecurityW(
                path.as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}

#[cfg(windows)]
fn with_private_descriptor<T>(
    directory: bool,
    apply: impl FnOnce(windows_sys::Win32::Security::PSECURITY_DESCRIPTOR) -> io::Result<T>,
) -> io::Result<T> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    let sddl = if directory {
        "D:P(A;OICI;FA;;;OW)"
    } else {
        "D:P(A;;FA;;;OW)"
    };
    let sddl: Vec<u16> = OsString::from(sddl).encode_wide().chain(Some(0)).collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: The string is NUL-terminated and Windows initializes descriptor.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let result = apply(descriptor);
    // SAFETY: The descriptor was allocated by the conversion API and is released once.
    unsafe {
        LocalFree(descriptor);
    }
    result
}

struct PrivateDirectory {
    path: PathBuf,
    retained: bool,
}

impl PrivateDirectory {
    fn new_in(parent: &Path) -> io::Result<Self> {
        let path = parent.join(format!("rust-srec-baidupcs-login-{}", uuid::Uuid::new_v4()));
        create_private_directory(&path)?;
        Ok(Self {
            path,
            retained: false,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn keep(mut self) -> PathBuf {
        self.retained = true;
        self.path.clone()
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        if !self.retained
            && let Err(error) = std::fs::remove_dir_all(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), %error, "Could not remove private BaiduPCS-Go staging directory");
        }
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(windows)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    with_private_descriptor(true, |descriptor| {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>())
                .map_err(io::Error::other)?,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: All pointers remain valid for the call. The protected owner-only
        // DACL applies at creation, so no permissive inherited-ACL window exists.
        if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}

#[cfg(test)]
mod tests;
