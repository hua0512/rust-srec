use super::*;
use crate::pipeline::processors::{CompressionProcessor, DanmakuFactoryProcessor, RemuxProcessor};

fn script(dir: &Path, name: &str, windows: &str, unix: &str) -> String {
    #[cfg(windows)]
    let (name, text) = (format!("{name}.cmd"), format!("@echo off\r\n{windows}"));
    #[cfg(not(windows))]
    let (name, text) = (format!("{name}.sh"), format!("#!/bin/sh\n{unix}"));
    let _ = (windows, unix);
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    path.to_string_lossy().into_owned()
}

async fn assert_skip(
    processor: &dyn Processor,
    path: &Path,
    reason: &str,
    json_reason: &str,
    command_logs: bool,
) {
    std::fs::write(path, b"input").unwrap();
    let input = path.to_string_lossy().into_owned();
    let result = processor
        .process(
            &ProcessorInput {
                inputs: vec![input.clone()],
                ..Default::default()
            },
            &ProcessorContext::noop("skip-contract"),
        )
        .await
        .unwrap();
    assert_eq!(result.outputs.as_slice(), std::slice::from_ref(&input));
    assert_eq!(result.skipped_inputs, [(input.clone(), reason.to_owned())]);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(result.metadata.as_deref().unwrap()).unwrap(),
        serde_json::json!({"status":"skipped","reason":json_reason,"input":input})
    );
    assert!(result.items_produced.is_empty());
    assert!(result.succeeded_inputs.is_empty());
    assert!(result.failed_inputs.is_empty());
    assert!(result.input_size_bytes.is_none());
    assert!(result.output_size_bytes.is_none());
    assert_eq!(!result.logs.is_empty(), command_logs);
}

#[tokio::test]
async fn processor_driver_child() {
    let Ok(case) = std::env::var("SREC_PROCESSOR_DRIVER_CASE") else {
        return;
    };
    tokio::time::timeout(Duration::from_secs(25), async {
        let dir = tempfile::tempdir().unwrap();
        let error_binary = script(
            dir.path(),
            "no-stream",
            "echo Error: Output file does not contain any stream 1>&2\r\nexit /b 1\r\n",
            "echo 'Error: Output file does not contain any stream' >&2\nexit 1\n",
        );
        match case.as_str() {
            "early-skips" => {
                let audio = AudioExtractProcessor::with_ffmpeg_path(&error_binary);
                assert_skip(
                    &audio,
                    &dir.path().join("image.png"),
                    "input is an image, no audio to extract",
                    "already_image",
                    false,
                )
                .await;
                assert_skip(
                    &audio,
                    &dir.path().join("text.txt"),
                    "not a supported media format for audio extraction",
                    "unsupported_media_format",
                    false,
                )
                .await;
                assert_skip(
                    &audio,
                    &dir.path().join("silent.mp4"),
                    "input file contains no audio stream",
                    "no_audio_stream",
                    false,
                )
                .await;
                let thumbnail = ThumbnailProcessor::with_ffmpeg_path(&error_binary);
                assert_skip(
                    &thumbnail,
                    &dir.path().join("photo.jpg"),
                    "input is already an image",
                    "already_image",
                    false,
                )
                .await;
                assert_skip(
                    &thumbnail,
                    &dir.path().join("plain.txt"),
                    "not a supported video format for thumbnail extraction",
                    "unsupported_video_format",
                    false,
                )
                .await;
                assert_skip(
                    &MetadataProcessor::with_ffmpeg_path(&error_binary),
                    &dir.path().join("meta.txt"),
                    "format does not support metadata embedding",
                    "unsupported_format",
                    false,
                )
                .await;
                assert_skip(
                    &RemuxProcessor::with_ffmpeg_path(&error_binary),
                    &dir.path().join("remux.txt"),
                    "not a supported media format for remuxing",
                    "unsupported_media_format",
                    false,
                )
                .await;
            }
            "command-skips" => {
                assert_skip(
                    &AudioExtractProcessor::with_ffmpeg_path(&error_binary),
                    &dir.path().join("audio.mp4"),
                    "input file contains no audio stream",
                    "no_audio_stream",
                    true,
                )
                .await;
                assert_skip(
                    &ThumbnailProcessor::with_ffmpeg_path(&error_binary),
                    &dir.path().join("video.mp4"),
                    "no extractable video frames",
                    "no_video_frames",
                    true,
                )
                .await;
            }
            "media" => {
                for kind in [Kind::Audio, Kind::Metadata, Kind::Thumbnail] {
                    let mut fixture = super::fixture(kind, Behavior::Success, true);
                    let skipped = fixture
                        .dir
                        .path()
                        .join("skip.txt")
                        .to_string_lossy()
                        .into_owned();
                    std::fs::write(&skipped, b"passthrough").unwrap();
                    fixture.input.inputs.insert(1, skipped.clone());
                    fixture.input.outputs.insert(
                        1,
                        fixture
                            .dir
                            .path()
                            .join("unused-output")
                            .to_string_lossy()
                            .into_owned(),
                    );
                    let result = fixture
                        .processor
                        .process(&fixture.input, &ProcessorContext::noop("mixed-driver"))
                        .await
                        .unwrap();
                    assert_eq!(
                        result.outputs,
                        [
                            fixture.input.outputs[0].clone(),
                            skipped.clone(),
                            fixture.input.outputs[2].clone()
                        ]
                    );
                    assert_eq!(
                        result.succeeded_inputs,
                        [
                            fixture.input.inputs[0].clone(),
                            fixture.input.inputs[2].clone()
                        ]
                    );
                    assert_eq!(result.skipped_inputs[0].0, skipped);
                    assert!(Path::new(&skipped).exists());
                    assert!(!Path::new(&fixture.input.outputs[1]).exists());
                    assert!(result.input_size_bytes.is_none());
                    let metadata: serde_json::Value =
                        serde_json::from_str(result.metadata.as_deref().unwrap()).unwrap();
                    assert_eq!(metadata["batch"], true);
                    assert_eq!(metadata["inputs"], 3);
                    if matches!(kind, Kind::Metadata) {
                        assert_eq!(metadata["input_removed"], true);
                    }
                }
                for kind in [Kind::Audio, Kind::Metadata, Kind::Thumbnail] {
                    let mut fixture = super::fixture(kind, Behavior::Success, false);
                    let extra = fixture
                        .dir
                        .path()
                        .join("unused-extra.mp4")
                        .to_string_lossy()
                        .into_owned();
                    fixture.input.outputs.push(extra.clone());
                    let result = fixture
                        .processor
                        .process(&fixture.input, &ProcessorContext::noop("unary-overrides"))
                        .await
                        .unwrap();
                    assert_eq!(result.outputs, [fixture.input.outputs[0].clone()]);
                    assert!(!Path::new(&extra).exists());
                    assert_ne!(
                        serde_json::from_str::<serde_json::Value>(
                            result.metadata.as_deref().unwrap()
                        )
                        .unwrap()["batch"],
                        true
                    );
                }
            }
            "remux" => {
                for failure in [false, true] {
                    let mut fixture = super::fixture(
                        Kind::Metadata,
                        if failure {
                            Behavior::BatchFailure
                        } else {
                            Behavior::Success
                        },
                        true,
                    );
                    let binary = serde_json::from_str::<serde_json::Value>(
                        fixture.input.config.as_deref().unwrap(),
                    )
                    .unwrap()["ffmpeg_path"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    fixture.processor = Box::new(RemuxProcessor::with_ffmpeg_path(binary));
                    let skipped = fixture
                        .dir
                        .path()
                        .join("keep.txt")
                        .to_string_lossy()
                        .into_owned();
                    std::fs::write(&skipped, b"passthrough").unwrap();
                    fixture.input.inputs.insert(1, skipped.clone());
                    fixture.input.outputs.insert(
                        1,
                        fixture
                            .dir
                            .path()
                            .join("unused.mp4")
                            .to_string_lossy()
                            .into_owned(),
                    );
                    for output in &fixture.input.outputs {
                        std::fs::write(output, b"existing").unwrap();
                    }
                    let result = fixture
                        .processor
                        .process(
                            &fixture.input,
                            &ProcessorContext::noop("incremental-driver"),
                        )
                        .await;
                    if failure {
                        assert!(result.is_err());
                        assert!(
                            !Path::new(&fixture.input.outputs[0]).exists(),
                            "remux explicit-error cleanup removes the earlier publication"
                        );
                        assert_eq!(
                            std::fs::read(&fixture.input.outputs[2]).unwrap(),
                            b"existing"
                        );
                        for source in &fixture.input.inputs {
                            assert!(Path::new(source).exists());
                        }
                    } else {
                        let result = result.unwrap();
                        assert_eq!(
                            result.outputs,
                            [
                                fixture.input.outputs[0].clone(),
                                skipped.clone(),
                                fixture.input.outputs[2].clone()
                            ]
                        );
                        assert!(!Path::new(&fixture.input.inputs[0]).exists());
                        assert!(!Path::new(&fixture.input.inputs[2]).exists());
                        assert!(Path::new(&skipped).exists());
                    }
                    wait_for_cleanup(fixture.dir.path()).await;
                }
            }
            "empty-overrides" => {
                for (kind, suffix) in [(Kind::Audio, "good-input_audio.aac"), (Kind::Metadata, "good-input_meta.mp4"), (Kind::Thumbnail, "good-input.jpg")] {
                    let mut fixture = super::fixture(kind, Behavior::Success, false);
                    fixture.input.outputs = vec![String::new()];
                    let result = fixture.processor.process(&fixture.input, &ProcessorContext::noop("generated-name")).await.unwrap();
                    assert!(result.outputs[0].ends_with(suffix));
                    assert!(Path::new(&result.outputs[0]).exists());
                    if !matches!(kind, Kind::Thumbnail) {
                        let mut fixture = super::fixture(kind, Behavior::Success, false);
                        let configured = fixture.dir.path().join("configured-output.mp4").to_string_lossy().into_owned();
                        let mut config: serde_json::Value = serde_json::from_str(fixture.input.config.as_deref().unwrap()).unwrap();
                        config["output_path"] = serde_json::json!(configured);
                        fixture.input.config = Some(config.to_string());
                        let result = fixture.processor.process(&fixture.input, &ProcessorContext::noop("configured-name")).await.unwrap();
                        assert_eq!(result.outputs, [configured]);
                        assert!(!Path::new(&fixture.input.outputs[0]).exists());
                    }
                }
                let mut fixture = super::fixture(Kind::Metadata, Behavior::Success, false);
                let binary = serde_json::from_str::<serde_json::Value>(fixture.input.config.as_deref().unwrap()).unwrap()["ffmpeg_path"].as_str().unwrap().to_owned();
                fixture.input.outputs = vec![String::new(), "ignored-extra".into()];
                let result = RemuxProcessor::with_ffmpeg_path(binary).process(&fixture.input, &ProcessorContext::noop("remux-empty")).await.unwrap();
                assert!(result.outputs[0].ends_with("good-input_remux.mp4"));
            }
            "remux-cancel" => {
                let binary = script(dir.path(), "batch-hold", "setlocal\r\n:args\r\nif \"%~1\"==\"\" goto run\r\nset \"output=%~1\"\r\nshift\r\ngoto args\r\n:run\r\necho complete> \"%output%\"\r\nif not \"%output:second=%\"==\"%output%\" powershell.exe -WindowStyle Hidden -NoProfile -NonInteractive -Command \"Start-Sleep -Seconds 10\"\r\nexit /b 0\r\n", "for output do :; done\nprintf complete > \"$output\"\ncase \"$output\" in *second*) sleep 10;; esac\nexit 0\n");
                let inputs: Vec<String> = ["first-input.mp4", "second-input.mp4"].iter().map(|name| dir.path().join(name).to_string_lossy().into_owned()).collect();
                for input in &inputs { std::fs::write(input, b"source").unwrap(); }
                let outputs: Vec<String> = ["first.mp4", "second.mp4"].iter().map(|name| dir.path().join(name).to_string_lossy().into_owned()).collect();
                let input = ProcessorInput { inputs: inputs.clone(), outputs: outputs.clone(), config: Some(serde_json::json!({"remove_input_on_success":true}).to_string()), ..Default::default() };
                let processor = RemuxProcessor::with_ffmpeg_path(binary);
                let ctx = ProcessorContext::noop("remux-cancel-policy");
                let mut task = Box::pin(processor.process(&input, &ctx));
                tokio::select! {
                    result = &mut task => panic!("remux must still be processing its second item: {result:?}"),
                    () = async {
                        tokio::time::timeout(Duration::from_secs(5), async {
                            loop {
                                let second_started = std::fs::read_dir(dir.path()).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with("second.tmp-"));
                                if Path::new(&outputs[0]).exists() && second_started { break; }
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                        }).await.unwrap();
                    } => {}
                }
                drop(task);
                wait_for_cleanup(dir.path()).await;
                assert!(Path::new(&outputs[0]).exists(), "cancellation preserves the already-published remux output");
                assert!(!Path::new(&outputs[1]).exists());
                for source in inputs { assert!(Path::new(&source).exists()); }
            }
            "aliases" => {
                for kind in KINDS {
                    let mut fixture = super::fixture(kind, Behavior::Success, false);
                    let alias = fixture.dir.path().join("hardlink-output.mp4");
                    std::fs::hard_link(&fixture.input.inputs[0], &alias).unwrap();
                    fixture.input.outputs = vec![alias.to_string_lossy().into_owned()];
                    assert!(fixture.processor.process(&fixture.input, &ProcessorContext::noop("native-alias")).await.is_err());
                    assert_eq!(std::fs::read(&fixture.input.inputs[0]).unwrap(), b"source");
                    fixture.input.outputs = vec!["invalid\0destination".to_owned()];
                    assert!(fixture.processor.process(&fixture.input, &ProcessorContext::noop("identity-io-error")).await.is_err());
                }
                #[cfg(unix)]
                for kind in KINDS {
                    let mut fixture = super::fixture(kind, Behavior::Success, true);
                    let linked = fixture.dir.path().join("linked");
                    std::os::unix::fs::symlink(fixture.dir.path(), &linked).unwrap();
                    let output = fixture.dir.path().join("new-output.mp4");
                    fixture.input.outputs = vec![output.to_string_lossy().into_owned(), linked.join("new-output.mp4").to_string_lossy().into_owned()];
                    assert!(fixture.processor.process(&fixture.input, &ProcessorContext::noop("missing-leaf-alias")).await.is_err());
                    assert!(!output.exists());
                    wait_for_cleanup(fixture.dir.path()).await;
                }
            }
            other => panic!("unknown fixture case {other}"),
        }
    })
    .await
    .expect("processor fixture must finish");
}

#[tokio::test]
async fn actual_processors_preserve_skip_mapping_and_publication_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let empty_probe = script(dir.path(), "empty-probe", "exit /b 0\r\n", "exit 0\n");
    let failed_probe = script(dir.path(), "failed-probe", "exit /b 1\r\n", "exit 1\n");
    let audio_probe = script(
        dir.path(),
        "audio-probe",
        "echo audio\r\nexit /b 0\r\n",
        "echo audio\nexit 0\n",
    );
    for (case, probe) in [
        ("early-skips", &empty_probe),
        ("command-skips", &failed_probe),
        ("media", &audio_probe),
        ("remux", &audio_probe),
        ("empty-overrides", &audio_probe),
        ("remux-cancel", &audio_probe),
        ("aliases", &audio_probe),
    ] {
        let mut command = process_utils::tokio_command(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "pipeline::processors::output_tests::driver_contracts::processor_driver_child",
                "--nocapture",
            ])
            .env("SREC_PROCESSOR_DRIVER_CASE", case)
            .env("FFPROBE_PATH", probe)
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(30), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}

#[tokio::test]
async fn selected_artifacts_keep_strict_cardinality() {
    for xml in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let selected = dir.path().join(if xml { "one.xml" } else { "one.mp4" });
        let auxiliary = dir.path().join(if xml { "info.txt" } else { "one.ass" });
        std::fs::write(&selected, b"source").unwrap();
        std::fs::write(&auxiliary, b"source").unwrap();
        let processor: Box<dyn Processor> = if xml {
            Box::new(DanmakuFactoryProcessor::new())
        } else {
            Box::new(AssBurnInProcessor::new())
        };
        let input = ProcessorInput {
            inputs: vec![
                selected.to_string_lossy().into_owned(),
                auxiliary.to_string_lossy().into_owned(),
            ],
            outputs: vec!["one-output".into(), "extra-output".into()],
            ..Default::default()
        };
        let error = processor
            .process(&input, &ProcessorContext::noop("selected-count"))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("selected"));
        assert!(error.contains("outputs=2"));
    }
}

#[tokio::test]
async fn compression_precedence_remains_one_archive_for_multiple_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.txt");
    std::fs::write(&source, b"source").unwrap();
    let second_source = dir.path().join("second.txt");
    std::fs::write(&second_source, b"second source").unwrap();
    let configured = dir
        .path()
        .join("configured.zip")
        .to_string_lossy()
        .into_owned();
    let ignored = dir
        .path()
        .join("ignored.zip")
        .to_string_lossy()
        .into_owned();
    let input = ProcessorInput {
        inputs: vec![
            source.to_string_lossy().into_owned(),
            second_source.to_string_lossy().into_owned(),
        ],
        outputs: vec![ignored.clone()],
        config: Some(serde_json::json!({"output_path":configured,"format":"zip"}).to_string()),
        ..Default::default()
    };
    let result = CompressionProcessor::new()
        .process(&input, &ProcessorContext::noop("archive-precedence"))
        .await
        .unwrap();
    assert_eq!(result.outputs, [configured]);
    assert!(!Path::new(&ignored).exists());
}

#[tokio::test]
async fn zero_selected_inputs_passthrough_before_output_count_validation() {
    for xml in [false, true] {
        let processor: Box<dyn Processor> = if xml {
            Box::new(DanmakuFactoryProcessor::new())
        } else {
            Box::new(AssBurnInProcessor::new())
        };
        let inputs = vec!["notes.txt".to_owned(), "picture.png".to_owned()];
        let input = ProcessorInput {
            inputs: inputs.clone(),
            outputs: vec!["unused-output".to_owned()],
            config: Some(serde_json::json!({"passthrough_inputs":true}).to_string()),
            ..Default::default()
        };
        let result = processor
            .process(&input, &ProcessorContext::noop("empty-selection"))
            .await
            .unwrap();
        assert_eq!(result.outputs, inputs);
        let metadata: serde_json::Value =
            serde_json::from_str(result.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(
            metadata,
            serde_json::json!({"status":"skipped","reason":if xml {"no_danmu_xml_inputs"} else {"no_video_inputs"}})
        );
        assert!(result.skipped_inputs.is_empty());
    }
}
