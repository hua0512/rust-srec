#[cfg(test)]
mod fixtures;
mod flv;
mod generic;
mod hls;

use crate::cut::{CutTarget, Unavailable};
use crate::output::pipe_hls_strategy::SegmentedProtocol;
use crate::output::provider::PipeStream;
use crate::utils::spans;
use crate::{config::ProgramConfig, error::AppError};
use mesio_engine::{
    DownloadHandle, DownloadRequest, DownloadTerminal, DownloaderSession, MesioConfig,
    MesioDownloader,
};
use pipeline_common::CancellationToken;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{Instrument, Level, Span, error, info, span, warn};

/// What a URL recording needs besides its download session.
#[derive(Clone, Copy)]
pub(crate) struct Recording<'a> {
    pub(crate) url: &'a str,
    pub(crate) output_dir: &'a Path,
    pub(crate) config: &'a ProgramConfig,
    pub(crate) name_template: &'a str,
    pub(crate) token: &'a CancellationToken,
    pub(crate) cut_target: &'a CutTarget,
}

/// How long a session whose items ran out may take to report how it
/// ended.
const TERMINAL_REPORT_WAIT: Duration = Duration::from_secs(5);

/// How a recording of a URL ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordingEnd {
    Finished,
    /// The source restarted its stream; everything before the restart was
    /// written, and what follows needs a new recording.
    SourceRestarted,
}

/// Ends a download session on every exit path: cancels it and reports the
/// events it dropped.
pub(crate) struct SessionCleanup {
    handle: DownloadHandle,
    progress: Option<JoinHandle<()>>,
    span: Span,
    finished: bool,
}

impl SessionCleanup {
    /// `progress` renders the session's events into `span`.
    pub(crate) fn new(
        handle: DownloadHandle,
        progress: Option<JoinHandle<()>>,
        span: Span,
    ) -> Self {
        Self {
            handle,
            progress,
            span,
            finished: false,
        }
    }

    /// How the session ended, once its items ran out: whether its source
    /// restarted the stream. Call before [`Self::finish`], which cancels
    /// the session.
    pub(crate) async fn recording_end(&self) -> RecordingEnd {
        let terminal = tokio::time::timeout(TERMINAL_REPORT_WAIT, self.handle.join()).await;
        match terminal {
            Ok(Some(Ok(DownloadTerminal::Restarted))) => RecordingEnd::SourceRestarted,
            _ => RecordingEnd::Finished,
        }
    }

    /// Also waits for the progress task to render the last events. Drop the
    /// session's item stream first: it can hold the event sender that task
    /// waits on.
    pub(crate) async fn finish(mut self) {
        self.finished = true;
        self.handle.cancel();
        if let Some(task) = self.progress.take()
            && let Err(error) = task.await
        {
            warn!(%error, "download progress task failed");
        }
        spans::summarize_dropped_events(&self.handle, &self.span);
    }
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        // An early return: the progress task is left to end with the event
        // stream.
        if !self.finished {
            self.handle.cancel();
            spans::summarize_dropped_events(&self.handle, &self.span);
        }
    }
}

/// Determine the type of input and process accordingly
pub async fn process_inputs(
    inputs: &[String],
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    token: &CancellationToken,
    cut_target: &CutTarget,
) -> Result<(), AppError> {
    if inputs.is_empty() {
        return Err(AppError::InvalidInput(
            "No input files or URLs provided".to_string(),
        ));
    }

    let inputs_len = inputs.len();

    // Create a span for overall processing
    let processing_span = span!(Level::INFO, "processing_inputs", count = inputs_len);
    processing_span.in_scope(|| {
        info!(
            inputs_count = inputs_len,
            "Starting processing of {} input{}",
            inputs_len,
            if inputs_len == 1 { "" } else { "s" }
        );
    });

    // Pipe output is one continuous stream, with no file to cut.
    let pipe_mode = PipeStream::of(config.output_format).is_some();
    let _pipe_output = pipe_mode.then(|| cut_target.set_unavailable(Unavailable::PipeOutput));

    // Inputs whose source restarted its stream: they are reported once all
    // inputs are processed.
    let mut restarted = Vec::new();

    let downloader = MesioDownloader::new(MesioConfig {
        flv: config.flv_config.clone().unwrap_or_default(),
        hls: config.hls_config.clone().unwrap_or_default(),
        dash: config.dash_config.clone().unwrap_or_default(),
        token: token.clone(),
    });

    // Process each input
    for (index, input) in inputs.iter().enumerate() {
        let input_index = index + 1;

        // trim urls for better usability
        let input = input.trim();

        // Create a span for this specific input
        let input_span = span!(Level::INFO, "process_input", index = input_index, input = %input);

        // Process based on input type
        if input.starts_with("http://") || input.starts_with("https://") {
            let request = DownloadRequest::from_url(input)?
                .with_protocol(config.protocol.clone())
                .with_cancel(token.clone());
            let session = downloader.start(request).await?;
            let recording = Recording {
                url: input,
                output_dir,
                config,
                name_template,
                token,
                cut_target,
            };

            match session {
                DownloaderSession::Flv(session) => {
                    flv::process_flv_stream(recording, session)
                        .instrument(input_span.clone())
                        .await?;
                }
                DownloaderSession::Hls(session) => {
                    hls::process_hls_stream(SegmentedProtocol::Hls, recording, session)
                        .instrument(input_span.clone())
                        .await?;
                }
                // DASH output is fMP4 `HlsData`, processed and written like
                // HLS.
                DownloaderSession::Dash(session) => {
                    let end = hls::process_hls_stream(SegmentedProtocol::Dash, recording, session)
                        .instrument(input_span.clone())
                        .await?;
                    if end == RecordingEnd::SourceRestarted {
                        warn!(
                            input,
                            "The stream restarted; this recording ends here. Run mesio again to record the new stream."
                        );
                        restarted.push(input.to_string());
                    }
                }
            }
        } else {
            // It's a file path
            let _local_file =
                (!pipe_mode).then(|| cut_target.set_unavailable(Unavailable::LocalFile));
            let path = PathBuf::from(input);
            if path.exists() && path.is_file() {
                // For files, check the extension to determine the type
                if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
                    match extension.to_lowercase().as_str() {
                        "flv" => {
                            flv::process_file(&path, output_dir, config, token)
                                .instrument(input_span.clone())
                                .await?;
                        }
                        // "m3u8" | "m3u" => {
                        //     hls::process_hls_file(&path, output_dir, config, &progress_manager).await?;
                        // },
                        _ => {
                            error!("Unsupported file extension for: {input}");
                            return Err(AppError::InvalidInput(format!(
                                "Unsupported file extension: {input}"
                            )));
                        }
                    }
                } else {
                    error!("File without extension: {input}");
                    return Err(AppError::InvalidInput(format!(
                        "File without extension: {input}"
                    )));
                }
            } else {
                error!(
                    "Input is neither a valid URL nor an existing file: {}",
                    input
                );
                return Err(AppError::InvalidInput(format!("Invalid input: {input}")));
            }
        }
    }

    if !restarted.is_empty() {
        return Err(AppError::StreamRestarted(restarted.join(", ")));
    }
    Ok(())
}
