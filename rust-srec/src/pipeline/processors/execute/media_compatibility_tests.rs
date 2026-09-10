//! Native positive compatibility fixtures, separate from the archived blocked
//! adversarial experiments. Select these tests by exact name after source review.
//! Input values are real fixture paths and their ordinary JSON serialization.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    ExecuteCommandProcessor, Processor, ProcessorContext, ProcessorInput, SHELL, ShellKind,
};
use crate::pipeline::processors::utils::run_command_with_logs;

const BUILD_TIMEOUT: Duration = Duration::from_secs(30);
const PROCESS_TIMEOUT: Duration = Duration::from_secs(10);
const ARGUMENT_TEMPLATES: &[&str] = &[
    "{input}",
    "{input1}",
    "{input2}",
    "{input3}",
    "{input4}",
    "{inputs_json}",
    "media={input1}",
    "{outputs_json}",
];

struct Fixture {
    directory: tempfile::TempDir,
    media: Vec<(PathBuf, Vec<u8>)>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("execute-media-compat-")
            .tempdir()
            .unwrap();
        let media_directory = directory.path().join("media files (素材)");
        tokio::fs::create_dir(&media_directory).await.unwrap();
        let mut media = Vec::new();
        for (index, name) in [
            "Recording 01.wav",
            "演奏 (take 2).wav",
            "Progress 50%.wav",
            "Encore!.wav",
            "R&B recording.wav",
        ]
        .into_iter()
        .enumerate()
        {
            let path = media_directory.join(name);
            let bytes = tiny_wave(index as i16 + 1);
            tokio::fs::write(&path, &bytes).await.unwrap();
            media.push((path, bytes));
        }
        Self { directory, media }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn input(&self, output: &Path) -> ProcessorInput {
        ProcessorInput {
            inputs: self.media.iter().map(|(path, _)| path_text(path)).collect(),
            outputs: vec![path_text(output)],
            ..Default::default()
        }
    }

    async fn helper(&self, name: &str) -> PathBuf {
        assert!(matches!(name, "media_capture_helper" | "media_copy_helper"));
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/pipeline/processors/execute")
            .join(format!("{name}.rs"));
        let executable = self
            .root()
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        // The compiler and any linker descendants use the same contained,
        // hidden process utility as processor children. No shell builds helpers.
        let mut command = process_utils::tokio_command("rustc");
        command
            .args(["--edition=2024", "--crate-name", name])
            .arg(&source)
            .arg("-o")
            .arg(&executable);
        let result = tokio::time::timeout(BUILD_TIMEOUT, run_command_with_logs(&mut command, None))
            .await
            .expect("building the std-only fixture helper must be bounded")
            .unwrap();
        assert!(
            result.status.success(),
            "helper build failed: {:?}",
            result.logs
        );
        executable
    }

    async fn run(&self, input: &ProcessorInput) {
        let processor = ExecuteCommandProcessor::new().with_timeout(5);
        let context = ProcessorContext::noop("ordinary-media-compatibility");
        let result = tokio::time::timeout(PROCESS_TIMEOUT, processor.process(input, &context))
            .await
            .expect("ordinary-media compatibility process must be bounded")
            .unwrap();
        assert_eq!(result.outputs, input.outputs);
    }
}

fn path_text(path: &Path) -> String {
    path.to_str()
        .expect("fixture paths use Unicode filenames")
        .to_owned()
}

/// Four mono PCM samples with a complete RIFF/WAVE header. Each fixture has a
/// different sample value, so byte equality also identifies the selected input.
fn tiny_wave(sample: i16) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&44_u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1_u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&8000_u32.to_le_bytes());
    bytes.extend_from_slice(&16000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&8_u32.to_le_bytes());
    for value in [0, sample, -sample, 0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(bytes.len(), 52);
    bytes
}

fn trusted_path_word(path: &Path) -> String {
    let value = path_text(path);
    match SHELL {
        ShellKind::Posix => format!("'{}'", value.replace('\'', "'\\''")),
        ShellKind::Cmd => {
            assert!(
                !value.contains('"'),
                "Windows fixture paths cannot contain double quotes"
            );
            format!("\"{value}\"")
        }
    }
}

fn expected_arguments(input: &ProcessorInput) -> Vec<String> {
    vec![
        input.inputs[0].clone(),
        input.inputs[1].clone(),
        input.inputs[2].clone(),
        input.inputs[3].clone(),
        input.inputs[4].clone(),
        serde_json::to_string(&input.inputs).unwrap(),
        format!("media={}", input.inputs[1]),
        serde_json::to_string(&input.outputs).unwrap(),
    ]
}

async fn assert_arguments(path: &Path, expected: &[String]) {
    let bytes = tokio::fs::read(path).await.unwrap();
    let mut reader = Cursor::new(&bytes);
    let mut header = [0; 4];
    for tag in [
        b"ARGV",
        #[cfg(windows)]
        b"SH32",
    ] {
        reader.read_exact(&mut header).unwrap();
        assert_eq!(&header, tag);
        reader.read_exact(&mut header).unwrap();
        assert_eq!(
            u32::from_le_bytes(header) as usize,
            expected.len(),
            "parser tag {tag:?}"
        );
        for argument in expected {
            reader.read_exact(&mut header).unwrap();
            assert_eq!(
                u32::from_le_bytes(header) as usize,
                argument.len(),
                "parser tag {tag:?}"
            );
            let mut actual = vec![0; argument.len()];
            reader.read_exact(&mut actual).unwrap();
            assert_eq!(
                actual,
                argument.as_bytes(),
                "native argument bytes changed for parser {tag:?}"
            );
        }
    }
    assert_eq!(
        reader.position() as usize,
        bytes.len(),
        "unexpected trailing arguments or bytes"
    );
}

#[tokio::test]
async fn ordinary_media_program_arguments_round_trip() {
    let fixture = Fixture::new().await;
    let helper = fixture.helper("media_capture_helper").await;
    let capture = fixture.root().join("program-arguments.bin");
    let mut input = fixture.input(&capture);
    let mut args = vec![path_text(fixture.root()), "{output}".into()];
    args.extend(
        ARGUMENT_TEMPLATES
            .iter()
            .map(|argument| (*argument).to_owned()),
    );
    input.config = Some(serde_json::json!({"program":path_text(&helper),"args":args}).to_string());
    fixture.run(&input).await;
    assert_arguments(&capture, &expected_arguments(&input)).await;
}

#[derive(Clone, Copy)]
enum WordStyle {
    Bare,
    Double,
    #[cfg(unix)]
    Single,
}

impl WordStyle {
    fn render(self, word: &str) -> String {
        match self {
            Self::Bare => word.into(),
            Self::Double => format!("\"{word}\""),
            #[cfg(unix)]
            Self::Single => format!("'{word}'"),
        }
    }
}

#[tokio::test]
async fn ordinary_media_shell_arguments_round_trip() {
    let fixture = Fixture::new().await;
    let helper = fixture.helper("media_capture_helper").await;
    let styles = [
        WordStyle::Bare,
        WordStyle::Double,
        #[cfg(unix)]
        WordStyle::Single,
    ];
    for (index, style) in styles.into_iter().enumerate() {
        let capture = fixture.root().join(format!("shell-arguments-{index}.bin"));
        let mut input = fixture.input(&capture);
        let words = ARGUMENT_TEMPLATES
            .iter()
            .map(|word| style.render(word))
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!(
            "{} {} \"{{output}}\" {words}",
            trusted_path_word(&helper),
            trusted_path_word(fixture.root())
        );
        input.config = Some(serde_json::json!({"command":command}).to_string());
        fixture.run(&input).await;
        assert_arguments(&capture, &expected_arguments(&input)).await;
    }
}

#[tokio::test]
async fn ordinary_media_file_redirection_preserves_bytes() {
    let fixture = Fixture::new().await;
    let helper = fixture.helper("media_copy_helper").await;
    for (index, (source, expected)) in fixture.media.iter().enumerate() {
        let output = source
            .parent()
            .unwrap()
            .join(format!("Copied 素材 ({index}) 50%!.wav"));
        let input = ProcessorInput {
            inputs: vec![path_text(source)], outputs: vec![path_text(&output)],
            config: Some(serde_json::json!({"command":format!("{} < \"{{input}}\" > \"{{output}}\"", trusted_path_word(&helper))}).to_string()),
            ..Default::default()
        };
        fixture.run(&input).await;
        assert_eq!(tokio::fs::read(&output).await.unwrap(), *expected);
        assert_eq!(tokio::fs::read(source).await.unwrap(), *expected);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn ordinary_media_posix_pipe_preserves_bytes() {
    let fixture = Fixture::new().await;
    let helper = fixture.helper("media_copy_helper").await;
    let (source, expected) = &fixture.media[2];
    let output = source.parent().unwrap().join("Piped 演奏 (copy) 50%.wav");
    let helper = trusted_path_word(&helper);
    let input = ProcessorInput {
        inputs: vec![path_text(source)], outputs: vec![path_text(&output)],
        config: Some(serde_json::json!({"command":format!("{helper} < \"{{input}}\" | {helper} > \"{{output}}\"")}).to_string()),
        ..Default::default()
    };
    fixture.run(&input).await;
    assert_eq!(tokio::fs::read(&output).await.unwrap(), *expected);
    assert_eq!(tokio::fs::read(source).await.unwrap(), *expected);
}
