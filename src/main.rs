use std::path::{Path, PathBuf};
use std::process::exit;
use std::str::FromStr;

use clap::CommandFactory;
use clap::{Command, Parser, ValueEnum};
use flv::data::FlvData;
use flv::parser_async::FlvDecoderStream;
use flv_fix::context::StreamerContext;
use flv_fix::operators::RepairStrategy;
use flv_fix::operators::script_filler::ScriptFillerConfig;
use flv_fix::pipeline::{BoxStream, FlvPipeline, PipelineConfig};
use flv_fix::writer_task::FlvWriterTask;
use futures::StreamExt;
use tokio::fs::File;
use tokio::io::BufReader;
use tracing::{Level, debug, error, info};
use tracing_subscriber::FmtSubscriber;

// Define CLI arguments
#[derive(Parser)]
#[command(
    author = "hua0512 <https://github.com/hua0512>",
    version,
    about = "FLV processing and repair tool",
    long_about = "A tool for processing, repairing and optimizing FLV (Flash Video) files.\n\
                  Part of the stream-rec project: https://github.com/hua0512/rust-rec\n\
                  \n\
                  This tool fixes common issues in FLV streams such as timestamp anomalies,\n\
                  out-of-order frames, duration problems, and metadata inconsistencies.\n\
                  It supports processing individual files or entire directories of FLV files."
)]
struct CliArgs {
    /// Input FLV file(s) or directory to process
    #[arg(
        required = true,
        help = "Path to FLV file(s) or directory containing FLV files"
    )]
    input: Vec<PathBuf>,

    /// Output directory for processed files
    #[arg(
        short,
        long,
        help = "Directory where processed files will be saved (default: ./fix)"
    )]
    output_dir: Option<PathBuf>,

    /// Maximum file size with optional unit (B, KB, MB, GB, TB)
    /// Examples: "4GB", "500MB", "2048KB"
    #[arg(
        short,
        long,
        default_value = "0",
        help = "Maximum size for output files with optional unit (B, KB, MB, GB, TB). Examples: \"4GB\", \"500MB\". Use 0 for unlimited."
    )]
    max_size: String,

    /// Maximum duration with optional unit (s, m, h)
    /// Examples: "30m", "1.5h", "90s"
    #[arg(
        short = 'd',
        long,
        default_value = "0",
        help = "Maximum duration for output files with optional unit (s, m, h). Examples: \"30m\", \"1.5h\", \"90s\". Use 0 for unlimited."
    )]
    max_duration: String,

    /// Filter duplicate tags
    #[arg(
        short,
        long,
        default_value = "true",
        help = "Remove duplicate video/audio frames"
    )]
    filter_duplicates: bool,

    /// Enable verbose logging
    #[arg(short, long, help = "Enable detailed debug logging")]
    verbose: bool,

    /// Enable keyframe index injection
    #[arg(
        short = 'k',
        long,
        default_value = "true",
        help = "Inject keyframe index in metadata for better seeking"
    )]
    keyframe_index: bool,

    /// Buffer size for processing channels
    #[arg(
        short = 'b',
        long,
        default_value = "16",
        help = "Buffer size for internal processing channels"
    )]
    buffer_size: usize,
}

// Error types for parsing
#[derive(Debug)]
enum ParseError {
    InvalidFormat,
    InvalidNumber,
    InvalidUnit,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidFormat => write!(f, "Invalid format"),
            ParseError::InvalidNumber => write!(f, "Invalid number"),
            ParseError::InvalidUnit => write!(f, "Invalid unit"),
        }
    }
}

impl std::error::Error for ParseError {}

// Function to parse size with units
fn parse_size(size_str: &str) -> Result<u64, ParseError> {
    // Trim whitespace and handle case-insensitivity
    let size_str = size_str.trim().to_lowercase();

    // Handle empty string
    if size_str.is_empty() {
        return Err(ParseError::InvalidFormat);
    }

    // Split the numeric part and the unit
    let mut numeric_part = String::new();
    let mut unit_part = String::new();

    for c in size_str.chars() {
        if c.is_ascii_digit() || c == '.' {
            numeric_part.push(c);
        } else {
            unit_part.push(c);
        }
    }

    // Handle cases with no unit (assume bytes)
    if unit_part.is_empty() {
        let bytes = numeric_part
            .parse::<u64>()
            .map_err(|_| ParseError::InvalidNumber)?;
        return Ok(bytes);
    }

    // Parse the numeric part
    let value = numeric_part
        .parse::<f64>()
        .map_err(|_| ParseError::InvalidNumber)?;

    // Parse the unit and convert to bytes
    match unit_part.trim() {
        "b" => Ok(value as u64),
        "kb" => Ok((value * 1024.0) as u64),
        "mb" => Ok((value * 1024.0 * 1024.0) as u64),
        "gb" => Ok((value * 1024.0 * 1024.0 * 1024.0) as u64),
        "tb" => Ok((value * 1024.0 * 1024.0 * 1024.0 * 1024.0) as u64),
        _ => Err(ParseError::InvalidUnit),
    }
}

// Function to parse time with units
fn parse_time(time_str: &str) -> Result<f32, ParseError> {
    // Trim whitespace and handle case-insensitivity
    let time_str = time_str.trim().to_lowercase();

    // Handle empty string
    if time_str.is_empty() {
        return Err(ParseError::InvalidFormat);
    }

    // Try to parse as a simple number (seconds)
    if let Ok(seconds) = time_str.parse::<f32>() {
        return Ok(seconds);
    }

    // Split the numeric part and the unit
    let mut numeric_part = String::new();
    let mut unit_part = String::new();

    for c in time_str.chars() {
        if c.is_ascii_digit() || c == '.' {
            numeric_part.push(c);
        } else {
            unit_part.push(c);
        }
    }

    // Parse the numeric part
    let value = numeric_part
        .parse::<f32>()
        .map_err(|_| ParseError::InvalidNumber)?;

    // Parse the unit and convert to seconds
    match unit_part.trim() {
        "s" => Ok(value),
        "m" => Ok(value * 60.0),
        "h" => Ok(value * 3600.0),
        _ => Err(ParseError::InvalidUnit),
    }
}

// Convert bytes to a human-readable format
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

// Convert seconds to a human-readable format
fn format_duration(seconds: f32) -> String {
    if seconds >= 3600.0 {
        let hours = seconds / 3600.0;
        format!("{:.2}h", hours)
    } else if seconds >= 60.0 {
        let minutes = seconds / 60.0;
        format!("{:.2}m", minutes)
    } else {
        format!("{:.2}s", seconds)
    }
}

// Process a single FLV file
async fn process_file(
    input_path: &Path,
    output_dir: &Path,
    config: PipelineConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let start_time = std::time::Instant::now();

    // Create output directory if it doesn't exist
    tokio::fs::create_dir_all(output_dir).await?;

    // Create base name for output files
    let base_name = input_path
        .file_stem()
        .ok_or("Invalid filename")?
        .to_string_lossy()
        .to_string();

    info!(
        path = %input_path.display(),
        "Starting to process file"
    );

    // Create streamer context and pipeline
    let context = StreamerContext::default();
    let pipeline = FlvPipeline::with_config(context, config);

    // Open the file and create decoder stream
    let file = File::open(input_path).await?;
    let file_reader = BufReader::new(file);
    let file_size = file_reader.get_ref().metadata().await?.len();
    let decoder_stream = FlvDecoderStream::with_capacity(
        file_reader,
        32 * 1024, // Input buffer capacity
    );

    // Create the input stream for the pipeline
    let input_stream: BoxStream<FlvData> = decoder_stream.boxed();

    // Process the stream through the pipeline
    let processed_stream = pipeline.process(input_stream);

    // Create writer task and run it
    let mut writer_task = FlvWriterTask::new(output_dir.to_path_buf(), base_name).await?;
    writer_task.run(processed_stream).await?;

    let elapsed = start_time.elapsed();
    let total_tags_written = writer_task.total_tags_written();
    let files_created = writer_task.files_created();

    info!(
        path = %input_path.display(),
        input_size = %format_bytes(file_size),
        duration = ?elapsed,
        tags_processed = total_tags_written,
        files_created = files_created,
        "Processing complete"
    );

    Ok(())
}

// Find FLV files in directory
async fn find_flv_files(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file() && path.extension().map_or(false, |ext| ext == "flv") {
            files.push(path);
        }
    }

    Ok(files)
}

// Process multiple input paths (files or directories)
async fn process_inputs(
    inputs: &[PathBuf],
    output_dir: &Path,
    config: PipelineConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut files_to_process = Vec::new();

    // Collect all FLV files to process
    for input_path in inputs {
        if input_path.is_file() && input_path.extension().map_or(false, |ext| ext == "flv") {
            files_to_process.push(input_path.clone());
        } else if input_path.is_dir() {
            let flv_files = find_flv_files(input_path).await?;
            files_to_process.extend(flv_files);
        } else {
            error!(
                "Input {} is not a valid FLV file or directory",
                input_path.display()
            );
        }
    }

    // Process each file
    if files_to_process.is_empty() {
        error!("No FLV files found in the specified input paths");
        return Err("No FLV files found".into());
    }

    info!("Found {} FLV files to process", files_to_process.len());
    for file in files_to_process {
        if let Err(e) = process_file(&file, output_dir, config.clone()).await {
            error!(
                file = %file.display(),
                error = ?e,
                "Failed to process file"
            );
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    // Parse command-line arguments, show help on error
    let args = match CliArgs::try_parse() {
        Ok(args) => args,
        Err(e) => {
            // If the error is that help was requested, just exit (clap handles the help display)
            if e.kind() == clap::error::ErrorKind::DisplayHelp {
                exit(0);
            }

            // For other errors, show a more helpful message
            // let mut cmd = CliArgs::command();
            eprintln!("{}\n", e);
            // eprintln!("For more information, try '--help'");
            // eprintln!("\nUsage: {}", cmd.render_usage());
            exit(1);
        }
    };

    // Setup logging
    let log_level = if args.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };
    let subscriber = FmtSubscriber::builder().with_max_level(log_level).finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    info!("FLV Processing Tool - Part of the stream-rec project by hua0512");
    info!("GitHub: https://github.com/hua0512/rust-srec");

    // Parse size and duration with units
    let file_size_limit = match parse_size(&args.max_size) {
        Ok(size) => size,
        Err(e) => {
            error!("Invalid size format '{}': {}", args.max_size, e);
            exit(1);
        }
    };

    let duration_limit = match parse_time(&args.max_duration) {
        Ok(duration) => duration,
        Err(e) => {
            error!("Invalid duration format '{}': {}", args.max_duration, e);
            exit(1);
        }
    };

    // Log the parsed values
    if file_size_limit > 0 {
        info!("File size limit set to {}", format_bytes(file_size_limit));
    } else {
        info!("No file size limit set");
    }

    if duration_limit > 0.0 {
        info!("Duration limit set to {}", format_duration(duration_limit));
    } else {
        info!("No duration limit set");
    }

    // Configure pipeline
    let config = PipelineConfig {
        duplicate_tag_filtering: args.filter_duplicates,
        file_size_limit: file_size_limit,
        duration_limit: duration_limit,
        repair_strategy: RepairStrategy::Strict, // Fixed to Strict
        continuity_mode: flv_fix::operators::ContinuityMode::Reset, // Fixed to Reset
        keyframe_index_config: if args.keyframe_index {
            Some(ScriptFillerConfig::default())
        } else {
            None
        },
        channel_buffer_size: args.buffer_size,
    };

    // Determine output directory
    let output_dir = args.output_dir.unwrap_or_else(|| PathBuf::from("./fix"));

    // Process input files
    match process_inputs(&args.input, &output_dir, config).await {
        Ok(_) => {
            info!("All processing completed");
        }
        Err(e) => {
            error!(error = ?e, "Processing failed");
            exit(1);
        }
    }
}
