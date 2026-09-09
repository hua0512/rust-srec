//! Join engine tasks without cutting short confirmed writers' final events.

use futures::future::join_all;
use tokio::task::JoinError;
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};

use super::{FfmpegSource, TASK_SETTLEMENT_TIMEOUT};
use crate::downloader::engine::{DownloadFailureKind, EngineStartError};

struct AuxiliaryTask {
    name: &'static str,
    handle: AbortOnDropHandle<()>,
    result: Option<Result<(), JoinError>>,
}

async fn join_pending(tasks: &mut [AuxiliaryTask]) {
    join_all(tasks.iter_mut().map(|task| async {
        if task.result.is_none() {
            task.result = Some((&mut task.handle).await);
        }
    }))
    .await;
}

pub(crate) async fn settle_engine_tasks(
    process: AbortOnDropHandle<(bool, Option<String>)>,
    forced_settlement: CancellationToken,
    source: FfmpegSource,
    tasks: Vec<(&'static str, AbortOnDropHandle<()>)>,
) -> Result<(), EngineStartError> {
    let process_result = process.await;
    let confirmed = process_result
        .as_ref()
        .is_ok_and(|(confirmed, _)| *confirmed);
    if !confirmed {
        forced_settlement.cancel();
    }
    let mut tasks: Vec<_> = tasks
        .into_iter()
        .map(|(name, handle)| AuxiliaryTask {
            name,
            handle,
            result: None,
        })
        .collect();
    let timed_out = if confirmed {
        join_pending(&mut tasks).await;
        false
    } else {
        match tokio::time::timeout(TASK_SETTLEMENT_TIMEOUT, join_pending(&mut tasks)).await {
            Ok(()) => false,
            Err(_) => {
                for task in &tasks {
                    task.handle.abort();
                }
                // Completed joins retain their results across timeout cancellation;
                // polling an already-consumed JoinHandle again would panic.
                join_pending(&mut tasks).await;
                true
            }
        }
    };
    let mut results = Vec::with_capacity(tasks.len());
    for task in tasks {
        match task.result {
            Some(result) => results.push((task.name, result)),
            None => {
                return Err(EngineStartError::new(
                    DownloadFailureKind::Other,
                    format!("{} task was not joined", task.name),
                ));
            }
        }
    }
    settlement_result(process_result, source, results, timed_out)
}

fn settlement_result(
    process: Result<(bool, Option<String>), JoinError>,
    source: FfmpegSource,
    tasks: impl IntoIterator<Item = (&'static str, Result<(), JoinError>)>,
    timed_out: bool,
) -> Result<(), EngineStartError> {
    let mut errors = Vec::new();
    match process {
        Ok((true, _)) => {}
        Ok((false, error)) => errors.push(format!(
            "process cleanup was not confirmed: {}",
            error.unwrap_or_else(|| "unknown cleanup failure".to_string())
        )),
        Err(error) => errors.push(format!("process waiter task failed: {error}")),
    }
    if timed_out {
        let subject = match source {
            FfmpegSource::Direct => "event reader did not settle",
            FfmpegSource::Streamlink => "auxiliary tasks did not settle",
        };
        errors.push(format!(
            "{subject} within {}s after unconfirmed process cleanup",
            TASK_SETTLEMENT_TIMEOUT.as_secs()
        ));
    }
    for (name, result) in tasks {
        if let Err(error) = result
            && !(timed_out && error.is_cancelled())
        {
            errors.push(format!("{name} task failed: {error}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        let engine = match source {
            FfmpegSource::Direct => "FFmpeg",
            FfmpegSource::Streamlink => "Streamlink",
        };
        Err(EngineStartError::new(
            DownloadFailureKind::Other,
            format!("{engine} task settlement failed: {}", errors.join("; ")),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::time::{Duration, Instant};

    #[tokio::test(start_paused = true)]
    async fn confirmed_cleanup_waits_for_late_auxiliary_work_without_a_timeout() {
        let finished = Arc::new(AtomicBool::new(false));
        let completed = finished.clone();
        let start = Instant::now();
        let process = AbortOnDropHandle::new(tokio::spawn(async { (true, None) }));
        let reader = AbortOnDropHandle::new(tokio::spawn(async move {
            tokio::time::sleep(TASK_SETTLEMENT_TIMEOUT + Duration::from_secs(1)).await;
            completed.store(true, Ordering::SeqCst);
        }));
        settle_engine_tasks(
            process,
            CancellationToken::new(),
            FfmpegSource::Direct,
            vec![("event reader", reader)],
        )
        .await
        .unwrap();
        assert!(finished.load(Ordering::SeqCst));
        assert!(start.elapsed() > TASK_SETTLEMENT_TIMEOUT);
    }

    #[tokio::test]
    async fn waiter_task_failure_cancels_auxiliaries_and_keeps_the_join_error() {
        let forced = CancellationToken::new();
        let reader_stop = forced.clone();
        let reader =
            AbortOnDropHandle::new(tokio::spawn(async move { reader_stop.cancelled().await }));
        let process: AbortOnDropHandle<(bool, Option<String>)> =
            AbortOnDropHandle::new(tokio::spawn(async { panic!("fixture waiter panic") }));
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            settle_engine_tasks(
                process,
                forced.clone(),
                FfmpegSource::Direct,
                vec![("event reader", reader)],
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(forced.is_cancelled());
        assert!(error.message.contains("process waiter task failed"));
        assert!(error.message.contains("fixture waiter panic"));
        assert!(!error.message.contains("event reader task failed"));
    }

    #[tokio::test(start_paused = true)]
    async fn unconfirmed_cleanup_aborts_and_joins_stuck_tasks_and_retains_panic_context() {
        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = DropFlag(dropped.clone());
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let stuck = AbortOnDropHandle::new(tokio::spawn(async move {
            let _flag = flag;
            ready_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        }));
        ready_rx.await.unwrap();
        let panicked: AbortOnDropHandle<()> =
            AbortOnDropHandle::new(tokio::spawn(async { panic!("fixture reader panic") }));
        let process = AbortOnDropHandle::new(tokio::spawn(async {
            (false, Some("fixture cleanup failure".to_string()))
        }));
        let forced = CancellationToken::new();
        let error = settle_engine_tasks(
            process,
            forced.clone(),
            FfmpegSource::Streamlink,
            vec![("stdout pipe", stuck), ("event reader", panicked)],
        )
        .await
        .unwrap_err();
        assert!(forced.is_cancelled());
        assert!(
            dropped.load(Ordering::SeqCst),
            "aborted tasks must be joined before return"
        );
        let error = error.to_string();
        assert!(error.contains("fixture cleanup failure"));
        assert!(error.contains("auxiliary tasks did not settle"));
        assert!(error.contains("event reader task failed"));
        assert!(
            !error.contains("stdout pipe task failed"),
            "expected timeout cancellation is not a second failure"
        );
    }
}
