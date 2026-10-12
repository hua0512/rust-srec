use crate::cut::{Boundary, CutSession, CutTarget};
use crate::output::pipe_flv_strategy::PipeFlvStrategy;
use crate::output::provider::PipeStream;
use crate::processor::generic::{
    process_pipe_stream, process_pipe_stream_with_processing, process_stream,
    process_stream_with_span,
};
use crate::utils::{expand_name_url, format_bytes, spans};
use crate::{config::ProgramConfig, error::AppError};
use flv::data::FlvData;
use flv::parser_async::FlvDecoderStream;
use flv_fix::FlvPipeline;
use flv_fix::FlvWriterConfig;
use flv_fix::writer::FlvWriter;
use flv_fix::{LimitOperator, ReinjectPolicy};
use futures::{Stream, StreamExt};
use mesio_engine::DownloadSession;
use pipeline_common::{
    CancellationToken, ChannelSpec, Pipeline, PipelineError, PipelineProvider, ProtocolWriter,
    StreamerContext, WriterStats, config::PipelineConfig,
};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tokio::fs::File;
use tokio::io::BufReader;
use tracing::{Instrument, Level, Span, info, span, warn};

async fn process_raw_stream(
    stream: Pin<Box<dyn Stream<Item = Result<FlvData, PipelineError>> + Send>>,
    output_dir: &Path,
    base_name: &str,
    pipeline_common_config: &PipelineConfig,
) -> Result<WriterStats, AppError> {
    let (tx, rx) = tokio::sync::mpsc::channel(pipeline_common_config.channel_size);
    let mut writer = FlvWriter::new(FlvWriterConfig {
        output_dir: output_dir.to_path_buf(),
        base_name: base_name.to_string(),
    });

    // Capture the current span to propagate to the blocking task
    let current_span = Span::current();
    let writer_task = tokio::task::spawn_blocking(move || {
        let _enter = current_span.enter();
        writer.run(rx.into())
    });

    let mut stream = stream;
    while let Some(item_result) = stream.next().await {
        if tx.send(item_result).await.is_err() {
            break;
        }
    }
    drop(tx);

    writer_task
        .await
        .map_err(|e| AppError::Writer(e.to_string()))?
        .map_err(|e| AppError::Writer(e.to_string()))
}

/// What raw FLV recordings pass through: `LimitOperator` alone, without
/// limits, so a cut can open the next file with the stream's header, metadata
/// and sequence headers. Until a cut, it forwards every item unchanged; it
/// keeps the timestamps the source sent.
struct RawCutPipeline {
    context: Arc<StreamerContext>,
}

impl PipelineProvider for RawCutPipeline {
    type Item = FlvData;
    type Config = ();

    fn with_config(context: Arc<StreamerContext>, _: &PipelineConfig, (): ()) -> Self {
        Self { context }
    }

    fn build_pipeline(&self) -> Pipeline<FlvData> {
        Pipeline::new(Arc::clone(&self.context)).add_processor(
            LimitOperator::new(Arc::clone(&self.context))
                .with_reinject_policy(ReinjectPolicy::AtBoundary),
        )
    }
}

/// Process a single FLV file
pub async fn process_file(
    input_path: &Path,
    output_dir: &Path,
    config: &ProgramConfig,
    token: &CancellationToken,
) -> Result<(), AppError> {
    // Check if we're in pipe output mode
    let pipe_stream = PipeStream::of(config.output_format);
    let is_pipe_mode = pipe_stream.is_some();

    // Only create output directory for file mode
    if !is_pipe_mode {
        tokio::fs::create_dir_all(output_dir).await?;
    }

    let base_name = input_path
        .file_stem()
        .ok_or_else(|| AppError::InvalidInput("Invalid filename".to_string()))?
        .to_string_lossy()
        .to_string();

    let start_time = std::time::Instant::now();

    // Create span for file processing
    let file_span = span!(Level::INFO, "process_flv_file", path = %input_path.display());
    file_span.in_scope(|| {
        info!(
            path = %input_path.display(),
            processing_enabled = config.enable_processing,
            "Starting to process file"
        );
    });

    let file = File::open(input_path).await?;
    let file_reader = BufReader::new(file);
    let file_size = file_reader.get_ref().metadata().await?.len();
    let decoder_stream = FlvDecoderStream::with_capacity(file_reader, 4 * 1024 * 1024) // 4MB buffer for better I/O throughput
        .map(|r| r.map_err(|e| PipelineError::Strategy(Box::new(e))));

    // Use pipe output strategy when stdout mode is active
    let stats = if let Some(pipe_stream) = pipe_stream {
        // Pipe mode: write to stdout using PipeFlvStrategy
        let pipe_stats = if config.enable_processing {
            // Processing enabled: run through FlvPipeline before writing to stdout
            info!(
                path = %input_path.display(),
                processing_enabled = true,
                output_mode = %config.output_format,
                "Starting pipe output with FLV processing"
            );
            process_pipe_stream_with_processing::<FlvPipeline, _>(
                Box::pin(decoder_stream),
                &config.pipeline_config,
                config.flv_pipeline_config.clone(),
                PipeFlvStrategy::new().with_stream(pipe_stream),
                "flv",
            )
            .await?
        } else {
            // Raw output: bypass processing pipeline
            process_pipe_stream(
                Box::pin(decoder_stream),
                &config.pipeline_config,
                PipeFlvStrategy::new().with_stream(pipe_stream),
                "flv",
            )
            .await?
        };

        // Log completion statistics for pipe mode
        let elapsed = start_time.elapsed();
        info!(
            path = %input_path.display(),
            input_size = %format_bytes(file_size),
            duration = ?elapsed,
            tags_written = pipe_stats.items_written,
            bytes_written = pipe_stats.bytes_written,
            segment_count = pipe_stats.segment_count,
            output_mode = %config.output_format,
            processing_enabled = config.enable_processing,
            "FLV pipe output complete"
        );

        return Ok(());
    } else if config.enable_processing {
        // we need to expand base_name with %i for output file numbering
        let base_name = format!("{base_name}_p%i");
        // Create a span for pipeline processing
        let pipeline_span = span!(Level::INFO, "flv_pipeline");
        spans::init_processing_span(&pipeline_span, "Processing FLV tags");

        process_stream::<FlvPipeline, FlvWriter>(
            &config.pipeline_config,
            config.flv_pipeline_config.clone(),
            Box::pin(decoder_stream),
            "Writing FLV output",
            |_writer_span| {
                FlvWriter::new(FlvWriterConfig {
                    output_dir: output_dir.to_path_buf(),
                    base_name: base_name.to_string(),
                })
            },
            None,
            token.clone(),
        )
        .instrument(pipeline_span)
        .await?
    } else {
        // Create a span for raw stream writing
        let write_span = span!(Level::INFO, "flv_write_raw");
        spans::init_writing_span(&write_span, "Writing raw FLV");

        process_raw_stream(
            Box::pin(decoder_stream),
            output_dir,
            &base_name,
            &config.pipeline_config,
        )
        .instrument(write_span)
        .await?
    };

    let elapsed = start_time.elapsed();
    info!(
        path = %input_path.display(),
        input_size = %format_bytes(file_size),
        duration = ?elapsed,
        tags_written = stats.items_written,
        files_created = stats.files_created,
        processing_enabled = config.enable_processing,
        "Processing complete"
    );

    Ok(())
}

/// Process an FLV stream
pub async fn process_flv_stream(
    url_str: &str,
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    session: DownloadSession<FlvData>,
    token: &CancellationToken,
    cut_target: &CutTarget,
) -> Result<u64, AppError> {
    // Check if we're in pipe output mode
    let pipe_stream = PipeStream::of(config.output_format);
    let is_pipe_mode = pipe_stream.is_some();

    // Only create output directory for file mode
    if !is_pipe_mode {
        tokio::fs::create_dir_all(output_dir).await?;
    }

    let start_time = Instant::now();

    // Create span for FLV stream download
    // Note: Progress bars are disabled in pipe mode via main.rs configuration
    let download_span = span!(Level::INFO, "download_flv", url = %url_str);

    // Only initialize download span visuals if not in pipe mode
    if !is_pipe_mode {
        spans::init_download_span(&download_span, format!("Downloading {}", url_str));
    }

    let base_name = expand_name_url(name_template, url_str)?;
    let DownloadSession {
        items: stream,
        events,
        handle,
    } = session;
    let progress_task = if is_pipe_mode {
        None
    } else {
        Some(tokio::spawn(spans::render_download_events(
            events,
            download_span.clone(),
        )))
    };

    let stream = stream.map(|r| r.map_err(|e| PipelineError::Strategy(Box::new(e))));

    // Use pipe output strategy when stdout mode is active
    let stats = if let Some(pipe_stream) = pipe_stream {
        // Pipe mode: write to stdout using PipeFlvStrategy
        // Check if processing is enabled to determine whether to use FlvPipeline
        let pipe_stats = if config.enable_processing {
            // Processing enabled: run through FlvPipeline before writing to stdout
            info!(
                url = %url_str,
                processing_enabled = true,
                output_mode = %config.output_format,
                "Starting pipe output with FLV processing"
            );
            process_pipe_stream_with_processing::<FlvPipeline, _>(
                Box::pin(stream),
                &config.pipeline_config,
                config.flv_pipeline_config.clone(),
                PipeFlvStrategy::new().with_stream(pipe_stream),
                "flv",
            )
            .await?
        } else {
            // Raw output: bypass processing pipeline
            process_pipe_stream(
                Box::pin(stream),
                &config.pipeline_config,
                PipeFlvStrategy::new().with_stream(pipe_stream),
                "flv",
            )
            .await?
        };

        // Log completion statistics for pipe mode
        let elapsed = start_time.elapsed();
        info!(
            url = %url_str,
            duration = ?elapsed,
            tags_written = pipe_stats.items_written,
            bytes_written = pipe_stats.bytes_written,
            segment_count = pipe_stats.segment_count,
            output_mode = %config.output_format,
            processing_enabled = config.enable_processing,
            "FLV pipe output complete"
        );

        handle.cancel();
        spans::summarize_dropped_events(&handle, &download_span);
        return Ok(pipe_stats.items_written as u64);
    } else {
        let cut = CutSession::start(cut_target, token, Boundary::Keyframe);
        let new_writer = |_: &Span| {
            FlvWriter::new(FlvWriterConfig {
                output_dir: output_dir.to_path_buf(),
                base_name: base_name.clone(),
            })
        };
        let result = if config.enable_processing {
            process_stream::<FlvPipeline, FlvWriter>(
                &config.pipeline_config,
                config.flv_pipeline_config.clone(),
                Box::pin(stream),
                "Writing FLV output",
                new_writer,
                Some(&cut),
                token.clone(),
            )
            .await
        } else {
            let writer_span = span!(Level::INFO, "writer_processing");
            spans::init_writing_span(&writer_span, "Writing raw FLV");
            process_stream_with_span::<RawCutPipeline, FlvWriter>(
                &config.pipeline_config,
                (),
                Box::pin(stream),
                ChannelSpec::items(config.pipeline_config.channel_size),
                writer_span,
                new_writer,
                Some(&cut),
                // Not the stop token: the pipeline would stop at once and
                // drop the media still queued, which raw output writes. It
                // ends when the download does.
                CancellationToken::new(),
            )
            .await
        };
        cut.finish(result.is_err()).await;
        result?
    };

    let elapsed = start_time.elapsed();
    handle.cancel();
    if let Some(task) = progress_task
        && let Err(error) = task.await
    {
        warn!(%error, "download progress task failed");
    }
    spans::summarize_dropped_events(&handle, &download_span);

    // Log completion (goes to stderr in pipe mode)
    info!(
        url = %url_str,
        duration = ?elapsed,
        tags_written = stats.items_written,
        files_created = stats.files_created,
        output_mode = %config.output_format,
        "FLV processing complete"
    );

    Ok(stats.items_written as u64)
}

#[cfg(test)]
mod tests {
    use flv_fix::FlvPipelineConfig;
    use pipeline_common::ManualSplitStatus;

    use super::*;
    use crate::cut::{CutOutcome, CutSource, Unavailable};
    use crate::processor::fixtures::{
        FRAME_MS, files, flv_recording, frames, item_stream, read_flv,
    };

    const GOPS: u32 = 3;
    const FRAMES: u32 = 10;

    fn writer(dir: &Path) -> impl FnOnce(&Span) -> FlvWriter {
        let output_dir = dir.to_path_buf();
        move |_| {
            FlvWriter::new(FlvWriterConfig {
                output_dir,
                base_name: "rec-%i".to_owned(),
            })
        }
    }

    fn pipeline_config() -> PipelineConfig {
        PipelineConfig::builder().channel_size(16).build()
    }

    async fn record_raw(dir: &Path, items: Vec<FlvData>, cut: Option<&CutSession>) {
        process_stream_with_span::<RawCutPipeline, FlvWriter>(
            &pipeline_config(),
            (),
            item_stream(items),
            ChannelSpec::items(16),
            Span::none(),
            writer(dir),
            cut,
            CancellationToken::new(),
        )
        .await
        .expect("raw recording");
    }

    /// Requests a cut before the recording's first item; the first keyframe
    /// cannot end an empty file, so the cut lands on the second.
    fn start_with_cut(token: &CancellationToken) -> (CutTarget, CutSession) {
        let (target, _notices) = CutTarget::with_notice_receiver();
        let cut = CutSession::start(&target, token, Boundary::Keyframe);
        assert!(matches!(
            target.trigger(CutSource::Key),
            CutOutcome::Requested { .. }
        ));
        (target, cut)
    }

    #[tokio::test]
    async fn a_fixed_recording_is_cut_at_the_next_keyframe_without_losing_media() {
        let dir = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        let (_target, cut) = start_with_cut(&token);
        let flv_config = FlvPipelineConfig::builder()
            .duplicate_tag_filtering(false)
            .continuity_mode(flv_fix::ContinuityMode::Reset)
            .build();
        let result = process_stream::<FlvPipeline, FlvWriter>(
            &pipeline_config(),
            flv_config,
            item_stream(flv_recording(0, GOPS, FRAMES)),
            "Writing FLV output",
            writer(dir.path()),
            Some(&cut),
            token.clone(),
        )
        .await;
        let control = cut.control();
        cut.finish(result.is_err()).await;
        result.expect("fixed recording");
        assert_eq!(control.snapshot().status, ManualSplitStatus::Completed);

        let files = files(dir.path(), "flv");
        assert_eq!(files.len(), 2);
        let first = read_flv(&files[0]);
        let second = read_flv(&files[1]);
        assert_eq!(frames(&first).len(), FRAMES as usize);
        assert_eq!(
            frames(&first).len() + frames(&second).len(),
            (GOPS * FRAMES) as usize
        );
        let first_video = second
            .iter()
            .find(|tag| tag.is_video_tag())
            .expect("video in the second file");
        assert!(first_video.is_video_sequence_header());
        assert!(frames(&second)[0].is_key_frame_nalu());
    }

    /// `GopSort` holds the last group of pictures until its input closes, so
    /// the cut's keyframe reaches the limiter only after forwarding has ended.
    #[tokio::test]
    async fn a_cut_accepted_before_the_input_ends_is_honoured_by_queued_media() {
        let dir = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        let (target, cut) = start_with_cut(&token);
        let result = process_stream::<FlvPipeline, FlvWriter>(
            &pipeline_config(),
            FlvPipelineConfig::default(),
            item_stream(flv_recording(0, 2, FRAMES)),
            "Writing FLV output",
            writer(dir.path()),
            Some(&cut),
            token.clone(),
        )
        .await;
        assert_eq!(
            target.trigger(CutSource::Key),
            CutOutcome::Unavailable(Unavailable::Stopping)
        );
        let control = cut.control();
        cut.finish(result.is_err()).await;
        result.expect("fixed recording");
        assert_eq!(control.snapshot().status, ManualSplitStatus::Completed);
        let files = files(dir.path(), "flv");
        assert_eq!(files.len(), 2);
        assert_eq!(frames(&read_flv(&files[1])).len(), FRAMES as usize);
    }

    #[tokio::test]
    async fn a_recording_can_be_cut_repeatedly() {
        let dir = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        let (target, cut) = start_with_cut(&token);
        let control = cut.control();
        let (items, receiver) = futures::channel::mpsc::unbounded();
        let mut recording = flv_recording(0, GOPS, FRAMES).into_iter();
        // The initialization, the first group and the second keyframe.
        for item in recording.by_ref().take(4 + 2 * FRAMES as usize + 1) {
            items.unbounded_send(Ok(item)).unwrap();
        }
        let output = dir.path().to_path_buf();
        let record = async {
            process_stream_with_span::<RawCutPipeline, FlvWriter>(
                &pipeline_config(),
                (),
                Box::pin(receiver),
                ChannelSpec::items(16),
                Span::none(),
                writer(&output),
                Some(&cut),
                CancellationToken::new(),
            )
            .await
        };
        let drive = async {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while control.snapshot().status != ManualSplitStatus::Completed {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the first cut completes");
            let first = control.snapshot().request_id;
            assert!(matches!(
                target.trigger(CutSource::Stdin),
                CutOutcome::Requested { .. }
            ));
            assert!(control.snapshot().request_id > first);
            for item in recording {
                items.unbounded_send(Ok(item)).unwrap();
            }
            drop(items);
        };
        let (result, ()) = tokio::join!(record, drive);
        cut.finish(result.is_err()).await;
        result.expect("raw recording");
        assert_eq!(control.snapshot().status, ManualSplitStatus::Completed);

        let files = files(dir.path(), "flv");
        assert_eq!(files.len(), 3);
        for file in &files {
            assert_eq!(frames(&read_flv(file)).len(), FRAMES as usize);
        }
    }

    #[tokio::test]
    async fn the_raw_stage_writes_exactly_what_the_direct_path_writes_until_a_cut() {
        let direct = tempfile::tempdir().unwrap();
        let staged = tempfile::tempdir().unwrap();
        let items = flv_recording(60_000, GOPS, FRAMES);
        process_raw_stream(
            item_stream(items.clone()),
            direct.path(),
            "rec-%i",
            &pipeline_config(),
        )
        .await
        .expect("direct recording");
        let (target, _notices) = CutTarget::with_notice_receiver();
        let cut = CutSession::start(&target, &CancellationToken::new(), Boundary::Keyframe);
        record_raw(staged.path(), items, Some(&cut)).await;
        cut.finish(false).await;

        let direct = files(direct.path(), "flv");
        let staged = files(staged.path(), "flv");
        assert_eq!(direct.len(), 1);
        assert_eq!(staged.len(), 1);
        // The writer rewrites the metadata on close, with its own creation
        // date; everything else is compared.
        let media = |path| {
            read_flv(path)
                .into_iter()
                .filter(|tag| !tag.is_script_tag())
                .map(|tag| (tag.tag_type(), tag.timestamp_ms, tag.data().clone()))
                .collect::<Vec<_>>()
        };
        let direct_media = media(&direct[0]);
        assert_eq!(direct_media.len(), (2 * GOPS * FRAMES + 2) as usize);
        assert_eq!(direct_media, media(&staged[0]));
    }

    #[tokio::test]
    async fn a_raw_recording_is_cut_with_the_timestamps_the_source_sent() {
        const START_MS: u32 = 60_000;
        let dir = tempfile::tempdir().unwrap();
        let (_target, cut) = start_with_cut(&CancellationToken::new());
        record_raw(
            dir.path(),
            flv_recording(START_MS, GOPS, FRAMES),
            Some(&cut),
        )
        .await;
        let control = cut.control();
        cut.finish(false).await;
        assert_eq!(control.snapshot().status, ManualSplitStatus::Completed);

        let files = files(dir.path(), "flv");
        assert_eq!(files.len(), 2);
        let first = read_flv(&files[0]);
        let second = read_flv(&files[1]);
        assert_eq!(frames(&first).len(), FRAMES as usize);
        assert_eq!(frames(&second).len(), ((GOPS - 1) * FRAMES) as usize);

        // The second file opens with the stream's metadata at 0, then the
        // codec configuration at the cut's timestamp, then the keyframe.
        let boundary = START_MS + FRAMES * FRAME_MS;
        assert!(second[0].is_script_tag());
        assert_eq!(second[0].timestamp_ms, 0);
        assert!(second[1].is_video_sequence_header());
        assert!(second[2].is_audio_sequence_header());
        assert!(second[3].is_key_frame_nalu());
        assert!(second[1..].iter().all(|tag| tag.timestamp_ms >= boundary));
        assert!(second[1..4].iter().all(|tag| tag.timestamp_ms == boundary));
        assert_eq!(
            first.last().map(|tag| tag.timestamp_ms),
            Some(boundary - FRAME_MS)
        );
    }

    #[tokio::test]
    async fn a_stopped_raw_recording_refuses_cuts_and_still_writes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        let (target, _notices) = CutTarget::with_notice_receiver();
        let cut = CutSession::start(&target, &token, Boundary::Keyframe);
        token.cancel();
        assert_eq!(
            target.trigger(CutSource::Key),
            CutOutcome::Unavailable(Unavailable::Stopping)
        );
        record_raw(dir.path(), flv_recording(0, GOPS, FRAMES), Some(&cut)).await;
        cut.finish(false).await;

        let files = files(dir.path(), "flv");
        assert_eq!(files.len(), 1);
        assert_eq!(frames(&read_flv(&files[0])).len(), (GOPS * FRAMES) as usize);
    }
}
