use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;
use tokio::io::AsyncRead;

/// A reader adapter that wraps a bytes stream for AsyncRead compatibility
pub struct BytesStreamReader {
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    current_chunk: Option<Bytes>,
    position: usize,
}

impl BytesStreamReader {
    /// Create a new BytesStreamReader from a reqwest bytes stream
    pub fn new(stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self {
            stream: Box::pin(stream),
            current_chunk: None,
            position: 0,
        }
    }
}

impl AsyncRead for BytesStreamReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;

        loop {
            // If we have a chunk with data remaining, copy it to the buffer
            if let Some(chunk) = &self.current_chunk {
                if self.position < chunk.len() {
                    let bytes_to_copy = std::cmp::min(buf.remaining(), chunk.len() - self.position);
                    buf.put_slice(&chunk[self.position..self.position + bytes_to_copy]);
                    self.position += bytes_to_copy;
                    return Poll::Ready(Ok(()));
                }
                // We've consumed this chunk entirely
                self.current_chunk = None;
                self.position = 0;
            }

            // Need to get a new chunk from the stream
            match self.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    if chunk.is_empty() {
                        continue; // Skip empty chunks
                    }
                    self.current_chunk = Some(chunk);
                    self.position = 0;
                    // Continue the loop to process this chunk
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Err(std::io::Error::other(format!("Download error: {e}"))));
                }
                Poll::Ready(None) => {
                    // End of stream
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}
