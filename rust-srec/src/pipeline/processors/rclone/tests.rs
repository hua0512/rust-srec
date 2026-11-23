use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::super::test_utils::{test_exit_code, test_exit_status, utc_datetime};
use super::*;
use crate::pipeline::{JobPhase, JobProgressSnapshot};

struct MockAttempt {
    moved_inputs: Vec<PathBuf>,
    succeeds: bool,
}

#[derive(Clone, Copy)]
enum MkdirOutcome {
    Succeed,
    Fail,
    /// Never completes, like an rclone stuck retrying a throttled remote.
    Hang,
}

/// How long an armed [`FakeRemote`] holds a resolver between finding a
/// directory missing and creating it, waiting for a second resolver.
/// Race tests run on paused time, where this elapses only once every
/// other task is blocked (blocking file IO holds the clock), so a second
/// resolver that can still arrive always does. Shorter than
/// [`MKDIR_TIMEOUT`], so a lone `mkdir` waits it out instead of failing.
const RACE_WINDOW: Duration = Duration::from_secs(10);

/// Bound on a test's jobs; on paused time a deadlock reaches it at once.
const TEST_DEADLINE: Duration = Duration::from_secs(3600);

struct FakeFolder {
    remote: String,
    parent: Option<usize>,
    name: String,
}

/// Remote filesystem that, like Google Drive, keeps same-name sibling
/// folders. Every resolver (mkdir or transfer) looks a path segment up
/// and creates it when missing, with no re-check in between — the gap
/// separate rclone processes race in. Once armed, the first two
/// creations wait in that gap for each other, so two unsynchronized
/// resolvers of one missing directory deterministically both create it.
struct FakeRemote {
    /// Folder IDs are indices; a `None` parent is the remote's root.
    folders: Mutex<Vec<FakeFolder>>,
    race_armed: AtomicBool,
    race_slots: AtomicUsize,
    race_barrier: tokio::sync::Barrier,
}

impl FakeRemote {
    fn new() -> Self {
        Self {
            folders: Mutex::default(),
            race_armed: AtomicBool::new(false),
            race_slots: AtomicUsize::new(0),
            race_barrier: tokio::sync::Barrier::new(2),
        }
    }

    fn arm_race(&self) {
        self.race_armed.store(true, Ordering::SeqCst);
    }

    async fn create_dir_all(&self, dir: &str) {
        let (remote, path) = dir.split_once(':').expect("fake paths name a remote");
        let mut parent = None;
        for name in path.split('/').filter(|segment| !segment.is_empty()) {
            let existing = self.folders.lock().unwrap().iter().position(|folder| {
                folder.remote == remote && folder.parent == parent && folder.name == name
            });
            let id = match existing {
                Some(id) => id,
                None => {
                    if self.race_armed.load(Ordering::SeqCst)
                        && self.race_slots.fetch_add(1, Ordering::SeqCst) < 2
                    {
                        // A lone resolver times out and proceeds.
                        let _ = tokio::time::timeout(RACE_WINDOW, self.race_barrier.wait()).await;
                    }
                    let mut folders = self.folders.lock().unwrap();
                    folders.push(FakeFolder {
                        remote: remote.to_owned(),
                        parent,
                        name: name.to_owned(),
                    });
                    folders.len() - 1
                }
            };
            parent = Some(id);
        }
    }

    fn folder_path(folders: &[FakeFolder], id: usize) -> String {
        let folder = &folders[id];
        match folder.parent {
            Some(parent) => format!("{}/{}", Self::folder_path(folders, parent), folder.name),
            None => format!("{}:{}", folder.remote, folder.name),
        }
    }

    /// Every folder's path, sorted; duplicate folders repeat a path.
    fn dir_paths(&self) -> Vec<String> {
        let folders = self.folders.lock().unwrap();
        let mut paths: Vec<String> = (0..folders.len())
            .map(|id| Self::folder_path(&folders, id))
            .collect();
        paths.sort();
        paths
    }
}

struct MockRcloneCommandRunner {
    attempts: Mutex<VecDeque<MockAttempt>>,
    manifests: Mutex<Vec<Vec<String>>>,
    commands: Mutex<Vec<Vec<String>>>,
    /// `mkdir` invocations, kept apart from `commands` and answered from
    /// `mkdir_outcomes` (default: succeed) rather than `attempts`.
    mkdirs: Mutex<Vec<Vec<String>>>,
    mkdir_outcomes: Mutex<VecDeque<MkdirOutcome>>,
    mkdir_started: tokio::sync::Notify,
    remote: FakeRemote,
}

impl MockRcloneCommandRunner {
    fn new(attempts: Vec<MockAttempt>) -> Self {
        Self {
            attempts: Mutex::new(attempts.into()),
            manifests: Mutex::new(Vec::new()),
            commands: Mutex::new(Vec::new()),
            mkdirs: Mutex::new(Vec::new()),
            mkdir_outcomes: Mutex::new(VecDeque::new()),
            mkdir_started: tokio::sync::Notify::new(),
            remote: FakeRemote::new(),
        }
    }

    fn with_mkdir_outcomes(self, outcomes: Vec<MkdirOutcome>) -> Self {
        *self.mkdir_outcomes.lock().unwrap() = outcomes.into();
        self
    }

    fn manifests(&self) -> Vec<Vec<String>> {
        self.manifests.lock().unwrap().clone()
    }

    fn commands(&self) -> Vec<Vec<String>> {
        self.commands.lock().unwrap().clone()
    }

    fn mkdirs(&self) -> Vec<Vec<String>> {
        self.mkdirs.lock().unwrap().clone()
    }

    /// The directory each recorded `mkdir` targeted.
    fn mkdir_targets(&self) -> Vec<String> {
        self.mkdirs()
            .iter()
            .map(|args| {
                let index = args.iter().position(|arg| arg == "mkdir").unwrap();
                args[index + 1].clone()
            })
            .collect()
    }

    async fn run_mkdir(&self, target: &str) -> CommandOutput {
        self.mkdir_started.notify_one();

        let outcome = self
            .mkdir_outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(MkdirOutcome::Succeed);
        let succeeds = match outcome {
            MkdirOutcome::Succeed => {
                self.remote.create_dir_all(target).await;
                true
            }
            MkdirOutcome::Fail => false,
            MkdirOutcome::Hang => std::future::pending().await,
        };
        CommandOutput {
            status: test_exit_status(succeeds),
            duration: 0.0,
            logs: if succeeds {
                Vec::new()
            } else {
                vec![crate::pipeline::job_queue::JobLogEntry::error(
                    "simulated mkdir failure",
                )]
            },
            fatal_error: false,
            last_stats_error: None,
        }
    }
}

#[async_trait]
impl RcloneCommandRunner for MockRcloneCommandRunner {
    async fn run(
        &self,
        command: &mut Command,
        context: &ProcessorContext,
    ) -> Result<CommandOutput> {
        let args: Vec<String> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        if let Some(index) = args.iter().position(|arg| arg == "mkdir") {
            let target = args[index + 1].clone();
            self.mkdirs.lock().unwrap().push(args);
            return Ok(self.run_mkdir(&target).await);
        }

        // Single-file commands (copyto/moveto) carry no --files-from;
        // only batch commands record a manifest.
        let mut file_destinations = Vec::new();
        if let Some(manifest_index) = args.iter().position(|arg| arg == "--files-from") {
            let manifest_path = PathBuf::from(
                args.get(manifest_index + 1)
                    .ok_or_else(|| crate::Error::Other("missing files-from path".to_string()))?,
            );
            let manifest: Vec<String> = tokio::fs::read_to_string(&manifest_path)
                .await
                .map_err(|e| {
                    crate::Error::io_path("reading test rclone manifest", &manifest_path, e)
                })?
                .lines()
                .map(str::to_string)
                .collect();
            // `--files-from <manifest> <op> <base_dir> <destination>`
            let root = args[manifest_index + 4].trim_end_matches('/');
            file_destinations.extend(manifest.iter().map(|line| format!("{root}/{line}")));
            self.manifests.lock().unwrap().push(manifest);
        } else if let Some(index) = args
            .iter()
            .position(|arg| arg == "copyto" || arg == "moveto")
        {
            // `<op> <source> <destination>`
            file_destinations.push(args[index + 2].clone());
        }
        self.commands.lock().unwrap().push(args);
        // A transfer resolves each file's directory the same way.
        for destination in &file_destinations {
            if let Some(dir) = RcloneProcessor::remote_parent(destination) {
                self.remote.create_dir_all(dir).await;
            }
        }

        let attempt = self
            .attempts
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| crate::Error::Other("unexpected rclone attempt".to_string()))?;
        for input in attempt.moved_inputs {
            tokio::fs::remove_file(&input)
                .await
                .map_err(|e| crate::Error::io_path("moving test input", &input, e))?;
        }
        // Like a stats line of the real transfer.
        context
            .progress
            .report(JobProgressSnapshot::new(ProgressKind::Rclone));

        Ok(CommandOutput {
            status: test_exit_status(attempt.succeeds),
            duration: 0.0,
            logs: if attempt.succeeds {
                Vec::new()
            } else {
                vec![crate::pipeline::job_queue::JobLogEntry::error(
                    "simulated partial move failure",
                )]
            },
            fatal_error: false,
            last_stats_error: None,
        })
    }
}

fn expected_local_destination(dt: jiff::Timestamp) -> String {
    format!(
        "remote:/{}/StreamerName",
        pipeline_common::expand_path_template_at("%Y/%m/%d", Some(dt.as_millisecond()))
    )
}

/// A move batch that fails after transferring some inputs reports each
/// file's own result: the moved ones as completed, the rest as failed, so
/// the worker pool fails the job without recording transferred files as
/// failed uploads.
#[tokio::test(start_paused = true)]
async fn batch_move_partial_failure_reports_per_file_results() {
    let temp_dir = tempfile::tempdir().unwrap();
    let names = ["a.mp4", "b.mp4", "c.mp4", "d.mp4"];
    let paths: Vec<PathBuf> = names
        .iter()
        .map(|name| temp_dir.path().join(name))
        .collect();
    for path in &paths {
        tokio::fs::write(path, b"video").await.unwrap();
    }
    // Every attempt moves one more input and then fails.
    let runner = Arc::new(MockRcloneCommandRunner::new(
        paths[..3]
            .iter()
            .map(|path| MockAttempt {
                moved_inputs: vec![path.clone()],
                succeeds: false,
            })
            .collect(),
    ));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let inputs: Vec<String> = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let input = ProcessorInput {
        inputs: inputs.clone(),
        outputs: vec!["remote:/records".to_string()],
        config: Some(r#"{"operation":"move"}"#.to_string()),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("partial-move"))
        .await
        .unwrap();

    assert_eq!(runner.commands().len(), 3);
    assert_eq!(output.succeeded_inputs, inputs[..3]);
    assert_eq!(output.failed_inputs.len(), 1);
    assert_eq!(output.failed_inputs[0].0, inputs[3]);
    assert!(output.failed_inputs[0].1.contains("rclone batch failed"));
    assert!(output.outputs.is_empty());
    assert_eq!(
        output
            .uploads
            .iter()
            .map(|item| (
                item.local_path.as_str(),
                item.status,
                item.remote_path.is_some()
            ))
            .collect::<Vec<_>>(),
        vec![
            (inputs[0].as_str(), UploadItemStatus::Completed, true),
            (inputs[1].as_str(), UploadItemStatus::Completed, true),
            (inputs[2].as_str(), UploadItemStatus::Completed, true),
            (inputs[3].as_str(), UploadItemStatus::Failed, false),
        ]
    );
    assert!(paths[3].exists(), "the unmoved source stays for a retry");
}

#[tokio::test(start_paused = true)]
async fn batch_move_retries_only_inputs_that_still_exist() {
    let temp_dir = tempfile::tempdir().unwrap();
    let first = temp_dir.path().join("first.mp4");
    let second = temp_dir.path().join("second.jpg");
    tokio::fs::write(&first, b"video").await.unwrap();
    tokio::fs::write(&second, b"thumbnail").await.unwrap();

    let runner = Arc::new(MockRcloneCommandRunner::new(vec![
        MockAttempt {
            moved_inputs: vec![first.clone()],
            succeeds: false,
        },
        MockAttempt {
            moved_inputs: vec![second.clone()],
            succeeds: false,
        },
    ]));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let inputs = vec![
        first.to_string_lossy().into_owned(),
        second.to_string_lossy().into_owned(),
    ];
    let input = ProcessorInput {
        inputs: inputs.clone(),
        outputs: vec!["remote:/records".to_string()],
        config: Some(r#"{"operation":"move"}"#.to_string()),
        ..Default::default()
    };
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(16);
    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(64);
    let ctx = ProcessorContext::new(
        "batch-move",
        crate::pipeline::ProgressReporter::new("batch-move", progress_tx),
        super::super::traits::JobLogSink::new(log_tx, Arc::default()),
        tokio_util::sync::CancellationToken::new(),
    );

    let output = processor.process(&input, &ctx).await.unwrap();
    drop(ctx);

    // The failed attempt is noted in the job log. Progress shows the
    // retrying phase only for the wait, and attempt 2 from its start.
    let warnings: Vec<String> = std::iter::from_fn(|| log_rx.try_recv().ok())
        .filter(|entry| entry.level == crate::pipeline::job_queue::LogLevel::Warn)
        .map(|entry| entry.message)
        .collect();
    assert_eq!(
        warnings,
        [
            "rclone move attempt 1 of 3 failed: rclone batch failed with exit code 1: simulated partial move failure; retrying in 2 s"
        ]
    );
    let progress: Vec<_> = std::iter::from_fn(|| progress_rx.try_recv().ok())
        .map(|update| {
            let snapshot = update.snapshot;
            (snapshot.phase, snapshot.attempt, snapshot.max_attempts)
        })
        .collect();
    assert_eq!(
        progress,
        [
            (None, None, None),
            (Some(JobPhase::Retrying), Some(2), Some(3)),
            (None, Some(2), Some(3)),
            (None, Some(2), Some(3)),
        ]
    );
    assert_eq!(output.succeeded_inputs, inputs);
    assert!(output.failed_inputs.is_empty());
    assert!(!first.exists());
    assert!(!second.exists());
    assert_eq!(
        runner.manifests(),
        vec![
            vec!["first.mp4".to_string(), "second.jpg".to_string()],
            vec!["second.jpg".to_string()],
        ]
    );
    assert!(runner.commands().iter().all(|args| {
        args.iter().any(|arg| arg == "move") && !args.iter().any(|arg| arg == "copy")
    }));
    assert!(std::fs::read_dir(temp_dir.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rclone_files_")
    }));
}

#[tokio::test]
async fn batch_move_skips_inputs_moved_by_a_previous_job_attempt() {
    let temp_dir = tempfile::tempdir().unwrap();
    let already_moved = temp_dir.path().join("already-moved.mp4");
    let pending = temp_dir.path().join("pending.jpg");
    tokio::fs::write(&pending, b"thumbnail").await.unwrap();

    let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
        moved_inputs: vec![pending.clone()],
        succeeds: true,
    }]));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let inputs = vec![
        already_moved.to_string_lossy().into_owned(),
        pending.to_string_lossy().into_owned(),
    ];
    let input = ProcessorInput {
        inputs: inputs.clone(),
        outputs: vec!["remote:/records".to_string()],
        config: Some(r#"{"operation":"move"}"#.to_string()),
        ..Default::default()
    };

    let output = processor
        .process(
            &input,
            &ProcessorContext::noop("resumed-batch-move").with_retry(true),
        )
        .await
        .unwrap();

    assert_eq!(output.succeeded_inputs, inputs);
    assert_eq!(runner.manifests(), vec![vec!["pending.jpg".to_string()]]);
    assert!(!pending.exists());
}

#[tokio::test]
async fn single_move_skips_input_moved_by_a_previous_job_attempt() {
    let temp_dir = tempfile::tempdir().unwrap();
    let already_moved = temp_dir.path().join("already-moved.mp4");
    let runner = Arc::new(MockRcloneCommandRunner::new(Vec::new()));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let input_path = already_moved.to_string_lossy().into_owned();
    let input = ProcessorInput {
        inputs: vec![input_path.clone()],
        outputs: vec!["remote:/records".to_string()],
        config: Some(r#"{"operation":"move"}"#.to_string()),
        ..Default::default()
    };

    let output = processor
        .process(
            &input,
            &ProcessorContext::noop("resumed-single-move").with_retry(true),
        )
        .await
        .unwrap();

    assert_eq!(output.succeeded_inputs, vec![input_path]);
    assert!(output.outputs.is_empty());
    assert!(runner.commands().is_empty());
}

#[tokio::test]
async fn initial_move_rejects_missing_inputs_before_any_transfer() {
    let temp_dir = tempfile::tempdir().unwrap();
    let present = temp_dir.path().join("present.mp4");
    tokio::fs::write(&present, b"video").await.unwrap();
    let present_input = present.to_string_lossy().into_owned();
    let missing = temp_dir
        .path()
        .join("missing.mp4")
        .to_string_lossy()
        .into_owned();
    let another_missing = temp_dir
        .path()
        .join("missing.jpg")
        .to_string_lossy()
        .into_owned();

    for inputs in [
        vec![missing.clone()],
        vec![present_input.clone(), missing.clone()],
        vec![missing.clone(), another_missing],
    ] {
        let runner = Arc::new(MockRcloneCommandRunner::new(Vec::new()));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs,
            outputs: vec!["remote:/records".to_owned()],
            config: Some(r#"{"operation":"move"}"#.to_owned()),
            ..Default::default()
        };
        let error = processor
            .process(&input, &ProcessorContext::noop("initial-move"))
            .await
            .unwrap_err();
        assert!(
            matches!(error, crate::Error::Validation(ref message) if message.contains(&missing))
        );
        assert!(
            runner.commands().is_empty(),
            "invalid batch must fail before rclone runs"
        );
        assert!(runner.manifests().is_empty());
        assert_eq!(tokio::fs::read(&present).await.unwrap(), b"video");
    }
}

#[tokio::test]
async fn retried_move_accepts_batch_already_consumed_by_previous_attempt() {
    let temp_dir = tempfile::tempdir().unwrap();
    let inputs: Vec<String> = ["already-moved.mp4", "already-moved.jpg"]
        .iter()
        .map(|name| temp_dir.path().join(name).to_string_lossy().into_owned())
        .collect();
    let runner = Arc::new(MockRcloneCommandRunner::new(Vec::new()));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let input = ProcessorInput {
        inputs: inputs.clone(),
        outputs: vec!["remote:/records".to_owned()],
        config: Some(r#"{"operation":"move"}"#.to_owned()),
        ..Default::default()
    };
    let output = processor
        .process(
            &input,
            &ProcessorContext::noop("resumed-complete-batch").with_retry(true),
        )
        .await
        .unwrap();
    assert_eq!(output.succeeded_inputs, inputs);
    assert!(output.outputs.is_empty());
    assert_eq!(output.uploads.len(), 2);
    assert!(
        output
            .uploads
            .iter()
            .all(|upload| upload.status == UploadItemStatus::Completed)
    );
    assert!(runner.commands().is_empty());
    assert!(runner.mkdirs().is_empty(), "nothing left to upload");
    let metadata: serde_json::Value =
        serde_json::from_str(output.metadata.as_deref().unwrap()).unwrap();
    assert_eq!(metadata["resumed_inputs"], 2);
    assert_eq!(metadata["attempts"], 0);
}

#[tokio::test]
async fn single_move_succeeds_when_rclone_fails_after_consuming_the_source() {
    let temp_dir = tempfile::tempdir().unwrap();
    let source = temp_dir.path().join("input.mp4");
    tokio::fs::write(&source, b"video").await.unwrap();

    let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
        moved_inputs: vec![source.clone()],
        succeeds: false,
    }]));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let input_path = source.to_string_lossy().into_owned();
    let input = ProcessorInput {
        inputs: vec![input_path.clone()],
        outputs: vec!["remote:/records".to_string()],
        config: Some(r#"{"operation":"move"}"#.to_string()),
        ..Default::default()
    };

    let output = processor
        .process(&input, &ProcessorContext::noop("single-move-consumed"))
        .await
        .unwrap();

    assert_eq!(output.succeeded_inputs, vec![input_path]);
    assert!(output.outputs.is_empty());
    assert!(!source.exists());
    assert_eq!(runner.commands().len(), 1);
    assert!(runner.commands()[0].iter().any(|arg| arg == "moveto"));
}

fn succeeding_attempts(count: usize) -> Vec<MockAttempt> {
    (0..count)
        .map(|_| MockAttempt {
            moved_inputs: vec![],
            succeeds: true,
        })
        .collect()
}

/// Writes `names` into `dir` and returns their paths as job inputs.
async fn write_inputs(dir: &Path, names: &[&str]) -> Vec<String> {
    let mut inputs = Vec::new();
    for name in names {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        tokio::fs::write(&path, b"data").await.unwrap();
        inputs.push(path.to_string_lossy().into_owned());
    }
    inputs
}

fn upload_input(inputs: Vec<String>, destination_root: &str) -> ProcessorInput {
    ProcessorInput {
        inputs,
        config: Some(
            serde_json::json!({ "operation": "copy", "destination_root": destination_root })
                .to_string(),
        ),
        ..Default::default()
    }
}

/// The incident: a single-file upload and a batch upload from one
/// session start together into a day directory that does not exist yet.
/// Without coordinated directory creation each rclone process creates
/// `14` and the files split between two same-name folders.
#[tokio::test(start_paused = true)]
async fn concurrent_uploads_into_a_new_directory_share_one_remote_folder() {
    const DESTINATION: &str = "remote:records/Streamer/2026/10/14";
    let runner = Arc::new(MockRcloneCommandRunner::new(succeeding_attempts(2)));
    runner
        .remote
        .create_dir_all("remote:records/Streamer/2026/10")
        .await;
    runner.remote.arm_race();
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let xml_dir = tempfile::tempdir().unwrap();
    let video_dir = tempfile::tempdir().unwrap();
    let xml_upload = upload_input(
        write_inputs(xml_dir.path(), &["danmu.xml"]).await,
        DESTINATION,
    );
    let video_upload = upload_input(
        write_inputs(video_dir.path(), &["video.mp4", "cover.jpg"]).await,
        DESTINATION,
    );
    let xml_context = ProcessorContext::noop("xml-upload");
    let video_context = ProcessorContext::noop("video-upload");

    let (xml_result, video_result) = tokio::time::timeout(TEST_DEADLINE, async {
        tokio::join!(
            processor.process(&xml_upload, &xml_context),
            processor.process(&video_upload, &video_context),
        )
    })
    .await
    .expect("uploads finish");
    xml_result.unwrap();
    video_result.unwrap();

    assert_eq!(
        runner.remote.dir_paths(),
        [
            "remote:records",
            "remote:records/Streamer",
            "remote:records/Streamer/2026",
            "remote:records/Streamer/2026/10",
            "remote:records/Streamer/2026/10/14",
        ]
    );
}

/// Different day directories still share a missing month directory, so
/// creation is serialized per remote, not per destination path.
#[tokio::test(start_paused = true)]
async fn concurrent_uploads_to_sibling_directories_share_a_new_parent() {
    let runner = Arc::new(MockRcloneCommandRunner::new(succeeding_attempts(2)));
    runner
        .remote
        .create_dir_all("remote:records/Streamer/2026")
        .await;
    runner.remote.arm_race();
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let temp_dir = tempfile::tempdir().unwrap();
    let first = upload_input(
        write_inputs(temp_dir.path(), &["first.mp4"]).await,
        "remote:records/Streamer/2026/10/14",
    );
    let second = upload_input(
        write_inputs(temp_dir.path(), &["second.mp4"]).await,
        "remote:records/Streamer/2026/10/15",
    );
    let first_context = ProcessorContext::noop("first");
    let second_context = ProcessorContext::noop("second");

    let (first_result, second_result) = tokio::time::timeout(TEST_DEADLINE, async {
        tokio::join!(
            processor.process(&first, &first_context),
            processor.process(&second, &second_context),
        )
    })
    .await
    .expect("uploads finish");
    first_result.unwrap();
    second_result.unwrap();

    assert_eq!(
        runner.remote.dir_paths(),
        [
            "remote:records",
            "remote:records/Streamer",
            "remote:records/Streamer/2026",
            "remote:records/Streamer/2026/10",
            "remote:records/Streamer/2026/10/14",
            "remote:records/Streamer/2026/10/15",
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn recent_mkdir_is_reused_until_its_ttl_expires() {
    const DESTINATION: &str = "remote:records/Streamer/2026/10/14";
    let runner = Arc::new(MockRcloneCommandRunner::new(succeeding_attempts(3)));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let temp_dir = tempfile::tempdir().unwrap();
    let input = upload_input(
        write_inputs(temp_dir.path(), &["video.mp4"]).await,
        DESTINATION,
    );
    let context = ProcessorContext::noop("upload");

    processor.process(&input, &context).await.unwrap();
    processor.process(&input, &context).await.unwrap();
    assert_eq!(runner.mkdir_targets(), [DESTINATION]);

    tokio::time::advance(ENSURED_DIR_TTL + Duration::from_secs(1)).await;
    processor.process(&input, &context).await.unwrap();
    assert_eq!(runner.mkdir_targets(), [DESTINATION, DESTINATION]);
    assert_eq!(runner.commands().len(), 3);
}

/// The same path under another root folder is another directory, so a
/// recent `mkdir` under one root does not stand in for the other.
#[tokio::test]
async fn recent_mkdir_is_not_reused_under_other_backend_args() {
    let runner = Arc::new(MockRcloneCommandRunner::new(succeeding_attempts(2)));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let temp_dir = tempfile::tempdir().unwrap();
    let inputs = write_inputs(temp_dir.path(), &["video.mp4"]).await;
    let under_root = |root: &str| ProcessorInput {
        inputs: inputs.clone(),
        config: Some(
            serde_json::json!({
                "operation": "copy",
                "destination_root": "remote:records/day",
                "args": ["--drive-root-folder-id", root],
            })
            .to_string(),
        ),
        ..Default::default()
    };
    let context = ProcessorContext::noop("upload");

    processor.process(&under_root("a"), &context).await.unwrap();
    processor.process(&under_root("b"), &context).await.unwrap();

    assert_eq!(runner.mkdirs().len(), 2);
}

/// A throttled or hung `mkdir` is retried before the job gives up, like
/// a transfer: failed jobs are only retried by hand. Its waits show like
/// a transfer's, but without an attempt number, which would otherwise
/// stick to the transfer's reports.
#[tokio::test(start_paused = true)]
async fn transient_mkdir_failures_are_retried() {
    let runner = Arc::new(
        MockRcloneCommandRunner::new(succeeding_attempts(1))
            .with_mkdir_outcomes(vec![MkdirOutcome::Fail, MkdirOutcome::Hang]),
    );
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let temp_dir = tempfile::tempdir().unwrap();
    let input = upload_input(
        write_inputs(temp_dir.path(), &["video.mp4"]).await,
        "remote:records/day",
    );
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(16);
    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(64);
    let context = ProcessorContext::new(
        "upload",
        crate::pipeline::ProgressReporter::new("upload", progress_tx),
        super::super::traits::JobLogSink::new(log_tx, Arc::default()),
        tokio_util::sync::CancellationToken::new(),
    );

    processor.process(&input, &context).await.unwrap();
    drop(context);

    assert_eq!(runner.mkdirs().len(), 3);
    assert_eq!(runner.commands().len(), 1);
    let warnings: Vec<String> = std::iter::from_fn(|| log_rx.try_recv().ok())
        .filter(|entry| entry.level == crate::pipeline::job_queue::LogLevel::Warn)
        .map(|entry| entry.message)
        .collect();
    assert_eq!(
        warnings,
        [
            "rclone mkdir attempt 1 of 3 failed: rclone mkdir remote:records/day failed with exit code 1: simulated mkdir failure; retrying in 2 s".to_owned(),
            format!(
                "rclone mkdir attempt 2 of 3 failed: rclone mkdir remote:records/day timed out after {}s; retrying in 4 s",
                MKDIR_TIMEOUT.as_secs()
            ),
        ]
    );
    let progress: Vec<_> = std::iter::from_fn(|| progress_rx.try_recv().ok())
        .map(|update| {
            let snapshot = update.snapshot;
            (snapshot.phase, snapshot.attempt, snapshot.max_attempts)
        })
        .collect();
    // The last report is the mock transfer's.
    assert_eq!(
        progress,
        [
            (Some(JobPhase::Retrying), None, None),
            (None, None, None),
            (Some(JobPhase::Retrying), None, None),
            (None, None, None),
            (None, None, None),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn failed_mkdir_fails_the_job_before_any_transfer() {
    let runner = Arc::new(
        MockRcloneCommandRunner::new(succeeding_attempts(1))
            .with_mkdir_outcomes(vec![MkdirOutcome::Fail; 3]),
    );
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let temp_dir = tempfile::tempdir().unwrap();
    let input = upload_input(
        write_inputs(temp_dir.path(), &["video.mp4"]).await,
        "remote:records/day",
    );
    let context = ProcessorContext::noop("upload");

    let error = processor.process(&input, &context).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("rclone mkdir remote:records/day failed")
    );
    assert!(runner.commands().is_empty());

    assert_eq!(runner.mkdirs().len(), 3, "every attempt was used");

    // The failure is not remembered: the next job creates the directory.
    processor.process(&input, &context).await.unwrap();
    assert_eq!(runner.mkdirs().len(), 4);
    assert_eq!(runner.commands().len(), 1);
}

/// The preset's `max_retries` bounds both `mkdir` and transfer attempts.
#[tokio::test(start_paused = true)]
async fn configured_attempts_bound_mkdir_and_transfer_retries() {
    let failing = || MockAttempt {
        moved_inputs: vec![],
        succeeds: false,
    };
    let runner = Arc::new(
        MockRcloneCommandRunner::new(vec![failing(), failing(), failing()])
            .with_mkdir_outcomes(vec![MkdirOutcome::Fail]),
    );
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let temp_dir = tempfile::tempdir().unwrap();
    let inputs = write_inputs(temp_dir.path(), &["video.mp4"]).await;
    let input = |max_retries: u32| ProcessorInput {
        inputs: inputs.clone(),
        config: Some(
            serde_json::json!({
                "operation": "copy",
                "destination_root": "remote:records/day",
                "max_retries": max_retries,
            })
            .to_string(),
        ),
        ..Default::default()
    };
    let context = ProcessorContext::noop("upload");

    processor.process(&input(1), &context).await.unwrap_err();
    assert_eq!(runner.mkdirs().len(), 1);
    assert!(runner.commands().is_empty());

    processor.process(&input(2), &context).await.unwrap_err();
    assert_eq!(runner.commands().len(), 2);

    // Zero still makes one attempt.
    processor.process(&input(0), &context).await.unwrap_err();
    assert_eq!(runner.commands().len(), 3);
}

/// Cancelling a job while its `mkdir` holds the remote's lock must not
/// keep later jobs out.
#[tokio::test]
async fn cancelled_mkdir_releases_the_remote_lock() {
    let runner = Arc::new(
        MockRcloneCommandRunner::new(succeeding_attempts(1))
            .with_mkdir_outcomes(vec![MkdirOutcome::Hang]),
    );
    let processor = Arc::new(RcloneProcessor::with_command_runner(
        "rclone",
        runner.clone(),
    ));
    let temp_dir = tempfile::tempdir().unwrap();
    let input = upload_input(
        write_inputs(temp_dir.path(), &["video.mp4"]).await,
        "remote:records/day",
    );

    let holder = tokio::spawn({
        let processor = processor.clone();
        let input = input.clone();
        async move {
            processor
                .process(&input, &ProcessorContext::noop("holder"))
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), runner.mkdir_started.notified())
        .await
        .expect("holder runs mkdir");
    let lock = processor.dir_gate.remote("remote");
    assert!(lock.try_lock().is_err(), "the hung mkdir holds the lock");

    holder.abort();
    assert!(holder.await.unwrap_err().is_cancelled());
    assert!(lock.try_lock().is_ok(), "cancellation released the lock");

    processor
        .process(&input, &ProcessorContext::noop("next"))
        .await
        .unwrap();
    assert_eq!(runner.mkdirs().len(), 2);
}

#[tokio::test]
async fn mkdir_uses_transfer_flags_and_covers_nested_batch_directories() {
    let runner = Arc::new(MockRcloneCommandRunner::new(succeeding_attempts(1)));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let temp_dir = tempfile::tempdir().unwrap();
    let input = ProcessorInput {
        inputs: write_inputs(temp_dir.path(), &["a.mp4", "nested/b.mp4", "c.jpg"]).await,
        config: Some(
            serde_json::json!({
                "operation": "copy",
                "destination_root": "remote:records/day",
                "config_path": "/config/rclone.conf",
                "transfers": 2,
                "args": ["--drive-root-folder-id", "folder-id"],
            })
            .to_string(),
        ),
        ..Default::default()
    };

    processor
        .process(&input, &ProcessorContext::noop("batch"))
        .await
        .unwrap();

    assert_eq!(
        runner.mkdirs()[0],
        [
            "--config",
            "/config/rclone.conf",
            "--use-json-log",
            "--log-level",
            "NOTICE",
            "--retries",
            "1",
            "--low-level-retries",
            "3",
            "mkdir",
            "remote:records/day",
            "--transfers",
            "2",
            "--drive-root-folder-id",
            "folder-id",
        ]
    );
    assert_eq!(
        runner.mkdir_targets(),
        ["remote:records/day", "remote:records/day/nested"]
    );
}

#[test]
fn remote_name_and_parent_parse_destinations() {
    assert_eq!(RcloneProcessor::remote_name("gdrive:a/b"), Some("gdrive"));
    assert_eq!(
        RcloneProcessor::remote_name("gdrive,token=secret:a/b"),
        Some("gdrive")
    );
    assert_eq!(RcloneProcessor::remote_name(":drive,k=v:a"), Some(":drive"));
    assert_eq!(RcloneProcessor::remote_name("/local/a:b"), None);
    assert_eq!(RcloneProcessor::remote_name("relative/path"), None);
    if cfg!(windows) {
        assert_eq!(RcloneProcessor::remote_name(r"C:\records"), None);
    }

    assert_eq!(
        RcloneProcessor::remote_parent("gdrive:records/day/a.mp4"),
        Some("gdrive:records/day")
    );
    assert_eq!(
        RcloneProcessor::remote_parent("gdrive,k=v:records/a.mp4"),
        Some("gdrive,k=v:records")
    );
    assert_eq!(
        RcloneProcessor::remote_parent("gdrive:/records/a.mp4"),
        Some("gdrive:/records")
    );
    assert_eq!(
        RcloneProcessor::remote_parent(":drive,k=v:records/a.mp4"),
        Some(":drive,k=v:records")
    );
    // Parameter values may hold `/`, and quoted ones `:` and `,`.
    assert_eq!(
        RcloneProcessor::remote_parent(":s3,secret_access_key=a/b:bucket/day/a.mp4"),
        Some(":s3,secret_access_key=a/b:bucket/day")
    );
    assert_eq!(
        RcloneProcessor::split_remote(r#"drive,root='a:b,c''d',x="e:f":records/a.mp4"#),
        Some((r#"drive,root='a:b,c''d',x="e:f""#, "records/a.mp4"))
    );
    assert_eq!(
        RcloneProcessor::remote_name("drive,root='a:b':records"),
        Some("drive")
    );
    assert_eq!(RcloneProcessor::remote_parent("gdrive:/records"), None);
    assert_eq!(RcloneProcessor::remote_parent("gdrive:a.mp4"), None);
    assert_eq!(RcloneProcessor::remote_parent("/local/dir/a.mp4"), None);
}

#[tokio::test]
async fn copy_still_rejects_missing_inputs() {
    let temp_dir = tempfile::tempdir().unwrap();
    let input = ProcessorInput {
        inputs: vec![
            temp_dir
                .path()
                .join("missing.mp4")
                .to_string_lossy()
                .into_owned(),
        ],
        outputs: vec!["remote:/records".to_string()],
        config: Some(r#"{"operation":"copy"}"#.to_string()),
        ..Default::default()
    };

    for is_retry in [false, true] {
        let error = RcloneProcessor::new()
            .process(
                &input,
                &ProcessorContext::noop("missing-copy").with_retry(is_retry),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Input file does not exist"));
    }
}

/// The ordered flag block `process_single`/`process_batch` place before
/// the subcommand; `run_rclone_with_progress` parses stderr on the
/// assumption that every one of these is present.
const EXPECTED_STATS_FLAGS: [&str; 8] = [
    "--use-json-log",
    "--log-level",
    "NOTICE",
    "--stats-log-level",
    "NOTICE",
    "--stats",
    "1s",
    "--stats-one-line",
];

fn stats_flag_window(args: &[String]) -> Vec<&str> {
    let pos = args
        .iter()
        .position(|arg| arg == "--use-json-log")
        .expect("argv contains --use-json-log");
    args[pos..pos + EXPECTED_STATS_FLAGS.len()]
        .iter()
        .map(String::as_str)
        .collect()
}

#[tokio::test]
async fn single_copy_passes_json_stats_flags() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("clip.mp4");
    tokio::fs::write(&file, b"video").await.unwrap();

    let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
        moved_inputs: vec![],
        succeeds: true,
    }]));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let input = ProcessorInput {
        inputs: vec![file.to_string_lossy().into_owned()],
        outputs: vec!["remote:/records".to_string()],
        config: Some(r#"{"operation":"copy"}"#.to_string()),
        ..Default::default()
    };

    processor
        .process(&input, &ProcessorContext::noop("single-copy-flags"))
        .await
        .unwrap();

    let commands = runner.commands();
    assert_eq!(commands.len(), 1);
    let args = &commands[0];
    assert_eq!(stats_flag_window(args), EXPECTED_STATS_FLAGS);
    assert!(!args.iter().any(|arg| arg == "--stats-one-line-date"));
    assert!(args.iter().any(|arg| arg == "copyto"));
    assert!(args.windows(2).any(|pair| pair == ["--retries", "1"]));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--low-level-retries", "3"])
    );
}

#[tokio::test]
async fn batch_copy_passes_json_stats_flags() {
    let temp_dir = tempfile::tempdir().unwrap();
    let first = temp_dir.path().join("first.mp4");
    let second = temp_dir.path().join("second.mp4");
    tokio::fs::write(&first, b"a").await.unwrap();
    tokio::fs::write(&second, b"b").await.unwrap();

    let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
        moved_inputs: vec![],
        succeeds: true,
    }]));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let input = ProcessorInput {
        inputs: vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ],
        outputs: vec!["remote:/records".to_string()],
        config: Some(r#"{"operation":"copy"}"#.to_string()),
        ..Default::default()
    };

    processor
        .process(&input, &ProcessorContext::noop("batch-copy-flags"))
        .await
        .unwrap();

    let commands = runner.commands();
    assert_eq!(commands.len(), 1);
    let args = &commands[0];
    assert_eq!(stats_flag_window(args), EXPECTED_STATS_FLAGS);
    assert!(!args.iter().any(|arg| arg == "--stats-one-line-date"));
    assert!(args.iter().any(|arg| arg == "--files-from"));
    assert!(args.windows(2).any(|pair| pair == ["--retries", "1"]));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--low-level-retries", "3"])
    );
}

#[tokio::test]
async fn progress_flags_are_stripped_from_extra_args() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("clip.mp4");
    tokio::fs::write(&file, b"video").await.unwrap();

    let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
        moved_inputs: vec![],
        succeeds: true,
    }]));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let input = ProcessorInput {
        inputs: vec![file.to_string_lossy().into_owned()],
        outputs: vec!["remote:/records".to_string()],
        config: Some(
            r#"{"operation":"copy","args":["--progress","-P","--transfers","2"]}"#.to_string(),
        ),
        ..Default::default()
    };

    processor
        .process(&input, &ProcessorContext::noop("strip-progress"))
        .await
        .unwrap();

    let args = &runner.commands()[0];
    assert!(!args.iter().any(|arg| arg == "--progress" || arg == "-P"));
    // Surviving extra args still trail the built-in flag block so they
    // keep last-wins override semantics.
    let transfers_pos = args.iter().position(|arg| arg == "--transfers").unwrap();
    let one_line_pos = args
        .iter()
        .position(|arg| arg == "--stats-one-line")
        .unwrap();
    assert!(transfers_pos > one_line_pos);
    assert_eq!(args[transfers_pos + 1], "2");
}

/// End-to-end against a real rclone binary: runs the production flag
/// set on a bwlimit'd local copy and asserts progress snapshots stream
/// out of the JSON stats parsing while stats never leak into job logs.
/// Run manually: `cargo test -p rust-srec --lib live_rclone -- --ignored`.
#[tokio::test]
#[ignore = "requires an rclone binary on PATH"]
async fn live_rclone_copy_streams_progress_snapshots() {
    use crate::pipeline::processors::traits::JobLogSink;
    use std::sync::atomic::AtomicUsize;

    let temp_dir = tempfile::tempdir().unwrap();
    let src = temp_dir.path().join("src.bin");
    let dest_dir = temp_dir.path().join("dest");
    tokio::fs::create_dir(&dest_dir).await.unwrap();
    let payload = vec![0u8; 300 * 1024];
    tokio::fs::write(&src, &payload).await.unwrap();

    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(256);
    let ctx = ProcessorContext::new(
        "live-job",
        crate::pipeline::ProgressReporter::new("live-job", progress_tx),
        JobLogSink::new(log_tx, Arc::new(AtomicUsize::new(0))),
        tokio_util::sync::CancellationToken::new(),
    );

    let processor = RcloneProcessor::new();
    let dest = format!("{}/", dest_dir.to_string_lossy().replace('\\', "/"));
    let input = ProcessorInput {
        inputs: vec![src.to_string_lossy().into_owned()],
        outputs: vec![dest],
        // bwlimit throttles the copy across several 1s stats ticks.
        config: Some(r#"{"operation":"copy","bwlimit":"100k"}"#.to_string()),
        ..Default::default()
    };

    let output = processor.process(&input, &ctx).await.unwrap();
    assert_eq!(output.succeeded_inputs.len(), 1);
    assert!(dest_dir.join("src.bin").exists(), "file copied");

    drop(ctx);
    let mut snapshots = Vec::new();
    while let Ok(update) = progress_rx.try_recv() {
        assert_eq!(update.job_id, "live-job");
        snapshots.push(update.snapshot);
    }
    assert!(
        !snapshots.is_empty(),
        "no progress snapshots parsed from live rclone stderr"
    );
    let last = snapshots.last().unwrap();
    assert_eq!(last.bytes_total, Some(payload.len() as u64));
    assert!(last.percent.is_some(), "percent computed from stats");

    // Stats lines must be consumed as progress, never surface as logs.
    while let Ok(entry) = log_rx.try_recv() {
        assert!(
            !entry.message.contains("\"stats\""),
            "raw stats JSON leaked into job logs: {}",
            entry.message
        );
    }
}

#[test]
fn test_batch_remote_path() {
    let base = Path::new("/home/user/videos");
    // Flat file directly under the base.
    assert_eq!(
        RcloneProcessor::batch_remote_path(base, "remote:bucket/dest", "/home/user/videos/a.mp4"),
        Some("remote:bucket/dest/a.mp4".to_string())
    );
    // Nested file keeps its base-relative subpath.
    assert_eq!(
        RcloneProcessor::batch_remote_path(
            base,
            "remote:bucket/dest/",
            "/home/user/videos/2024/b.mp4"
        ),
        Some("remote:bucket/dest/2024/b.mp4".to_string())
    );
    // Input outside the base dir cannot be mapped.
    assert_eq!(
        RcloneProcessor::batch_remote_path(base, "remote:bucket", "/var/other/c.mp4"),
        None
    );
}

#[test]
fn test_find_common_base_dir_nested() {
    let paths = vec![
        "/home/user/videos/2024/file1.mp4".to_string(),
        "/home/user/videos/2023/file2.mp4".to_string(),
    ];
    let result = RcloneProcessor::find_common_base_dir(&paths);
    assert_eq!(result, Some(PathBuf::from("/home/user/videos")));
}

#[test]
fn test_find_common_base_dir_no_common() {
    let paths = vec![
        "/home/user1/file1.mp4".to_string(),
        "/var/data/file2.mp4".to_string(),
    ];
    let result = RcloneProcessor::find_common_base_dir(&paths);
    // Should return "/" as the common base on Unix
    assert!(result.is_some());
}

/// Connection-string parameters hold access keys and passwords; logs show
/// the remote name and path only.
#[test]
fn loggable_destination_elides_connection_string_parameters() {
    assert_eq!(
        RcloneProcessor::loggable_destination(
            ":s3,access_key_id=AKIA,secret_access_key=hunter2:bucket/records/a.mp4"
        ),
        ":s3,…:bucket/records/a.mp4"
    );
    assert_eq!(
        RcloneProcessor::loggable_destination("drive,pass=s3cret,token=abc:/videos"),
        "drive,…:/videos"
    );
    assert_eq!(
        RcloneProcessor::loggable_destination("gdrive:/videos/{streamer}"),
        "gdrive:/videos/{streamer}"
    );
    assert_eq!(
        RcloneProcessor::loggable_destination("remote:/a,b/c.mp4"),
        "remote:/a,b/c.mp4"
    );
    assert_eq!(
        RcloneProcessor::loggable_destination(r"C:\records\a.mp4"),
        r"C:\records\a.mp4"
    );
    assert_eq!(
        RcloneProcessor::loggable_destination("/local/records"),
        "/local/records"
    );
    // A local path is never read as a connection string.
    assert_eq!(
        RcloneProcessor::loggable_destination("/local/a,b/c:d.mp4"),
        "/local/a,b/c:d.mp4"
    );
    // Secrets holding `/` or quoted `:` are still elided.
    assert_eq!(
        RcloneProcessor::loggable_destination(":s3,secret_access_key=a/b:bucket/a.mp4"),
        ":s3,…:bucket/a.mp4"
    );
    assert_eq!(
        RcloneProcessor::loggable_destination("drive,pass='x:y':videos"),
        "drive,…:videos"
    );
}

#[test]
fn remote_destination_fills_every_identity_token() {
    let config: RcloneConfig = serde_json::from_str(
        r#"{"destination_root": "remote:/{platform}/{streamer_id}/{session_id}/{streamer}/{title}"}"#,
    )
    .unwrap();
    let input = ProcessorInput {
        streamer_id: "streamer-1".to_string(),
        session_id: "session-2".to_string(),
        streamer_name: Some("Name?".to_string()),
        session_title: Some("Title".to_string()),
        platform: Some("huya".to_string()),
        ..Default::default()
    };
    assert_eq!(
        RcloneProcessor::determine_remote_destination(&input, &config),
        "remote:/huya/streamer-1/session-2/Name_/Title"
    );

    let without_platform = ProcessorInput {
        platform: None,
        ..input
    };
    assert_eq!(
        RcloneProcessor::determine_remote_destination(&without_platform, &config),
        "remote://streamer-1/session-2/Name_/Title"
    );
}

#[test]
fn test_session_start_anchor_groups_jobs_that_cross_dates() {
    let session_start = utc_datetime(2024, 1, 1, 12, 0, 0);
    let first_created_at = utc_datetime(2024, 1, 2, 12, 0, 0);
    let second_created_at = utc_datetime(2024, 1, 3, 12, 0, 0);
    let base_input = ProcessorInput {
        inputs: vec!["/input.mp4".to_string()],
        outputs: vec![],
        streamer_id: "123".to_string(),
        session_id: "456".to_string(),
        streamer_name: Some("StreamerName".to_string()),
        session_title: Some("Live Title".to_string()),
        platform: None,
        session_start: Some(session_start),
        config: None,
        created_at: first_created_at,
        manifest: None,
    };
    let session_config: RcloneConfig = serde_json::from_str(
        r#"{"destination_root": "remote:/%Y/%m/%d/{streamer}/", "time_anchor": "session_start"}"#,
    )
    .unwrap();
    let job_config: RcloneConfig =
        serde_json::from_str(r#"{"destination_root": "remote:/%Y/%m/%d/{streamer}/"}"#).unwrap();
    let later_input = ProcessorInput {
        created_at: second_created_at,
        ..base_input.clone()
    };

    let first_session_destination =
        RcloneProcessor::determine_remote_destination(&base_input, &session_config);
    let second_session_destination =
        RcloneProcessor::determine_remote_destination(&later_input, &session_config);
    assert_eq!(
        first_session_destination,
        expected_local_destination(session_start)
    );
    assert_eq!(second_session_destination, first_session_destination);

    let first_job_destination =
        RcloneProcessor::determine_remote_destination(&base_input, &job_config);
    let second_job_destination =
        RcloneProcessor::determine_remote_destination(&later_input, &job_config);
    assert_eq!(
        first_job_destination,
        expected_local_destination(first_created_at)
    );
    assert_eq!(
        second_job_destination,
        expected_local_destination(second_created_at)
    );
}

#[test]
fn throughput_args_skips_empty_strings() {
    // Form submissions can produce `Some("")` for cleared text inputs;
    // those should not turn into empty CLI values.
    let cfg = RcloneConfig {
        bwlimit: Some(String::new()),
        multi_thread_cutoff: Some(String::new()),
        ..Default::default()
    };
    assert!(cfg.throughput_args().is_empty());
}

#[test]
fn rclone_config_distinguishes_zero_tpslimit_from_unset() {
    let zero: RcloneConfig = serde_json::from_str(r#"{"tpslimit": 0}"#).unwrap();
    let unset: RcloneConfig = serde_json::from_str(r#"{}"#).unwrap();
    assert_eq!(zero.tpslimit, Some(0.0));
    assert_eq!(unset.tpslimit, None);
    assert_eq!(zero.throughput_args(), vec!["--tpslimit", "0"]);
    assert!(unset.throughput_args().is_empty());
}

#[test]
fn rclone_config_round_trips_all_throughput_fields() {
    let json = r#"{
        "bwlimit": "10M:100k",
        "bwlimit_file": "1M",
        "transfers": 4,
        "checkers": 8,
        "tpslimit": 2.5,
        "tpslimit_burst": 5,
        "multi_thread_streams": 2,
        "multi_thread_cutoff": "250M"
    }"#;
    let cfg: RcloneConfig = serde_json::from_str(json).unwrap();
    let expected: Vec<String> = [
        "--bwlimit",
        "10M:100k",
        "--bwlimit-file",
        "1M",
        "--transfers",
        "4",
        "--checkers",
        "8",
        "--tpslimit",
        "2.5",
        "--tpslimit-burst",
        "5",
        "--multi-thread-streams",
        "2",
        "--multi-thread-cutoff",
        "250M",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(cfg.throughput_args(), expected);
}

#[tokio::test(start_paused = true)]
async fn step_retry_defers_rclone_retries_to_workflow_policy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file = temp_dir.path().join("clip.mp4");
    tokio::fs::write(&file, b"video").await.unwrap();
    let file_str = file.to_string_lossy().into_owned();

    // Transfer runs only once when workflow step retry is active, despite max_retries=3.
    let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
        moved_inputs: vec![],
        succeeds: false,
    }]));
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let input = ProcessorInput {
        inputs: vec![file_str.clone()],
        config: Some(r#"{"destination_root":"remote:/videos","max_retries":3}"#.to_string()),
        ..Default::default()
    };
    let ctx = ProcessorContext::noop("step-retry-transfer").with_step_retry(true);

    let err = processor.process(&input, &ctx).await.unwrap_err();
    assert!(!err.is_non_retryable_step_error());
    assert_eq!(runner.commands().len(), 1);

    // Same for mkdir: fails after 1 attempt instead of sleeping under the dir lock.
    let mkdir_runner = Arc::new(
        MockRcloneCommandRunner::new(vec![]).with_mkdir_outcomes(vec![MkdirOutcome::Fail]),
    );
    let mkdir_processor = RcloneProcessor::with_command_runner("rclone", mkdir_runner.clone());
    let err = mkdir_processor
        .process(
            &input,
            &ProcessorContext::noop("step-retry-mkdir").with_step_retry(true),
        )
        .await
        .unwrap_err();
    assert!(!err.is_non_retryable_step_error());
    assert_eq!(mkdir_runner.mkdirs().len(), 1);
    assert!(mkdir_runner.commands().is_empty());
}

#[tokio::test(start_paused = true)]
async fn rate_limit_and_permanent_failures_stop_in_run_retries_with_distinct_retryability() {
    use crate::pipeline::job_queue::JobLogEntry;

    struct ScriptedRunner {
        outputs: Mutex<VecDeque<(Vec<PathBuf>, CommandOutput)>>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl RcloneCommandRunner for ScriptedRunner {
        async fn run(
            &self,
            command: &mut Command,
            _context: &ProcessorContext,
        ) -> Result<CommandOutput> {
            let args: Vec<String> = command
                .as_std()
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            if args.iter().any(|a| a == "mkdir") {
                return Ok(CommandOutput {
                    status: test_exit_code(0),
                    duration: 0.0,
                    logs: Vec::new(),
                    fatal_error: false,
                    last_stats_error: None,
                });
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (moved, output) = self
                .outputs
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra rclone attempt");
            for path in moved {
                tokio::fs::remove_file(&path).await.unwrap();
            }
            Ok(output)
        }
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let file_a = temp_dir.path().join("a.mp4");
    let file_b = temp_dir.path().join("b.mp4");
    tokio::fs::write(&file_a, b"a").await.unwrap();
    tokio::fs::write(&file_b, b"b").await.unwrap();
    let file_a_str = file_a.to_string_lossy().into_owned();
    let file_b_str = file_b.to_string_lossy().into_owned();

    // 1. Transient NewFs network errors stay retryable (not misclassified as permanent).
    let runner = Arc::new(ScriptedRunner {
        outputs: Mutex::new(VecDeque::from([
            (
                vec![],
                CommandOutput {
                    status: test_exit_code(1),
                    duration: 0.0,
                    logs: vec![JobLogEntry::error(
                        "Failed to create file system for \"remote:/videos\": dial tcp: i/o timeout",
                    )],
                    fatal_error: false,
                    last_stats_error: None,
                },
            ),
            (
                vec![],
                CommandOutput {
                    status: test_exit_code(0),
                    duration: 0.0,
                    logs: Vec::new(),
                    fatal_error: false,
                    last_stats_error: None,
                },
            ),
        ])),
        calls: AtomicUsize::new(0),
    });
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let input = ProcessorInput {
        inputs: vec![file_a_str.clone()],
        config: Some(r#"{"destination_root":"remote:/videos","max_retries":3}"#.to_string()),
        ..Default::default()
    };
    processor
        .process(&input, &ProcessorContext::noop("transient-newfs"))
        .await
        .expect("transient NewFs timeout should retry and succeed");
    assert_eq!(runner.calls.load(Ordering::SeqCst), 2);

    // 2. Rate-limit errors (including exit 7 + fatalError where the rate-limit detail only
    //    arrived via JSON stats.lastError alongside a generic Error line, or with no Error line)
    //    skip remaining in-run retries while staying retryable for the DAG.
    for (exit_code, fatal_error, logs, last_stats_error, expected_in_err) in [
        (
            1,
            false,
            vec![JobLogEntry::error(
                "googleapi: Error 403: Quota exceeded, rateLimitExceeded",
            )],
            None,
            "rateLimitExceeded",
        ),
        (
            7,
            true,
            vec![JobLogEntry::error("context canceled")],
            Some("googleapi: Error 403: User rate limit exceeded, uploadLimitExceeded"),
            "context canceled",
        ),
        (
            7,
            true,
            vec![],
            Some("googleapi: Error 403: User rate limit exceeded, uploadLimitExceeded"),
            "uploadLimitExceeded",
        ),
    ] {
        let runner = Arc::new(ScriptedRunner {
            outputs: Mutex::new(VecDeque::from([(
                vec![],
                CommandOutput {
                    status: test_exit_code(exit_code),
                    duration: 0.0,
                    logs,
                    fatal_error,
                    last_stats_error: last_stats_error.map(str::to_owned),
                },
            )])),
            calls: AtomicUsize::new(0),
        });
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let err = processor
            .process(&input, &ProcessorContext::noop("rate-limit"))
            .await
            .unwrap_err();
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
        assert!(
            !err.is_non_retryable_step_error(),
            "rate-limit error must stay retryable for workflow backoff: {err}"
        );
        assert!(
            err.to_string().contains(expected_in_err),
            "expected {expected_in_err:?} in {err}"
        );
    }

    // 3. Permanent errors (exit 2 usage error, exit 7 auth failure, missing config section)
    //    stop immediately and return NonRetryablePipeline.
    for (exit_code, fatal_error, msg) in [
        (
            2,
            false,
            "invalid argument \"bogus\" for \"--bwlimit\" flag",
        ),
        (7, true, "oauth2: cannot fetch token: invalid_grant"),
        (1, false, "didn't find section in config file (\"missing\")"),
    ] {
        let runner = Arc::new(ScriptedRunner {
            outputs: Mutex::new(VecDeque::from([(
                vec![],
                CommandOutput {
                    status: test_exit_code(exit_code),
                    duration: 0.0,
                    logs: vec![JobLogEntry::error(msg)],
                    fatal_error,
                    last_stats_error: None,
                },
            )])),
            calls: AtomicUsize::new(0),
        });
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let err = processor
            .process(&input, &ProcessorContext::noop("permanent"))
            .await
            .unwrap_err();
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1, "{msg}");
        assert!(
            matches!(err, crate::Error::NonRetryablePipeline(_)),
            "expected NonRetryablePipeline, got {err:?}"
        );
    }

    // 4. Partial move batch with a permanent error preserves per-file results and flags non_retryable.
    let runner = Arc::new(ScriptedRunner {
        outputs: Mutex::new(VecDeque::from([(
            vec![file_a.clone()],
            CommandOutput {
                status: test_exit_code(7),
                duration: 0.0,
                logs: vec![JobLogEntry::error(
                    "oauth2: token has been expired or revoked",
                )],
                fatal_error: true,
                last_stats_error: None,
            },
        )])),
        calls: AtomicUsize::new(0),
    });
    let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
    let input = ProcessorInput {
        inputs: vec![file_a_str.clone(), file_b_str.clone()],
        config: Some(
            r#"{"destination_root":"remote:/videos","operation":"move","max_retries":3}"#
                .to_string(),
        ),
        ..Default::default()
    };
    let output = processor
        .process(&input, &ProcessorContext::noop("partial-permanent"))
        .await
        .unwrap();
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    assert!(output.non_retryable);
    assert_eq!(output.succeeded_inputs, vec![file_a_str]);
    assert_eq!(output.failed_inputs.len(), 1);
    assert_eq!(output.failed_inputs[0].0, file_b_str);
}
