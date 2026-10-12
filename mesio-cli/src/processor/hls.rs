use crate::cut::{Boundary, CutSession, CutTarget};
use crate::output::pipe_hls_strategy::{PipeHlsStrategy, SegmentedProtocol};
use crate::processor::generic::process_pipe_stream;
use crate::utils::spans;
use crate::{config::ProgramConfig, error::AppError, utils::expand_name_url};
use futures::{StreamExt, stream};
use hls::HlsData;
use hls_fix::{HlsPipeline, HlsWriter, HlsWriterConfig};
use mesio_engine::{DownloadError, DownloadSession};
use pipeline_common::CancellationToken;
use pipeline_common::PipelineError;
use std::path::Path;
use std::time::Instant;
use tracing::{Level, debug, info, span, warn};
use tracing_indicatif::span_ext::IndicatifSpanExt;

/// Process an HLS stream, or a DASH stream (also delivered as `HlsData`)
/// with `protocol` naming it in spans and logs.
#[allow(clippy::too_many_arguments)]
pub async fn process_hls_stream(
    protocol: SegmentedProtocol,
    url_str: &str,
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    session: DownloadSession<HlsData>,
    token: &CancellationToken,
    cut_target: &CutTarget,
) -> Result<u64, AppError> {
    // Check if we're in pipe output mode
    let is_pipe_mode = config.output_format.is_pipe();

    // Only create output directory for file mode
    if !is_pipe_mode {
        tokio::fs::create_dir_all(output_dir).await?;
    }

    let start_time = Instant::now();
    let label = protocol.label();

    let base_name = expand_name_url(name_template, url_str)?;
    // Create the writer progress span up-front so downloads inherit it
    // Note: Progress bars are disabled in pipe mode via main.rs configuration
    let writer_span = span!(Level::INFO, "writer_processing");

    // Only initialize span visuals if not in pipe mode
    if !is_pipe_mode {
        spans::init_writing_span(&writer_span, format!("Writing {label} {base_name}"));
    }

    // Span names must be literals.
    let download_span = match protocol {
        SegmentedProtocol::Hls => {
            span!(parent: &writer_span, Level::INFO, "download_hls", url = %url_str)
        }
        SegmentedProtocol::Dash => {
            span!(parent: &writer_span, Level::INFO, "download_dash", url = %url_str)
        }
    };

    // Only initialize download span visuals if not in pipe mode
    if !is_pipe_mode {
        spans::init_hls_download_span(&download_span, format!("Downloading {}", url_str));
    }

    let DownloadSession {
        items: mut stream,
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
    let cleanup_session = |progress_task: Option<tokio::task::JoinHandle<()>>| {
        let handle = handle.clone();
        async move {
            handle.cancel();
            if let Some(task) = progress_task
                && let Err(error) = task.await
            {
                warn!(%error, "download progress task failed");
            }
        }
    };

    // Peek at the first segment to determine the file extension. An end
    // marker before any data (a DASH stream that ended at once) carries no
    // output.
    let (first_segment, extension) = loop {
        match stream.next().await {
            Some(Ok(HlsData::EndMarker(_))) => continue,
            Some(Ok(segment @ HlsData::TsData(_))) => break (segment, "ts"),
            Some(Ok(segment @ HlsData::M4sData(_))) => break (segment, "m4s"),
            // The item stream can hold the session's event sender, so it
            // is dropped before waiting for the event stream to end.
            Some(Err(e)) => {
                drop(stream);
                cleanup_session(progress_task).await;
                return Err(AppError::Download(e));
            }
            None => {
                info!("{label} stream is empty.");
                drop(stream);
                cleanup_session(progress_task).await;
                return Err(AppError::Download(DownloadError::source_exhausted(
                    format!("{label} stream is empty"),
                )));
            }
        }
    };

    info!(
        "Detected {label} stream type: {}. Saving with .{} extension.",
        extension.to_uppercase(),
        extension
    );

    // Prepend the first segment back to the stream
    let stream_with_first_segment = stream::once(async { Ok(first_segment) }).chain(stream);
    let stream = stream_with_first_segment;

    let hls_pipe_config = config.hls_pipeline_config.clone();
    debug!("Pipeline config: {:?}", hls_pipe_config);

    let stream = stream.map(|r| r.map_err(|e| PipelineError::Strategy(Box::new(e))));

    // Use pipe output strategy when stdout mode is active
    let stats = if is_pipe_mode {
        // Pipe mode: write directly to stdout using PipeHlsStrategy
        let pipe_stats = process_pipe_stream(
            Box::pin(stream),
            &config.pipeline_config,
            PipeHlsStrategy::for_protocol(protocol),
            extension,
        )
        .await?;

        // Log completion statistics for pipe mode
        let elapsed = start_time.elapsed();
        info!(
            url = %url_str,
            duration = ?elapsed,
            items_written = pipe_stats.items_written,
            bytes_written = pipe_stats.bytes_written,
            segment_count = pipe_stats.segment_count,
            output_mode = %config.output_format,
            "{label} pipe output complete"
        );

        handle.cancel();
        spans::summarize_dropped_events(&handle, &download_span);
        return Ok(pipe_stats.items_written as u64);
    } else {
        let max_file_size = if config.pipeline_config.max_file_size > 0 {
            Some(config.pipeline_config.max_file_size)
        } else {
            None
        };

        let cut = CutSession::start(cut_target, token, Boundary::Segment);
        let result = crate::processor::generic::process_stream_with_span::<HlsPipeline, HlsWriter>(
            &config.pipeline_config,
            hls_pipe_config,
            Box::pin(stream),
            HlsPipeline::channel_spec(config.pipeline_config.channel_size),
            writer_span.clone(),
            |_writer_span| {
                HlsWriter::new(HlsWriterConfig {
                    output_dir: output_dir.to_path_buf(),
                    base_name: base_name.to_string(),
                    extension: extension.to_string(),
                    max_file_size,
                })
            },
            Some(&cut),
            token.clone(),
        )
        .await;
        cut.finish(result.is_err()).await;
        result?
    };

    // Only update progress bar finish message if not in pipe mode
    if !is_pipe_mode {
        download_span.pb_set_finish_message(&format!("Downloaded {}", url_str));
    }
    handle.cancel();
    if let Some(task) = progress_task
        && let Err(error) = task.await
    {
        warn!(%error, "download progress task failed");
    }
    spans::summarize_dropped_events(&handle, &download_span);
    drop(download_span);

    let elapsed = start_time.elapsed();

    // Log completion (goes to stderr in pipe mode)
    info!(
        url = %url_str,
        items = stats.items_written,
        files = stats.files_created,
        duration = ?elapsed,
        output_mode = %config.output_format,
        "{label} download complete"
    );

    Ok(stats.items_written as u64)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use hls_fix::HlsPipelineConfig;
    use m3u8_rs::MediaSegment;
    use mp4::test_support::{make_box, make_full_box};
    use pipeline_common::{ManualSplitStatus, config::PipelineConfig};

    use super::*;
    use crate::cut::{CutOutcome, CutSource};
    use crate::processor::fixtures::{files, item_stream};

    const SYNC: u32 = 0x0200_0000;
    const NON_SYNC: u32 = 0x0101_0000;

    /// An init segment with one video track.
    fn init() -> Bytes {
        let mut track_header = vec![0; 12];
        track_header[8..12].copy_from_slice(&1u32.to_be_bytes());
        let mut handler = vec![0; 8];
        handler[4..8].copy_from_slice(b"vide");
        let mut track = make_full_box(b"tkhd", 0, 0, &track_header);
        track.extend(make_box(b"mdia", &make_full_box(b"hdlr", 0, 0, &handler)));
        let mut defaults = vec![0; 20];
        defaults[..4].copy_from_slice(&1u32.to_be_bytes());
        let mut movie = make_box(b"trak", &track);
        movie.extend(make_box(b"mvex", &make_full_box(b"trex", 0, 0, &defaults)));
        Bytes::from(make_box(b"moov", &movie))
    }

    /// A one-sample fragment with `flags`, its payload marked by `value`.
    fn fragment(flags: u32, value: u8) -> Bytes {
        let mut fragment = make_full_box(b"tfhd", 0, 0x020000, &1u32.to_be_bytes());
        let mut samples = 1u32.to_be_bytes().to_vec();
        samples.extend_from_slice(&flags.to_be_bytes());
        fragment.extend(make_full_box(b"trun", 0, 4, &samples));
        let mut data = make_box(b"moof", &make_box(b"traf", &fragment));
        data.extend(make_box(b"mdat", &[value; 16]));
        Bytes::from(data)
    }

    fn segment() -> MediaSegment {
        MediaSegment {
            duration: 1.0,
            ..Default::default()
        }
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[tokio::test]
    async fn an_fmp4_recording_is_cut_at_the_next_independent_fragment() {
        let dir = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        let (target, _notices) = CutTarget::with_notice_receiver();
        let cut = CutSession::start(&target, &token, Boundary::Segment);
        // Requested before any media: the first fragment cannot end an empty
        // file, and the second is not independent.
        assert!(matches!(
            target.trigger(CutSource::Key),
            CutOutcome::Requested { .. }
        ));

        let init = init();
        let fragments = [
            fragment(SYNC, 1),
            fragment(NON_SYNC, 2),
            fragment(SYNC, 3),
            fragment(NON_SYNC, 4),
        ];
        let mut items = vec![HlsData::mp4_init(segment(), init.clone())];
        items.extend(
            fragments
                .iter()
                .map(|data| HlsData::mp4_segment(segment(), data.clone())),
        );

        let output_dir = dir.path().to_path_buf();
        let result = crate::processor::generic::process_stream_with_span::<HlsPipeline, HlsWriter>(
            &PipelineConfig::builder().channel_size(16).build(),
            HlsPipelineConfig::builder().build(),
            item_stream(items),
            HlsPipeline::channel_spec(16),
            tracing::Span::none(),
            move |_| {
                HlsWriter::new(HlsWriterConfig {
                    output_dir,
                    base_name: "rec-%i".to_owned(),
                    extension: "m4s".to_owned(),
                    max_file_size: None,
                })
            },
            Some(&cut),
            token.clone(),
        )
        .await;
        let control = cut.control();
        cut.finish(result.is_err()).await;
        result.expect("fMP4 recording");
        assert_eq!(control.snapshot().status, ManualSplitStatus::Completed);

        let files = files(dir.path(), "m4s");
        assert_eq!(files.len(), 2);
        let first = std::fs::read(&files[0]).unwrap();
        let second = std::fs::read(&files[1]).unwrap();
        assert!(first.starts_with(&init));
        assert!(second.starts_with(&init));
        for (index, fragment) in fragments.iter().enumerate() {
            let in_first = contains(&first, fragment);
            let in_second = contains(&second, fragment);
            assert_eq!(
                (in_first, in_second),
                (index < 2, index >= 2),
                "fragment {index}"
            );
        }
    }
}
