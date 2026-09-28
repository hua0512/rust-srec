//! Continuous acquisition with private chunks and asynchronous file finalization.
//!
//! Chunk timestamps stay on the producer timeline. Concat manifests specify
//! both inpoint and duration; inferring duration from container metadata adds
//! audio-tail padding at every join. Recovery manifests outlive failed jobs.

use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use chrono::{DateTime, Utc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::warn;

use super::{
    DownloadConfig, DownloadFailureKind, DownloadHandle, EngineStartError, SegmentEvent,
    SegmentInfo,
};

pub(super) const LIST_NAME: &str = "chunks.csv";

#[cfg(test)]
mod tests;

pub(super) fn supports(config: &DownloadConfig) -> bool {
    matches!(
        config.output_format.as_str(),
        "mp4" | "mkv" | "flv" | "ts" | "mov"
    )
}

pub(super) fn configure_args(args: &mut Vec<String>, config: &DownloadConfig) {
    // The engine builds a normal segmented command for the private config.
    // These final options override its wall-clock naming and timestamp reset.
    args.pop();
    args.extend([
        "-reset_timestamps".into(),
        "0".into(),
        "-strftime".into(),
        "0".into(),
        "-segment_list".into(),
        config
            .output_dir
            .join(LIST_NAME)
            .to_string_lossy()
            .replace('\\', "/"),
        "-segment_list_type".into(),
        "csv".into(),
        "-segment_list_size".into(),
        "0".into(),
    ]);
    args.push(chunk_pattern(&config.output_dir));
}

/// The segment muxer expands `%` in the whole output pattern, so a literal `%`
/// in the directory must be doubled. The `-segment_list` path is not a pattern.
fn chunk_pattern(output_dir: &Path) -> String {
    PathBuf::from(output_dir.to_string_lossy().replace('%', "%%"))
        .join("chunk-%08d.mkv")
        .to_string_lossy()
        .replace('\\', "/")
}

fn failure(message: impl Into<String>) -> EngineStartError {
    EngineStartError::new(DownloadFailureKind::Processing, message)
}

fn io_error(op: &'static str, path: &Path, source: std::io::Error) -> EngineStartError {
    // Root-wide classification requires recording a gate failure first. These
    // adapter errors stay ordinary I/O; producer output errors retain their gate event.
    EngineStartError::new(
        DownloadFailureKind::Io,
        crate::Error::io_path(op, path, source).to_string(),
    )
}

async fn send(handle: &DownloadHandle, event: SegmentEvent) -> Result<(), EngineStartError> {
    handle
        .event_tx
        .send(event)
        .await
        .map_err(|_| failure("Recording event receiver closed"))
}

async fn reserve_output_path(
    directory: &Path,
    name: &str,
    format: &str,
) -> Result<PathBuf, EngineStartError> {
    for suffix in 0..=u32::MAX {
        let filename = if suffix == 0 {
            format!("{name}.{format}")
        } else {
            format!("{name}-{suffix:03}.{format}")
        };
        let path = directory.join(filename);
        // Reserving, rather than checking existence first, also protects against
        // another recording choosing the same template while this one finalizes.
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error("reserve recording file", &path, error)),
        }
    }
    Err(failure(
        "No available recording filename after collision suffixes",
    ))
}

#[derive(Debug)]
struct Chunk {
    name: String,
    start: f64,
    end: f64,
}

#[derive(Default)]
struct ChunkList {
    file: Option<tokio::fs::File>,
    pending: String,
    entries: VecDeque<Chunk>,
}

impl ChunkList {
    /// Returns the timing record for the closed chunk at `path`, or `None` when
    /// the producer has not written one. FFmpeg writes a record before opening
    /// the next chunk, so only the final chunk of a crashed or killed producer
    /// can lack one.
    async fn next(
        &mut self,
        directory: &Path,
        path: &Path,
    ) -> Result<Option<Chunk>, EngineStartError> {
        let list_path = directory.join(LIST_NAME);
        if self.file.is_none() {
            self.file = Some(
                tokio::fs::File::open(&list_path)
                    .await
                    .map_err(|e| io_error("open chunk list", &list_path, e))?,
            );
        }
        if let Some(file) = &mut self.file {
            file.read_to_string(&mut self.pending)
                .await
                .map_err(|e| io_error("read chunk list", &list_path, e))?;
        }
        while let Some(end) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=end).collect();
            let fields: Vec<_> = line.trim().split(',').collect();
            if fields.len() != 3 {
                return Err(failure("Invalid chunk timing record"));
            }
            let name = fields[0].trim_matches('"');
            // Filenames are generated locally, never taken from the source playlist.
            if !name.starts_with("chunk-")
                || !name.ends_with(".mkv")
                || !name[6..name.len() - 4].bytes().all(|c| c.is_ascii_digit())
            {
                return Err(failure("Unexpected path in chunk timing record"));
            }
            let start = fields[1]
                .parse::<f64>()
                .map_err(|_| failure("Invalid chunk start time"))?;
            let end = fields[2]
                .parse::<f64>()
                .map_err(|_| failure("Invalid chunk end time"))?;
            if !start.is_finite() || !end.is_finite() || end < start {
                return Err(failure("Invalid chunk time range"));
            }
            self.entries.push_back(Chunk {
                name: name.to_string(),
                start,
                end,
            });
        }
        let Some(chunk) = self.entries.pop_front() else {
            return Ok(None);
        };
        if path.file_name().and_then(|name| name.to_str()) != Some(&chunk.name) {
            return Err(failure("Chunk events and timing records are out of order"));
        }
        Ok(Some(chunk))
    }
}

struct Group {
    index: u32,
    path: PathBuf,
    manifest: PathBuf,
    recovery: PathBuf,
    chunks: Vec<Chunk>,
    bytes: u64,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    reason: Option<&'static str>,
    request_id: Option<u64>,
    published: bool,
}

impl Drop for Group {
    /// A group that never published leaves only its reservation at `path`.
    /// Drop also covers a finalizer aborted by forced shutdown, so this uses
    /// synchronous calls on a single small file. Anything with content is
    /// kept: the reservation may have been replaced by something else.
    fn drop(&mut self) {
        if self.published {
            return;
        }
        match std::fs::metadata(&self.path) {
            Ok(metadata) if metadata.is_file() && metadata.len() == 0 => {
                if let Err(error) = std::fs::remove_file(&self.path) {
                    warn!(path = %self.path.display(), %error, "Could not remove unused recording file reservation");
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                warn!(path = %self.path.display(), %error, "Could not inspect unused recording file reservation");
            }
        }
    }
}

impl Group {
    async fn new(
        handle: &DownloadHandle,
        directory: &Path,
        index: u32,
        started_at: DateTime<Utc>,
    ) -> Result<Self, EngineStartError> {
        let config = handle.config_snapshot();
        let name = pipeline_common::expand_filename_template(
            &config.filename_template,
            Some(config.initial_segment_index + index),
        );
        let path = reserve_output_path(&config.output_dir, &name, &config.output_format).await?;
        // Owning the reservation first lets `Drop` release it on every later error.
        let group = Self {
            index,
            path,
            manifest: directory.join(format!("group-{index}.ffconcat")),
            recovery: directory.join(format!("group-{index}.json")),
            chunks: Vec::new(),
            bytes: 0,
            started_at,
            ended_at: started_at,
            reason: None,
            request_id: None,
            published: false,
        };
        tokio::fs::write(&group.manifest, b"ffconcat version 1.0\n")
            .await
            .map_err(|e| io_error("write recovery manifest", &group.manifest, e))?;
        let metadata = serde_json::json!({
            "output_path": group.path, "manifest": group.manifest, "session_id": config.session_id,
            "download_id": handle.id, "segment_index": config.initial_segment_index + index,
            "started_at": started_at, "format": config.output_format,
        });
        let bytes = serde_json::to_vec_pretty(&metadata).map_err(|e| failure(e.to_string()))?;
        tokio::fs::write(&group.recovery, bytes)
            .await
            .map_err(|e| io_error("write recovery metadata", &group.recovery, e))?;
        send(
            handle,
            SegmentEvent::SegmentStarted {
                path: group.path.clone(),
                sequence: index,
                started_at,
            },
        )
        .await?;
        Ok(group)
    }

    async fn append(&mut self, chunk: Chunk, info: &SegmentInfo) -> Result<(), EngineStartError> {
        let text = format!(
            "file '{}'\ninpoint {:.6}\nduration {:.6}\n",
            chunk.name,
            chunk.start,
            chunk.end - chunk.start
        );
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&self.manifest)
            .await
            .map_err(|e| io_error("open recovery manifest", &self.manifest, e))?;
        file.write_all(text.as_bytes())
            .await
            .map_err(|e| io_error("append recovery manifest", &self.manifest, e))?;
        self.bytes = self.bytes.saturating_add(info.size_bytes);
        self.ended_at = info.completed_at;
        self.chunks.push(chunk);
        Ok(())
    }

    fn duration(&self) -> f64 {
        self.chunks
            .first()
            .zip(self.chunks.last())
            .map_or(0.0, |(first, last)| last.end - first.start)
    }
}

async fn ingest(
    handle: &DownloadHandle,
    directory: &Path,
    mut events: mpsc::Receiver<SegmentEvent>,
    jobs: mpsc::Sender<Group>,
    completed: &AtomicU32,
) -> Result<Option<SegmentEvent>, EngineStartError> {
    let config = handle.config_snapshot();
    let mut group: Option<Group> = None;
    let mut index = 0;
    let mut list = ChunkList::default();
    let mut terminal = None;
    // A closed chunk still waiting for its timing record. Only the producer's
    // final chunk may stay here; any later chunk event makes it an error.
    let mut unrecorded: Option<SegmentInfo> = None;
    while let Some(event) = events.recv().await {
        if matches!(
            event,
            SegmentEvent::SegmentStarted { .. } | SegmentEvent::SegmentCompleted(_)
        ) && let Some(info) = unrecorded.take()
        {
            let chunk = list.next(directory, &info.path).await?.ok_or_else(|| {
                failure("Closed chunk has no finalized timing record; recovery files retained")
            })?;
            if let Some(group) = &mut group {
                group.append(chunk, &info).await?;
            }
            handle.manual_split.enable();
        }
        match event {
            SegmentEvent::SegmentStarted { started_at, .. } => {
                if let Some(current) = &mut group
                    && !current.chunks.is_empty()
                {
                    let request = handle.manual_split.begin();
                    current.reason = if request.is_some() {
                        Some("manual")
                    } else if config.max_segment_duration_secs > 0
                        && current.duration() >= config.max_segment_duration_secs as f64
                    {
                        Some("duration_limit")
                    } else if config.max_segment_size_bytes > 0
                        && current.bytes >= config.max_segment_size_bytes
                    {
                        Some("size_limit")
                    } else {
                        None
                    };
                    current.request_id = request;
                    if current.reason.is_some()
                        && let Some(finished) = group.take()
                    {
                        jobs.try_send(finished).map_err(|_| {
                            failure(
                                "Recording finalization backlog is full; recovery files retained",
                            )
                        })?;
                        index += 1;
                    }
                }
                if group.is_none() {
                    group = Some(Group::new(handle, directory, index, started_at).await?);
                }
            }
            SegmentEvent::SegmentCompleted(info) => match list.next(directory, &info.path).await? {
                Some(chunk) => {
                    if let Some(group) = &mut group {
                        group.append(chunk, &info).await?;
                    }
                    // At least one closed, playable chunk is required before offering a cut.
                    handle.manual_split.enable();
                }
                None => unrecorded = Some(info),
            },
            SegmentEvent::Progress(mut progress) => {
                progress.segments_completed = completed.load(Ordering::Acquire);
                progress.current_segment = group
                    .as_ref()
                    .map(|group| group.path.to_string_lossy().into_owned());
                send(handle, SegmentEvent::Progress(progress)).await?;
            }
            event @ SegmentEvent::OutputIoError { .. } => send(handle, event).await?,
            event @ (SegmentEvent::DownloadCompleted { .. }
            | SegmentEvent::DownloadFailed { .. }) => {
                terminal = Some(event);
                break;
            }
        }
    }
    handle.manual_split.end_acquisition();
    if let Some(info) = unrecorded {
        // A crashed or killed producer never closes its last chunk, so FFmpeg
        // never records its timing. Its length cannot be trusted, so it stays
        // out of the group and remains in the staging directory for manual
        // inspection; every recorded chunk before it is still finalized.
        match list.next(directory, &info.path).await? {
            Some(chunk) => {
                if let Some(group) = &mut group {
                    group.append(chunk, &info).await?;
                }
            }
            None => warn!(
                path = %info.path.display(),
                "Final recording chunk has no timing record; retained in the staging directory"
            ),
        }
    }
    if let Some(group) = group
        && !group.chunks.is_empty()
    {
        jobs.send(group)
            .await
            .map_err(|_| failure("Recording finalizer stopped; recovery files retained"))?;
    }
    Ok(terminal)
}

async fn finalize(
    handle: &DownloadHandle,
    binary: &str,
    group: &mut Group,
) -> Result<u64, EngineStartError> {
    let format = handle.config_snapshot().output_format;
    let temporary = group.manifest.with_extension(format!("partial.{format}"));
    let mut command = process_utils::tokio_command(binary);
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&group.manifest)
        .args(["-map", "0", "-c", "copy"]);
    if matches!(format.as_str(), "mp4" | "mov") {
        command.args(["-movflags", "+faststart"]);
    }
    command
        .arg(&temporary)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::utils::configure_ffmpeg_locale(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| io_error("start recording finalizer", &temporary, e))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| failure("Recording finalizer stderr is unavailable"))?;
    let diagnostics = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        let mut reader = super::utils::OutputRecordReader::new(stderr);
        let mut detail = String::new();
        let mut io_kind = None;
        while let Some(line) = reader.next_record().await? {
            io_kind = io_kind.or_else(|| super::utils::output_io_error_kind(&line));
            if detail.len() < 8192 {
                detail.extend(line.chars().take(8192 - detail.len()));
                detail.push('\n');
            }
        }
        Ok::<_, std::io::Error>((detail, io_kind))
    }));
    // Stopping a recording does not bound finalization. The group's media is
    // already captured, and killing a long remux (plus faststart) would turn a
    // playable recording into staged chunks, so an ordinary stop waits for it.
    // Only the absolute deadline published by manager shutdown, which bounds
    // the process, can abandon it; chunks and manifests then stay for recovery.
    let mut deadlines = handle.stop_deadline_updates();
    let status = loop {
        let deadline = *deadlines.borrow_and_update();
        tokio::select! {
            result = child.wait() => break result.map_err(|e| io_error("wait for recording finalizer", &temporary, e))?,
            changed = deadlines.changed() => { if changed.is_err() { return Err(failure("Finalizer deadline channel closed")); } },
            _ = async { match deadline { Some(deadline) => tokio::time::sleep_until(deadline).await, None => std::future::pending::<()>().await } } => {
                super::utils::terminate_and_reap(&mut child, "recording finalizer", super::utils::PROCESS_CLEANUP_TIMEOUT).await.map_err(failure)?;
                return Err(failure(format!(
                    "Recording finalization stopped at the shutdown deadline; chunks and manifest retained at {}",
                    group.manifest.display()
                )));
            }
        }
    };
    let (detail, io_kind) = diagnostics
        .await
        .map_err(|e| failure(e.to_string()))?
        .map_err(|e| io_error("read recording finalizer diagnostics", &temporary, e))?;
    if !status.success() || io_kind.is_some() {
        if let Some(io_kind) = io_kind {
            send(
                handle,
                SegmentEvent::OutputIoError {
                    output_dir: handle.config_snapshot().output_dir,
                    io_kind,
                    detail: detail.clone(),
                },
            )
            .await?;
            return Err(EngineStartError::new(
                DownloadFailureKind::OutputRootUnavailable { io_kind },
                format!("Recording finalization failed; recovery files retained: {detail}"),
            ));
        }
        return Err(failure(format!(
            "Recording finalizer exited with {status}; recovery files retained: {detail}"
        )));
    }
    let size = tokio::fs::metadata(&temporary)
        .await
        .map_err(|e| io_error("inspect finalized recording", &temporary, e))?
        .len();
    if size == 0 {
        return Err(failure(
            "Recording finalizer produced an empty file; recovery files retained",
        ));
    }
    tokio::fs::rename(&temporary, &group.path)
        .await
        .map_err(|e| io_error("publish finalized recording", &group.path, e))?;
    group.published = true;
    Ok(size)
}

async fn finalize_groups(
    handle: &DownloadHandle,
    binary: &str,
    directory: &Path,
    mut jobs: mpsc::Receiver<Group>,
    completed: &AtomicU32,
) -> Result<(u64, f64), EngineStartError> {
    let mut bytes = 0u64;
    let mut duration = 0.0;
    while let Some(mut group) = jobs.recv().await {
        let size = finalize(handle, binary, &mut group).await?;
        bytes = bytes.saturating_add(size);
        duration += group.duration();
        send(
            handle,
            SegmentEvent::SegmentCompleted(SegmentInfo {
                path: group.path.clone(),
                index: group.index,
                duration_secs: group.duration(),
                size_bytes: size,
                started_at: Some(group.started_at),
                completed_at: group.ended_at,
                split_reason_code: group.reason.map(str::to_owned),
                split_reason_details_json: group
                    .request_id
                    .map(|id| serde_json::json!({"request_id": id}).to_string()),
            }),
        )
        .await?;
        completed.fetch_add(1, Ordering::Release);
        // Only task-owned chunks whose final recording is published are disposable.
        for path in group
            .chunks
            .iter()
            .map(|chunk| directory.join(&chunk.name))
            .chain([group.manifest.clone(), group.recovery.clone()])
        {
            if let Err(error) = tokio::fs::remove_file(&path).await {
                warn!(path = %path.display(), %error, "Could not remove finalized recording staging file");
            }
        }
    }
    Ok((bytes, duration))
}

pub(super) async fn run<F, Fut>(
    handle: Arc<DownloadHandle>,
    binary: &str,
    producer: F,
) -> Result<(), EngineStartError>
where
    F: FnOnce(Arc<DownloadHandle>) -> Fut,
    Fut: Future<Output = Result<(), EngineStartError>>,
{
    let mut config = handle.config_snapshot();
    let directory = config
        .output_dir
        .join(format!(".srec-chunks-{}", handle.id));
    tokio::fs::create_dir(&directory)
        .await
        .map_err(|e| io_error("create recording staging directory", &directory, e))?;
    config.output_dir = directory.clone();
    config.output_format = "mkv".into();
    config.filename_template = "chunk-%08d".into();
    config.max_segment_duration_secs = 2;
    config.max_segment_size_bytes = 0;
    let (events_tx, events_rx) = mpsc::channel(32);
    let inner = Arc::new(handle.with_output(config, events_tx));
    let (jobs_tx, jobs_rx) = mpsc::channel(2);
    let completed = AtomicU32::new(0);
    let producer = producer(inner);
    let ingest = async {
        let result = ingest(&handle, &directory, events_rx, jobs_tx, &completed).await;
        if result.is_err() {
            handle.manual_split.close(true);
            handle.cancellation_token.cancel();
        }
        result
    };
    let finalize = async {
        let result = finalize_groups(&handle, binary, &directory, jobs_rx, &completed).await;
        if result.is_err() {
            handle.manual_split.close(true);
            handle.cancellation_token.cancel();
        }
        result
    };
    // A stop reaches the producer through the shared cancellation token and
    // bounds only acquisition, with the producer's own graceful-stop timeout.
    // Finalization of captured groups is bounded by shutdown alone; see `finalize`.
    let (producer_result, ingest_result, finalized) = tokio::join!(producer, ingest, finalize);
    let (bytes, duration) = finalized?;
    let terminal = ingest_result?;
    producer_result?;
    let terminal = match terminal {
        Some(SegmentEvent::DownloadCompleted { engine_signal, .. }) => {
            SegmentEvent::DownloadCompleted {
                total_bytes: bytes,
                total_duration_secs: duration,
                total_segments: completed.load(Ordering::Acquire),
                engine_signal,
            }
        }
        Some(terminal) => terminal,
        None => {
            return Err(failure(
                "Recording producer ended without a terminal result; recovery files retained",
            ));
        }
    };
    send(&handle, terminal).await?;
    if let Err(error) = tokio::fs::remove_file(directory.join(LIST_NAME)).await {
        warn!(%error, "Could not remove recording chunk list");
    }
    if let Err(error) = tokio::fs::remove_dir(&directory).await {
        warn!(path = %directory.display(), %error, "Recording staging directory retained");
    }
    Ok(())
}
