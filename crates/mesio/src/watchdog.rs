//! A stream adapter that fails a media stream which stops producing media.
//!
//! The transport-level read timeout only sees bytes. A server that trickles a
//! few bytes, repeats a header, or keeps a connection open while its encoder
//! produces nothing keeps that timeout from ever firing, so the session looks
//! healthy while writing no media at all. This watchdog watches the decoded
//! items instead: an item that carries media resets the deadline, and the
//! deadline firing ends the stream with a `Timeout`.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::{Stream, StreamExt};
use std::future::Future;
use tokio::time::{Instant, Sleep};

use crate::DownloadError;

/// Wraps `inner` and fails it when no media item arrives for `timeout`.
///
/// `is_media` decides which items count as media: FLV headers, split markers
/// and end-of-sequence markers do not, only audio/video tags do.
pub(crate) struct StallWatchdog<S, F> {
    inner: S,
    is_media: F,
    timeout: Duration,
    deadline: Pin<Box<Sleep>>,
    fired: bool,
}

impl<S, F> StallWatchdog<S, F> {
    /// Wraps `inner`; `is_media` sees each `Ok` payload.
    ///
    /// `T` is bound here, from the stream's item type, so the predicate's
    /// argument type is inferred at the call site. Without that bound a caller
    /// who does not annotate the result cannot resolve `Stream::next`: `T`
    /// only appears in the `Stream` impl's where clause, which inference does
    /// not use in that direction.
    pub(crate) fn new<T>(inner: S, timeout: Duration, is_media: F) -> Self
    where
        S: Stream<Item = Result<T, DownloadError>>,
        F: FnMut(&T) -> bool,
    {
        Self {
            inner,
            is_media,
            timeout,
            deadline: Box::pin(tokio::time::sleep(timeout)),
            fired: false,
        }
    }
}

impl<S, T, F> Stream for StallWatchdog<S, F>
where
    S: Stream<Item = Result<T, DownloadError>> + Unpin,
    F: FnMut(&T) -> bool + Unpin,
{
    type Item = Result<T, DownloadError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.fired {
            return Poll::Ready(None);
        }

        match this.inner.poll_next_unpin(cx) {
            Poll::Ready(Some(Ok(item))) => {
                if (this.is_media)(&item) {
                    this.deadline
                        .as_mut()
                        .reset(Instant::now() + this.timeout);
                }
                Poll::Ready(Some(Ok(item)))
            }
            Poll::Ready(other) => Poll::Ready(other),
            Poll::Pending => {
                if this.deadline.as_mut().poll(cx).is_ready() {
                    this.fired = true;
                    return Poll::Ready(Some(Err(DownloadError::Timeout {
                        reason: format!(
                            "stalled: no media received for {:.1}s",
                            this.timeout.as_secs_f64()
                        ),
                    })));
                }
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use tokio_stream::wrappers::ReceiverStream;

    type BoxedStream = Pin<Box<dyn Stream<Item = Result<u8, DownloadError>> + Send>>;

    fn boxed<S>(stream: S) -> BoxedStream
    where
        S: Stream<Item = Result<u8, DownloadError>> + Send + 'static,
    {
        Box::pin(stream)
    }

    /// Feeds `items` at `gap` intervals through a channel-backed stream.
    fn timed_stream(
        items: Vec<Result<u8, DownloadError>>,
        gap: Duration,
    ) -> (BoxedStream, tokio::task::JoinHandle<()>) {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let driver = tokio::spawn(async move {
            for item in items {
                tokio::time::sleep(gap).await;
                if tx.send(item).await.is_err() {
                    return;
                }
            }
        });
        (boxed(ReceiverStream::new(rx)), driver)
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_that_never_yields_media_fails_after_the_timeout() {
        let mut watchdog = StallWatchdog::new(
            boxed(stream::pending::<Result<u8, DownloadError>>()),
            Duration::from_secs(5),
            |_: &u8| true,
        );

        assert!(matches!(
            watchdog.next().await,
            Some(Err(DownloadError::Timeout { .. }))
        ));
        assert!(watchdog.next().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn media_items_reset_the_deadline() {
        // 3 items 40 ms apart, each arriving inside the 50 ms window.
        let (stream, _driver) = timed_stream(vec![Ok(1), Ok(2), Ok(3)], Duration::from_millis(40));
        let mut watchdog = StallWatchdog::new(stream, Duration::from_millis(50), |_: &u8| true);

        for expected in [1u8, 2, 3] {
            assert!(matches!(watchdog.next().await, Some(Ok(v)) if v == expected));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn non_media_items_do_not_reset_the_deadline() {
        // The first non-media item arrives at 40 ms; the deadline set at
        // construction (50 ms) must fire before the second one at 80 ms.
        let (stream, _driver) = timed_stream(vec![Ok(0), Ok(0), Ok(0)], Duration::from_millis(40));
        let mut watchdog = StallWatchdog::new(stream, Duration::from_millis(50), |_: &u8| false);

        assert!(matches!(watchdog.next().await, Some(Ok(0))));
        assert!(matches!(
            watchdog.next().await,
            Some(Err(DownloadError::Timeout { .. }))
        ));
    }
}
