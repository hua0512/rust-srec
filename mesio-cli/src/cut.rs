//! Lossless cutting: finishing the file being recorded at the next safe media
//! boundary and continuing the recording in a new file, on request from the
//! keyboard, stdin or `SIGUSR1`.
//!
//! [`CutTarget`] is shared by the threads that receive requests and the
//! recording in progress; [`CutSession`] makes one recording cuttable.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use flv_fix::FlvWriter;
use hls_fix::HlsWriter;
use pipeline_common::{
    CancellationToken, ManualSplitControl, ManualSplitError, ManualSplitExpiryReason,
    ManualSplitStatus, SplitReason,
};
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// How long a request waits for a safe boundary. Finishing the file once the
/// boundary is found is not limited.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a waiting request is checked for expiry.
const EXPIRY_INTERVAL: Duration = Duration::from_secs(1);

/// Acknowledgements waiting to be logged. Beyond this, they are dropped
/// rather than block the thread that received the request.
const NOTICE_CAPACITY: usize = 16;

/// Where a cut request came from, for its acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutSource {
    Key,
    Stdin,
    Signal,
}

impl CutSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Stdin => "stdin",
            Self::Signal => "SIGUSR1",
        }
    }
}

/// The boundary a recording can be cut at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    /// FLV: the next keyframe (or audio packet, for audio-only streams).
    Keyframe,
    /// HLS and DASH: the next segment that starts independently.
    Segment,
}

impl fmt::Display for Boundary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Keyframe => "keyframe",
            Self::Segment => "independently decodable segment",
        })
    }
}

/// Why a request was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    NoRecording,
    Stopping,
    PipeOutput,
    LocalFile,
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoRecording => "no recording is in progress",
            Self::Stopping => "the recording is stopping",
            Self::PipeOutput => "output is written to a pipe, not to files",
            Self::LocalFile => "local files are processed, not recorded",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutOutcome {
    Requested {
        boundary: Boundary,
        timeout: Duration,
    },
    AlreadyPending,
    Unavailable(Unavailable),
}

enum Target {
    Idle,
    Unavailable(Unavailable),
    Active(Active),
}

struct Active {
    control: Arc<ManualSplitControl>,
    /// The program's stop token: a stop refuses new requests at once.
    token: CancellationToken,
    boundary: Boundary,
    timeout: Duration,
}

pub(crate) struct Notice {
    pub(crate) source: CutSource,
    pub(crate) outcome: CutOutcome,
}

/// Routes cut requests to the recording in progress, if any.
#[derive(Clone)]
pub struct CutTarget {
    target: Arc<Mutex<Target>>,
    notices: Option<SyncSender<Notice>>,
}

impl CutTarget {
    /// Starts the thread that acknowledges requests. If it cannot start,
    /// requests still work, unacknowledged, and the error is returned for
    /// the caller to report once logging is set up.
    pub fn new() -> (Self, io::Result<()>) {
        let (sender, receiver) = mpsc::sync_channel(NOTICE_CAPACITY);
        match thread::Builder::new()
            .name("mesio-cut-notice".to_owned())
            .spawn(move || log_notices(receiver))
        {
            Ok(_) => (Self::with_notices(Some(sender)), Ok(())),
            Err(error) => (Self::with_notices(None), Err(error)),
        }
    }

    fn with_notices(notices: Option<SyncSender<Notice>>) -> Self {
        Self {
            target: Arc::new(Mutex::new(Target::Idle)),
            notices,
        }
    }

    /// A target whose acknowledgements are received by the caller.
    #[cfg(test)]
    pub(crate) fn with_notice_receiver() -> (Self, Receiver<Notice>) {
        let (sender, receiver) = mpsc::sync_channel(NOTICE_CAPACITY);
        (Self::with_notices(Some(sender)), receiver)
    }

    /// Requests a cut of the recording in progress. It neither blocks nor
    /// logs, so the signal thread can call it: the outcome is acknowledged on
    /// a thread of its own, and dropped if that thread has fallen behind.
    pub fn trigger(&self, source: CutSource) -> CutOutcome {
        let outcome = self.request();
        if let Some(notices) = &self.notices {
            // A full queue means the log output is stalled; the request
            // itself has already been made.
            let _ = notices.try_send(Notice { source, outcome });
        }
        outcome
    }

    fn request(&self) -> CutOutcome {
        // The lock is held through the request, so a recording that has
        // retired its target cannot accept one afterwards.
        let target = self.lock();
        let active = match &*target {
            Target::Idle => return CutOutcome::Unavailable(Unavailable::NoRecording),
            Target::Unavailable(reason) => return CutOutcome::Unavailable(*reason),
            Target::Active(active) => active,
        };
        if active.token.is_cancelled() {
            return CutOutcome::Unavailable(Unavailable::Stopping);
        }
        let previous = active.control.snapshot().request_id;
        match active.control.request(active.timeout) {
            // A request already in progress is returned instead of a new one.
            Ok(state) if state.request_id == previous => CutOutcome::AlreadyPending,
            Ok(_) => CutOutcome::Requested {
                boundary: active.boundary,
                timeout: active.timeout,
            },
            Err(ManualSplitError::Unavailable) => CutOutcome::Unavailable(Unavailable::NoRecording),
            Err(ManualSplitError::Closed) => CutOutcome::Unavailable(Unavailable::Stopping),
        }
    }

    /// Answers requests with `reason` until the guard is dropped.
    pub fn set_unavailable(&self, reason: Unavailable) -> UnavailableGuard {
        *self.lock() = Target::Unavailable(reason);
        UnavailableGuard(self.clone())
    }

    fn set(&self, target: Target) {
        *self.lock() = target;
    }

    fn lock(&self) -> MutexGuard<'_, Target> {
        self.target.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Returns the target to idle when dropped.
pub struct UnavailableGuard(CutTarget);

impl Drop for UnavailableGuard {
    fn drop(&mut self) {
        self.0.set(Target::Idle);
    }
}

fn log_notices(notices: Receiver<Notice>) {
    for Notice { source, outcome } in notices {
        let source = source.as_str();
        match outcome {
            CutOutcome::Requested { boundary, timeout } => info!(
                source,
                "Cut requested: the current file ends at the next {boundary} (waiting up to {}s)",
                timeout.as_secs()
            ),
            CutOutcome::AlreadyPending => info!(source, "A cut is already pending"),
            CutOutcome::Unavailable(reason) => info!(source, "Cannot cut now: {reason}"),
        }
    }
}

/// Writers whose file rotations complete a cut.
pub trait CutWriter {
    fn on_file_opened(&mut self, callback: impl Fn(&Path) + Send + Sync + 'static);
    fn on_file_closed(
        &mut self,
        callback: impl Fn(&Path, Option<&SplitReason>) + Send + Sync + 'static,
    );
}

impl CutWriter for FlvWriter {
    fn on_file_opened(&mut self, callback: impl Fn(&Path) + Send + Sync + 'static) {
        self.set_on_segment_start_callback(move |path, _| callback(path));
    }

    fn on_file_closed(
        &mut self,
        callback: impl Fn(&Path, Option<&SplitReason>) + Send + Sync + 'static,
    ) {
        self.set_on_segment_complete_callback(move |path, _, _, _, reason| callback(path, reason));
    }
}

impl CutWriter for HlsWriter {
    fn on_file_opened(&mut self, callback: impl Fn(&Path) + Send + Sync + 'static) {
        self.set_on_segment_start_callback(move |path, _| callback(path));
    }

    fn on_file_closed(
        &mut self,
        callback: impl Fn(&Path, Option<&SplitReason>) + Send + Sync + 'static,
    ) {
        self.set_on_segment_complete_callback(move |path, _, _, _, reason| callback(path, reason));
    }
}

/// A file closed by a cut whose successor has not been opened yet.
struct ManualClose {
    request_id: u64,
    path: PathBuf,
}

/// Makes one recording cuttable, from [`CutSession::start`] until
/// [`CutSession::finish`] or drop.
pub struct CutSession {
    target: CutTarget,
    control: Arc<ManualSplitControl>,
    boundary: Boundary,
    manual_close: Arc<Mutex<Option<ManualClose>>>,
    monitor_stop: CancellationToken,
    monitor: Option<JoinHandle<()>>,
    settled: bool,
}

impl CutSession {
    /// Directs requests from `target` to a new recording until it finishes.
    /// `token` is the program's stop token. Requires a Tokio runtime.
    pub fn start(target: &CutTarget, token: &CancellationToken, boundary: Boundary) -> Self {
        Self::start_with(target, token, boundary, REQUEST_TIMEOUT, EXPIRY_INTERVAL)
    }

    fn start_with(
        target: &CutTarget,
        token: &CancellationToken,
        boundary: Boundary,
        timeout: Duration,
        expiry_interval: Duration,
    ) -> Self {
        let control = Arc::new(ManualSplitControl::default());
        control.enable();
        target.set(Target::Active(Active {
            control: Arc::clone(&control),
            token: token.clone(),
            boundary,
            timeout,
        }));
        let monitor_stop = CancellationToken::new();
        let monitor = tokio::spawn(expire_requests(
            Arc::clone(&control),
            boundary,
            timeout,
            expiry_interval,
            monitor_stop.clone(),
        ));
        Self {
            target: target.clone(),
            control,
            boundary,
            manual_close: Arc::default(),
            monitor_stop,
            monitor: Some(monitor),
            settled: false,
        }
    }

    /// The control the recording's pipeline claims cuts from.
    pub fn control(&self) -> Arc<ManualSplitControl> {
        Arc::clone(&self.control)
    }

    /// Completes each cut once `writer` has opened the file after it, as the
    /// writer may fail to open one, or the recording may end first.
    pub fn attach(&self, writer: &mut impl CutWriter) {
        let manual_close = Arc::clone(&self.manual_close);
        writer.on_file_closed(move |path, reason| {
            if let Some(SplitReason::Manual { request_id }) = reason {
                *lock(&manual_close) = Some(ManualClose {
                    request_id: *request_id,
                    path: path.to_path_buf(),
                });
            }
        });
        let manual_close = Arc::clone(&self.manual_close);
        let control = Arc::clone(&self.control);
        writer.on_file_opened(move |path| {
            let Some(close) = lock(&manual_close).take() else {
                return;
            };
            control.complete(close.request_id);
            info!(
                finished = %close.path.display(),
                next = %path.display(),
                "Cut complete; recording continues in a new file"
            );
        });
    }

    /// Refuses further requests once the recording's input has ended. One
    /// already accepted may still be honoured by media queued in the
    /// pipeline.
    pub fn end_forwarding(&self) {
        self.target.set(Target::Unavailable(Unavailable::Stopping));
    }

    /// Ends the session once the recording has been settled; `failed` when it
    /// ended with an error.
    pub async fn finish(mut self, failed: bool) {
        self.settle(failed);
        if let Some(monitor) = self.monitor.take()
            && let Err(error) = monitor.await
        {
            warn!(%error, "Cut expiry task failed");
        }
    }

    fn settle(&mut self, failed: bool) {
        if self.settled {
            return;
        }
        self.settled = true;
        self.target.set(Target::Idle);
        let before = self.control.snapshot().status;
        self.control.close(failed);
        self.monitor_stop.cancel();

        let unfinished = lock(&self.manual_close).take();
        if let Some(close) = unfinished {
            info!(
                finished = %close.path.display(),
                "Cut finished the file, but the recording ended before a new file started"
            );
        } else if before == ManualSplitStatus::Pending {
            info!(
                "Cut cancelled: the recording ended before a {} arrived",
                self.boundary
            );
        } else if before == ManualSplitStatus::Finalizing {
            warn!("Cut did not complete: the recording ended while the file was being finished");
        }
    }
}

impl Drop for CutSession {
    fn drop(&mut self) {
        // Reached without `finish` only when the recording returned early.
        self.settle(true);
    }
}

/// Expires requests that found no boundary in time, and reports each expiry
/// once.
async fn expire_requests(
    control: Arc<ManualSplitControl>,
    boundary: Boundary,
    timeout: Duration,
    interval: Duration,
    stop: CancellationToken,
) {
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut reported = None;
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = ticks.tick() => {}
        }
        control.expire();
        let state = control.snapshot();
        if state.status == ManualSplitStatus::Expired && reported != Some(state.request_id) {
            reported = Some(state.request_id);
            info!(
                "Cut request expired after {}s: {}; recording continues in the same file",
                timeout.as_secs(),
                expiry_reason(state.expiry_reason, boundary)
            );
        }
    }
}

fn expiry_reason(reason: Option<ManualSplitExpiryReason>, boundary: Boundary) -> String {
    match reason {
        Some(ManualSplitExpiryReason::NoKeyframe) => "no keyframe arrived".to_owned(),
        Some(ManualSplitExpiryReason::NoIndependentSegment) => {
            "no segment arrived that starts independently".to_owned()
        }
        Some(ManualSplitExpiryReason::FinalizationBacklog) => {
            "earlier files were still being finished".to_owned()
        }
        Some(ManualSplitExpiryReason::NoMedia) | None => {
            format!("no media arrived that could start a file at a {boundary}")
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> (CutTarget, Receiver<Notice>) {
        CutTarget::with_notice_receiver()
    }

    #[tokio::test]
    async fn requests_reach_only_the_active_recording() {
        let (target, notices) = target();
        let token = CancellationToken::new();
        assert_eq!(
            target.trigger(CutSource::Key),
            CutOutcome::Unavailable(Unavailable::NoRecording)
        );

        let session = CutSession::start(&target, &token, Boundary::Keyframe);
        assert_eq!(
            target.trigger(CutSource::Signal),
            CutOutcome::Requested {
                boundary: Boundary::Keyframe,
                timeout: REQUEST_TIMEOUT,
            }
        );
        assert_eq!(target.trigger(CutSource::Stdin), CutOutcome::AlreadyPending);
        let control = session.control();
        session.finish(false).await;
        assert_eq!(control.snapshot().status, ManualSplitStatus::Cancelled);

        // A request made between recordings is not carried into the next.
        assert_eq!(
            target.trigger(CutSource::Key),
            CutOutcome::Unavailable(Unavailable::NoRecording)
        );
        let next = CutSession::start(&target, &token, Boundary::Segment);
        assert_eq!(next.control().snapshot().status, ManualSplitStatus::Idle);
        next.finish(false).await;

        let sources: Vec<_> = notices.try_iter().map(|notice| notice.source).collect();
        assert_eq!(
            sources,
            [
                CutSource::Key,
                CutSource::Signal,
                CutSource::Stdin,
                CutSource::Key
            ]
        );
    }

    #[tokio::test]
    async fn a_stopping_or_ending_recording_refuses_requests() {
        let (target, _notices) = target();
        let token = CancellationToken::new();
        let session = CutSession::start(&target, &token, Boundary::Keyframe);
        session.end_forwarding();
        assert_eq!(
            target.trigger(CutSource::Key),
            CutOutcome::Unavailable(Unavailable::Stopping)
        );
        let control = session.control();
        drop(session);
        assert_eq!(control.snapshot().status, ManualSplitStatus::Idle);
        assert!(!control.snapshot().supported);

        let session = CutSession::start(&target, &token, Boundary::Keyframe);
        token.cancel();
        assert_eq!(
            target.trigger(CutSource::Signal),
            CutOutcome::Unavailable(Unavailable::Stopping)
        );
        session.finish(false).await;
    }

    #[tokio::test]
    async fn unavailable_modes_answer_until_released() {
        let (target, _notices) = target();
        {
            let _guard = target.set_unavailable(Unavailable::PipeOutput);
            assert_eq!(
                target.trigger(CutSource::Key),
                CutOutcome::Unavailable(Unavailable::PipeOutput)
            );
        }
        assert_eq!(
            target.trigger(CutSource::Key),
            CutOutcome::Unavailable(Unavailable::NoRecording)
        );
    }

    #[tokio::test]
    async fn a_request_without_a_boundary_expires() {
        let (target, _notices) = target();
        let token = CancellationToken::new();
        let session = CutSession::start_with(
            &target,
            &token,
            Boundary::Keyframe,
            Duration::from_millis(20),
            Duration::from_millis(5),
        );
        target.trigger(CutSource::Key);
        let control = session.control();
        tokio::time::timeout(Duration::from_secs(5), async {
            while control.snapshot().status != ManualSplitStatus::Expired {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the request expires");
        assert_eq!(
            control.snapshot().expiry_reason,
            Some(ManualSplitExpiryReason::NoMedia)
        );
        session.finish(false).await;
    }

    #[tokio::test]
    async fn a_full_notice_queue_drops_acknowledgements_without_blocking() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let target = CutTarget::with_notices(Some(sender));
        for _ in 0..3 {
            assert_eq!(
                target.trigger(CutSource::Signal),
                CutOutcome::Unavailable(Unavailable::NoRecording)
            );
        }
    }

    type OpenedCallback = Box<dyn Fn(&Path) + Send + Sync>;
    type ClosedCallback = Box<dyn Fn(&Path, Option<&SplitReason>) + Send + Sync>;

    #[derive(Default)]
    struct FakeWriter {
        opened: Option<OpenedCallback>,
        closed: Option<ClosedCallback>,
    }

    impl CutWriter for FakeWriter {
        fn on_file_opened(&mut self, callback: impl Fn(&Path) + Send + Sync + 'static) {
            self.opened = Some(Box::new(callback));
        }

        fn on_file_closed(
            &mut self,
            callback: impl Fn(&Path, Option<&SplitReason>) + Send + Sync + 'static,
        ) {
            self.closed = Some(Box::new(callback));
        }
    }

    impl FakeWriter {
        fn open(&self, path: &str) {
            (self.opened.as_ref().unwrap())(Path::new(path));
        }

        fn close(&self, path: &str, reason: Option<&SplitReason>) {
            (self.closed.as_ref().unwrap())(Path::new(path), reason);
        }
    }

    #[tokio::test]
    async fn a_cut_completes_when_the_next_file_opens() {
        let (target, _notices) = target();
        let token = CancellationToken::new();
        let session = CutSession::start(&target, &token, Boundary::Keyframe);
        let mut writer = FakeWriter::default();
        session.attach(&mut writer);
        let control = session.control();

        writer.open("a.flv");
        target.trigger(CutSource::Key);
        let request_id = control.begin().expect("the pipeline claims the request");
        // An automatic rotation is not a cut.
        writer.close("a.flv", Some(&SplitReason::SizeLimit));
        writer.open("b.flv");
        assert_eq!(control.snapshot().status, ManualSplitStatus::Finalizing);

        writer.close("b.flv", Some(&SplitReason::Manual { request_id }));
        assert_eq!(control.snapshot().status, ManualSplitStatus::Finalizing);
        writer.open("c.flv");
        assert_eq!(control.snapshot().status, ManualSplitStatus::Completed);
        session.finish(false).await;
    }

    #[tokio::test]
    async fn a_cut_whose_next_file_never_opens_does_not_complete() {
        let (target, _notices) = target();
        let token = CancellationToken::new();
        let session = CutSession::start(&target, &token, Boundary::Segment);
        let mut writer = FakeWriter::default();
        session.attach(&mut writer);
        let control = session.control();

        writer.open("a.m4s");
        target.trigger(CutSource::Key);
        let request_id = control.begin().expect("the pipeline claims the request");
        writer.close("a.m4s", Some(&SplitReason::Manual { request_id }));
        session.finish(false).await;
        assert_eq!(control.snapshot().status, ManualSplitStatus::Cancelled);
    }
}
