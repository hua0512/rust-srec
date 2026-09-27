//! Manual file rotation shared by recorders and their ordered media pipelines.

use std::time::{Duration, Instant};

use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualSplitStatus {
    Idle,
    Pending,
    Finalizing,
    Completed,
    Expired,
    Cancelled,
    Failed,
}

impl ManualSplitStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Pending => "pending",
            Self::Finalizing => "finalizing",
            Self::Completed => "completed",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn is_pending(self) -> bool {
        matches!(self, Self::Pending | Self::Finalizing)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualSplitSnapshot {
    pub supported: bool,
    pub unavailable_reason: &'static str,
    pub request_id: u64,
    pub revision: u64,
    pub status: ManualSplitStatus,
}

impl Default for ManualSplitSnapshot {
    fn default() -> Self {
        Self {
            supported: false,
            unavailable_reason: "unsupported_recording_mode",
            request_id: 0,
            revision: 0,
            status: ManualSplitStatus::Idle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ManualSplitError {
    #[error("Lossless cutting is unavailable for this recording")]
    Unavailable,
    #[error("The recording is stopping or has ended")]
    Closed,
}

#[derive(Debug, Clone, Default)]
struct State {
    snapshot: ManualSplitSnapshot,
    deadline: Option<Instant>,
    closed: bool,
    acquisition_ended: bool,
}

/// One controller per recording attempt. The watch lock serializes request,
/// boundary selection, expiration, and shutdown without crossing an await.
#[derive(Debug)]
pub struct ManualSplitControl {
    state: watch::Sender<State>,
}

impl Default for ManualSplitControl {
    fn default() -> Self {
        Self {
            state: watch::channel(State::default()).0,
        }
    }
}

impl ManualSplitControl {
    pub fn snapshot(&self) -> ManualSplitSnapshot {
        self.state.borrow().snapshot.clone()
    }

    pub fn enable(&self) {
        self.state.send_if_modified(|state| {
            if state.closed || state.acquisition_ended || state.snapshot.supported {
                return false;
            }
            state.snapshot.supported = true;
            state.snapshot.unavailable_reason = "";
            state.snapshot.revision += 1;
            true
        });
    }

    pub fn request(&self, timeout: Duration) -> Result<ManualSplitSnapshot, ManualSplitError> {
        let mut result = Err(ManualSplitError::Unavailable);
        self.state.send_if_modified(|state| {
            if state.closed || state.acquisition_ended {
                result = Err(ManualSplitError::Closed);
                return false;
            }
            if !state.snapshot.supported {
                return false;
            }
            if state.snapshot.status.is_pending() {
                result = Ok(state.snapshot.clone());
                return false;
            }
            state.snapshot.request_id += 1;
            state.snapshot.status = ManualSplitStatus::Pending;
            state.snapshot.revision += 1;
            state.deadline = Some(Instant::now() + timeout);
            result = Ok(state.snapshot.clone());
            true
        });
        result
    }

    /// Called only when the ordered media path can split safely. Once claimed,
    /// the request cannot expire while the writer finalizes its output.
    pub fn begin(&self) -> Option<u64> {
        let mut request_id = None;
        self.state.send_if_modified(|state| {
            if state.closed
                || state.acquisition_ended
                || state.snapshot.status != ManualSplitStatus::Pending
            {
                return false;
            }
            if state
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                state.snapshot.status = ManualSplitStatus::Expired;
            } else {
                state.snapshot.status = ManualSplitStatus::Finalizing;
                request_id = Some(state.snapshot.request_id);
            }
            state.deadline = None;
            state.snapshot.revision += 1;
            true
        });
        request_id
    }

    pub fn complete(&self, request_id: u64) {
        self.state.send_if_modified(|state| {
            if state.closed
                || state.snapshot.request_id != request_id
                || state.snapshot.status != ManualSplitStatus::Finalizing
            {
                return false;
            }
            state.snapshot.status = ManualSplitStatus::Completed;
            state.snapshot.revision += 1;
            true
        });
    }

    pub fn fail(&self, request_id: u64) {
        self.state.send_if_modified(|state| {
            if state.closed
                || state.snapshot.request_id != request_id
                || !state.snapshot.status.is_pending()
            {
                return false;
            }
            state.snapshot.status = ManualSplitStatus::Failed;
            state.snapshot.revision += 1;
            state.deadline = None;
            true
        });
    }

    /// EOF fences new requests but lets a file already cut finish remuxing.
    pub fn end_acquisition(&self) {
        self.state.send_if_modified(|state| {
            if state.closed || state.acquisition_ended {
                return false;
            }
            state.acquisition_ended = true;
            state.snapshot.supported = false;
            state.snapshot.unavailable_reason = "recording_ended";
            if state.snapshot.status == ManualSplitStatus::Pending {
                state.snapshot.status = ManualSplitStatus::Cancelled;
            }
            state.deadline = None;
            state.snapshot.revision += 1;
            true
        });
    }

    /// Fence future requests and settle an outstanding request on EOF/stop/error.
    pub fn close(&self, failed: bool) {
        self.state.send_if_modified(|state| {
            if state.closed {
                return false;
            }
            state.closed = true;
            state.deadline = None;
            state.snapshot.supported = false;
            state.snapshot.unavailable_reason = "recording_ended";
            if state.snapshot.status.is_pending() {
                state.snapshot.status = if failed {
                    ManualSplitStatus::Failed
                } else {
                    ManualSplitStatus::Cancelled
                };
            }
            state.snapshot.revision += 1;
            true
        });
    }

    pub fn expire(&self) {
        self.state.send_if_modified(|state| {
            if !state
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                return false;
            }
            state.deadline = None;
            state.snapshot.status = ManualSplitStatus::Expired;
            state.snapshot.revision += 1;
            true
        });
    }

    pub fn updates(&self) -> ManualSplitUpdates {
        ManualSplitUpdates {
            receiver: self.state.subscribe(),
        }
    }
}

pub struct ManualSplitUpdates {
    receiver: watch::Receiver<State>,
}

impl ManualSplitUpdates {
    pub async fn changed(&mut self) -> Result<ManualSplitSnapshot, watch::error::RecvError> {
        self.receiver.changed().await?;
        Ok(self.receiver.borrow_and_update().snapshot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_coalesce_and_only_the_claimed_request_completes() {
        let control = ManualSplitControl::default();
        assert_eq!(
            control.request(Duration::from_secs(30)),
            Err(ManualSplitError::Unavailable)
        );
        control.enable();
        let first = control.request(Duration::from_secs(30)).unwrap();
        assert_eq!(first, control.request(Duration::from_secs(30)).unwrap());
        assert_eq!(control.begin(), Some(first.request_id));
        assert_eq!(control.begin(), None);
        control.complete(first.request_id + 1);
        assert_eq!(control.snapshot().status, ManualSplitStatus::Finalizing);
        control.complete(first.request_id);
        assert_eq!(control.snapshot().status, ManualSplitStatus::Completed);
        assert!(control.request(Duration::from_secs(30)).unwrap().request_id > first.request_id);
    }

    #[test]
    fn expired_or_cancelled_requests_cannot_cut_later() {
        let control = ManualSplitControl::default();
        control.enable();
        control.request(Duration::ZERO).unwrap();
        assert_eq!(control.begin(), None);
        assert_eq!(control.snapshot().status, ManualSplitStatus::Expired);
        control.request(Duration::from_secs(30)).unwrap();
        control.close(false);
        control.enable();
        assert_eq!(control.begin(), None);
        assert_eq!(control.snapshot().status, ManualSplitStatus::Cancelled);
        assert_eq!(
            control.request(Duration::from_secs(30)),
            Err(ManualSplitError::Closed)
        );
    }

    #[test]
    fn expiration_without_media_and_writer_failure_are_observable() {
        let control = ManualSplitControl::default();
        control.enable();
        control.request(Duration::ZERO).unwrap();
        control.expire();
        assert_eq!(control.snapshot().status, ManualSplitStatus::Expired);
        let request = control.request(Duration::from_secs(30)).unwrap();
        control.begin();
        control.close(true);
        control.complete(request.request_id);
        assert_eq!(control.snapshot().status, ManualSplitStatus::Failed);
    }

    #[test]
    fn eof_fences_requests_but_allows_an_already_cut_file_to_finalize() {
        let control = ManualSplitControl::default();
        control.enable();
        let request = control.request(Duration::from_secs(30)).unwrap();
        control.begin();
        control.end_acquisition();
        control.enable();
        assert!(!control.snapshot().supported);
        assert_eq!(
            control.request(Duration::from_secs(30)),
            Err(ManualSplitError::Closed)
        );
        control.complete(request.request_id);
        assert_eq!(control.snapshot().status, ManualSplitStatus::Completed);
    }
}
