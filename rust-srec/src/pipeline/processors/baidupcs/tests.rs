use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use super::super::test_utils::{test_exit_status, utc_datetime};
use super::*;
use crate::pipeline::{JobPhase, ProgressReporter};

struct MockAttempt {
    exit_ok: bool,
    lines: Vec<String>,
}

struct MockBaiduPcsCommandRunner {
    attempts: Mutex<VecDeque<MockAttempt>>,
    commands: Mutex<Vec<Vec<String>>>,
    envs: Mutex<Vec<Vec<(String, String)>>>,
}

impl MockBaiduPcsCommandRunner {
    fn new(attempts: Vec<MockAttempt>) -> Self {
        Self {
            attempts: Mutex::new(attempts.into()),
            commands: Mutex::new(Vec::new()),
            envs: Mutex::new(Vec::new()),
        }
    }

    fn commands(&self) -> Vec<Vec<String>> {
        self.commands.lock().unwrap().clone()
    }

    fn envs(&self) -> Vec<Vec<(String, String)>> {
        self.envs.lock().unwrap().clone()
    }

    fn processor(runner: Arc<Self>) -> BaiduPcsProcessor {
        BaiduPcsProcessor::with_command_runner("BaiduPCS-Go", runner)
    }
}

#[async_trait]
impl BaiduPcsCommandRunner for MockBaiduPcsCommandRunner {
    async fn run(
        &self,
        command: Command,
        context: &ProcessorContext,
        tracker: Option<Arc<UploadTracker>>,
        _config_lease: &ConfigLease,
    ) -> Result<CommandOutput> {
        let args: Vec<String> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let envs: Vec<(String, String)> = command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        self.commands.lock().unwrap().push(args);
        self.envs.lock().unwrap().push(envs);

        let attempt = self
            .attempts
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| crate::Error::Other("unexpected BaiduPCS-Go attempt".to_string()))?;
        let logs: Vec<JobLogEntry> = attempt.lines.into_iter().map(JobLogEntry::info).collect();
        // The real runner streams every line to the job log and an
        // upload's tracker as it arrives.
        for entry in &logs {
            context.log_sink.try_send(entry.clone());
            if let Some(tracker) = &tracker {
                tracker.record_line(&entry.message);
            }
        }
        Ok(CommandOutput {
            status: test_exit_status(attempt.exit_ok),
            duration: 0.0,
            logs,
        })
    }
}

fn queue_line(id: u32, path: &str) -> String {
    format!("[{id}] {MARKER_QUEUED}{path}")
}

fn success_line(id: u32, remote: &str) -> String {
    format!("[{id}] 上传文件成功, {MARKER_REMOTE_PATH}{remote}")
}

fn finished_line() -> String {
    "上传结束, 时间: 10s, 总大小: 1.000000GB".to_string()
}

fn info_logs(lines: &[String]) -> Vec<JobLogEntry> {
    lines.iter().cloned().map(JobLogEntry::info).collect()
}

#[tokio::test]
async fn upload_command_shape_env_and_success() {
    let temp_dir = tempfile::tempdir().unwrap();
    let first = temp_dir.path().join("a.flv");
    let second = temp_dir.path().join("b.flv");
    tokio::fs::write(&first, b"video-a").await.unwrap();
    tokio::fs::write(&second, b"video-b").await.unwrap();
    let first_str = first.to_string_lossy().into_owned();
    let second_str = second.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
        exit_ok: true,
        lines: vec![
            queue_line(1, &first_str),
            queue_line(2, &second_str),
            success_line(1, "/rec/a.flv"),
            success_line(2, "/rec/b.flv"),
            finished_line(),
        ],
    }]));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![first_str.clone(), second_str.clone()],
        config: Some(
            serde_json::json!({
                "destination_root": "/rec",
                "policy": "overwrite",
                "norapid": true,
                "args": ["--x"],
                "config_dir": "/cfg/bpcs",
            })
            .to_string(),
        ),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("shape"))
        .await
        .unwrap();

    assert_eq!(
        runner.commands(),
        vec![vec![
            "upload".to_string(),
            "--policy".to_string(),
            "overwrite".to_string(),
            "--norapid".to_string(),
            "--x".to_string(),
            first_str.clone(),
            second_str.clone(),
            "/rec".to_string(),
        ]]
    );
    assert!(
        runner.envs()[0]
            .iter()
            .any(|(key, value)| key == "BAIDUPCS_GO_CONFIG_DIR" && value == "/cfg/bpcs")
    );
    assert_eq!(output.uploads.len(), 2);
    assert!(
        output
            .uploads
            .iter()
            .all(|item| item.status == UploadItemStatus::Completed)
    );
    assert_eq!(output.uploads[0].remote_path.as_deref(), Some("/rec/a.flv"));
    assert_eq!(output.uploads[0].size_bytes, Some(7));
    assert_eq!(output.outputs, vec![first_str, second_str]);
}

#[test]
fn parse_outcomes_success_skip_and_fatal_skip() {
    let inputs = vec![
        "/rec/a.flv".to_string(),
        "/rec/b.flv".to_string(),
        "/rec/c.flv".to_string(),
        "/rec/d.flv".to_string(),
    ];
    let logs = info_logs(&[
        queue_line(1, "/rec/a.flv"),
        queue_line(2, "/rec/b.flv"),
        queue_line(3, "/rec/c.flv"),
        queue_line(4, "/rec/d.flv"),
        success_line(1, "/remote/a.flv"),
        format!("[2] 秒传成功, {MARKER_REMOTE_PATH}/remote/b.flv"),
        "[3] 目标文件已存在, 跳过...".to_string(),
        "[4] 文件大小超过128G, 无法上传, 跳过...".to_string(),
        finished_line(),
    ]);

    let outcomes = parse_upload_outcomes(&logs, &inputs);

    assert!(outcomes.finished);
    assert_eq!(
        outcomes.completed.get("/rec/a.flv"),
        Some(&Some("/remote/a.flv".to_string()))
    );
    assert_eq!(
        outcomes.completed.get("/rec/b.flv"),
        Some(&Some("/remote/b.flv".to_string()))
    );
    assert!(outcomes.skipped.contains_key("/rec/c.flv"));
    assert!(outcomes.permanent.contains_key("/rec/d.flv"));
    assert!(outcomes.failed.is_empty());
    assert_eq!(outcomes.orphan_successes, 0);
    assert_eq!(outcomes.orphan_failures, 0);
    assert!(!outcomes.saw_failure_table);
}

#[test]
fn parse_outcomes_ignores_retry_and_rapid_fallback_notes() {
    let inputs = vec!["/rec/a.flv".to_string()];
    let logs = info_logs(&[
        queue_line(1, "/rec/a.flv"),
        "[1] 上传文件失败, 上传文件错误, 重试 1/3".to_string(),
        "[1] 跳过秒传失败, 开始秒传...".to_string(),
        success_line(1, "/remote/a.flv"),
        finished_line(),
    ]);

    let outcomes = parse_upload_outcomes(&logs, &inputs);

    assert!(outcomes.failed.is_empty());
    assert!(outcomes.completed.contains_key("/rec/a.flv"));
}

#[test]
fn parse_outcomes_failure_table_and_terminal_failure() {
    let inputs = vec!["/rec/a.flv".to_string(), "/rec/b.flv".to_string()];
    let logs = info_logs(&[
        queue_line(1, "/rec/a.flv"),
        queue_line(2, "/rec/b.flv"),
        success_line(1, "/remote/a.flv"),
        "[2] 上传文件失败, 上传文件错误".to_string(),
        finished_line(),
        "以下文件上传失败: ".to_string(),
        "任务ID  本地路径".to_string(),
        "2       /rec/b.flv".to_string(),
    ]);

    let outcomes = parse_upload_outcomes(&logs, &inputs);

    assert!(outcomes.saw_failure_table);
    assert!(outcomes.completed.contains_key("/rec/a.flv"));
    assert!(outcomes.failed.contains_key("/rec/b.flv"));
    assert_eq!(outcomes.orphan_failures, 0);
}

#[test]
fn parse_outcomes_illegal_filename_is_permanent_failure() {
    let inputs = vec!["/rec/bad+name.flv".to_string()];
    let logs = info_logs(&[
        "[0] /rec/bad+name.flv 文件路径含有非法字符，已跳过!".to_string(),
        "未检测到上传的文件.".to_string(),
    ]);

    let outcomes = parse_upload_outcomes(&logs, &inputs);

    assert!(outcomes.permanent.contains_key("/rec/bad+name.flv"));
    assert!(outcomes.failed.is_empty());
    assert!(!outcomes.finished);
}

#[test]
fn parse_outcomes_unmapped_markers_become_orphans() {
    let inputs = vec!["/rec/a.flv".to_string()];
    let logs = info_logs(&[
        queue_line(1, "/other/unrelated.mp4"),
        success_line(1, "/remote/unrelated.mp4"),
        "[2] 上传文件失败, 上传文件错误".to_string(),
        finished_line(),
    ]);

    let outcomes = parse_upload_outcomes(&logs, &inputs);

    assert!(outcomes.completed.is_empty());
    assert_eq!(outcomes.orphan_successes, 1);
    assert_eq!(outcomes.orphan_failures, 1);
}

#[test]
fn tracker_prepares_until_a_bar_and_names_a_file_only_while_it_uploads_alone() {
    let inputs = ["/rec/a.flv".to_string(), "/rec/b.flv".to_string()];
    let sizes = HashMap::from([(inputs[0].clone(), 10), (inputs[1].clone(), 30)]);
    let tracker = UploadTracker::new(
        ProgressReporter::noop("prepare"),
        UploadCounts::default(),
        &inputs,
        &sizes,
    );
    let phase_and_file = || {
        let snapshot = tracker.state.lock().snapshot();
        (snapshot.phase, snapshot.current_file)
    };
    let a = Some("a.flv".to_string());

    tracker.record_line("[1] 准备上传: /rec/a.flv");
    assert_eq!(phase_and_file(), (Some(JobPhase::Preparing), a.clone()));
    tracker.record_line("[2] 准备上传: /rec/b.flv");
    assert_eq!(phase_and_file(), (Some(JobPhase::Preparing), None));
    tracker.record_bar("2", 5, Some(1.0));
    assert_eq!(phase_and_file(), (None, None));
    tracker.record_line(&success_line(2, "/rec/b.flv"));
    assert_eq!(phase_and_file(), (Some(JobPhase::Preparing), a));
}

#[tokio::test(start_paused = true)]
async fn retries_only_files_without_confirmed_outcome() {
    let temp_dir = tempfile::tempdir().unwrap();
    let first = temp_dir.path().join("a.flv");
    let second = temp_dir.path().join("b.flv");
    tokio::fs::write(&first, b"video-a").await.unwrap();
    tokio::fs::write(&second, b"video-b").await.unwrap();
    let first_str = first.to_string_lossy().into_owned();
    let second_str = second.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![
        // Exit code 0 despite a failed file: only the failure table
        // reports it.
        MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &first_str),
                queue_line(2, &second_str),
                success_line(1, "/rec/a.flv"),
                finished_line(),
                "以下文件上传失败: ".to_string(),
                format!("2  {second_str}"),
            ],
        },
        MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &second_str),
                success_line(1, "/rec/b.flv"),
                finished_line(),
            ],
        },
    ]));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![first_str.clone(), second_str.clone()],
        config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
        ..Default::default()
    };
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(16);
    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(64);
    let ctx = ProcessorContext::new(
        "retry",
        crate::pipeline::ProgressReporter::new("retry", progress_tx),
        super::super::traits::JobLogSink::new(log_tx, Arc::default()),
        tokio_util::sync::CancellationToken::new(),
    );

    let output = processor.process(&input, &ctx).await.unwrap();

    // The failed attempt is noted in the job log.
    let warnings: Vec<String> = std::iter::from_fn(|| log_rx.try_recv().ok())
        .filter(|entry| entry.level == crate::pipeline::job_queue::LogLevel::Warn)
        .map(|entry| entry.message)
        .collect();
    assert_eq!(
        warnings,
        ["BaiduPCS-Go upload attempt 1 of 3 left 1 file(s) not uploaded; retrying in 2 s"]
    );
    // Files done count across attempts: the second starts from the file
    // the first uploaded.
    let files: Vec<_> = std::iter::from_fn(|| progress_rx.try_recv().ok())
        .filter_map(|update| update.snapshot.files_done.zip(update.snapshot.files_total))
        .collect();
    assert_eq!(files, [(0, 2), (1, 2), (1, 2), (2, 2)]);
    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    assert!(commands[0].contains(&first_str));
    assert!(commands[0].contains(&second_str));
    assert!(!commands[1].contains(&first_str));
    assert!(commands[1].contains(&second_str));
    assert!(
        output
            .uploads
            .iter()
            .all(|item| item.status == UploadItemStatus::Completed)
    );
}

#[tokio::test]
async fn success_markers_survive_nonzero_exit() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("a.flv");
    tokio::fs::write(&file, b"video").await.unwrap();
    let file_str = file.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
        exit_ok: false,
        lines: vec![
            queue_line(1, &file_str),
            success_line(1, "/rec/a.flv"),
            finished_line(),
        ],
    }]));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![file_str],
        config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("nonzero-exit"))
        .await
        .unwrap();

    assert_eq!(runner.commands().len(), 1);
    assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
}

#[tokio::test(start_paused = true)]
async fn fails_after_exhausting_attempts() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("a.flv");
    tokio::fs::write(&file, b"video").await.unwrap();
    let file_str = file.to_string_lossy().into_owned();

    let attempt = || MockAttempt {
        exit_ok: false,
        lines: vec![queue_line(1, &file_str)],
    };
    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![attempt(), attempt()]));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![file_str.clone()],
        config: Some(r#"{"destination_root":"/rec","max_retries":2}"#.to_string()),
        ..Default::default()
    };

    let error = processor
        .process(&input, &ProcessorContext::noop("exhausted"))
        .await
        .unwrap_err();

    assert_eq!(runner.commands().len(), 2);
    let message = error.to_string();
    assert!(message.contains("after 2 attempt(s)"), "got: {message}");
    assert!(message.contains("no per-file outcome"), "got: {message}");
}

/// A batch in which some files uploaded reports every file's own result
/// instead of failing the job as a whole with no records.
#[tokio::test]
async fn partial_failure_reports_per_file_results_and_keeps_sources() {
    let temp_dir = tempfile::tempdir().unwrap();
    let uploaded = temp_dir.path().join("a.flv");
    let failed = temp_dir.path().join("b.flv");
    tokio::fs::write(&uploaded, b"video-a").await.unwrap();
    tokio::fs::write(&failed, b"video-b").await.unwrap();
    let uploaded_str = uploaded.to_string_lossy().into_owned();
    let failed_str = failed.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
        exit_ok: false,
        lines: vec![
            queue_line(1, &uploaded_str),
            success_line(1, "/rec/a.flv"),
            queue_line(2, &failed_str),
        ],
    }]));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![uploaded_str.clone(), failed_str.clone()],
        config: Some(
            r#"{"destination_root":"/rec","max_retries":1,"remove_source_after_upload":true}"#
                .to_string(),
        ),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("partial"))
        .await
        .unwrap();

    assert_eq!(output.succeeded_inputs, vec![uploaded_str.clone()]);
    assert_eq!(output.failed_inputs.len(), 1);
    assert_eq!(output.failed_inputs[0].0, failed_str);
    assert_eq!(
        output
            .uploads
            .iter()
            .map(|item| (
                item.local_path.as_str(),
                item.status,
                item.remote_path.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                uploaded_str.as_str(),
                UploadItemStatus::Completed,
                Some("/rec/a.flv")
            ),
            (failed_str.as_str(), UploadItemStatus::Failed, None),
        ]
    );
    assert_eq!(output.outputs, input.inputs);
    assert!(
        uploaded.exists(),
        "sources are removed only after a fully successful batch"
    );
}

/// Every file lands directly under the destination folder, so two inputs
/// with the same name are refused before anything is uploaded.
#[tokio::test]
async fn same_named_inputs_are_rejected_before_uploading() {
    let temp_dir = tempfile::tempdir().unwrap();
    let first = temp_dir.path().join("x").join("rec.flv");
    let second = temp_dir.path().join("y").join("rec.flv");
    for path in [&first, &second] {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(path, b"video").await.unwrap();
    }
    let runner = Arc::new(MockBaiduPcsCommandRunner::new(Vec::new()));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ],
        config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
        ..Default::default()
    };

    let error = processor
        .process(&input, &ProcessorContext::noop("collision"))
        .await
        .unwrap_err();

    assert!(matches!(error, crate::Error::Validation(_)));
    assert!(
        error
            .to_string()
            .contains("would both upload as /rec/rec.flv"),
        "{error}"
    );
    assert!(runner.commands().is_empty());
}

#[tokio::test]
async fn missing_input_rejected_on_initial_run() {
    let temp_dir = tempfile::tempdir().unwrap();
    let missing = temp_dir.path().join("missing.flv");

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(Vec::new()));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![missing.to_string_lossy().into_owned()],
        config: Some(
            r#"{"destination_root":"/rec","remove_source_after_upload":true}"#.to_string(),
        ),
        ..Default::default()
    };

    let error = processor
        .process(&input, &ProcessorContext::noop("missing"))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("Input file does not exist"));
    assert!(runner.commands().is_empty());
}

#[tokio::test]
async fn remove_source_deletes_confirmed_uploads() {
    let temp_dir = tempfile::tempdir().unwrap();
    let uploaded = temp_dir.path().join("a.flv");
    let skipped = temp_dir.path().join("b.flv");
    tokio::fs::write(&uploaded, b"video-a").await.unwrap();
    tokio::fs::write(&skipped, b"video-b").await.unwrap();
    let uploaded_str = uploaded.to_string_lossy().into_owned();
    let skipped_str = skipped.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
        exit_ok: true,
        lines: vec![
            queue_line(1, &uploaded_str),
            queue_line(2, &skipped_str),
            success_line(1, "/rec/a.flv"),
            "[2] 目标文件已存在, 跳过...".to_string(),
            finished_line(),
        ],
    }]));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![uploaded_str.clone(), skipped_str.clone()],
        config: Some(
            r#"{"destination_root":"/rec","remove_source_after_upload":true}"#.to_string(),
        ),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("remove-source"))
        .await
        .unwrap();

    assert!(!uploaded.exists());
    assert!(skipped.exists());
    assert_eq!(output.outputs, vec![skipped_str]);
    assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
    assert_eq!(output.uploads[1].status, UploadItemStatus::Skipped);
    // Sizes were captured before deletion.
    assert_eq!(output.uploads[0].size_bytes, Some(7));
}

#[tokio::test]
async fn remove_source_resume_skips_absent_inputs() {
    let temp_dir = tempfile::tempdir().unwrap();
    let already_uploaded = temp_dir.path().join("gone.flv");
    let pending = temp_dir.path().join("pending.flv");
    tokio::fs::write(&pending, b"video").await.unwrap();
    let gone_str = already_uploaded.to_string_lossy().into_owned();
    let pending_str = pending.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
        exit_ok: true,
        lines: vec![
            queue_line(1, &pending_str),
            success_line(1, "/rec/pending.flv"),
            finished_line(),
        ],
    }]));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![gone_str.clone(), pending_str.clone()],
        config: Some(
            r#"{"destination_root":"/rec","remove_source_after_upload":true}"#.to_string(),
        ),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("resume").with_retry(true))
        .await
        .unwrap();

    let commands = runner.commands();
    assert_eq!(commands.len(), 1);
    assert!(!commands[0].contains(&gone_str));
    assert!(commands[0].contains(&pending_str));
    assert!(
        output
            .uploads
            .iter()
            .all(|item| item.status == UploadItemStatus::Completed)
    );
    assert_eq!(
        output.uploads[0].remote_path.as_deref(),
        Some("/rec/gone.flv")
    );
}

#[tokio::test]
async fn unmatched_successes_complete_unresolved_inputs() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("a.flv");
    tokio::fs::write(&file, b"video").await.unwrap();
    let file_str = file.to_string_lossy().into_owned();

    // The CLI echoes a path shape that matches no input, so the success
    // marker cannot be attributed; the clean-finish escape hatch must
    // still complete the file.
    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
        exit_ok: true,
        lines: vec![
            queue_line(1, "/entirely/different/echo.mp4"),
            success_line(1, "/rec/echo.mp4"),
            finished_line(),
        ],
    }]));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![file_str.clone()],
        config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("escape-hatch"))
        .await
        .unwrap();

    assert_eq!(runner.commands().len(), 1);
    assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
    assert_eq!(output.uploads[0].remote_path.as_deref(), Some("/rec/a.flv"));
}

#[tokio::test]
async fn destination_placeholders_expand_with_time_anchor() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("a.flv");
    tokio::fs::write(&file, b"video").await.unwrap();
    let file_str = file.to_string_lossy().into_owned();

    let created_at = utc_datetime(2026, 8, 5, 12, 0, 0);
    let expected_dir = format!(
        "/rec/StreamerName/{}",
        pipeline_common::expand_path_template_at("%Y%m", Some(created_at.as_millisecond()))
    );
    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
        exit_ok: true,
        lines: vec![
            queue_line(1, &file_str),
            success_line(1, &format!("{expected_dir}/a.flv")),
            finished_line(),
        ],
    }]));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let input = ProcessorInput {
        inputs: vec![file_str],
        config: Some(r#"{"destination_root":"/rec/{streamer}/%Y%m"}"#.to_string()),
        streamer_name: Some("StreamerName".to_string()),
        created_at,
        ..Default::default()
    };

    processor
        .process(&input, &ProcessorContext::noop("placeholders"))
        .await
        .unwrap();

    let commands = runner.commands();
    assert_eq!(commands[0].last(), Some(&expected_dir));
}

#[test]
fn normalize_remote_dir_forces_absolute_forward_slash() {
    assert_eq!(BaiduPcsProcessor::normalize_remote_dir(""), "/");
    assert_eq!(BaiduPcsProcessor::normalize_remote_dir("/"), "/");
    assert_eq!(BaiduPcsProcessor::normalize_remote_dir("rec/a/"), "/rec/a");
    assert_eq!(
        BaiduPcsProcessor::normalize_remote_dir("\\rec\\a"),
        "/rec/a"
    );
    assert_eq!(BaiduPcsProcessor::normalize_remote_dir(" /rec "), "/rec");
}

#[test]
fn computed_remote_path_joins_file_name() {
    assert_eq!(
        BaiduPcsProcessor::computed_remote_path("/rec", "D:\\videos\\a.flv"),
        "/rec/a.flv"
    );
    assert_eq!(
        BaiduPcsProcessor::computed_remote_path("/", "/videos/a.flv"),
        "/a.flv"
    );
}

struct MockAuthenticator {
    has_credentials: bool,
    relogin_result: bool,
    relogin_calls: std::sync::atomic::AtomicUsize,
}

impl MockAuthenticator {
    fn new(has_credentials: bool, relogin_result: bool) -> Arc<Self> {
        Arc::new(Self {
            has_credentials,
            relogin_result,
            relogin_calls: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn relogin_calls(&self) -> usize {
        self.relogin_calls
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait]
impl crate::baidupcs::BaiduPcsAuthenticator for MockAuthenticator {
    async fn has_credentials(&self, _config_dir: Option<&str>) -> bool {
        self.has_credentials
    }

    async fn save_credentials(
        &self,
        _config_dir: Option<&str>,
        _material: &crate::baidupcs::LoginMaterial,
    ) -> Result<()> {
        Ok(())
    }

    async fn delete_credentials(&self, _config_dir: Option<&str>) -> Result<()> {
        Ok(())
    }

    async fn relogin(
        &self,
        _binary_path: &str,
        _config_dir: Option<&str>,
        _proxy: &ProxyTarget,
    ) -> Result<bool> {
        self.relogin_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(self.relogin_result)
    }
}

fn who_line(uid: u64) -> String {
    format!("当前帐号 uid: {uid}, 用户名: some_user, 性别: unknown, 年龄: 0.0")
}

#[tokio::test]
async fn logged_out_session_triggers_relogin_before_upload() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("a.flv");
    tokio::fs::write(&file, b"video").await.unwrap();
    let file_str = file.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![
        // The pre-flight `who` probe reports no account.
        MockAttempt {
            exit_ok: true,
            lines: vec![who_line(0)],
        },
        MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &file_str),
                success_line(1, "/rec/a.flv"),
                finished_line(),
            ],
        },
    ]));
    let authenticator = MockAuthenticator::new(true, true);
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone())
        .with_authenticator(authenticator.clone());
    let input = ProcessorInput {
        inputs: vec![file_str],
        config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("relogin-preflight"))
        .await
        .unwrap();

    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0], vec!["who".to_string()]);
    assert_eq!(commands[1][0], "upload");
    assert_eq!(authenticator.relogin_calls(), 1);
    assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
}

#[tokio::test(start_paused = true)]
async fn failed_attempt_triggers_relogin_before_retry() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("a.flv");
    tokio::fs::write(&file, b"video").await.unwrap();
    let file_str = file.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![
        // Pre-flight probe: logged in, so no up-front re-login.
        MockAttempt {
            exit_ok: true,
            lines: vec![who_line(42)],
        },
        // First upload attempt fails without per-file outcomes (the
        // shape an expired session produces).
        MockAttempt {
            exit_ok: false,
            lines: vec![queue_line(1, &file_str)],
        },
        MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &file_str),
                success_line(1, "/rec/a.flv"),
                finished_line(),
            ],
        },
    ]));
    let authenticator = MockAuthenticator::new(true, true);
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone())
        .with_authenticator(authenticator.clone());
    let input = ProcessorInput {
        inputs: vec![file_str],
        config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("relogin-retry"))
        .await
        .unwrap();

    assert_eq!(authenticator.relogin_calls(), 1);
    assert_eq!(runner.commands().len(), 3, "who + failed upload + retry");
    assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
}

#[tokio::test]
async fn no_stored_credentials_skips_probe_entirely() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("a.flv");
    tokio::fs::write(&file, b"video").await.unwrap();
    let file_str = file.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
        exit_ok: true,
        lines: vec![
            queue_line(1, &file_str),
            success_line(1, "/rec/a.flv"),
            finished_line(),
        ],
    }]));
    let authenticator = MockAuthenticator::new(false, true);
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone())
        .with_authenticator(authenticator.clone());
    let input = ProcessorInput {
        inputs: vec![file_str],
        config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
        ..Default::default()
    };

    processor
        .process(&input, &ProcessorContext::noop("no-stored"))
        .await
        .unwrap();

    assert_eq!(authenticator.relogin_calls(), 0);
    let commands = runner.commands();
    assert_eq!(commands.len(), 1, "no who probe without stored credentials");
    assert_eq!(commands[0][0], "upload");
}

/// A file BaiduPCS-Go will never accept (illegal name, over the size
/// limit) is not retried: the remaining attempts, and the re-login that
/// precedes a retry, are spent only on files a retry could still upload.
#[tokio::test(start_paused = true)]
async fn permanent_failures_are_not_retried() {
    let temp_dir = tempfile::tempdir().unwrap();
    let huge = temp_dir.path().join("huge.flv");
    let flaky = temp_dir.path().join("flaky.flv");
    tokio::fs::write(&huge, b"video").await.unwrap();
    tokio::fs::write(&flaky, b"video").await.unwrap();
    let huge_str = huge.to_string_lossy().into_owned();
    let flaky_str = flaky.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![
        // Probe: logged in.
        MockAttempt {
            exit_ok: true,
            lines: vec![who_line(42)],
        },
        MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &huge_str),
                queue_line(2, &flaky_str),
                "[1] 文件大小超过128G, 无法上传, 跳过...".to_string(),
                "[2] 上传文件失败, 网络错误".to_string(),
                finished_line(),
            ],
        },
        MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &flaky_str),
                success_line(1, "/rec/flaky.flv"),
                finished_line(),
            ],
        },
    ]));
    let authenticator = MockAuthenticator::new(true, true);
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone())
        .with_authenticator(authenticator.clone());
    let input = ProcessorInput {
        inputs: vec![huge_str.clone(), flaky_str.clone()],
        config: Some(r#"{"destination_root":"/rec","max_retries":3}"#.to_string()),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("permanent"))
        .await
        .unwrap();

    let commands = runner.commands();
    assert_eq!(commands.len(), 3, "who + first attempt + one retry");
    assert!(
        !commands[2].contains(&huge_str),
        "the oversized file is not re-sent"
    );
    assert!(commands[2].contains(&flaky_str));
    assert_eq!(output.failed_inputs.len(), 1);
    assert_eq!(output.failed_inputs[0].0, huge_str);
    assert!(output.failed_inputs[0].1.contains("128G"));
    assert_eq!(output.succeeded_inputs, vec![flaky_str]);
    assert_eq!(output.uploads[0].status, UploadItemStatus::Failed);
    assert_eq!(output.uploads[1].status, UploadItemStatus::Completed);

    // Only permanent failures: no retry runs at all, and the job fails.
    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
        exit_ok: true,
        lines: vec![
            queue_line(1, &huge_str),
            "[1] 文件大小超过128G, 无法上传, 跳过...".to_string(),
            finished_line(),
        ],
    }]));
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
    let error = processor
        .process(
            &ProcessorInput {
                inputs: vec![huge_str.clone()],
                config: Some(r#"{"destination_root":"/rec","max_retries":3}"#.to_string()),
                ..Default::default()
            },
            &ProcessorContext::noop("permanent-only"),
        )
        .await
        .unwrap_err();
    assert_eq!(runner.commands().len(), 1);
    assert!(error.to_string().contains("after 1 attempt(s)"), "{error}");
}

/// A batch whose paths do not fit one command line is uploaded in several
/// invocations within the same attempt, with every file's result kept.
#[tokio::test]
async fn large_batches_are_split_to_fit_the_command_line() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut inputs = Vec::new();
    for name in ["a.flv", "b.flv", "c.flv"] {
        let path = temp_dir.path().join(name);
        tokio::fs::write(&path, b"video").await.unwrap();
        inputs.push(path.to_string_lossy().into_owned());
    }
    let attempt_for = |paths: &[String]| MockAttempt {
        exit_ok: true,
        lines: paths
            .iter()
            .enumerate()
            .flat_map(|(index, path)| {
                let id = index as u32 + 1;
                let remote = format!(
                    "/rec/{}",
                    Path::new(path).file_name().unwrap().to_string_lossy()
                );
                [queue_line(id, path), success_line(id, &remote)]
            })
            .chain([finished_line()])
            .collect(),
    };
    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![
        attempt_for(&inputs[..2]),
        attempt_for(&inputs[2..]),
    ]));
    // Two paths fit, three do not.
    let budget = inputs[0].len() * 2 + 8;
    let processor =
        MockBaiduPcsCommandRunner::processor(runner.clone()).with_upload_args_budget(budget);
    let input = ProcessorInput {
        inputs: inputs.clone(),
        config: Some(r#"{"destination_root":"/rec","max_retries":1}"#.to_string()),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("chunked"))
        .await
        .unwrap();

    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    assert!(commands[0].contains(&inputs[0]) && commands[0].contains(&inputs[1]));
    assert!(!commands[0].contains(&inputs[2]));
    assert!(commands[1].contains(&inputs[2]));
    assert_eq!(output.succeeded_inputs, inputs);
    assert!(
        output
            .uploads
            .iter()
            .all(|item| item.status == UploadItemStatus::Completed)
    );
    assert_eq!(
        chunk_by_argument_budget(&inputs, 1)
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>(),
        vec![1, 1, 1],
        "an oversized path still forms its own invocation"
    );
}

/// The `who` probe prints the account's uid and name; those lines stay
/// out of the job log while the upload's own output still reaches it.
#[tokio::test]
async fn login_probe_output_stays_out_of_the_job_log() {
    use std::sync::atomic::AtomicUsize;

    use super::super::traits::JobLogSink;
    use crate::pipeline::progress::ProgressReporter;

    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("a.flv");
    tokio::fs::write(&file, b"video").await.unwrap();
    let file_str = file.to_string_lossy().into_owned();

    let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![
        MockAttempt {
            exit_ok: true,
            lines: vec![who_line(42)],
        },
        MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &file_str),
                success_line(1, "/rec/a.flv"),
                finished_line(),
            ],
        },
    ]));
    let authenticator = MockAuthenticator::new(true, true);
    let processor = MockBaiduPcsCommandRunner::processor(runner.clone())
        .with_authenticator(authenticator.clone());
    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(64);
    let ctx = ProcessorContext::new(
        "probe-log",
        ProgressReporter::noop("probe-log"),
        JobLogSink::new(log_tx, Arc::new(AtomicUsize::new(0))),
        tokio_util::sync::CancellationToken::new(),
    );

    processor
        .process(
            &ProcessorInput {
                inputs: vec![file_str.clone()],
                config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
                ..Default::default()
            },
            &ctx,
        )
        .await
        .unwrap();
    drop(ctx);

    let mut logged = Vec::new();
    while let Some(entry) = log_rx.recv().await {
        logged.push(entry.message);
    }
    assert_eq!(runner.commands()[0], vec!["who".to_string()]);
    assert!(
        !logged.iter().any(|line| line.contains("uid: 42")),
        "{logged:?}"
    );
    assert!(
        logged.iter().any(|line| line.contains(&file_str)),
        "{logged:?}"
    );
}
