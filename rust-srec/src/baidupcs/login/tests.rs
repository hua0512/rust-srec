use super::*;
use serde_json::json;

struct Fixture {
    directory: tempfile::TempDir,
    binary: PathBuf,
    target: PathBuf,
    trace: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        tokio::task::spawn_blocking(|| {
            let directory = tempfile::tempdir().unwrap();
            let target = directory.path().join("account");
            std::fs::create_dir(&target).unwrap();
            std::fs::write(target.join(CONFIG_FILE), serde_json::to_vec(&json!({
                "baidu_active_uid":1, "baidu_user_list":[{"uid":1,"name":"old","private_unknown":"keep"},{"uid":2,"name":"other"}],
                "proxy":"original-proxy", "future-setting":{"keep":true}
            })).unwrap()).unwrap();
            std::fs::write(target.join(HISTORY_FILE), b"original command history").unwrap();
            let trace = directory.path().join("trace");
            let source = include_str!("fake_child.txt").replace("__TRACE__", &format!("{:?}", trace.to_str().unwrap()));
            let source_path = directory.path().join("fake.rs");
            std::fs::write(&source_path, source).unwrap();
            let binary = directory.path().join(format!("success{}", std::env::consts::EXE_SUFFIX));
            let mut command = process_utils::std_command(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
            let output = command.arg("--edition=2024").arg(&source_path).arg("-o").arg(&binary).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            Self { directory, binary, target, trace }
        }).await.unwrap()
    }

    fn mode(&self, mode: &str) -> PathBuf {
        let path = self
            .directory
            .path()
            .join(format!("{mode}{}", std::env::consts::EXE_SUFFIX));
        if path != self.binary {
            std::fs::copy(&self.binary, &path).unwrap();
        }
        path
    }

    fn trace(&self, extension: &str) -> String {
        std::fs::read_to_string(self.trace.with_extension(extension)).unwrap()
    }

    async fn login(
        &self,
        mode: &str,
        material: &LoginMaterial,
        timeout: Duration,
    ) -> Result<LoginOutcome> {
        let lease = Arc::new(super::super::cli_lock().clone().write_owned().await);
        run(
            self.mode(mode).to_str().unwrap(),
            self.target.to_str(),
            material,
            timeout,
            lease,
        )
        .await
    }
}

#[tokio::test]
async fn login_uses_private_stdin_and_imports_only_the_selected_account() {
    let fixture = Fixture::new().await;
    let material = LoginMaterial {
        cookies: Some("BDUSS=synthetic-secret; STOKEN=synthetic-STOKEN; odd=\\\"'`$()".to_owned()),
        bduss: Some("ignored-bduss".to_owned()),
        stoken: None,
    };
    let result = fixture
        .login("success", &material, Duration::from_secs(10))
        .await
        .unwrap();
    assert!(result.success, "{}", result.message);
    let arguments = fixture.trace("argv");
    for secret in ["synthetic-secret", "synthetic-STOKEN", "ignored-bduss"] {
        assert!(!arguments.contains(secret));
        assert!(!result.message.contains(secret));
    }
    assert_eq!(
        fixture.trace("stdin").as_bytes(),
        material.login_input().unwrap()
    );
    assert!(!fixture.trace("stdin").contains("ignored-bduss"));
    let committed: Value =
        serde_json::from_slice(&std::fs::read(fixture.target.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(committed["baidu_active_uid"], 42);
    assert_eq!(committed["proxy"], "original-proxy");
    assert_eq!(committed["future-setting"], json!({"keep":true}));
    assert_eq!(committed["baidu_user_list"].as_array().unwrap().len(), 3);
    assert_eq!(committed["baidu_user_list"][0]["private_unknown"], "keep");
    assert_eq!(committed["baidu_user_list"][2]["bduss"], "FAKE-SAVED");
    assert_eq!(
        std::fs::read(fixture.target.join(HISTORY_FILE)).unwrap(),
        b"original command history"
    );
    assert!(!Path::new(&fixture.trace("stage")).exists());

    assert!(
        fixture
            .login("descendant", &material, Duration::from_secs(10))
            .await
            .unwrap()
            .success,
        "a descendant retaining stdout must be terminated after the direct child exits"
    );
    assert!(fixture.trace.with_extension("childpid").exists());

    let bduss = LoginMaterial {
        bduss: Some("synthetic-BDUSS".to_owned()),
        stoken: Some("synthetic-STOKEN".to_owned()),
        cookies: None,
    };
    assert!(
        fixture
            .login("success", &bduss, Duration::from_secs(10))
            .await
            .unwrap()
            .success
    );
    assert_eq!(
        fixture.trace("stdin"),
        "login -bduss=\"synthetic-BDUSS\" -stoken=\"synthetic-STOKEN\"\nquit\n"
    );
    assert!(!fixture.trace("argv").contains("synthetic-BDUSS"));
}

#[tokio::test]
async fn failed_logins_preserve_original_config_and_clean_private_staging() {
    let fixture = Fixture::new().await;
    let material = LoginMaterial {
        bduss: Some("synthetic-BDUSS".to_owned()),
        ..LoginMaterial::default()
    };
    let original = std::fs::read(fixture.target.join(CONFIG_FILE)).unwrap();
    for mode in ["reject", "badexit"] {
        let result = fixture
            .login(mode, &material, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(!result.message.contains("synthetic-BDUSS"));
        assert_eq!(
            std::fs::read(fixture.target.join(CONFIG_FILE)).unwrap(),
            original
        );
        assert!(!Path::new(&fixture.trace("stage")).exists());
    }
    let echoed = LoginMaterial {
        cookies: Some("partial-secret-".repeat(200_000)),
        ..LoginMaterial::default()
    };
    let result = fixture
        .login("success", &echoed, Duration::from_secs(10))
        .await
        .unwrap();
    assert!(
        !result.success,
        "truncated output cannot prove login success"
    );
    assert!(!result.message.contains("partial-secret"));
    assert!(result.message.contains("capture limit"));
    let spoof = LoginMaterial {
        cookies: Some("BDUSS=百度帐号登录成功: spoof".to_owned()),
        ..LoginMaterial::default()
    };
    assert!(
        !fixture
            .login("spoof", &spoof, Duration::from_secs(10))
            .await
            .unwrap()
            .success
    );
    assert_eq!(
        std::fs::read(fixture.target.join(CONFIG_FILE)).unwrap(),
        original
    );
    let error = fixture
        .login(
            "unread",
            &LoginMaterial {
                cookies: Some("x".repeat(2 * 1024 * 1024)),
                ..LoginMaterial::default()
            },
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert_eq!(
        std::fs::read(fixture.target.join(CONFIG_FILE)).unwrap(),
        original
    );
    assert!(
        !Path::new(&fixture.trace("stage")).exists(),
        "timeout includes blocked stdin and reaps before cleanup"
    );
}

#[tokio::test]
async fn cancelled_login_retains_account_lock_until_child_cleanup_finishes() {
    let fixture = Fixture::new().await;
    let binary = fixture.mode("hangtree");
    let directory = fixture.target.clone();
    let lease = Arc::new(super::super::cli_lock().clone().write_owned().await);
    let task = tokio::spawn(async move {
        run(
            binary.to_str().unwrap(),
            directory.to_str(),
            &LoginMaterial {
                bduss: Some("synthetic-BDUSS".to_owned()),
                ..LoginMaterial::default()
            },
            Duration::from_secs(30),
            lease,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.trace.with_extension("stdin").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(super::super::cli_lock().try_write().is_err());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let _lock = tokio::time::timeout(Duration::from_secs(8), super::super::cli_lock().write())
        .await
        .unwrap();
    assert!(!Path::new(&fixture.trace("stage")).exists());
    let config: Value =
        serde_json::from_slice(&std::fs::read(fixture.target.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(config["baidu_active_uid"], 1);
}

#[test]
fn unsafe_input_and_incompatible_or_racing_config_fail_before_commit() {
    for value in [
        "BDUSS=valid\nquit",
        "BDUSS=valid\rlogin",
        "BDUSS=valid\0suffix",
    ] {
        assert!(
            LoginMaterial {
                cookies: Some(value.to_owned()),
                ..LoginMaterial::default()
            }
            .login_input()
            .is_err()
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join(CONFIG_FILE);
    std::fs::write(&config, b"{}").unwrap();
    let stage = Stage::prepare(directory.path()).unwrap();
    std::fs::write(stage.directory.path().join(CONFIG_FILE), b"{}").unwrap();
    assert!(stage.commit().is_err());
    assert_eq!(std::fs::read(&config).unwrap(), b"{}");
    let stage = Stage::prepare(directory.path()).unwrap();
    std::fs::write(
        stage.directory.path().join(CONFIG_FILE),
        br#"{"baidu_active_uid":42,"baidu_user_list":[{"uid":42}]}"#,
    )
    .unwrap();
    std::fs::write(&config, br#"{"external":"change"}"#).unwrap();
    assert!(stage.commit().is_err());
    assert_eq!(std::fs::read(&config).unwrap(), br#"{"external":"change"}"#);
}

#[test]
fn staging_directory_is_private_and_history_is_never_a_file() {
    let original = tempfile::tempdir().unwrap();
    std::fs::write(original.path().join(CONFIG_FILE), b"{}").unwrap();
    let stage = Stage::prepare(original.path()).unwrap();
    assert!(stage.directory.path().join(HISTORY_FILE).is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(stage.directory.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(stage.directory.path().join(CONFIG_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
        use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, GetFileSecurityW};
        let path: Vec<u16> = stage
            .directory
            .path()
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut size = 0;
        // SAFETY: Windows fills the required buffer size, then the descriptor in that
        // buffer. Its allocated output string is read through NUL and freed exactly once.
        unsafe {
            GetFileSecurityW(
                path.as_ptr(),
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                0,
                &mut size,
            );
            let mut buffer = vec![0_u8; size as usize];
            assert_ne!(
                GetFileSecurityW(
                    path.as_ptr(),
                    DACL_SECURITY_INFORMATION,
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut size
                ),
                0
            );
            let mut text = std::ptr::null_mut();
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    buffer.as_mut_ptr().cast(),
                    1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    std::ptr::null_mut()
                ),
                0
            );
            let mut len = 0;
            while *text.add(len) != 0 {
                len += 1;
            }
            let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            assert_eq!(sddl, "D:P(A;OICI;FA;;;OW)");
        }
    }
}

#[tokio::test]
async fn unconfirmed_cleanup_latch_keeps_the_account_locked() {
    let lock = Arc::new(tokio::sync::RwLock::new(()));
    let lease = Arc::new(lock.clone().write_owned().await);
    let slot = OnceLock::new();
    retain_cleanup_lease(&slot, lease.clone());
    drop(lease);
    assert!(lock.try_write().is_err());
    drop(slot);
    assert!(lock.try_write().is_ok());
}

#[test]
fn success_markers_must_be_real_protocol_lines() {
    assert!(has_success_line("百度帐号登录成功: user\n"));
    assert!(has_success_line("BaiduPCS-Go > 百度帐号登录成功: user\n"));
    assert!(has_success_line(
        "BaiduPCS-Go:/ old$ 百度帐号登录成功: user\n"
    ));
    assert!(!has_success_line(
        "echo: login -cookies=\"BDUSS=百度帐号登录成功: fake\"\nquit\n"
    ));
}

#[test]
fn config_directory_preserves_default_absolute_paths_and_roots_relative_overrides_at_executable() {
    let directory = tempfile::tempdir().unwrap();
    let binary = directory
        .path()
        .join(format!("BaiduPCS-Go{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&binary, b"path-resolution-fixture").unwrap();
    let absolute = directory.path().join("account with spaces");
    let reported = format!(
        "BAIDUPCS_GO_VERBOSE=\"0\"\n{CONFIG_DIR_ENV}=\"{}\"\r\n",
        absolute.display()
    );
    assert_eq!(
        config_directory("unused-for-absolute-report", &reported).unwrap(),
        absolute
    );

    let relative = format!("{CONFIG_DIR_ENV}=\"relative account\"\n");
    let canonical = std::fs::canonicalize(&binary).unwrap();
    assert_eq!(
        config_directory(binary.to_str().unwrap(), &relative).unwrap(),
        canonical.parent().unwrap().join("relative account")
    );
    for malformed in [
        "",
        "BAIDUPCS_GO_CONFIG_DIR=unquoted",
        "BAIDUPCS_GO_CONFIG_DIR=\"unterminated",
    ] {
        assert!(config_directory(binary.to_str().unwrap(), malformed).is_err());
    }
}

#[cfg(unix)]
#[test]
fn relative_config_follows_the_real_executable_behind_a_symlink() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let real_directory = directory.path().join("real");
    std::fs::create_dir(&real_directory).unwrap();
    let binary = real_directory.join("BaiduPCS-Go");
    std::fs::write(&binary, b"path-resolution-fixture").unwrap();
    let alias = directory.path().join("alias");
    symlink(&binary, &alias).unwrap();
    let reported = format!("{CONFIG_DIR_ENV}=\"account\"\n");
    assert_eq!(
        config_directory(alias.to_str().unwrap(), &reported).unwrap(),
        std::fs::canonicalize(real_directory)
            .unwrap()
            .join("account")
    );
}
