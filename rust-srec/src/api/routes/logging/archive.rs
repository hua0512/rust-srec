use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use bytes::Bytes;
use futures::Stream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::{CancellationToken, DropGuard};
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::Error;
use crate::api::error::ApiError;

use super::{LogFileInternal, scan_log_files_matching};

const MAX_CONCURRENT_ARCHIVES: usize = 2;
const MAX_ARCHIVE_FILES: usize = 10_000;
const CHUNK_SIZE: usize = 64 * 1024;
const BUFFERED_CHUNKS: usize = 4;

/// Owns the archive download capacity shared by all logging routes.
pub(crate) struct LogArchiveService {
    slots: Arc<Semaphore>,
}

impl LogArchiveService {
    pub(crate) fn new() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_ARCHIVES)),
        }
    }

    pub(super) async fn download(
        &self,
        log_dir: PathBuf,
        from: Option<chrono::NaiveDate>,
        to: Option<chrono::NaiveDate>,
    ) -> Result<Body, ApiError> {
        let permit = self.slots.clone().try_acquire_owned().map_err(|_| {
            ApiError::too_many_requests("Log archive downloads are busy. Try again shortly.", 5)
        })?;
        let (files, permit) = tokio::task::spawn_blocking(move || {
            let files = scan_log_files_matching(
                &log_dir,
                |file| {
                    from.is_none_or(|from| file.date >= from) && to.is_none_or(|to| file.date <= to)
                },
                MAX_ARCHIVE_FILES,
            )?;
            Ok::<_, ApiError>((files, permit))
        })
        .await
        .map_err(|error| ApiError::internal(format!("Failed to scan log archive: {error}")))??;
        Ok(Body::from_stream(ArchiveStream::new(files, permit)))
    }
}

struct ArchiveStream {
    receiver: mpsc::Receiver<Bytes>,
    task: JoinHandle<Result<(), ApiError>>,
    _cancel_on_drop: DropGuard,
    permit: Option<Arc<OwnedSemaphorePermit>>,
    finished: bool,
}

impl ArchiveStream {
    fn new(files: Vec<LogFileInternal>, permit: OwnedSemaphorePermit) -> Self {
        let (sender, receiver) = mpsc::channel(BUFFERED_CHUNKS);
        let cancellation = CancellationToken::new();
        let cancel_on_drop = cancellation.clone().drop_guard();
        let permit = Arc::new(permit);
        let producer_permit = permit.clone();
        let task = tokio::task::spawn_blocking(move || {
            // Cancellation cannot stop a running blocking task. Keep its slot until it
            // observes cancellation, as well as while the response still owns buffered data.
            let _permit = producer_permit;
            build_archive_zip(&files, ChannelWriter(sender), &cancellation)
        });
        Self {
            receiver,
            task,
            _cancel_on_drop: cancel_on_drop,
            permit: Some(permit),
            finished: false,
        }
    }
}

impl Stream for ArchiveStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(None);
        }
        match this.receiver.poll_recv(cx) {
            Poll::Ready(Some(bytes)) => return Poll::Ready(Some(Ok(bytes))),
            Poll::Pending => return Poll::Pending,
            Poll::Ready(None) => {}
        }
        // Channel EOF alone is not success: a panic or read/write failure must abort the
        // HTTP body, even if the ZIP writer's destructor emitted a partial archive footer.
        let result = std::task::ready!(Pin::new(&mut this.task).poll(cx));
        this.finished = true;
        drop(this.permit.take());
        Poll::Ready(match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(Err(io::Error::other(error.message))),
            Err(error) => Some(Err(io::Error::other(format!(
                "Log archive task failed: {error}"
            )))),
        })
    }
}

struct ChannelWriter(mpsc::Sender<Bytes>);

impl Write for ChannelWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let len = buffer.len().min(CHUNK_SIZE);
        if len == 0 {
            return Ok(0);
        }
        self.0
            .blocking_send(Bytes::copy_from_slice(&buffer[..len]))
            .map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "Log archive download closed")
            })?;
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn build_archive_zip(
    files: &[LogFileInternal],
    output: impl Write,
    cancellation: &CancellationToken,
) -> Result<(), ApiError> {
    let output = BufWriter::with_capacity(CHUNK_SIZE, output);
    let mut zip = ZipWriter::new_stream(output);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    let mut buffer = [0u8; CHUNK_SIZE];

    for file in files {
        if cancellation.is_cancelled() {
            return Err(ApiError::internal("Log archive download cancelled"));
        }
        let input = std::fs::File::open(&file.path)
            .map_err(|source| Error::io_path("open log archive input", &file.path, source))?;
        // Snapshot each file's scanned length so a growing active log cannot extend the
        // export indefinitely. ZIP64 remains enabled even for initially small log files.
        let mut input = input.take(file.size_bytes);
        zip.start_file(&file.filename, options)
            .map_err(|error| ApiError::internal(format!("Failed to add zip entry: {error}")))?;
        while input.limit() > 0 {
            if cancellation.is_cancelled() {
                return Err(ApiError::internal("Log archive download cancelled"));
            }
            let read = input
                .read(&mut buffer)
                .map_err(|source| Error::io_path("read log archive input", &file.path, source))?;
            if read == 0 {
                return Err(ApiError::internal(
                    "Log file was truncated during archive download",
                ));
            }
            zip.write_all(&buffer[..read]).map_err(|error| {
                ApiError::internal(format!("Failed to write zip entry: {error}"))
            })?;
        }
    }

    let mut output = zip
        .finish()
        .map_err(|error| ApiError::internal(format!("Failed to finish zip: {error}")))?
        .into_inner();
    output
        .flush()
        .map_err(|error| ApiError::internal(format!("Failed to flush zip: {error}")))
}

#[cfg(test)]
mod tests;
