use crate::context::StreamerContext;
use crate::error::FlvError;
use crate::operators::limit::{self, LimitConfig};
use crate::operators::{
    ContinuityMode, DefragmentOperator, GopSortOperator, HeaderCheckOperator, LimitOperator,
    RepairStrategy, ScriptFilterOperator, SplitOperator, TimeConsistencyOperator,
    TimingRepairConfig, TimingRepairOperator, defragment, time_consistency,
};
use bytes::buf::Limit;
use flv::data::FlvData;
use futures::FutureExt;
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
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            duplicate_tag_filtering: true,
            file_size_limit: 2 * 1024 * 1024 * 1024, // 2 GB
            duration_limit: 0.0,
            repair_strategy: RepairStrategy::Strict,
            continuity_mode: ContinuityMode::Reset,
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
        let (defrag_tx, defrag_rx) = tokio::sync::mpsc::channel(16);
        let (header_check_tx, header_check_rx) = tokio::sync::mpsc::channel(16);
        let (limit_tx, limit_rx) = tokio::sync::mpsc::channel(16);
        let (gop_sort_tx, gop_sort_rx) = tokio::sync::mpsc::channel(16);
        let (script_filter_tx, script_filter_rx) = tokio::sync::mpsc::channel(16);
        let (timing_repair_tx, timing_repair_rx) = tokio::sync::mpsc::channel(16);
        let (split_tx, split_rx) = tokio::sync::mpsc::channel(16);
        let (time_consistency_tx, time_consistency_rx) = tokio::sync::mpsc::channel(16);
        let (time_consistency_2_tx, time_consistency_2_rx) = tokio::sync::mpsc::channel(16);
        let (input_tx, input_rx) = tokio::sync::mpsc::channel(16);

        // Create all operators
        let defrag_operator = DefragmentOperator::new(context.clone());
        let header_check_operator = HeaderCheckOperator::new(context.clone());
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
        let script_filter_operator = ScriptFilterOperator::new(context.clone());
        let timing_repair_operator =
            TimingRepairOperator::new(context.clone(), TimingRepairConfig::default());
        let split_operator = SplitOperator::new(context.clone());
        let time_consistency_operator =
            TimeConsistencyOperator::new(context.clone(), config.continuity_mode);
        let time_consistency_2_operator =
            TimeConsistencyOperator::new(context.clone(), config.continuity_mode);

        // Store all task handles
        let mut task_handles: Vec<JoinHandle<()>> = Vec::with_capacity(10);

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
        task_handles.push(tokio::spawn(async move {
            script_filter_operator
                .process(time_consistency_2_rx, script_filter_tx)
                .await;
        }));

        let output_stream = tokio_stream::wrappers::ReceiverStream::new(script_filter_rx)
            .map(move |item| item)
            .boxed();

        output_stream
    }
}

mod test {
    use super::*;
    use chrono::Local;
    use flv::data::FlvData;
    use flv::header::FlvHeader;
    use flv::tag::{FlvTag, FlvTagType};
    use flv::writer::FlvWriter;
    use futures::StreamExt;
    use std::path::Path;
    use tokio::fs::File;
    use tokio::io::{AsyncReadExt, BufReader};
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::ReceiverStream;
    use bytes::{BytesMut, Bytes, Buf};
    use std::io::Cursor;

    #[tokio::test]
    async fn test_process() -> Result<(), Box<dyn std::error::Error>> {
        // Source and destination paths
        let input_path = Path::new("D:/Downloads/07_47_26-今天能超过10个人吗？.flv");
        let output_dir = Path::new("D:/Downloads/");
        let base_name = "processed_output";
        let extension = "flv";

        // Skip if test file doesn't exist
        if !input_path.exists() {
            println!("Test file not found, skipping test");
            return Ok(());
        }

        // Open the input file
        let file = File::open(input_path).await?;
        let mut reader = BufReader::new(file);
        
        // Create the context
        let context = StreamerContext::default();

        // Create the pipeline with default configuration
        let pipeline = FlvPipeline::new(context);

        // Create channel for the FlvData stream
        let (tx, rx) = mpsc::channel(32);

        // Start a task to parse the input file
        tokio::spawn(async move {
            // Read the FLV header first (9 bytes)
            let mut header_buf = BytesMut::with_capacity(9);
            header_buf.resize(9, 0);
            if let Err(e) = reader.read_exact(&mut header_buf).await {
                eprintln!("Failed to read header: {}", e);
                return;
            }

            // Parse the header using a cursor
            let mut cursor = Cursor::new(header_buf.freeze());
            let header = match FlvHeader::parse(&mut cursor) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("Failed to parse header: {}", e);
                    return;
                }
            };

            // Send the header to the pipeline
            if let Err(e) = tx.send(Ok(FlvData::Header(header))).await {
                eprintln!("Failed to send header: {}", e);
                return;
            }

            // Skip first 4 bytes (first previous tag size)
            let mut prev_tag_size_buf = [0u8; 4];
            if let Err(e) = reader.read_exact(&mut prev_tag_size_buf).await {
                eprintln!("Failed to read initial previous tag size: {}", e);
                return;
            }

            let mut tag_count = 0;
            
            // Process tags
            loop {
                // We first need to determine how much data we need to read for the tag
                // Read first byte to determine tag type and data size
                let mut peek_buf = [0u8; 4];  // Enough to read tag type (1 byte) and data size (3 bytes)
                
                match reader.read_exact(&mut peek_buf).await {
                    Ok(_) => {
                        // Calculate data size from bytes 1-3
                        let data_size = ((peek_buf[1] as u32) << 16) | 
                                       ((peek_buf[2] as u32) << 8) | 
                                       (peek_buf[3] as u32);
                        
                        // Allocate a buffer for the complete tag: 11 bytes header + data size
                        let mut tag_buf = BytesMut::with_capacity(11 + data_size as usize);
                        
                        // Add the 4 bytes we've already read
                        tag_buf.extend_from_slice(&peek_buf);
                        
                        // Resize to fit the whole tag and read the rest
                        tag_buf.resize(11 + data_size as usize, 0);
                        if let Err(e) = reader.read_exact(&mut tag_buf[4..]).await {
                            eprintln!("Error reading rest of tag: {}", e);
                            break;
                        }
                        
                        // Parse the tag
                        let mut tag_cursor = Cursor::new(tag_buf.freeze());
                        match FlvTag::demux(&mut tag_cursor) {
                            Ok(tag) => {
                                tag_count += 1;
                                if let Err(e) = tx.send(Ok(FlvData::Tag(tag))).await {
                                    eprintln!("Error sending tag: {}", e);
                                    break;
                                }
                            },
                            Err(e) => {
                                eprintln!("Error parsing tag: {}", e);
                                break;
                            }
                        }
                        
                        // Skip previous tag size
                        if let Err(e) = reader.read_exact(&mut prev_tag_size_buf).await {
                            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                                break;  // End of file
                            }
                            eprintln!("Error reading previous tag size: {}", e);
                            break;
                        }
                    },
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::UnexpectedEof {
                            // End of file reached
                            break;
                        }
                        eprintln!("Error reading tag header: {}", e);
                        break;
                    }
                }
            }
            
            println!("Parsed {} tags from input file", tag_count);
        });

        // Create the input stream
        let input_stream = ReceiverStream::new(rx).boxed();

        // Process the stream
        let processed_stream = pipeline.process(input_stream);

        // Process and write results
        futures::pin_mut!(processed_stream);
        
        let mut current_writer: Option<FlvWriter<std::fs::File>> = None;
        let mut file_counter = 0;
        let mut total_count = 0;
        let mut current_file_count = 0;

        while let Some(result) = processed_stream.next().await {
            match result {
                Ok(FlvData::Header(header)) => {
                    // Close the current writer if it exists
                    if let Some(mut writer) = current_writer.take() {
                        writer.flush()?;
                        println!("Wrote {} tags to file {}", current_file_count, file_counter);
                        current_file_count = 0;
                    }

                    // Create a new file with timestamp
                    file_counter += 1;
                    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
                    let output_path = output_dir.join(format!(
                        "{}_{}_part{}.{}",
                        base_name, timestamp, file_counter, extension
                    ));
                    
                    println!("Creating new output file: {:?}", output_path);
                    
                    let output_file = std::fs::File::create(&output_path)?;
                    current_writer = Some(FlvWriter::new(output_file, &header)?);
                },
                Ok(FlvData::Tag(tag)) => {
                    // If we don't have a writer, create one with a default header
                    if current_writer.is_none() {
                        file_counter += 1;
                        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
                        let output_path = output_dir.join(format!(
                            "{}_{}_part{}.{}",
                            base_name, timestamp, file_counter, extension
                        ));
                        
                        println!("Creating initial output file: {:?}", output_path);
                        
                        let default_header = FlvHeader::new(true, true);
                        let output_file = std::fs::File::create(&output_path)?;
                        current_writer = Some(FlvWriter::new(output_file, &default_header)?);
                    }

                    // Write the tag to the current writer
                    if let Some(writer) = &mut current_writer {
                        writer.write_tag(tag.tag_type, tag.data, tag.timestamp_ms)?;
                        total_count += 1;
                        current_file_count += 1;
                    }
                },
                Err(e) => eprintln!("Error: {}", e),
                _ => {}
            }
        }

        // Flush and close the final writer
        if let Some(mut writer) = current_writer {
            writer.flush()?;
            println!("Wrote {} tags to file {}", current_file_count, file_counter);
        }

        println!("Processed and wrote {} tags across {} files", total_count, file_counter);

        Ok(())
    }
}
