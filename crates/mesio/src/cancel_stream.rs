//! Stream wrapper that cancels a session token when the consumer drops the stream.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use tokio_util::sync::CancellationToken;

use crate::BoxMediaStream;

/// Wraps a [`BoxMediaStream`] and cancels `token` on drop so upstream tasks
/// shut down promptly when the caller stops reading.
pub(crate) struct CancelOnDropStream<T, E = crate::DownloadError> {
    inner: BoxMediaStream<T, E>,
    token: CancellationToken,
}

impl<T, E> CancelOnDropStream<T, E> {
    pub(crate) fn new(inner: BoxMediaStream<T, E>, token: CancellationToken) -> Self {
        Self { inner, token }
    }
}

impl<T, E> Stream for CancelOnDropStream<T, E> {
    type Item = Result<T, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<T, E> Drop for CancelOnDropStream<T, E> {
    fn drop(&mut self) {
        self.token.cancel();
    }
}
