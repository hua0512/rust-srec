//! Standalone server process composition.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{error, info, warn};

use crate::backend::{
    NotificationEvent, ServiceContainer, init_database_pools, init_logging, install_panic_hook,
    install_rustls_provider, run_migrations,
};
use crate::runtime::{
    RuntimeTermination, WorkerShutdownReason, is_worker_process, supervise_current_executable,
    wait_for_worker_start, worker_shutdown_schedule,
};
use crate::services::container::ServiceShutdownSchedule;

const WORKER_FAIL_STOP_EXIT_CODE: i32 = 70;

/// Run the standalone rust-srec server under its strict process supervisor.
pub async fn run() -> anyhow::Result<()> {
    load_dotenv();
    if is_worker_process() {
        run_worker().await
    } else {
        run_supervisor().await
    }
}

fn load_dotenv() {
    // Logging is not initialized in the supervisor, so malformed or unreadable
    // dotenv files must remain visible on stderr.
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        eprintln!("Failed to load .env: {error}");
    }
}

async fn run_supervisor() -> anyhow::Result<()> {
    // Register handlers before launching the worker. On Unix this closes the
    // startup window where a SIGTERM could otherwise take the default action
    // and leave the newly isolated process group without its supervisor.
    let shutdown_signal = supervisor_shutdown_signal()?;
    let supervised = supervise_current_executable(shutdown_signal).await?;
    supervised.finish(|outcome| match outcome {
        Ok(report) => match report.termination {
            RuntimeTermination::Clean => {}
            RuntimeTermination::CleanRecoveryPending => {
                eprintln!(
                    "Runtime generation {} exited cleanly, but {} earlier generation(s) still require recovery",
                    report.generation, report.unresolved_generations
                );
            }
            RuntimeTermination::ForcedRecoveryPending => {
                eprintln!(
                    "Runtime generation {} exceeded the cooperative cutoff and was forcefully contained after {:?}; recovery is required for {} generation(s)",
                    report.generation, report.elapsed, report.unresolved_generations
                );
            }
            RuntimeTermination::CrashedRecoveryPending => {
                let marker_detail = report
                    .marker_error
                    .as_deref()
                    .map_or(String::new(), |error| format!("; marker error: {error}"));
                eprintln!(
                    "Runtime generation {} exited without a clean containment proof (exit code: {:?}){}; recovery is required for {} generation(s)",
                    report.generation, report.exit_code, marker_detail, report.unresolved_generations
                );
            }
        },
        Err(error) => eprintln!("Runtime supervisor failed: {error}"),
    })
}

async fn run_worker() -> anyhow::Result<()> {
    // Registered before `wait_for_worker_start` so the streams capture terminal
    // signals from the first instant: until a handler exists a SIGTERM takes its
    // default action and kills this process mid-startup. A signal that arrives
    // during startup is observed at the select below and drains the runtime as
    // soon as `ServiceContainer` is ready.
    let direct_signal = worker_shutdown_signal()?;

    // The worker is already enrolled in its OS containment domain. It cannot
    // open SQLite, output files, or sockets until the supervisor has durably
    // installed the generation marker and sent START.
    let mut worker_control = wait_for_worker_start().await?;

    install_rustls_provider();

    let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "logs".to_string());
    let (logging_config, _guard) = init_logging(&log_dir)
        .map_err(|error| anyhow::anyhow!("Failed to initialize logging: {error}"))?;
    install_panic_hook(&log_dir);

    info!(
        generation = %worker_control.generation,
        "Starting contained rust-srec runtime v{}",
        env!("CARGO_PKG_VERSION")
    );
    if let Some(previous_generation) = worker_control.previous_dirty_generation {
        warn!(
            %previous_generation,
            unresolved_generations = %worker_control.unresolved_generations,
            "Previous contained runtime did not exit cleanly; startup recovery is required"
        );
    }

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:srec.db?mode=rwc".to_string());
    info!("Connecting to database: {}", database_url);
    let (pool, write_pool) = init_database_pools(&database_url).await?;

    info!("Running database migrations...");
    run_migrations(&pool).await?;
    info!("Database migrations complete");

    info!("Initializing services...");
    let container = Arc::new(ServiceContainer::new(pool, write_pool).await?);
    logging_config
        .apply_persisted_filter(container.config_service())
        .await;
    container.set_logging_config(logging_config.clone());
    container.initialize().await?;
    container.start_api_server().await?;

    let startup_event = NotificationEvent::SystemStartup {
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: chrono::Utc::now(),
    };
    if let Err(error) = container.notification_service().notify(startup_event).await {
        warn!(%error, "Failed to send startup notification");
    }
    info!("Contained rust-srec runtime started successfully");

    let (shutdown_reason, shutdown_started_at) = tokio::select! {
        control = worker_control.wait_for_shutdown() => {
            match control {
                Ok(WorkerShutdownReason::Signal) => {
                    let reason = WorkerShutdownReason::Signal;
                    info!(?reason, "Runtime supervisor requested shutdown");
                    (reason, Instant::now())
                }
                Ok(WorkerShutdownReason::SupervisorDisconnected) => fail_stop_worker(
                    "runtime supervisor disconnected before requesting shutdown",
                ),
                Err(error) => {
                    fail_stop_worker(&format!("runtime control pipe failed: {error}"))
                }
            }
        }
        reason = direct_signal => {
            // `run_supervisor` may observe the same signal and write a SHUTDOWN
            // frame that `WorkerControl::wait_for_shutdown` no longer reads.
            // Both routes yield `WorkerShutdownReason::Signal` and the same
            // drain below, and the parent's absolute deadline bounds this
            // shutdown either way.
            info!(?reason, "Signal delivered directly to the contained runtime");
            (reason, Instant::now())
        }
        failure = container.wait_for_runtime_failure() => {
            fail_stop_worker(&format!("critical runtime failure: {failure}"))
        }
    };

    let schedule = worker_shutdown_schedule(shutdown_started_at);
    let watchdog = schedule.arm_watchdog()?;

    let shutdown_event = NotificationEvent::SystemShutdown {
        reason: shutdown_reason.description().to_string(),
        timestamp: chrono::Utc::now(),
    };
    let notification_deadline = tokio::time::Instant::from_std(schedule.cooperative_at())
        .min(tokio::time::Instant::now() + Duration::from_secs(1));
    match tokio::time::timeout_at(
        notification_deadline,
        container.notification_service().notify(shutdown_event),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!(%error, "Failed to send shutdown notification"),
        Err(_) => warn!("Shutdown notification exceeded its deadline; continuing service drain"),
    }

    info!("Shutting down contained services...");
    let service_schedule = ServiceShutdownSchedule::new(
        tokio::time::Instant::from_std(schedule.cooperative_at()),
        tokio::time::Instant::from_std(schedule.force_at()),
        tokio::time::Instant::from_std(schedule.deadline()),
    )?;
    let shutdown_result = container.shutdown_with_schedule(service_schedule).await;
    watchdog.disarm()?;
    shutdown_result?;
    info!("Contained rust-srec runtime shutdown complete");

    Ok(())
}

fn fail_stop_worker(reason: &str) -> ! {
    // The supervisor only sees the exit status, so `reason` is the sole record
    // of which condition ended this worker. Log before exiting: the parent
    // reports `CrashedRecoveryPending` and nothing else names the cause.
    error!(%reason, "Contained runtime failing stop");
    // The parent owns the only bounded shutdown path. Exiting immediately lets
    // it contain descendants and retain the dirty generation for recovery.
    std::process::exit(WORKER_FAIL_STOP_EXIT_CODE);
}

/// Terminal signals addressed to the worker process itself.
///
/// `ContainedChild::spawn` puts the worker in its own process group, so the
/// streams `supervisor_shutdown_signal` registers in the parent do not see a
/// SIGTERM sent to the worker's PID or broadcast to every member of a service
/// cgroup. Observing those signals here routes them through the same
/// `WorkerShutdownReason::Signal` path as a SHUTDOWN frame on the control pipe,
/// so `container.shutdown()` still drains events and finalizes segments.
#[cfg(unix)]
fn worker_shutdown_signal()
-> anyhow::Result<impl std::future::Future<Output = WorkerShutdownReason> + Send + 'static> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => WorkerShutdownReason::Signal,
            _ = terminate.recv() => WorkerShutdownReason::Signal,
        }
    })
}

/// `ContainedChild::spawn` launches the worker with `CREATE_NO_WINDOW` inside a
/// Job Object, so it shares no console and receives no control events. Shutdown
/// reaches it only as a SHUTDOWN frame written by `run_supervisor`, which
/// `WorkerControl::wait_for_shutdown` already observes.
#[cfg(windows)]
fn worker_shutdown_signal()
-> anyhow::Result<impl std::future::Future<Output = WorkerShutdownReason> + Send + 'static> {
    Ok(std::future::pending::<WorkerShutdownReason>())
}

#[cfg(unix)]
fn supervisor_shutdown_signal()
-> anyhow::Result<impl std::future::Future<Output = WorkerShutdownReason> + Send + 'static> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => WorkerShutdownReason::Signal,
            _ = terminate.recv() => WorkerShutdownReason::Signal,
        }
    })
}

#[cfg(windows)]
fn supervisor_shutdown_signal()
-> anyhow::Result<impl std::future::Future<Output = WorkerShutdownReason> + Send + 'static> {
    use tokio::signal::windows;

    let mut ctrl_c = windows::ctrl_c()?;
    let mut ctrl_break = windows::ctrl_break()?;
    Ok(async move {
        tokio::select! {
            _ = ctrl_c.recv() => WorkerShutdownReason::Signal,
            _ = ctrl_break.recv() => WorkerShutdownReason::Signal,
        }
    })
}
