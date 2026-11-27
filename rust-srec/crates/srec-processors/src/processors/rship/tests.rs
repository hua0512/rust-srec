use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use srec_core::domain::processor::{JobLogEntry, JobProgressSnapshot};
use srec_core::domain::proxy::ProxyEndpoint;

use super::super::test_utils::{test_exit_code, utc_datetime};
use super::super::utils::CommandOutput;
use super::*;

struct ScriptedCall {
    exit_code: i32,
    stdout_json: Option<String>,
    logs: Vec<JobLogEntry>,
    moved_inputs: Vec<PathBuf>,
}

impl ScriptedCall {
    fn ok_json(json: impl Into<String>) -> Self {
        Self {
            exit_code: 0,
            stdout_json: Some(json.into()),
            logs: Vec::new(),
            moved_inputs: Vec::new(),
        }
    }

    fn ok() -> Self {
        Self {
            exit_code: 0,
            stdout_json: Some(r#"{"errors":[],"files":[]}"#.to_owned()),
            logs: Vec::new(),
            moved_inputs: Vec::new(),
        }
    }
}

struct RecordedInvocation {
    args: Vec<String>,
    envs: Vec<(String, Option<String>)>,
    manifest: Option<Vec<String>>,
}

struct MockRshipRunner {
    mkdir_outcomes: Mutex<VecDeque<ScriptedCall>>,
    transfer_outcomes: Mutex<VecDeque<ScriptedCall>>,
    mkdir_invocations: Mutex<Vec<RecordedInvocation>>,
    transfer_invocations: Mutex<Vec<RecordedInvocation>>,
    mkdir_hang: bool,
}

impl MockRshipRunner {
    fn new(transfer_outcomes: Vec<ScriptedCall>) -> Self {
        Self {
            mkdir_outcomes: Mutex::new(VecDeque::new()),
            transfer_outcomes: Mutex::new(transfer_outcomes.into()),
            mkdir_invocations: Mutex::new(Vec::new()),
            transfer_invocations: Mutex::new(Vec::new()),
            mkdir_hang: false,
        }
    }

    fn with_mkdir_outcomes(self, outcomes: Vec<ScriptedCall>) -> Self {
        *self.mkdir_outcomes.lock().unwrap() = outcomes.into();
        self
    }

    fn with_mkdir_hang(mut self) -> Self {
        self.mkdir_hang = true;
        self
    }

    fn mkdirs(&self) -> Vec<Vec<String>> {
        self.mkdir_invocations
            .lock()
            .unwrap()
            .iter()
            .map(|inv| inv.args.clone())
            .collect()
    }

    fn transfers(&self) -> Vec<Vec<String>> {
        self.transfer_invocations
            .lock()
            .unwrap()
            .iter()
            .map(|inv| inv.args.clone())
            .collect()
    }

    fn manifests(&self) -> Vec<Vec<String>> {
        self.transfer_invocations
            .lock()
            .unwrap()
            .iter()
            .filter_map(|inv| inv.manifest.clone())
            .collect()
    }

    fn transfer_envs(&self) -> Vec<Vec<(String, Option<String>)>> {
        self.transfer_invocations
            .lock()
            .unwrap()
            .iter()
            .map(|inv| inv.envs.clone())
            .collect()
    }
}

#[async_trait]
impl RshipCommandRunner for MockRshipRunner {
    async fn run(
        &self,
        command: &mut Command,
        context: &ProcessorContext,
        stop: &CancellationToken,
    ) -> Result<RshipCommandOutput> {
        let args: Vec<String> = command
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let envs: Vec<(String, Option<String>)> = command
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|val| val.to_string_lossy().into_owned()),
                )
            })
            .collect();

        if args.iter().any(|a| a == "mkdir") {
            self.mkdir_invocations
                .lock()
                .unwrap()
                .push(RecordedInvocation {
                    args,
                    envs,
                    manifest: None,
                });
            if self.mkdir_hang {
                stop.cancelled().await;
                return Err(srec_core::CoreError::Other("cancelled".to_owned()));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            let outcome = self
                .mkdir_outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(ScriptedCall::ok);
            for entry in &outcome.logs {
                context.log_sink.try_send(entry.clone());
            }
            return Ok(RshipCommandOutput {
                output: CommandOutput {
                    status: test_exit_code(outcome.exit_code),
                    duration: 0.0,
                    logs: outcome.logs,
                    fatal_error: false,
                    last_stats_error: None,
                },
                stdout_json: outcome.stdout_json,
            });
        }

        let manifest = if let Some(idx) = args.iter().position(|a| a == "--files-from") {
            let manifest_path = PathBuf::from(&args[idx + 1]);
            let content = tokio::fs::read_to_string(&manifest_path)
                .await
                .map_err(|e| {
                    srec_core::CoreError::io_path("reading test rship manifest", &manifest_path, e)
                })?;
            Some(content.lines().map(str::to_owned).collect())
        } else {
            None
        };

        self.transfer_invocations
            .lock()
            .unwrap()
            .push(RecordedInvocation {
                args,
                envs,
                manifest,
            });

        let outcome = self
            .transfer_outcomes
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| srec_core::CoreError::Other("unexpected rship attempt".to_owned()))?;

        for path in outcome.moved_inputs {
            let _ = tokio::fs::remove_file(&path).await;
        }
        for entry in &outcome.logs {
            context.log_sink.try_send(entry.clone());
        }
        context
            .progress
            .report(JobProgressSnapshot::new(ProgressKind::Rship));

        Ok(RshipCommandOutput {
            output: CommandOutput {
                status: test_exit_code(outcome.exit_code),
                duration: 0.0,
                logs: outcome.logs,
                fatal_error: false,
                last_stats_error: None,
            },
            stdout_json: outcome.stdout_json,
        })
    }
}

struct FixedProxyResolver(ProxyTarget);

#[async_trait]
impl ProxyRouteResolver for FixedProxyResolver {
    async fn target(&self, _own: &ProxyRoute) -> Result<ProxyTarget> {
        Ok(self.0.clone())
    }
}

#[test]
fn seeded_rship_migration_preset_parses_strictly_and_destination_paths_match_rship_rules() {
    let sql = include_str!("../../../../../migrations/20261125120000_add_rship_default_preset.sql");
    let json_start = sql.find("'{\"").expect("migration contains JSON literal") + 1;
    let json_end = sql[json_start..]
        .find("',")
        .expect("migration JSON literal ends before created_at")
        + json_start;
    let cfg: RshipConfig =
        super::super::utils::parse_config_strict(Some(&sql[json_start..json_end]), "rship")
            .expect("seeded preset JSON must satisfy strict RshipConfig deserialization");
    assert_eq!(
        cfg.destination_root.as_deref(),
        Some("gdrive:{streamer}/%Y-%m")
    );
    assert_eq!(cfg.operation, RshipOperation::Copy);

    // `split_remote` matches `rship`'s `TargetPath::parse` (`len >= 2`, ASCII `[A-Za-z0-9_-]`),
    // and Windows drive roots (`C:/`) or POSIX `/` are never truncated to `C:` or `""`.
    assert_eq!(
        RshipProcessor::split_remote("gdrive:archive/2026"),
        Some(("gdrive", "archive/2026"))
    );
    assert_eq!(RshipProcessor::split_remote("C:/recordings"), None);
    assert_eq!(RshipProcessor::split_remote("a:b"), None);
    assert_eq!(RshipProcessor::split_remote("Alice.Ch:2026"), None);
    assert_eq!(
        RshipProcessor::single_remote_path("C:/", "clip.mp4", true),
        "C:/clip.mp4"
    );
    assert_eq!(
        RshipProcessor::single_remote_path("/", "clip.mp4", true),
        "/clip.mp4"
    );
}

#[tokio::test]
async fn single_copy_and_move_build_expected_argv_and_reconcile_json_report() {
    let temp_dir = tempfile::tempdir().unwrap();
    let clip = temp_dir.path().join("stream.mp4");
    tokio::fs::write(&clip, b"video-bytes").await.unwrap();
    let clip_str = clip.to_string_lossy().into_owned();

    let runner = Arc::new(MockRshipRunner::new(vec![
        ScriptedCall::ok_json(
            r#"{"errors":[],"files":[{"path":"stream.mp4","status":"skipped","source_deleted":false}]}"#,
        ),
        ScriptedCall {
            exit_code: 0,
            stdout_json: Some(
                r#"{"errors":[],"files":[{"path":"stream.mp4","status":"transferred","bytes":11,"source_deleted":true}]}"#
                    .to_owned(),
            ),
            logs: Vec::new(),
            moved_inputs: vec![clip.clone()],
        },
    ]));
    let processor = RshipProcessor::with_command_runner("/opt/bin/rship", runner.clone());

    let session_start = utc_datetime(2026, 7, 15, 23, 30, 0);
    let expected_day =
        pipeline_common::expand_path_template_at("%Y-%m-%d", Some(session_start.as_millisecond()));

    let copy_input = ProcessorInput {
        inputs: vec![clip_str.clone()],
        streamer_name: Some("Alice".to_owned()),
        platform: Some("huya".to_owned()),
        session_start: Some(session_start),
        created_at: utc_datetime(2026, 7, 16, 1, 0, 0),
        config: Some(
            serde_json::json!({
                "destination_root": "  gdrive:archive/{streamer}/%Y-%m-%d/  ",
                "config_path": "/etc/rship/config.toml",
                "state_dir": "/var/lib/rship",
                "operation": "copy",
                "time_anchor": "session_start",
                "bwlimit": "20M",
                "transfers": 6,
                "adaptive": true,
                "part_concurrency": 4,
                "chunk_size": "64M",
                "max_buffer_mem": "32M",
                "tpslimit": 8.5,
                "drive_stop_on_upload_limit": true,
                "checksum": true,
                "strict_source": true
            })
            .to_string(),
        ),
        ..Default::default()
    };

    let copy_out = processor
        .process(&copy_input, &ProcessorContext::noop("single-copy"))
        .await
        .unwrap();

    let expected_remote_dir = format!("gdrive:archive/Alice/{expected_day}");
    let expected_remote_file = format!("{expected_remote_dir}/stream.mp4");
    assert_eq!(runner.mkdirs().len(), 1);
    assert!(
        runner.mkdirs()[0].ends_with(&["mkdir".to_owned(), expected_remote_dir.clone()]),
        "mkdir args: {:?}",
        runner.mkdirs()[0]
    );

    let expected_copy_args: Vec<String> = [
        "--config",
        "/etc/rship/config.toml",
        "--state-dir",
        "/var/lib/rship",
        "--tpslimit",
        "8.5",
        "--drive-stop-on-upload-limit",
        "copyto",
        &clip_str,
        &expected_remote_file,
        "--json",
        "--json-progress",
        "--stats",
        "1s",
        "--retries",
        "1",
        "--bwlimit",
        "20M",
        "--transfers",
        "6",
        "--adaptive",
        "--part-concurrency",
        "4",
        "--chunk-size",
        "64M",
        "--max-buffer-mem",
        "32M",
        "--checksum",
        "--strict-source",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(runner.transfers()[0], expected_copy_args);
    assert_eq!(copy_out.outputs, vec![clip_str.clone()]);
    assert!(copy_out.succeeded_inputs.is_empty());
    assert_eq!(
        copy_out.skipped_inputs,
        vec![(clip_str.clone(), "unchanged on remote".to_owned())]
    );
    assert_eq!(copy_out.uploads[0].status, UploadItemStatus::Skipped);

    // Single-file sync is rejected before invoking rship.
    let sync_err = processor
        .process(
            &ProcessorInput {
                inputs: vec![clip_str.clone()],
                config: Some(r#"{"destination_root":"gdrive:archive","operation":"sync"}"#.into()),
                ..Default::default()
            },
            &ProcessorContext::noop("single-sync"),
        )
        .await
        .unwrap_err();
    assert!(matches!(sync_err, srec_core::CoreError::Validation(_)));

    // Move consumes outputs (`outputs` empty) and marks transferred item Completed.
    let move_out = processor
        .process(
            &ProcessorInput {
                inputs: vec![clip_str.clone()],
                config: Some(r#"{"destination_root":"gdrive:archive","operation":"move"}"#.into()),
                ..Default::default()
            },
            &ProcessorContext::noop("single-move"),
        )
        .await
        .unwrap();
    assert!(move_out.outputs.is_empty());
    assert_eq!(move_out.succeeded_inputs, vec![clip_str]);
    assert_eq!(move_out.uploads[0].status, UploadItemStatus::Completed);
}

#[tokio::test(start_paused = true)]
async fn batch_planning_skips_nfc_paths_and_multi_attempt_accumulation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sub_dir = temp_dir.path().join("day1");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();
    // Decomposed NFD filename (`e` + combining acute `\u{301}`) vs NFC in `rship` JSON report.
    let nfd_name = "cafe\u{301}.mp4";
    let file_a = sub_dir.join(nfd_name);
    let file_b = sub_dir.join("b.mp4");
    let file_c = sub_dir.join("c.mp4");
    for f in [&file_a, &file_b, &file_c] {
        tokio::fs::write(f, b"segment").await.unwrap();
    }
    let inputs: Vec<String> = [&file_a, &file_b, &file_c]
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    let runner = Arc::new(MockRshipRunner::new(vec![
        // 1. Clean `sync` batch: `rship` reports planning-skipped `b.mp4`, NFC `café.mp4` in `files`,
        //    and `high_delete_warning` in stdout JSON (suppressed from stderr under `--json-progress`).
        ScriptedCall::ok_json(
            r#"{"errors":[],"high_delete_warning":"High deletion ratio: 120 of 300 destination files deleted (40.0%)","files":[
                {"path":"caf\u00e9.mp4","status":"transferred"},
                {"path":"b.mp4","status":"skipped"},
                {"path":"c.mp4","status":"server_side_copied"}
            ]}"#,
        ),
        // 2 & 3. Two-attempt `copy` batch:
        //    Attempt 1: `café.mp4` transferred, `b.mp4` skipped, `c.mp4` fails transiently.
        //    Attempt 2: receives ONLY `c.mp4` in `--files-from` and fails before writing JSON.
        //    Earlier `Completed` (`café.mp4`) and `Skipped` (`b.mp4`) must be preserved!
        ScriptedCall {
            exit_code: 1,
            stdout_json: Some(
                r#"{"errors":["Timeout: connection reset"],"files":[
                    {"path":"caf\u00e9.mp4","status":"transferred"},
                    {"path":"b.mp4","status":"skipped"},
                    {"path":"c.mp4","status":"failed","error":"connection reset","error_kind":"timeout","retry_class":"retryable"}
                ]}"#
                .to_owned(),
            ),
            logs: Vec::new(),
            moved_inputs: Vec::new(),
        },
        ScriptedCall {
            exit_code: 1,
            stdout_json: None,
            logs: vec![JobLogEntry::error("✗ error: transport disconnected")],
            moved_inputs: Vec::new(),
        },
    ]));
    let processor = RshipProcessor::with_command_runner("rship", runner.clone());

    let sync_out = processor
        .process(
            &ProcessorInput {
                inputs: inputs.clone(),
                config: Some(r#"{"destination_root":"r2:bucket/show","operation":"sync"}"#.into()),
                ..Default::default()
            },
            &ProcessorContext::noop("batch-sync"),
        )
        .await
        .unwrap();
    let sync_args = &runner.transfers()[0];
    assert!(
        !sync_args.contains(&"--no-traverse".to_owned()),
        "sync must preserve destination traversal: {sync_args:?}"
    );
    assert_eq!(
        sync_out.succeeded_inputs,
        vec![inputs[0].clone(), inputs[2].clone()]
    );
    assert_eq!(
        sync_out.skipped_inputs,
        vec![(inputs[1].clone(), "unchanged on remote".to_owned())]
    );
    assert!(
        sync_out
            .logs
            .iter()
            .any(|entry| entry.level == LogLevel::Warn
                && entry.message.contains("High deletion ratio")),
        "high_delete_warning must be surfaced in job logs: {:?}",
        sync_out.logs
    );

    let copy_partial = processor
        .process(
            &ProcessorInput {
                inputs: inputs.clone(),
                config: Some(
                    r#"{"destination_root":"r2:bucket/show","operation":"copy","max_retries":2}"#
                        .into(),
                ),
                ..Default::default()
            },
            &ProcessorContext::noop("batch-copy-retry"),
        )
        .await
        .unwrap();
    assert!(runner.transfers()[1].contains(&"--no-traverse".to_owned()));
    assert_eq!(runner.manifests()[1], vec![nfd_name, "b.mp4", "c.mp4"]);
    assert_eq!(
        runner.manifests()[2],
        vec!["c.mp4"],
        "retry of copy batch must exclude inputs already transferred or skipped"
    );
    assert!(!copy_partial.non_retryable);
    assert_eq!(copy_partial.succeeded_inputs, vec![inputs[0].clone()]);
    assert_eq!(
        copy_partial.skipped_inputs,
        vec![(inputs[1].clone(), "unchanged on remote".to_owned())]
    );
    assert_eq!(
        copy_partial.failed_inputs,
        vec![(inputs[2].clone(), "connection reset".to_owned())]
    );
}

#[tokio::test(start_paused = true)]
async fn flag_parsing_deduplicates_clap_flags_skips_option_values_and_reaps_timed_out_mkdir() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_a = temp_dir.path().join("a.mp4");
    let file_b = temp_dir.path().join("b.mp4");
    tokio::fs::write(&file_a, b"a").await.unwrap();
    tokio::fs::write(&file_b, b"b").await.unwrap();

    // 1. `--files-from` in extra args is rejected rather than crashing clap.
    let reject_proc =
        RshipProcessor::with_command_runner("rship", Arc::new(MockRshipRunner::new(vec![])));
    let files_from_err = reject_proc
        .process(
            &ProcessorInput {
                inputs: vec![file_a.to_string_lossy().into_owned()],
                config: Some(
                    r#"{"destination_root":"gdrive:team/sub","args":["--files-from","/tmp/list"]}"#
                        .into(),
                ),
                ..Default::default()
            },
            &ProcessorContext::noop("reject-files-from"),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        files_from_err,
        srec_core::CoreError::Validation(_)
    ));

    // 2. Option values (`--exclude --proxy`, `--exclude -v`) are not misread as flags;
    //    `-c` and `--state-dir` in `args` suppress structured `--config` / `--state-dir`;
    //    `-vvvv` reaches `mkdir`; and `RemoteDirGate` deduplicates concurrent `mkdir` calls.
    let runner = Arc::new(MockRshipRunner::new(vec![
        ScriptedCall::ok(),
        ScriptedCall::ok(),
    ]));
    let processor = Arc::new(RshipProcessor::with_command_runner("rship", runner.clone()));

    let config = serde_json::json!({
        "destination_root": "gdrive:team/sub",
        "config_path": "/ignored/by/extra_config.toml",
        "state_dir": "/ignored/by/extra_state",
        "bwlimit": "10M",
        "tpslimit": 0.0,
        "args": [
            "--config", "/override/config.toml",
            "--state-dir", "/override/state",
            "--bwlimit", "50M",
            "--retries", "5",
            "--stats=2s",
            "--low-level-retries", "9",
            "--connect-timeout", "15s",
            "--log-file", "/tmp/rship.log",
            "--fsync", "always",
            "-vvvv",
            "--exclude", "--proxy",
            "--exclude", "-v",
            "--json",
            "--tui",
            "--progress"
        ]
    })
    .to_string();

    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(16);
    let ctx1 = ProcessorContext::new(
        "dedup-1",
        srec_core::domain::processor::ProgressReporter::noop("dedup-1"),
        super::super::traits::JobLogSink::new(log_tx, Arc::default()),
        CancellationToken::new(),
    );
    let ctx2 = ProcessorContext::noop("dedup-2");

    let input_a = ProcessorInput {
        inputs: vec![file_a.to_string_lossy().into_owned()],
        config: Some(config.clone()),
        ..Default::default()
    };
    let input_b = ProcessorInput {
        inputs: vec![file_b.to_string_lossy().into_owned()],
        config: Some(config),
        ..Default::default()
    };
    let (res_a, res_b) = tokio::join!(
        processor.process(&input_a, &ctx1),
        processor.process(&input_b, &ctx2),
    );
    res_a.unwrap();
    res_b.unwrap();
    drop(ctx1);

    assert_eq!(runner.mkdirs().len(), 1);
    let mkdir_args = &runner.mkdirs()[0];
    assert_eq!(
        mkdir_args
            .iter()
            .filter(|a| a.as_str() == "--config")
            .count(),
        1,
        "mkdir must not duplicate --config: {mkdir_args:?}"
    );
    assert_eq!(
        mkdir_args
            .iter()
            .filter(|a| a.as_str() == "--state-dir")
            .count(),
        1,
        "mkdir must not duplicate --state-dir: {mkdir_args:?}"
    );
    assert!(mkdir_args.contains(&"-vvvv".to_owned()));
    assert!(
        mkdir_args
            .windows(2)
            .any(|w| w == ["--log-file", "/tmp/rship.log"])
    );
    assert!(
        !mkdir_args.contains(&"-v".to_owned())
            && !mkdir_args.contains(&"--proxy".to_owned())
            && !mkdir_args.contains(&"always".to_owned()),
        "values of transfer flags must not leak into mkdir: {mkdir_args:?}"
    );

    let transfer_args = &runner.transfers()[0];
    assert!(
        transfer_args.windows(2).any(|w| w == ["--tpslimit", "0"]),
        "tpslimit: 0 should pass --tpslimit 0: {transfer_args:?}"
    );
    for unique_flag in [
        "--bwlimit",
        "--retries",
        "--low-level-retries",
        "--state-dir",
        "--json",
        "--json-progress",
    ] {
        let count = transfer_args
            .iter()
            .filter(|a| *a == unique_flag || a.starts_with(&format!("{unique_flag}=")))
            .count();
        assert_eq!(count, 1, "duplicate {unique_flag}: {transfer_args:?}");
    }
    let mut warned = false;
    while let Ok(entry) = log_rx.try_recv() {
        if entry.level == LogLevel::Warn && entry.message.contains("--progress/--tui") {
            warned = true;
        }
    }
    assert!(warned);

    // 3. `run_mkdir` timeout cancels `stop` so `run_logged_command_until` reaps the child.
    let hang_runner = Arc::new(MockRshipRunner::new(vec![]).with_mkdir_hang());
    let hang_proc = RshipProcessor::with_command_runner("rship", hang_runner.clone());
    let timeout_err = hang_proc
        .process(
            &ProcessorInput {
                inputs: vec![file_a.to_string_lossy().into_owned()],
                config: Some(
                    r#"{"destination_root":"gdrive:team/hang","max_retries":1}"#.to_owned(),
                ),
                ..Default::default()
            },
            &ProcessorContext::noop("mkdir-timeout"),
        )
        .await
        .unwrap_err();
    assert!(
        timeout_err.to_string().contains("timed out"),
        "{timeout_err}"
    );
    assert_eq!(hang_runner.mkdirs().len(), 1);
}

#[tokio::test]
async fn proxy_routes_set_rship_proxy_reject_conflicting_arg_and_scrub_credentials() {
    let temp_dir = tempfile::tempdir().unwrap();
    let clip = temp_dir.path().join("clip.mp4");
    tokio::fs::write(&clip, b"video").await.unwrap();
    let clip_str = clip.to_string_lossy().into_owned();

    let secret = "s3cr3t!pass";
    let encoded_secret = "s3cr3t%21pass";
    let endpoint = ProxyEndpoint::new(
        "socks5h://127.0.0.1:1080",
        Some(("alice".to_owned(), secret.to_owned())),
    );

    let runner = Arc::new(MockRshipRunner::new(vec![ScriptedCall {
        exit_code: 5,
        stdout_json: Some(format!(
            r#"{{"ok":false,"fatal":true,"command":"copyto","exit_code":5,"error":"failed to initialize HTTP transport","error_category":"transient","error_kind":"network","retry_class":"retryable","causes":["proxy connect failed for socks5h://alice:{encoded_secret}@127.0.0.1:1080 ({secret})"],"errors":["failed to initialize HTTP transport"]}}"#
        )),
        logs: vec![JobLogEntry::error(format!(
            "rship: proxy connect failed for socks5h://alice:{encoded_secret}@127.0.0.1:1080 ({secret})"
        ))],
        moved_inputs: Vec::new(),
    }]));
    let processor = RshipProcessor::with_command_runner("rship", runner.clone()).with_routes(
        Arc::new(FixedProxyResolver(ProxyTarget::Explicit(endpoint))),
    );

    let conflict_err = processor
        .process(
            &ProcessorInput {
                inputs: vec![clip_str.clone()],
                config: Some(
                    r#"{"destination_root":"/tmp/out","args":["--proxy","http://1.2.3.4:8080"]}"#
                        .into(),
                ),
                ..Default::default()
            },
            &ProcessorContext::noop("proxy-conflict"),
        )
        .await
        .unwrap_err();
    assert!(matches!(conflict_err, srec_core::CoreError::Validation(_)));
    assert!(runner.transfers().is_empty());

    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(16);
    let ctx = ProcessorContext::new(
        "proxy-scrub",
        srec_core::domain::processor::ProgressReporter::noop("proxy-scrub"),
        super::super::traits::JobLogSink::new(log_tx, Arc::default()),
        CancellationToken::new(),
    );
    let err = processor
        .process(
            &ProcessorInput {
                inputs: vec![clip_str.clone()],
                config: Some(r#"{"destination_root":"/tmp/out","max_retries":1}"#.into()),
                ..Default::default()
            },
            &ctx,
        )
        .await
        .unwrap_err();
    drop(ctx);

    let err_text = err.to_string();
    assert!(
        !err_text.contains(secret) && !err_text.contains(encoded_secret),
        "returned error leaked proxy secret: {err_text}"
    );
    assert!(
        err_text.contains("proxy connect failed"),
        "structured error message should be preserved in failure summary: {err_text}"
    );
    while let Ok(entry) = log_rx.try_recv() {
        assert!(
            !entry.message.contains(secret) && !entry.message.contains(encoded_secret),
            "job log leaked proxy secret: {}",
            entry.message
        );
    }

    let argv_joined = runner.transfers()[0].join(" ");
    assert!(!argv_joined.contains(secret) && !argv_joined.contains(encoded_secret));
    assert!(runner.transfer_envs()[0].iter().any(|(k, v)| {
        k == "RSHIP_PROXY" && v.as_deref() == Some("socks5h://alice:s3cr3t%21pass@127.0.0.1:1080")
    }));

    let direct_runner = Arc::new(MockRshipRunner::new(vec![ScriptedCall::ok()]));
    let direct_processor = RshipProcessor::with_command_runner("rship", direct_runner.clone())
        .with_routes(Arc::new(FixedProxyResolver(ProxyTarget::Direct)));
    direct_processor
        .process(
            &ProcessorInput {
                inputs: vec![clip_str],
                config: Some(r#"{"destination_root":"/tmp/out"}"#.into()),
                ..Default::default()
            },
            &ProcessorContext::noop("proxy-direct"),
        )
        .await
        .unwrap();
    assert!(
        direct_runner.transfer_envs()[0]
            .iter()
            .any(|(k, v)| k == "RSHIP_PROXY" && v.is_none())
    );
}

#[tokio::test(start_paused = true)]
async fn failure_classification_uses_structured_error_classes_exit_codes_and_conflicts() {
    let temp_dir = tempfile::tempdir().unwrap();
    let clip_a = temp_dir.path().join("Access Denied.mp4");
    let clip_b = temp_dir.path().join("b.mp4");
    tokio::fs::write(&clip_a, b"video").await.unwrap();
    tokio::fs::write(&clip_b, b"video").await.unwrap();
    let clip_a_str = clip_a.to_string_lossy().into_owned();
    let clip_b_str = clip_b.to_string_lossy().into_owned();
    let single_input = ProcessorInput {
        inputs: vec![clip_a_str.clone()],
        config: Some(r#"{"destination_root":"/tmp/out","max_retries":3}"#.into()),
        ..Default::default()
    };

    // 1. Transient `timeout` (`retry_class: "retryable"`) and `integrity` (`retry_class: "integrity"`)
    //    retry even if the filename contains words like "Access Denied", because classification uses
    //    structured fields rather than prose.
    let retry_runner = Arc::new(MockRshipRunner::new(vec![
        ScriptedCall {
            exit_code: 5,
            stdout_json: Some(
                r#"{"errors":["Timeout: stat source `/rec/Access Denied.mp4` timed out"],"files":[{
                    "path":"Access Denied.mp4",
                    "status":"failed",
                    "error":"stat timed out",
                    "error_kind":"timeout",
                    "retry_class":"retryable"
                }]}"#
                    .to_owned(),
            ),
            logs: Vec::new(),
            moved_inputs: Vec::new(),
        },
        ScriptedCall {
            exit_code: 5,
            stdout_json: Some(
                r#"{"errors":["Integrity: upload checksum mismatch"],"files":[{
                    "path":"Access Denied.mp4",
                    "status":"failed",
                    "error":"upload checksum mismatch (expected md5 aaa, got bbb)",
                    "error_kind":"integrity",
                    "retry_class":"integrity"
                }]}"#
                    .to_owned(),
            ),
            logs: Vec::new(),
            moved_inputs: Vec::new(),
        },
        ScriptedCall::ok(),
    ]));
    RshipProcessor::with_command_runner("rship", retry_runner.clone())
        .process(&single_input, &ProcessorContext::noop("structured-retry"))
        .await
        .expect("retryable structured error must retry and succeed");
    assert_eq!(retry_runner.transfers().len(), 3);

    // 2. Rate-limit (exit 6 / `rate_limited`) and quota exhaustion (exit 8 / `quota_exceeded`)
    //    yield the worker after 1 attempt while keeping the DAG step retryable; `auth` (exit 4)
    //    aborts as non-retryable.
    let quota_runner = Arc::new(MockRshipRunner::new(vec![ScriptedCall {
        exit_code: 8,
        stdout_json: Some(
            r#"{"ok":false,"exit_code":8,"error_category":"quota_exceeded","errors":["QuotaExceeded: 403 userRateLimitExceeded"],"files":[{
                "path":"Access Denied.mp4",
                "status":"failed",
                "error":"403 userRateLimitExceeded: User rate limit exceeded",
                "error_kind":"quota_exceeded",
                "error_category":"quota_exceeded",
                "retry_class":"permanent"
            }]}"#
                .to_owned(),
        ),
        logs: Vec::new(),
        moved_inputs: Vec::new(),
    }]));
    let quota_err = RshipProcessor::with_command_runner("rship", quota_runner.clone())
        .process(&single_input, &ProcessorContext::noop("quota"))
        .await
        .unwrap_err();
    assert_eq!(quota_runner.transfers().len(), 1);
    assert!(!quota_err.is_non_retryable_step_error());

    let rate_runner = Arc::new(MockRshipRunner::new(vec![ScriptedCall {
        exit_code: 6,
        stdout_json: None,
        logs: vec![JobLogEntry::error("rship: 429 Too Many Requests")],
        moved_inputs: Vec::new(),
    }]));
    let rate_err = RshipProcessor::with_command_runner("rship", rate_runner.clone())
        .process(&single_input, &ProcessorContext::noop("rate-limit-exit6"))
        .await
        .unwrap_err();
    assert_eq!(rate_runner.transfers().len(), 1);
    assert!(!rate_err.is_non_retryable_step_error());

    let perm_runner = Arc::new(MockRshipRunner::new(vec![ScriptedCall {
        exit_code: 4,
        stdout_json: Some(
            r#"{"ok":false,"exit_code":4,"error_category":"auth","errors":["PermissionDenied: 401 invalid_grant"],"files":[{
                "path":"Access Denied.mp4",
                "status":"failed",
                "error":"401 invalid_grant: token revoked",
                "error_kind":"permission_denied",
                "error_category":"auth",
                "retry_class":"permanent"
            }]}"#
                .to_owned(),
        ),
        logs: Vec::new(),
        moved_inputs: Vec::new(),
    }]));
    let perm_err = RshipProcessor::with_command_runner("rship", perm_runner.clone())
        .process(&single_input, &ProcessorContext::noop("perm-auth"))
        .await
        .unwrap_err();
    assert_eq!(perm_runner.transfers().len(), 1);
    assert!(perm_err.is_non_retryable_step_error());

    // 3. Mixed batch (1 permanent `auth` + 1 transient `network`) stays retryable even though
    //    `rship`'s top-level `error_category` prioritizes `auth` (exit code 4) over `transient`!
    let mixed_runner = Arc::new(MockRshipRunner::new(vec![ScriptedCall {
        exit_code: 4,
        stdout_json: Some(
            r#"{"ok":false,"exit_code":4,"error_category":"auth","errors":["PermissionDenied: 403 Access Denied","Network: connection reset"],"files":[
                {"path":"Access Denied.mp4","status":"failed","error":"403 Access Denied","error_kind":"permission_denied","error_category":"auth","retry_class":"permanent"},
                {"path":"b.mp4","status":"failed","error":"connection reset","error_kind":"network","error_category":"transient","retry_class":"retryable"}
            ]}"#
            .to_owned(),
        ),
        logs: Vec::new(),
        moved_inputs: Vec::new(),
    }]));
    let mixed_err = RshipProcessor::with_command_runner("rship", mixed_runner.clone())
        .process(
            &ProcessorInput {
                inputs: vec![clip_a_str.clone(), clip_b_str],
                config: Some(r#"{"destination_root":"/tmp/out","max_retries":1}"#.into()),
                ..Default::default()
            },
            &ProcessorContext::noop("mixed-batch"),
        )
        .await
        .unwrap_err();
    assert!(
        !mixed_err.is_non_retryable_step_error(),
        "mixed batch with a transient failure must remain retryable: {mixed_err}"
    );

    // 4. Exit code 2 with `conflicts` surfaces the conflict path/kind/detail in both the
    //    returned error and persisted job logs.
    let conflict_runner = Arc::new(MockRshipRunner::new(vec![ScriptedCall {
        exit_code: 2,
        stdout_json: Some(
            r#"{"errors":[],"conflicts":[{"nfc_path":"clip.mp4","kind":"both_modified","detail":"both sides changed since baseline"}],"files":[]}"#
                .into(),
        ),
        logs: Vec::new(),
        moved_inputs: Vec::new(),
    }]));
    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(16);
    let ctx = ProcessorContext::new(
        "exit-2-conflict",
        srec_core::domain::processor::ProgressReporter::noop("exit-2-conflict"),
        super::super::traits::JobLogSink::new(log_tx, Arc::default()),
        CancellationToken::new(),
    );
    let exit2_err = RshipProcessor::with_command_runner("rship", conflict_runner.clone())
        .process(&single_input, &ctx)
        .await
        .unwrap_err();
    drop(ctx);
    assert_eq!(conflict_runner.transfers().len(), 1);
    assert!(exit2_err.is_non_retryable_step_error());
    assert!(
        exit2_err.to_string().contains("both_modified"),
        "conflict summary missing from error: {exit2_err}"
    );
    let mut logged_conflict = false;
    while let Ok(entry) = log_rx.try_recv() {
        if entry.message.contains("both_modified") {
            logged_conflict = true;
        }
    }
    assert!(
        logged_conflict,
        "JSON conflict must be mirrored into job logs"
    );

    // 5. Real `rship mkdir` unconfigured-remote exit code (`7 = EXIT_CONFIG`, no `--json` on mkdir)
    //    aborts after 1 attempt as `NonRetryablePipeline`.
    let mkdir_perm_runner =
        Arc::new(
            MockRshipRunner::new(vec![]).with_mkdir_outcomes(vec![ScriptedCall {
                exit_code: 7,
                stdout_json: None,
                logs: vec![JobLogEntry::error(
                    "✗ error: remote `gdrive` is not configured in /app/config/rship/config.toml",
                )],
                moved_inputs: Vec::new(),
            }]),
        );
    let mkdir_perm_err = RshipProcessor::with_command_runner("rship", mkdir_perm_runner.clone())
        .process(
            &ProcessorInput {
                inputs: vec![clip_a_str],
                config: Some(r#"{"destination_root":"gdrive:videos","max_retries":3}"#.into()),
                ..Default::default()
            },
            &ProcessorContext::noop("mkdir-unconfigured"),
        )
        .await
        .unwrap_err();
    assert_eq!(mkdir_perm_runner.mkdirs().len(), 1);
    assert!(mkdir_perm_runner.transfers().is_empty());
    assert!(mkdir_perm_err.is_non_retryable_step_error());
}
