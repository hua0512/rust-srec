//! # FLV Processing Pipeline
//!
//! This module implements a processing pipeline for fixing and optimizing FLV (Flash Video) streams.
//! The pipeline consists of multiple operators that can transform, validate, and repair FLV data
//! to ensure proper playability and standards compliance.
//!
//! ## Pipeline Architecture
//!
//! Input → Defragment → HeaderCheck → Split → GopSort → TimeConsistency →
//!        TimingRepair → Limit → TimeConsistency2 → ScriptKeyframesFiller → ScriptFilter → Output
//!
//! Each operator addresses specific issues that can occur in FLV streams:
//!
//! - **Defragment**: Handles fragmented streams by buffering and validating segments
//! - **HeaderCheck**: Ensures streams begin with a valid FLV header
//! - **Split**: Divides content at appropriate points for better playability
//! - **GopSort**: Ensures video tags are properly ordered by GOP (Group of Pictures)
//! - **TimeConsistency**: Maintains consistent timestamps throughout the stream
//! - **TimingRepair**: Fixes timestamp anomalies like negative values or jumps
//! - **Limit**: Enforces file size and duration limits
//! - **ScriptKeyframesFiller**: Prepares metadata for proper seeking by adding keyframe placeholders
//! - **ScriptFilter**: Removes or modifies problematic script tags

use crate::context::StreamerContext;
use crate::operators::limit::LimitConfig;
use crate::operators::script_filler::ScriptFillerConfig;
use crate::operators::{
    ContinuityMode, DefragmentOperator, FlvOperator, GopSortOperator, HeaderCheckOperator,
    LimitOperator, RepairStrategy, ScriptFilterOperator, ScriptKeyframesFillerOperator,
    SplitOperator, TimeConsistencyOperator, TimingRepairConfig, TimingRepairOperator,
};
use flv::data::FlvData;
use flv::error::FlvError;
use futures::stream::{Stream, StreamExt};
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Type alias for a boxed stream of FLV data with error handling
pub type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, FlvError>> + Send>>;

/// Configuration options for the FLV processing pipeline
#[derive(Clone)]
pub struct PipelineConfig {
    /// Whether to filter duplicate tags
    pub duplicate_tag_filtering: bool,

    /// Maximum file size limit in bytes (0 = unlimited)
    pub file_size_limit: u64,

    /// Maximum duration limit in seconds (0 = unlimited)
    pub duration_limit: f32,

    /// Strategy for timestamp repair
    pub repair_strategy: RepairStrategy,

    /// Mode for timeline continuity
    pub continuity_mode: ContinuityMode,

    /// Configuration for keyframe index injection
    pub keyframe_index_config: Option<ScriptFillerConfig>,

    /// Channel buffer capacity for each stage of the pipeline
    pub channel_buffer_size: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            duplicate_tag_filtering: true,
            file_size_limit: 6 * 1024 * 1024 * 1024, // 6 GB
            duration_limit: 0.0,
            repair_strategy: RepairStrategy::Strict,
            continuity_mode: ContinuityMode::Reset,
            keyframe_index_config: Some(ScriptFillerConfig::default()),
            channel_buffer_size: 16, // Default buffer size
        }
    }
}

/// Main pipeline for processing FLV streams
pub struct FlvPipeline {
    context: Arc<StreamerContext>,
    config: PipelineConfig,
}

impl FlvPipeline {
    /// Create a new pipeline with default configuration
    pub fn new(context: StreamerContext) -> Self {
        Self {
            context: Arc::new(context),
            config: PipelineConfig::default(),
        }
    }

    /// Create a new pipeline with custom configuration
    pub fn with_config(context: StreamerContext, config: PipelineConfig) -> Self {
        Self {
            context: Arc::new(context),
            config,
        }
    }

    /// Process an FLV stream through the complete processing pipeline
    pub fn process(&self, input: BoxStream<FlvData>) -> BoxStream<FlvData> {
        let context = Arc::clone(&self.context);
        let config = self.config.clone();

        // Create channels for all operators
        let (defrag_tx, defrag_rx) = kanal::bounded_async(config.channel_buffer_size);
        let (header_check_tx, header_check_rx) = kanal::bounded_async(config.channel_buffer_size);
        let (gop_sort_tx, gop_sort_rx) = kanal::bounded_async(config.channel_buffer_size);
        let (timing_repair_tx, timing_repair_rx) = kanal::bounded_async(config.channel_buffer_size);
        let (split_tx, split_rx) = kanal::bounded_async(config.channel_buffer_size);
        let (time_consistency_tx, time_consistency_rx) =
            kanal::bounded_async(config.channel_buffer_size);
        let (limit_tx, limit_rx) = kanal::bounded_async(config.channel_buffer_size);
        let (script_filter_tx, script_filter_rx) = kanal::bounded_async(config.channel_buffer_size);

        let (time_consistency_2_tx, time_consistency_2_rx) =
            kanal::bounded_async(config.channel_buffer_size);
        let (keyframe_index_tx, keyframe_index_rx) =
            kanal::bounded_async(config.channel_buffer_size);
        let (input_tx, input_rx) = kanal::bounded_async(config.channel_buffer_size);

        // Create all operators
        let mut defrag_operator = DefragmentOperator::new(context.clone());
        let mut header_check_operator = HeaderCheckOperator::new(context.clone());
        let limit_config = LimitConfig {
            max_size_bytes: if config.file_size_limit > 0 {
                Some(config.file_size_limit)
            } else {
                None
            },
            max_duration_ms: if config.duration_limit > 0.0 {
                Some((config.duration_limit * 1000.0) as u32)
            } else {
                None
            },
            split_at_keyframes_only: true,
            use_retrospective_splitting: false,
            on_split: None,
        };
        let mut limit_operator = LimitOperator::with_config(context.clone(), limit_config);
        let mut gop_sort_operator = GopSortOperator::new(context.clone());
        let mut script_filter_operator = ScriptFilterOperator::new(context.clone());
        let timing_repair_operator =
            TimingRepairOperator::new(context.clone(), TimingRepairConfig::default());
        let mut split_operator = SplitOperator::new(context.clone());
        let mut time_consistency_operator =
            TimeConsistencyOperator::new(context.clone(), config.continuity_mode);
        let mut time_consistency_2_operator =
            TimeConsistencyOperator::new(context.clone(), config.continuity_mode);

        // Create the KeyframeIndexInjector operator if enabled
        let keyframe_index_operator = config.keyframe_index_config.map(|keyframe_config| {
            ScriptKeyframesFillerOperator::new(context.clone(), keyframe_config)
        });

        // Store all task handles
        let mut task_handles: Vec<JoinHandle<()>> = Vec::with_capacity(11);

        // Input conversion task
        task_handles.push(tokio::spawn(async move {
            futures::pin_mut!(input);
            while let Some(result) = input.next().await {
                if input_tx.send(result).await.is_err() {
                    break;
                }
            }
        }));

        // Processing pipeline tasks
        task_handles.push(tokio::spawn(async move {
            defrag_operator.process(input_rx, defrag_tx).await;
        }));
        task_handles.push(tokio::spawn(async move {
            header_check_operator
                .process(defrag_rx, header_check_tx)
                .await;
        }));
        task_handles.push(tokio::spawn(async move {
            split_operator.process(header_check_rx, split_tx).await;
        }));
        task_handles.push(tokio::spawn(async move {
            gop_sort_operator.process(split_rx, gop_sort_tx).await;
        }));
        task_handles.push(tokio::spawn(async move {
            time_consistency_operator
                .process(gop_sort_rx, time_consistency_tx)
                .await;
        }));
        task_handles.push(tokio::spawn(async move {
            timing_repair_operator
                .process(time_consistency_rx, timing_repair_tx)
                .await;
        }));
        task_handles.push(tokio::spawn(async move {
            limit_operator.process(timing_repair_rx, limit_tx).await;
        }));
        task_handles.push(tokio::spawn(async move {
            time_consistency_2_operator
                .process(limit_rx, time_consistency_2_tx)
                .await;
        }));

        // Conditionally create ScriptKeyframesFillerOperator task based on configuration
        if let Some(mut operator) = keyframe_index_operator {
            task_handles.push(tokio::spawn(async move {
                operator
                    .process(time_consistency_2_rx, keyframe_index_tx)
                    .await;
            }));

            // Script filter is the last operator in the pipeline
            task_handles.push(tokio::spawn(async move {
                script_filter_operator
                    .process(keyframe_index_rx, script_filter_tx)
                    .await;
            }));
        } else {
            // If ScriptKeyframesFillerOperator is disabled, connect time_consistency_2 directly to script_filter
            task_handles.push(tokio::spawn(async move {
                script_filter_operator
                    .process(time_consistency_2_rx, script_filter_tx)
                    .await;
            }));
        }

        let output_stream = async_stream::stream! {
            while let Ok(result) = script_filter_rx.recv().await {
                match result {
                    Ok(data) => yield Ok(data),
                    Err(e) => yield Err(e),
                }
            }
            // Channel closed when while loop exits
        };

        Box::pin(output_stream)
    }
}

#[cfg(test)]
/// Tests for the FLV processing pipeline
mod test {
    use super::*;
    use crate::writer_task::FlvWriterTask;
    use crate::{context::StreamerContext, writer_task::WriterError};

    use flv::parser_async::FlvDecoderStream;
    use futures::StreamExt;
    use std::path::Path;
    use std::sync::mpsc;
    use tracing::{debug, info};

    // Helper to initialize tracing for tests
    fn init_tracing() {
        let _ = tracing_subscriber::fmt::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer() // Write to test output
            .try_init();
    }

    #[tokio::test]
    #[ignore]
    async fn test_process() -> Result<(), Box<dyn std::error::Error>> {
        init_tracing(); // Initialize tracing for logging

        // Source and destination paths
        let input_path = Path::new("D:/test/999/testHEVC.flv");

        // Skip if test file doesn't exist
        if !input_path.exists() {
            info!(path = %input_path.display(), "Test file not found, skipping test");
            return Ok(());
        }

        // let input_path = Path::new("E:/test/2024-12-21_00_06_24.flv");
        let output_dir = input_path.parent().ok_or("Invalid input path")?.join("fix");
        tokio::fs::create_dir_all(&output_dir)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = ?e, "Output directory creation failed or already exists");
            });
        let base_name = input_path
            .file_stem()
            .ok_or("No file stem")?
            .to_string_lossy()
            .to_string();

        let start_time = std::time::Instant::now(); // Start timer
        info!(path = %input_path.display(), "Starting FLV processing pipeline test");

        // Create the context
        let context = StreamerContext::default();

        // Create the pipeline with default configuration
        let pipeline = FlvPipeline::new(context);

        // Start a task to parse the input file using async Decoder
        let file_reader = tokio::io::BufReader::new(tokio::fs::File::open(input_path).await?);
        let decoder_stream = FlvDecoderStream::with_capacity(
            file_reader,
            32 * 1024, // Input buffer capacity
        );

        // Create the input stream for the pipeline
        let input_stream = decoder_stream.boxed();

        // Process the stream through the pipeline
        let processed_stream = pipeline.process(input_stream);

        // Create a channel to bridge between async Stream and sync Receiver
        let (tx, rx) = mpsc::sync_channel(16);

        // Spawn a task to forward items from the stream to the channel
        let forward_task = tokio::spawn(async move {
            futures::pin_mut!(processed_stream);
            while let Some(result) = processed_stream.next().await {
                if tx.send(result).is_err() {
                    break; // Receiver dropped, exit the loop
                }
            }
        });

        // Run the writer task with the receiver
        let writer_handle = tokio::task::spawn_blocking(move || {
            let mut writer_task = FlvWriterTask::new(output_dir, base_name)?;

            writer_task.run(rx)?;

            Ok::<_, WriterError>((
                writer_task.total_tags_written(),
                writer_task.files_created(),
            ))
        });

        // Ensure the forwarding task completes
        forward_task.await?;
        let (total_tags_written, files_created) = writer_handle.await??;
        let elapsed = start_time.elapsed();

        info!(
            duration = ?elapsed,
            total_tags = total_tags_written,
            files_written = files_created,
            "Pipeline finished processing"
        );

        // Basic assertions (optional, but good for tests)
        assert!(
            files_created > 0,
            "Expected at least one output file to be created"
        );
        assert!(total_tags_written > 0, "Expected tags to be processed");

        Ok(())
    }
}
