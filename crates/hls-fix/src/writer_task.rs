use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::PathBuf,
};

use hls::{HlsData, M4sData};
use pipeline_common::{
    FormatStrategy, PostWriteAction, ProgressConfig, ProtocolWriter, SplitReason, WriterConfig,
    WriterError, WriterProgress, WriterState, WriterStats, WriterTask, expand_filename_template,
};

#[cfg(feature = "progress")]
use tracing::Span;
use tracing::{debug, info, warn};
#[cfg(feature = "progress")]
use tracing_indicatif::span_ext::IndicatifSpanExt;

use crate::analyzer::HlsAnalyzer;
use crate::output_state::OutputState;

pub struct HlsFormatStrategy {
    analyzer: HlsAnalyzer,
    target_duration: f32,
    max_file_size: Option<u64>,
    last_split_reason: Option<SplitReason>,
    output_state: OutputState,
}

#[derive(Debug, thiserror::Error)]
pub enum HlsStrategyError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Analyzer error: {0}")]
    Analyzer(String),
}

impl HlsFormatStrategy {
    pub fn new(max_file_size: Option<u64>) -> Self {
        Self {
            analyzer: HlsAnalyzer::new(),
            target_duration: 0.0,
            max_file_size,
            last_split_reason: None,
            output_state: OutputState::default(),
        }
    }

    fn reset_for_new_file(&mut self) -> Result<(), HlsStrategyError> {
        self.analyzer.reset();
        self.target_duration = 0.0;
        self.last_split_reason = None;
        self.output_state.reset_file();
        Ok(())
    }

    /// Feed an already-written segment to `HlsAnalyzer::analyze_segment` for stats.
    /// Validation failures (e.g. AV1 sample checks) are logged and swallowed so a single
    /// non-conformant segment never aborts `WriterTask::run_from_channel`; the bytes have
    /// already been handed to the buffered file writer by the time this runs.
    fn analyze_written_segment(&mut self, item: &HlsData) {
        if let Err(err) = self.analyzer.analyze_segment(item) {
            warn!(error = %err, "HLS segment analysis failed; segment written, continuing");
        }
    }

    fn update_status(&self, state: &WriterState) {
        // Decorate the current writer span with a progress bar; compiled out
        // when the `progress` feature is disabled (headless library builds).
        #[cfg(feature = "progress")]
        {
            let span = Span::current();
            span.pb_set_position(state.bytes_written_current_file);
            span.pb_set_message(&format!(
                "{} | {} segments | {:.1}s",
                state.current_path.display(),
                state.items_written_current_file,
                self.target_duration
            ));
        }
        #[cfg(not(feature = "progress"))]
        let _ = state;
    }
}

impl FormatStrategy<HlsData> for HlsFormatStrategy {
    type Writer = BufWriter<std::fs::File>;
    type StrategyError = HlsStrategyError;

    fn create_writer(&self, path: &std::path::Path) -> Result<Self::Writer, Self::StrategyError> {
        debug!("Creating writer for path: {}", path.display());
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        Ok(BufWriter::with_capacity(1024 * 1024, file))
    }

    fn write_item(
        &mut self,
        writer: &mut Self::Writer,
        item: &HlsData,
    ) -> Result<u64, Self::StrategyError> {
        match item {
            HlsData::TsData(ts) => {
                writer.write_all(ts.data())?;
                self.target_duration += ts.segment.duration;
                self.analyze_written_segment(item);
            }
            HlsData::M4sData(M4sData::InitSegment(init)) => {
                if self.output_state.is_repeated_init(init) {
                    return Ok(0);
                }
                writer.write_all(&init.data)?;
                self.analyze_written_segment(item);
            }
            HlsData::M4sData(M4sData::Segment(segment)) => {
                if let Some(init) = self.output_state.pending_init().cloned() {
                    writer.write_all(&init.data)?;
                    self.analyze_written_segment(&HlsData::M4sData(M4sData::InitSegment(init)));
                }
                writer.write_all(&segment.data)?;
                self.target_duration += segment.segment.duration;
                self.analyze_written_segment(item);
            }
            HlsData::EndMarker(reason) => {
                self.last_split_reason = reason.clone();
            }
        }
        Ok(self.output_state.record(item))
    }

    fn should_rotate_file(&self, _config: &WriterConfig, _state: &WriterState) -> bool {
        // HLS rotation requires the next media item, including any init replay.
        false
    }

    fn should_rotate_before_item(
        &mut self,
        item: &HlsData,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> bool {
        if self.output_state.would_exceed(item, self.max_file_size) {
            self.last_split_reason = Some(SplitReason::SizeLimit);
            true
        } else {
            false
        }
    }

    fn next_file_path(&self, config: &WriterConfig, state: &WriterState) -> PathBuf {
        let sequence = state.file_sequence_number;

        let file_name = expand_filename_template(&config.file_name_template, Some(sequence));
        config
            .base_path
            .join(format!("{}.{}", file_name, config.file_extension))
    }

    fn on_file_open(
        &mut self,
        _writer: &mut Self::Writer,
        path: &std::path::Path,
        _config: &WriterConfig,
        _state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        self.reset_for_new_file()?;

        info!(path = %path.display(), "Opening segment");

        // Initialize the span's progress bar (compiled out without `progress`).
        #[cfg(feature = "progress")]
        {
            let span = Span::current();
            span.pb_set_message(&format!("Writing {}", path.display()));

            // Set progress bar length from max_file_size if available
            if let Some(max_size) = self.max_file_size {
                span.pb_set_length(max_size);
            }
        }

        Ok(0)
    }

    fn on_file_close(
        &mut self,
        _writer: &mut Self::Writer,
        path: &std::path::Path,
        _config: &WriterConfig,
        state: &WriterState,
    ) -> Result<u64, Self::StrategyError> {
        let items_written = state.items_written_current_file;
        let duration_secs = self.target_duration;

        info!(
            path = %path.display(),
            items = items_written,
            duration_secs = ?duration_secs,
            "Closed segment"
        );

        Ok(0)
    }

    fn after_item_written(
        &mut self,
        item: &HlsData,
        _bytes_written: u64,
        state: &WriterState,
    ) -> Result<PostWriteAction, Self::StrategyError> {
        self.update_status(state);
        if matches!(item, HlsData::EndMarker(_)) {
            if state.bytes_written_current_file == 0 {
                return Ok(PostWriteAction::None);
            }

            let stats = self
                .analyzer
                .build_stats()
                .map_err(HlsStrategyError::Analyzer)?;
            debug!("HLS stats: {:?}", stats);
            Ok(PostWriteAction::RotateOnNextItem)
        } else {
            Ok(PostWriteAction::None)
        }
    }

    fn current_media_duration_secs(&self) -> f64 {
        self.target_duration as f64
    }

    fn close_context(&self) -> Option<SplitReason> {
        self.last_split_reason.clone()
    }
}

/// Typed configuration for HLS writer.
pub struct HlsWriterConfig {
    pub output_dir: PathBuf,
    pub base_name: String,
    pub extension: String,
    /// Maximum output bytes, including initialization. Split before a media
    /// item would exceed it; a first oversized item stays intact. Zero disables
    /// this limit, and control markers always retain their explicit reason.
    pub max_file_size: Option<u64>,
}

pub struct HlsWriter {
    writer_task: WriterTask<HlsData, HlsFormatStrategy>,
}

impl HlsWriter {
    pub fn new(config: HlsWriterConfig) -> Self {
        let writer_config =
            WriterConfig::new(config.output_dir, config.base_name, config.extension);
        let strategy = HlsFormatStrategy::new(config.max_file_size);
        let writer_task = WriterTask::new(writer_config, strategy);
        Self { writer_task }
    }

    /// Set a callback to be invoked when a new segment starts recording.
    pub fn set_on_segment_start_callback<F>(&mut self, callback: F)
    where
        F: Fn(&std::path::Path, u32) + Send + Sync + 'static,
    {
        self.writer_task.set_on_file_open_callback(callback);
    }

    /// Set a callback to be invoked when a segment is completed.
    pub fn set_on_segment_complete_callback<F>(&mut self, callback: F)
    where
        F: Fn(&std::path::Path, u32, f64, u64, Option<&SplitReason>) + Send + Sync + 'static,
    {
        self.writer_task.set_on_file_close_callback(callback);
    }

    /// Set a progress callback with default intervals (1MB bytes, 1000ms time).
    pub fn set_progress_callback<F>(&mut self, callback: F)
    where
        F: Fn(WriterProgress) + Send + Sync + 'static,
    {
        self.writer_task.set_progress_callback(callback);
    }

    /// Set a progress callback with custom intervals.
    pub fn set_progress_callback_with_config<F>(&mut self, callback: F, config: ProgressConfig)
    where
        F: Fn(WriterProgress) + Send + Sync + 'static,
    {
        self.writer_task
            .set_progress_callback_with_config(callback, config);
    }

    /// Get the total media duration in seconds across all files.
    pub fn media_duration_secs(&self) -> f64 {
        self.writer_task.get_state().media_duration_secs_total
    }
}

impl ProtocolWriter for HlsWriter {
    type Item = HlsData;

    fn get_state(&self) -> &WriterState {
        self.writer_task.get_state()
    }

    fn run(
        &mut self,
        input: pipeline_common::PipelineReceiver<HlsData>,
    ) -> Result<WriterStats, WriterError> {
        self.writer_task.run_from_channel(input, |item, state| {
            !item.is_end_marker() || state.current_file_path.is_some()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use pipeline_common::{
        PipelineError, PipelineProvider, StreamerContext, config::PipelineConfig,
    };
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::test_support::{INIT, MEDIA0, MEDIA1, OTHER_INIT, OTHER_MEDIA, init, media};

    // Buffer a finite input before running the synchronous writer. These tests
    // need no worker thread, so a failed assertion cannot strand a channel wait.
    fn write_items(writer: &mut HlsWriter, items: Vec<HlsData>) -> WriterStats {
        let (tx, rx) = tokio::sync::mpsc::channel(items.len().max(1));
        for item in items {
            tx.try_send(Ok(item)).unwrap();
        }
        drop(tx);
        writer.run(rx.into()).unwrap()
    }

    fn through_pipeline(
        items: Vec<HlsData>,
        limit: Option<u64>,
        limiter: Option<bool>,
    ) -> Vec<HlsData> {
        let Some(segment_limiter) = limiter else {
            return items;
        };
        let pipeline = crate::HlsPipeline::with_config(
            StreamerContext::arc_new(CancellationToken::new()),
            &PipelineConfig {
                max_file_size: limit.unwrap_or(0),
                ..Default::default()
            },
            crate::HlsPipelineConfig {
                segment_limiter,
                ..Default::default()
            },
        )
        .build_pipeline();
        let mut output = Vec::new();
        pipeline
            .run(items.into_iter().map(Ok::<_, PipelineError>), &mut |item| {
                output.push(item.unwrap())
            })
            .unwrap();
        output
    }

    fn assert_size_modes(
        items: Vec<HlsData>,
        limit: Option<u64>,
        expected: &[Vec<u8>],
        duration: f64,
    ) {
        // Raw, processed with writer fallback, and processed with both guards.
        for limiter in [None, Some(false), Some(true)] {
            let dir = tempfile::tempdir().unwrap();
            let completed = Arc::new(Mutex::new(Vec::new()));
            let events = completed.clone();
            let mut writer = HlsWriter::new(HlsWriterConfig {
                output_dir: dir.path().into(),
                base_name: "test-%i".into(),
                extension: "bin".into(),
                max_file_size: limit,
            });
            writer.set_on_segment_complete_callback(move |_, sequence, duration, size, reason| {
                events
                    .lock()
                    .unwrap()
                    .push((sequence, size, duration, reason.cloned()));
            });
            let output = through_pipeline(items.clone(), limit, limiter);
            let stats = write_items(&mut writer, output);
            assert_eq!(
                stats.files_created as usize,
                expected.len(),
                "limiter={limiter:?}, limit={limit:?}"
            );
            assert_eq!(
                stats.bytes_written,
                expected.iter().map(|file| file.len() as u64).sum::<u64>()
            );
            assert_eq!(stats.duration_secs, duration);
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                expected.len()
            );
            let events = completed.lock().unwrap();
            assert_eq!(events.len(), expected.len());
            for (index, data) in expected.iter().enumerate() {
                assert_eq!(
                    std::fs::read(dir.path().join(format!("test-{index:03}.bin"))).unwrap(),
                    *data
                );
                assert_eq!(events[index].0, index as u32);
                assert_eq!(events[index].1, data.len() as u64);
                assert_eq!(
                    events[index].3,
                    (index + 1 < expected.len()).then_some(SplitReason::SizeLimit)
                );
            }
            assert_eq!(events.iter().map(|event| event.2).sum::<f64>(), duration);
        }
    }

    #[test]
    fn raw_and_processed_mp4_share_init_aware_size_limits() {
        let combined = [INIT, MEDIA0, MEDIA1].concat();
        let split = [[INIT, MEDIA0].concat(), [INIT, MEDIA1].concat()];
        for (limit, expected) in [
            (None, vec![combined.clone()]),
            (Some(0), vec![combined.clone()]),
            (Some(combined.len() as u64), vec![combined.clone()]),
            (Some(combined.len() as u64 - 1), split.to_vec()),
            (Some(1), split.to_vec()),
        ] {
            // Repeating the map must not consume the byte budget twice. Trailing
            // and consecutive end markers must not create extra output files.
            assert_size_modes(
                vec![
                    init(INIT),
                    media(MEDIA0),
                    init(INIT),
                    media(MEDIA1),
                    HlsData::end_marker(),
                    HlsData::end_marker(),
                ],
                limit,
                &expected,
                2.0,
            );
        }
    }

    #[test]
    fn raw_and_processed_ts_share_predictive_size_limits() {
        let inputs: Vec<_> = [0u8, 1, 2]
            .into_iter()
            .map(|byte| {
                HlsData::ts(
                    MediaSegment {
                        duration: 1.0,
                        ..Default::default()
                    },
                    vec![byte; 10].into(),
                )
            })
            .collect();
        let all = [vec![0; 10], vec![1; 10], vec![2; 10]].concat();
        for (limit, expected) in [
            (None, vec![all.clone()]),
            (Some(0), vec![all.clone()]),
            (Some(30), vec![all]),
            (
                Some(20),
                vec![[vec![0; 10], vec![1; 10]].concat(), vec![2; 10]],
            ),
            (Some(15), vec![vec![0; 10], vec![1; 10], vec![2; 10]]),
            (Some(1), vec![vec![0; 10], vec![1; 10], vec![2; 10]]),
        ] {
            assert_size_modes(inputs.clone(), limit, &expected, 3.0);
        }
    }

    #[test]
    fn explicit_boundary_reason_wins_at_the_size_limit_without_an_empty_successor() {
        for reason in [
            SplitReason::Manual { request_id: 42 },
            SplitReason::DurationLimit,
        ] {
            for has_successor in [false, true] {
                for limiter in [None, Some(false), Some(true)] {
                    let first_file_size = (INIT.len() + MEDIA0.len()) as u64;
                    let limit = Some(first_file_size);
                    let mut items = vec![
                        init(INIT),
                        media(MEDIA0),
                        HlsData::end_marker_with_reason(reason.clone()),
                        HlsData::end_marker(),
                    ];
                    if has_successor {
                        // No new init arrives: both paths must replay the latest one.
                        items.push(media(MEDIA1));
                    }
                    let dir = tempfile::tempdir().unwrap();
                    let completed = Arc::new(Mutex::new(Vec::new()));
                    let events = completed.clone();
                    let mut writer = HlsWriter::new(HlsWriterConfig {
                        output_dir: dir.path().into(),
                        base_name: "test-%i".into(),
                        extension: "mp4".into(),
                        max_file_size: limit,
                    });
                    writer.set_on_segment_complete_callback(move |_, sequence, _, size, reason| {
                        events
                            .lock()
                            .unwrap()
                            .push((sequence, size, reason.cloned()));
                    });
                    let stats = write_items(&mut writer, through_pipeline(items, limit, limiter));
                    let count = if has_successor { 2 } else { 1 };
                    assert_eq!(stats.files_created, count);
                    assert_eq!(
                        std::fs::read_dir(dir.path()).unwrap().count(),
                        count as usize
                    );
                    let events = completed.lock().unwrap();
                    assert_eq!(events[0], (0, first_file_size, Some(reason.clone())));
                    assert_eq!(events.len(), count as usize);
                    if has_successor {
                        assert_eq!(events[1], (1, (INIT.len() + MEDIA1.len()) as u64, None));
                        assert_eq!(
                            std::fs::read(dir.path().join("test-001.mp4")).unwrap(),
                            [INIT, MEDIA1].concat()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn size_rotation_restores_init_and_accounts_for_its_bytes() {
        for limit in [1, (INIT.len() + MEDIA0.len()) as u64] {
            let dir = tempfile::tempdir().unwrap();
            let mut writer = HlsWriter::new(HlsWriterConfig {
                output_dir: dir.path().into(),
                base_name: "test-%i".into(),
                extension: "mp4".into(),
                max_file_size: Some(limit),
            });
            let stats = write_items(&mut writer, vec![init(INIT), media(MEDIA0), media(MEDIA1)]);
            assert_eq!(stats.files_created, 2);
            assert_eq!(
                stats.bytes_written,
                (2 * INIT.len() + MEDIA0.len() + MEDIA1.len()) as u64
            );
            assert_eq!(stats.duration_secs, 2.0);
            assert_eq!(
                std::fs::read(dir.path().join("test-000.mp4")).unwrap(),
                [INIT, MEDIA0].concat()
            );
            assert_eq!(
                std::fs::read(dir.path().join("test-001.mp4")).unwrap(),
                [INIT, MEDIA1].concat()
            );
        }
    }

    #[test]
    fn explicit_new_init_supersedes_the_cached_init_at_a_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = HlsWriter::new(HlsWriterConfig {
            output_dir: dir.path().into(),
            base_name: "test-%i".into(),
            extension: "mp4".into(),
            max_file_size: Some((INIT.len() + MEDIA0.len()) as u64),
        });
        let stats = write_items(
            &mut writer,
            vec![
                init(INIT),
                media(MEDIA0),
                HlsData::end_marker(),
                init(OTHER_INIT),
                media(OTHER_MEDIA),
            ],
        );
        assert_eq!(stats.files_created, 2);
        assert_eq!(stats.duration_secs, 2.0);
        assert_eq!(
            std::fs::read(dir.path().join("test-001.mp4")).unwrap(),
            [OTHER_INIT, OTHER_MEDIA].concat()
        );
    }

    #[test]
    fn pipeline_preserves_media_across_a_repeated_map_without_duplicate_headers() {
        use pipeline_common::{
            PipelineError, PipelineProvider, StreamerContext, config::PipelineConfig,
        };
        use tokio_util::sync::CancellationToken;

        let pipeline = crate::HlsPipeline::with_config(
            StreamerContext::arc_new(CancellationToken::new()),
            &PipelineConfig::default(),
            crate::HlsPipelineConfig::default(),
        )
        .build_pipeline();
        let inputs = [init(INIT), media(MEDIA0), init(INIT), media(MEDIA1)];
        let mut output = Vec::new();
        pipeline
            .run(
                inputs.into_iter().map(Ok::<_, PipelineError>),
                &mut |item| output.push(item.unwrap()),
            )
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut writer = HlsWriter::new(HlsWriterConfig {
            output_dir: dir.path().into(),
            base_name: "test-%i".into(),
            extension: "mp4".into(),
            max_file_size: None,
        });
        let stats = write_items(&mut writer, output);
        let expected = [INIT, MEDIA0, MEDIA1].concat();
        assert_eq!(stats.files_created, 1);
        assert_eq!(stats.duration_secs, 2.0);
        assert_eq!(stats.bytes_written, expected.len() as u64);
        assert_eq!(
            std::fs::read(dir.path().join("test-000.mp4")).unwrap(),
            expected
        );
    }

    #[test]
    fn rotates_on_max_file_size_without_losing_or_duplicating_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = HlsWriter::new(HlsWriterConfig {
            output_dir: dir.path().into(),
            base_name: "test-%i".into(),
            extension: "ts".into(),
            max_file_size: Some(15),
        });
        // Payloads are opaque to this writer; distinct bytes expose loss/reordering.
        let items = [0u8, 1, 2]
            .into_iter()
            .map(|value| {
                HlsData::ts(
                    MediaSegment {
                        duration: 1.0,
                        ..Default::default()
                    },
                    Bytes::from(vec![value; 10]),
                )
            })
            .collect();
        let stats = write_items(&mut writer, items);
        assert_eq!(stats.files_created, 3);
        assert_eq!(stats.bytes_written, 30);
        assert_eq!(stats.duration_secs, 3.0);
        for index in 0..3 {
            assert_eq!(
                std::fs::read(dir.path().join(format!("test-{index:03}.ts"))).unwrap(),
                vec![index as u8; 10]
            );
        }
    }

    #[test]
    fn ignores_leading_end_markers() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = HlsWriter::new(HlsWriterConfig {
            output_dir: dir.path().into(),
            base_name: "test-%i".into(),
            extension: "ts".into(),
            max_file_size: None,
        });
        let stats = write_items(
            &mut writer,
            vec![HlsData::end_marker(), HlsData::end_marker()],
        );
        assert_eq!(stats.files_created, 0);
        assert_eq!(stats.bytes_written, 0);
        assert_eq!(stats.duration_secs, 0.0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn av1_validation_failure_does_not_abort_or_drop_written_media() {
        use mp4::test_support::{make_init_with_video_sample_entry, make_media_segment_for_track};
        let dir = tempfile::tempdir().unwrap();
        let mut writer = HlsWriter::new(HlsWriterConfig {
            output_dir: dir.path().into(),
            base_name: "test-%i".into(),
            extension: "mp4".into(),
            max_file_size: None,
        });
        let init = HlsData::mp4_init(
            MediaSegment::empty(),
            make_init_with_video_sample_entry(1, *b"av01"),
        );
        let media = |sample: &[u8]| {
            HlsData::mp4_segment(
                MediaSegment {
                    duration: 1.0,
                    ..Default::default()
                },
                make_media_segment_for_track(1, sample),
            )
        };
        // These structural AV1 fixtures test OBU validation, not video decoding.
        // The temporal delimiter is disallowed by the analyzer's default policy.
        let rejected = media(&[0x12, 0x00]);
        let accepted = media(&[0x32, 0x01, 0xaa]);
        let mut analyzer = HlsAnalyzer::new();
        analyzer.analyze_segment(&init).unwrap();
        assert!(
            analyzer
                .analyze_segment(&rejected)
                .unwrap_err()
                .contains("AV1")
        );
        analyzer.analyze_segment(&accepted).unwrap();
        let expected = [init.as_ref(), rejected.as_ref(), accepted.as_ref()].concat();
        let stats = write_items(&mut writer, vec![init, rejected, accepted]);
        assert_eq!(stats.files_created, 1);
        assert_eq!(stats.bytes_written, expected.len() as u64);
        assert_eq!(stats.duration_secs, 2.0);
        assert_eq!(
            std::fs::read(dir.path().join("test-000.mp4")).unwrap(),
            expected
        );
    }
}
