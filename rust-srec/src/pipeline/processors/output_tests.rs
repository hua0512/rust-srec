mod driver_contracts;

use std::path::Path;
use std::time::Duration;

use super::{
    AssBurnInProcessor, AudioExtractProcessor, MetadataProcessor, Processor, ProcessorContext,
    ProcessorInput, ThumbnailProcessor,
};

#[derive(Clone, Copy, Debug)]
enum Kind {
    Thumbnail,
    Audio,
    Metadata,
    BurnIn,
}

const KINDS: [Kind; 4] = [Kind::Thumbnail, Kind::Audio, Kind::Metadata, Kind::BurnIn];

#[derive(Clone, Copy)]
enum Behavior {
    Missing,
    Empty,
    Failure,
    Success,
    BatchFailure,
    Hold,
}

fn fake_ffmpeg(dir: &Path, behavior: Behavior) -> String {
    #[cfg(windows)]
    let (name, script) = {
        let action = match behavior {
            Behavior::Missing => "exit /b 0\r\n",
            Behavior::Empty => "type nul > \"%output%\"\r\nexit /b 0\r\n",
            Behavior::Failure => "echo partial> \"%output%\"\r\nexit /b 7\r\n",
            Behavior::Success => "echo complete> \"%output%\"\r\nexit /b 0\r\n",
            Behavior::BatchFailure => {
                "echo partial> \"%output%\"\r\nif not \"%output:fail=%\"==\"%output%\" exit /b 7\r\nexit /b 0\r\n"
            }
            Behavior::Hold => {
                "echo partial> \"%output%\"\r\npowershell.exe -WindowStyle Hidden -NoProfile -NonInteractive -Command \"Start-Sleep -Seconds 10\"\r\nexit /b 0\r\n"
            }
        };
        (
            "ffmpeg.cmd",
            format!(
                "@echo off\r\nsetlocal\r\n:args\r\nif \"%~1\"==\"\" goto run\r\nset \"output=%~1\"\r\nshift\r\ngoto args\r\n:run\r\n{action}"
            ),
        )
    };
    #[cfg(not(windows))]
    let (name, script) = {
        let action = match behavior {
            Behavior::Missing => "exit 0\n",
            Behavior::Empty => ": > \"$output\"\nexit 0\n",
            Behavior::Failure => "printf partial > \"$output\"\nexit 7\n",
            Behavior::Success => "printf complete > \"$output\"\nexit 0\n",
            Behavior::BatchFailure => {
                "printf partial > \"$output\"\ncase \"$output\" in *fail*) exit 7;; esac\nexit 0\n"
            }
            Behavior::Hold => "printf partial > \"$output\"\nsleep 10\nexit 0\n",
        };
        (
            "ffmpeg.sh",
            format!("#!/bin/sh\nfor output do :; done\n{action}"),
        )
    };
    let path = dir.join(name);
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    path.to_string_lossy().into_owned()
}

struct Fixture {
    dir: tempfile::TempDir,
    processor: Box<dyn Processor>,
    input: ProcessorInput,
}

fn fixture(kind: Kind, behavior: Behavior, batch: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_ffmpeg(dir.path(), behavior);
    let processor: Box<dyn Processor> = match kind {
        Kind::Thumbnail => Box::new(ThumbnailProcessor::with_ffmpeg_path(&binary)),
        Kind::Audio => Box::new(AudioExtractProcessor::with_ffmpeg_path(&binary)),
        Kind::Metadata => Box::new(MetadataProcessor::with_ffmpeg_path(&binary)),
        Kind::BurnIn => Box::new(AssBurnInProcessor::new()),
    };
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for name in if batch {
        &["good", "fail"][..]
    } else {
        &["good"][..]
    } {
        let video = dir.path().join(format!("{name}-input.mp4"));
        std::fs::write(&video, b"source").unwrap();
        inputs.push(video.to_string_lossy().into_owned());
        if matches!(kind, Kind::BurnIn) {
            let ass = video.with_extension("ass");
            std::fs::write(&ass, b"source").unwrap();
            inputs.push(ass.to_string_lossy().into_owned());
        }
        let extension = match kind {
            Kind::Thumbnail => "jpg",
            Kind::Audio => "aac",
            _ => "mp4",
        };
        outputs.push(
            dir.path()
                .join(format!("{name}-output.{extension}"))
                .to_string_lossy()
                .into_owned(),
        );
    }
    let config = serde_json::json!({
        "ffmpeg_path": binary, "match_strategy": "stem", "passthrough_inputs": false,
        "remove_input_on_success": true, "delete_source_videos_on_success": true, "delete_source_ass_on_success": true,
    }).to_string();
    Fixture {
        dir,
        processor,
        input: ProcessorInput {
            inputs,
            outputs,
            config: Some(config),
            ..Default::default()
        },
    }
}

fn has_temps(dir: &Path) -> bool {
    std::fs::read_dir(dir).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")
    })
}

async fn wait_for_cleanup(dir: &Path) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while has_temps(dir) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("temporary outputs must be removed");
}

fn assert_sources_preserved(fixture: &Fixture) {
    for input in &fixture.input.inputs {
        assert_eq!(std::fs::read(input).unwrap(), b"source");
    }
}

#[tokio::test]
async fn processors_reject_missing_empty_and_failed_outputs_without_replacing_files() {
    for kind in KINDS {
        for behavior in [Behavior::Missing, Behavior::Empty, Behavior::Failure] {
            let fixture = fixture(kind, behavior, false);
            std::fs::write(&fixture.input.outputs[0], b"existing").unwrap();
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                fixture
                    .processor
                    .process(&fixture.input, &ProcessorContext::noop("test")),
            )
            .await
            .unwrap();
            assert!(result.is_err(), "{kind:?} must reject incomplete output");
            assert_sources_preserved(&fixture);
            assert_eq!(
                std::fs::read(&fixture.input.outputs[0]).unwrap(),
                b"existing"
            );
            wait_for_cleanup(fixture.dir.path()).await;
        }
    }
}

#[tokio::test]
async fn processors_publish_complete_single_and_batch_outputs() {
    for kind in KINDS {
        for batch in [false, true] {
            let fixture = fixture(kind, Behavior::Success, batch);
            let output = tokio::time::timeout(
                Duration::from_secs(10),
                fixture
                    .processor
                    .process(&fixture.input, &ProcessorContext::noop("test")),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(output.items_produced, fixture.input.outputs, "{kind:?}");
            for output in &fixture.input.outputs {
                assert_eq!(std::fs::read_to_string(output).unwrap().trim(), "complete");
            }
            wait_for_cleanup(fixture.dir.path()).await;
        }
    }
}

#[tokio::test]
async fn processor_batch_failure_preserves_sources_and_existing_destinations() {
    for kind in KINDS {
        let fixture = fixture(kind, Behavior::BatchFailure, true);
        for output in &fixture.input.outputs {
            std::fs::write(output, b"existing").unwrap();
        }
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            fixture
                .processor
                .process(&fixture.input, &ProcessorContext::noop("test")),
        )
        .await
        .unwrap();
        assert!(result.is_err(), "{kind:?} second input must fail");
        assert_sources_preserved(&fixture);
        for output in &fixture.input.outputs {
            assert_eq!(std::fs::read(output).unwrap(), b"existing");
        }
        wait_for_cleanup(fixture.dir.path()).await;
    }
}

#[tokio::test]
async fn processors_reject_output_aliases_of_any_batch_source() {
    for kind in KINDS {
        let mut fixture = fixture(kind, Behavior::Success, true);
        let alias = fixture.dir.path().join("alias.mp4");
        let later_video = fixture
            .input
            .inputs
            .iter()
            .rev()
            .find(|path| path.ends_with(".mp4"))
            .unwrap();
        std::fs::hard_link(later_video, &alias).unwrap();
        fixture.input.outputs[0] = alias.to_string_lossy().into_owned();
        assert!(
            fixture
                .processor
                .process(&fixture.input, &ProcessorContext::noop("test"))
                .await
                .is_err(),
            "{kind:?} must protect later inputs"
        );
        assert_sources_preserved(&fixture);
        assert_eq!(std::fs::read(alias).unwrap(), b"source");
        wait_for_cleanup(fixture.dir.path()).await;
    }
}

#[tokio::test]
async fn processor_cancellation_and_timeout_remove_partial_outputs() {
    for kind in KINDS {
        for timeout in [false, true] {
            let fixture = fixture(kind, Behavior::Hold, false);
            std::fs::write(&fixture.input.outputs[0], b"existing").unwrap();
            let ctx = ProcessorContext::noop("test");
            let mut work = Box::pin(fixture.processor.process(&fixture.input, &ctx));
            tokio::select! {
                result = &mut work => panic!("{kind:?} command must remain running: {result:?}"),
                () = async {
                    tokio::time::timeout(Duration::from_secs(5), async {
                        while !has_temps(fixture.dir.path()) { tokio::time::sleep(Duration::from_millis(20)).await; }
                    }).await.expect("processor must start writing its temporary output");
                } => {},
            }
            if timeout {
                assert!(
                    tokio::time::timeout(Duration::from_millis(30), work)
                        .await
                        .is_err()
                );
            } else {
                drop(work);
            }
            wait_for_cleanup(fixture.dir.path()).await;
            assert_sources_preserved(&fixture);
            assert_eq!(
                std::fs::read(&fixture.input.outputs[0]).unwrap(),
                b"existing"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires an installed FFmpeg binary"]
async fn thumbnail_seek_beyond_short_recording_does_not_publish_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("short.mp4");
    let output = dir.path().join("short.jpg");
    let mut command = tokio::process::Command::new("ffmpeg");
    command
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=32x32:r=1:d=1",
            "-c:v",
            "mpeg4",
        ])
        .arg(&input);
    let generated = tokio::time::timeout(
        Duration::from_secs(10),
        super::utils::run_command_with_logs(&mut command, None),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(generated.status.success());
    let processor = ThumbnailProcessor::with_ffmpeg_path("ffmpeg");
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        processor.process(
            &ProcessorInput {
                inputs: vec![input.to_string_lossy().into_owned()],
                outputs: vec![output.to_string_lossy().into_owned()],
                config: Some(r#"{"timestamp_secs":10}"#.to_owned()),
                ..Default::default()
            },
            &ProcessorContext::noop("test"),
        ),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    assert!(!output.exists());
    assert!(input.exists());
    wait_for_cleanup(dir.path()).await;
}
