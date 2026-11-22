use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use rust_srec::backend;
use rust_srec::downloader::DownloadProgressEvent;
use tauri::{Emitter, Listener, Manager};

mod boot_error;
mod desktop_media;
mod desktop_notifications;
mod desktop_settings;
mod json_file;
mod lifecycle;
mod sleep_inhibitor;
mod tool_check;
mod tray;
mod updates;
mod window_state;

pub(crate) use lifecycle::DesktopBackendState;

use boot_error::{
    BootFailureKind, BootFailurePayload, BootFailureStage, build_init_script,
    classify_boot_failure, emit_boot_progress, show_boot_error_window,
};
use desktop_notifications::{
    load_or_create_desktop_notifications_config, register_desktop_notifications_ipc,
    should_deliver_desktop_notification, show_desktop_notification,
};
use desktop_settings::{
    BackendMode, DesktopSettings, load_or_create_desktop_settings, normalize_remote_backend_url,
    reconcile_local_desktop_sessions, should_start_minimized,
};
use lifecycle::{
    BOOT_SETTLE_CAP, BootPhase, SHUTDOWN_HARD_CAP, active_recording_count, begin_exit,
    drain_backend_for_exit, exit_owned, finish_exit, mark_exit_settled,
    notify_desktop_shutting_down, request_desktop_quit, wait_for_exit_settled,
};
use tray::{APP_MENU_QUIT_ID, app_menu};
use window_state::{
    DEFAULT_WINDOW_HEIGHT, DEFAULT_WINDOW_WIDTH, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH,
    desktop_page_url, handle_main_window_event, on_main_window_close_requested,
    restore_main_window_state, show_main_window,
};

/// Connection URL for the desktop's SQLite database.
///
/// `RuntimeLease::acquire_for_database` and SQLx both parse this URL with
/// `SqliteConnectOptions::from_str`, which strips the scheme, treats the first
/// `?` as the start of the parameters, and percent-decodes the rest as the file
/// name. Escaping `%` and `?` is therefore all it takes for the path to come
/// back unchanged, Windows verbatim (`\\?\`) and UNC prefixes included. The
/// scheme is always `sqlite://` because a bare `sqlite:` in front of a path
/// that starts with `//` would read as that longer scheme and lose the root.
fn desktop_database_url(data_dir: &Path) -> String {
    let db_path = data_dir.join("srec.db");
    let escaped = db_path
        .to_string_lossy()
        .replace('%', "%25")
        .replace('?', "%3F");
    format!("sqlite://{escaped}?mode=rwc")
}

fn load_or_create_jwt_secret(data_dir: &Path) -> io::Result<String> {
    let secret_path = data_dir.join("jwt_secret");

    if secret_path.exists() {
        let secret = std::fs::read_to_string(&secret_path)?.trim().to_string();
        if secret.len() < 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "JWT secret at '{}' is too short (need >= 32 chars)",
                    secret_path.display()
                ),
            ));
        }
        return Ok(secret);
    }

    let mut bytes = [0u8; 32];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut bytes);
    let secret = hex::encode(bytes);

    let mut opts = OpenOptions::new();
    opts.create_new(true).write(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let mut file = opts.open(&secret_path)?;
    use std::io::Write;
    file.write_all(secret.as_bytes())?;
    file.sync_all()?;

    Ok(secret)
}

fn format_remote_host(remote_url: &str) -> String {
    match tauri::Url::parse(remote_url) {
        Ok(url) => match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            _ => remote_url.to_string(),
        },
        Err(_) => remote_url.to_string(),
    }
}

async fn create_main_window_and_fallback(
    app_handle: &tauri::AppHandle,
    state: &DesktopBackendState,
    backend_url: &str,
) -> bool {
    let desktop_notifications = state.desktop_notifications();
    let init_script = build_init_script(
        backend_url,
        None,
        &desktop_notifications,
        state.active_backend_mode,
        state.active_remote_url.as_deref(),
    );

    let webview_url = desktop_page_url("index.desktop.html");

    if app_handle.get_webview_window("main").is_none() {
        match tauri::WebviewWindowBuilder::new(app_handle, "main", webview_url)
            .title("Rust-Srec")
            .inner_size(
                f64::from(DEFAULT_WINDOW_WIDTH),
                f64::from(DEFAULT_WINDOW_HEIGHT),
            )
            .min_inner_size(f64::from(MIN_WINDOW_WIDTH), f64::from(MIN_WINDOW_HEIGHT))
            .visible(false)
            .initialization_script(init_script)
            .build()
        {
            Ok(window) => {
                restore_main_window_state(&window, state);
                #[cfg(feature = "devtools")]
                {
                    if cfg!(debug_assertions)
                        || std::env::var_os("RUST_SREC_DESKTOP_DEVTOOLS").is_some()
                    {
                        window.open_devtools();
                    }
                }
            }
            Err(e) => {
                show_boot_error_window(
                    app_handle,
                    BootFailurePayload::new(
                        BootFailureStage::Window,
                        format!("Failed to create main window: {e}"),
                    ),
                )
                .await;
                return false;
            }
        }
    }

    // Fallback: if the frontend never signals readiness (e.g. hard JS crash),
    // don't leave the user stuck on the splash forever.
    {
        let fallback_handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(6)).await;
            if exit_owned(&fallback_handle) {
                return;
            }

            let skip_show = fallback_handle
                .try_state::<DesktopBackendState>()
                .is_some_and(|state| {
                    state.frontend_ready_received.load(Ordering::SeqCst)
                        || state.start_minimized_boot.swap(false, Ordering::SeqCst)
                });

            if !skip_show && let Some(main) = fallback_handle.get_webview_window("main") {
                let is_visible = main.is_visible().unwrap_or(false);
                if !is_visible {
                    show_main_window(&fallback_handle);
                }
            }

            if let Some(splash) = fallback_handle.get_webview_window("splash") {
                let _ = splash.close();
            }
        });
    }

    true
}

async fn run_desktop_remote_init(
    app_handle: tauri::AppHandle,
    log_dir_str: String,
    remote_url: String,
) {
    let state = app_handle.state::<DesktopBackendState>();
    if state.init_cancel.is_cancelled() {
        return;
    }

    emit_boot_progress(&app_handle, "Initializing...", 0.2);

    let log_dir_str_clone = log_dir_str.clone();
    let logging_result =
        tokio::task::spawn_blocking(move || backend::init_logging(&log_dir_str_clone)).await;

    let (logging_config, log_guard) = match logging_result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            show_boot_error_window(
                &app_handle,
                BootFailurePayload::new(
                    BootFailureStage::Logging,
                    format!("Failed to initialize logging: {e}"),
                ),
            )
            .await;
            return;
        }
        Err(e) => {
            show_boot_error_window(
                &app_handle,
                BootFailurePayload::new(
                    BootFailureStage::Logging,
                    format!("Logging initialization task failed: {e}"),
                ),
            )
            .await;
            return;
        }
    };
    backend::install_panic_hook(&log_dir_str);
    state.set_log_guard(log_guard);
    logging_config.start_retention_cleanup(state.init_cancel.clone());

    if state.init_cancel.is_cancelled() {
        return;
    }

    log::info!(
        "Desktop remote mode initialized; targeting remote backend {}",
        remote_url
    );
    emit_boot_progress(&app_handle, "Connecting to remote server...", 0.8);

    let _ = create_main_window_and_fallback(&app_handle, &state, &remote_url).await;
}

async fn run_desktop_backend_init(
    app_handle: tauri::AppHandle,
    data_dir: PathBuf,
    log_dir_str: String,
    desktop_jwt_secret: Option<String>,
    boot_phase: &tokio::sync::watch::Sender<BootPhase>,
) {
    let overall = Instant::now();
    let state = app_handle.state::<DesktopBackendState>();
    let init_cancel = state.init_cancel.clone();

    if init_cancel.is_cancelled() {
        return;
    }

    emit_boot_progress(&app_handle, "Initializing...", 0.1);

    let database_url = desktop_database_url(&data_dir);
    // Parallelize logging init and migration pool creation for faster startup.
    let log_and_pool_start = Instant::now();
    let log_dir_str_clone = log_dir_str.clone();
    let logging_future =
        tokio::task::spawn_blocking(move || backend::init_logging(&log_dir_str_clone));

    let migration_pool_future = async {
        let mut retry_count = 0_u32;
        loop {
            let result = backend::init_migration_pool(&database_url).await;
            let should_retry = result.as_ref().is_err_and(|error| {
                classify_boot_failure(BootFailureStage::Database, &error.to_string())
                    == BootFailureKind::DatabaseBusy
                    && retry_count < 2
            });

            if !should_retry {
                break result;
            }

            retry_count += 1;
            tokio::time::sleep(Duration::from_millis(250 * u64::from(retry_count))).await;
        }
    };

    let (logging_result, migration_pool_result) =
        tokio::join!(logging_future, migration_pool_future);

    // Handle logging result
    let (logging_config, log_guard) = match logging_result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            show_boot_error_window(
                &app_handle,
                BootFailurePayload::new(
                    BootFailureStage::Logging,
                    format!("Failed to initialize logging: {e}"),
                ),
            )
            .await;
            return;
        }
        Err(e) => {
            show_boot_error_window(
                &app_handle,
                BootFailurePayload::new(
                    BootFailureStage::Logging,
                    format!("Logging initialization task failed: {e}"),
                ),
            )
            .await;
            return;
        }
    };
    backend::install_panic_hook(&log_dir_str);
    state.set_log_guard(log_guard);

    let log_and_pool_ms = log_and_pool_start.elapsed().as_millis();
    log::info!("Desktop init: logging + db pool took {}ms", log_and_pool_ms);

    let migration_pool = match migration_pool_result {
        Ok(pool) => pool,
        Err(e) => {
            show_boot_error_window(
                &app_handle,
                BootFailurePayload::new(
                    BootFailureStage::Database,
                    format!("Failed to open database: {e}"),
                ),
            )
            .await;
            return;
        }
    };

    if init_cancel.is_cancelled() {
        return;
    }

    emit_boot_progress(&app_handle, "Running database migrations...", 0.3);

    let migrations_start = Instant::now();

    let migrations_result = backend::run_migrations(&migration_pool).await;
    migration_pool.close().await;
    if let Err(e) = migrations_result {
        show_boot_error_window(
            &app_handle,
            BootFailurePayload::new(
                BootFailureStage::Migrations,
                format!("Database migration failed: {e}"),
            ),
        )
        .await;
        return;
    }

    let migrations_ms = migrations_start.elapsed().as_millis();
    log::info!("Desktop init: migrations took {}ms", migrations_ms);

    if init_cancel.is_cancelled() {
        return;
    }

    let (pool, write_pool) = match backend::init_database_pools(&database_url).await {
        Ok(pools) => pools,
        Err(e) => {
            show_boot_error_window(
                &app_handle,
                BootFailurePayload::new(
                    BootFailureStage::Database,
                    format!("Failed to open database: {e}"),
                ),
            )
            .await;
            return;
        }
    };

    if init_cancel.is_cancelled() {
        return;
    }

    emit_boot_progress(&app_handle, "Creating services...", 0.5);

    let container_start = Instant::now();

    let api_config = backend::ApiServerConfig {
        bind_address: "127.0.0.1".to_string(),
        port: 0,
        enable_cors: true,
        body_limit: 10 * 1024 * 1024,
    };

    let container = match backend::ServiceContainer::with_full_config(
        pool,
        write_pool,
        backend::ServiceContainerConfig {
            cache_ttl: Duration::from_secs(3600),
            event_capacity: 256,
            download_config: backend::DownloadManagerConfig::default(),
            pipeline_config: backend::PipelineManagerConfig::default(),
            api_config,
        },
    )
    .await
    {
        Ok(c) => Arc::new(c),
        Err(e) => {
            show_boot_error_window(
                &app_handle,
                BootFailurePayload::new(
                    BootFailureStage::Backend,
                    format!("Backend initialization failed: {e}"),
                ),
            )
            .await;
            return;
        }
    };

    let container_ms = container_start.elapsed().as_millis();
    log::info!(
        "Desktop init: service container build took {}ms",
        container_ms
    );

    // Desktop-safe output folder default.
    // Only override if the database still has the docker-first default.
    match container.config_service().get_global_config().await {
        Ok(mut global_config) => {
            if global_config.output_folder.trim().is_empty()
                || global_config.output_folder == "/app/output"
            {
                let output_dir = data_dir.join("output");
                if let Err(e) = std::fs::create_dir_all(&output_dir) {
                    log::warn!("Failed to create desktop output directory: {}", e);
                }
                global_config.output_folder = output_dir.to_string_lossy().to_string();
                if let Err(e) = container
                    .config_service()
                    .update_global_config(&global_config)
                    .await
                {
                    log::warn!("Failed to persist desktop output directory: {}", e);
                }
            }
        }
        Err(e) => {
            log::warn!("Failed to load global config: {}", e);
        }
    }

    logging_config
        .apply_persisted_filter(container.config_service())
        .await;
    container.set_logging_config(logging_config);

    // Publish the phase before the cancellation check. The exit path cancels
    // first and reads the phase second, so it either sees `Services` and waits
    // for this task to return, or this check returns before services start.
    boot_phase.send_replace(BootPhase::Services);

    if init_cancel.is_cancelled() {
        return;
    }

    emit_boot_progress(&app_handle, "Starting services...", 0.7);

    let services_start = Instant::now();

    let initialized = container.initialize().await;
    // Hand the container to the exit path before inspecting the result: a
    // failed initialize may already have started actors and downloads.
    state.set_container(container.clone());

    if let Err(e) = initialized {
        // Settle before creating any window so an overlapping `RunEvent::Exit`
        // cannot deadlock the main thread waiting on `BootPhase::Settled`.
        boot_phase.send_replace(BootPhase::Settled);
        show_boot_error_window(
            &app_handle,
            BootFailurePayload::new(
                BootFailureStage::Services,
                format!("Service initialization failed: {e}"),
            ),
        )
        .await;
        return;
    }

    if init_cancel.is_cancelled() {
        return;
    }

    let services_ms = services_start.elapsed().as_millis();
    log::info!(
        "Desktop init: service initialization took {}ms",
        services_ms
    );

    emit_boot_progress(&app_handle, "Starting API server...", 0.9);

    let api_server_start = Instant::now();

    let backend_addr = match desktop_jwt_secret {
        Some(secret) => {
            container
                .start_api_server_bound_with_jwt_secret(secret)
                .await
        }
        None => container.start_api_server_bound().await,
    };
    let backend_addr = match backend_addr {
        Ok(addr) => addr,
        Err(e) => {
            boot_phase.send_replace(BootPhase::Settled);
            show_boot_error_window(
                &app_handle,
                BootFailurePayload::new(
                    BootFailureStage::ApiServer,
                    format!("Failed to start API server: {e}"),
                ),
            )
            .await;
            return;
        }
    };

    // Before the main window exists, so the session the webview kept in
    // localStorage is already tracked (or revoked) when it makes a request.
    reconcile_local_desktop_sessions(&state, &container).await;

    let api_server_ms = api_server_start.elapsed().as_millis();
    log::info!(
        "Desktop init: api server bind/start took {}ms",
        api_server_ms
    );

    {
        let app_handle = app_handle.clone();
        let container = container.clone();
        tauri::async_runtime::spawn(async move {
            let cancellation_token = container.cancellation_token();
            let failure = tokio::select! {
                biased;
                failure = container.wait_for_runtime_failure() => failure,
                _ = cancellation_token.cancelled() => return,
            };
            log::error!("Critical backend failure: {}", failure);

            if !begin_exit(&app_handle) {
                return;
            }
            drain_backend_for_exit(&app_handle).await;
            finish_exit(&app_handle, 1);
        });
    }

    let total_ms = overall.elapsed().as_millis();
    log::info!("Desktop init: total {}ms", total_ms);
    log::info!(
        "Desktop init summary: log_pool={}ms migrations={}ms container={}ms services={}ms api_server={}ms total={}ms",
        log_and_pool_ms,
        migrations_ms,
        container_ms,
        services_ms,
        api_server_ms,
        total_ms
    );

    // Backend initialization is complete and stored on `DesktopBackendState`.
    // Settle before creating the main window so `WebviewWindowBuilder::build`
    // (which dispatches to the main thread) can never deadlock with an
    // overlapping `RunEvent::Exit` parked in `wait_for_boot_settled`.
    boot_phase.send_replace(BootPhase::Settled);

    if init_cancel.is_cancelled() {
        return;
    }

    let backend_url = format!("http://{}", backend_addr);
    if !create_main_window_and_fallback(&app_handle, &state, &backend_url).await {
        return;
    }

    if init_cancel.is_cancelled() {
        return;
    }

    // Spawn native desktop notification listener.
    // Subscribe to notification events from the backend and display OS notifications
    // for important events (stream online, errors).
    {
        let app_handle = app_handle.clone();
        let notification_rx = container.notification_service().subscribe();
        let cancellation_token = container.cancellation_token();
        tauri::async_runtime::spawn(async move {
            run_desktop_notification_listener(app_handle, notification_rx, cancellation_token)
                .await;
        });
    }

    // Spawn desktop runtime watcher for live tray status/icon updates and
    // active-recording sleep inhibition.
    {
        let app_handle = app_handle.clone();
        let container = container.clone();
        tauri::async_runtime::spawn(async move {
            run_desktop_runtime_watcher(app_handle, container).await;
        });
    }
}

/// Minimum interval between progress-driven tray refreshes while recording.
const TRAY_PROGRESS_THROTTLE: Duration = Duration::from_secs(2);

/// Keep the system tray status and OS sleep inhibitor synchronized with live
/// recording activity and desktop settings changes.
async fn run_desktop_runtime_watcher(
    app_handle: tauri::AppHandle,
    container: Arc<backend::ServiceContainer>,
) {
    let sleep_inhibitor = sleep_inhibitor::SleepInhibitorWorker::spawn();
    let mut rx = container.subscribe_download_events();
    let cancellation_token = container.cancellation_token();
    let settings_notify = app_handle
        .state::<DesktopBackendState>()
        .settings_notify
        .clone();

    let sync_now = || {
        if exit_owned(&app_handle) {
            return;
        }
        let summary = container.active_recording_summary();
        let prevent_sleep = app_handle
            .try_state::<DesktopBackendState>()
            .map(|state| state.desktop_settings().prevent_sleep_while_recording)
            .unwrap_or(true);
        sleep_inhibitor.sync(prevent_sleep, summary.active_count);
        #[cfg(desktop)]
        tray::update_tray(&app_handle, summary, false);
    };

    sync_now();
    let mut last_sync = Instant::now();
    let mut progress_dirty = false;

    loop {
        let flush_deadline = tokio::time::Instant::from_std(last_sync + TRAY_PROGRESS_THROTTLE);

        tokio::select! {
            _ = cancellation_token.cancelled() => {
                log::debug!("Desktop runtime watcher shutting down");
                break;
            }
            _ = settings_notify.notified() => {
                sync_now();
                last_sync = Instant::now();
                progress_dirty = false;
            }
            _ = async {
                if progress_dirty {
                    tokio::time::sleep_until(flush_deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                sync_now();
                last_sync = Instant::now();
                progress_dirty = false;
            }
            result = rx.recv() => {
                match result {
                    Ok(backend::DownloadManagerEvent::Terminal(_))
                    | Ok(backend::DownloadManagerEvent::Progress(
                        DownloadProgressEvent::DownloadStarted { .. },
                    )) => {
                        sync_now();
                        last_sync = Instant::now();
                        progress_dirty = false;
                    }
                    Ok(backend::DownloadManagerEvent::Progress(
                        DownloadProgressEvent::Progress { .. },
                    )) => {
                        if last_sync.elapsed() >= TRAY_PROGRESS_THROTTLE {
                            sync_now();
                            last_sync = Instant::now();
                            progress_dirty = false;
                        } else {
                            progress_dirty = true;
                        }
                    }
                    Ok(backend::DownloadManagerEvent::Progress(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("Desktop runtime watcher lagged by {} download events", n);
                        sync_now();
                        last_sync = Instant::now();
                        progress_dirty = false;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        log::debug!("Download event channel closed, stopping runtime watcher");
                        break;
                    }
                }
            }
        }
    }
}

/// Run the desktop notification listener loop.
/// Receives notification events from the backend and displays OS notifications
/// for whitelisted event types.
async fn run_desktop_notification_listener(
    app_handle: tauri::AppHandle,
    mut rx: tokio::sync::broadcast::Receiver<backend::NotificationEvent>,
    cancellation_token: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                log::debug!("Desktop notification listener shutting down");
                break;
            }
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let state = app_handle.state::<DesktopBackendState>();
                        let cfg = state.desktop_notifications();
                        if !should_deliver_desktop_notification(&cfg, &event) {
                            continue;
                        }

                        let title = event.title();
                        let body = event.description();

                        if let Err(e) = show_desktop_notification(&app_handle, title, body).await
                        {
                            log::warn!("Failed to show desktop notification: {}", e);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("Desktop notification listener lagged by {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        log::debug!("Notification channel closed, stopping listener");
                        break;
                    }
                }
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }));
        builder = builder.plugin(tauri_plugin_notification::init());
        builder = builder.plugin(tauri_plugin_opener::init());
        builder = builder.plugin(tauri_plugin_dialog::init());
        builder = builder.plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ));

        // Installed at runtime rather than behind a cfg so every target
        // compiles the menu code. Only macOS gets it: on Windows and Linux
        // `Builder::menu` would put a menu bar on every window.
        if cfg!(target_os = "macos") {
            builder = builder.menu(app_menu).on_menu_event(|app, event| {
                if event.id() == APP_MENU_QUIT_ID {
                    request_desktop_quit(app.clone());
                }
            });
        }
    }

    let app = builder
        .invoke_handler(tauri::generate_handler![
            boot_error::open_desktop_recovery_location,
            desktop_media::open_directory_in_explorer,
            desktop_media::open_recording_in_player,
            desktop_settings::open_recordings_folder,
            desktop_settings::get_desktop_settings,
            desktop_settings::update_desktop_settings,
            desktop_settings::set_autostart_enabled,
            lifecycle::restart_desktop,
            lifecycle::quit_desktop,
            desktop_notifications::test_desktop_notifications,
            tool_check::check_desktop_tools,
            updates::check_for_updates,
            updates::skip_update_version,
            desktop_settings::test_remote_backend_connection,
            desktop_settings::desktop_auto_login,
            desktop_settings::switch_to_local_backend
        ])
        .setup(move |app| {
            backend::install_rustls_provider();

            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let desktop_notifications =
                load_or_create_desktop_notifications_config(&data_dir).unwrap_or_default();
            let desktop_settings = match load_or_create_desktop_settings(&data_dir) {
                Ok(settings) => settings,
                Err(error) => {
                    log::warn!("Failed to load desktop settings, using defaults: {}", error);
                    DesktopSettings::default()
                }
            };
            let launch_args: Vec<String> = std::env::args().collect();
            let start_minimized_boot = should_start_minimized(&desktop_settings, &launch_args);
            let check_updates_on_startup = desktop_settings.check_updates_on_startup;

            let (active_backend_mode, active_remote_url) = match desktop_settings.backend_mode {
                BackendMode::Remote => match desktop_settings
                    .remote_url
                    .as_deref()
                    .map(normalize_remote_backend_url)
                {
                    Some(Ok(url)) => (BackendMode::Remote, Some(url)),
                    Some(Err(error)) => {
                        log::warn!(
                            "Saved remote backend URL is invalid ({}); falling back to local mode",
                            error
                        );
                        (BackendMode::Local, None)
                    }
                    None => {
                        log::warn!(
                            "Remote backend mode enabled without a URL; falling back to local mode"
                        );
                        (BackendMode::Local, None)
                    }
                },
                BackendMode::Local => (BackendMode::Local, None),
            };

            let log_dir = app.path().app_log_dir()?;
            std::fs::create_dir_all(&log_dir)?;
            let log_dir_str = log_dir.to_string_lossy().to_string();

            // Local mode runs an embedded SQLite-backed server behind a database
            // ownership lease. Remote mode opens neither the database nor an API.
            //
            // The lease is defense in depth against a second backend on the
            // same SQLite file, should the single-instance plugin not exit
            // early on some platform. It is keyed on the database rather than
            // on this app's data directory, so a standalone `rust-srec` server
            // pointed at the same file is excluded too: it acquires the same
            // lease before it starts.
            let (instance_lock, lease_error) = if active_backend_mode == BackendMode::Local {
                match backend::RuntimeLease::acquire_for_database(&desktop_database_url(&data_dir))
                {
                    Ok(lease) => (Some(lease), None),
                    Err(error) => {
                        log::warn!("Failed to acquire database runtime lease: {error}");
                        (None, Some(error))
                    }
                }
            } else {
                (None, None)
            };

            let initial_phase = if lease_error.is_some() {
                BootPhase::Settled
            } else {
                BootPhase::Preparing
            };
            let (boot_phase_tx, boot_phase_rx) = tokio::sync::watch::channel(initial_phase);

            app.manage(DesktopBackendState::new(
                instance_lock,
                data_dir.clone(),
                log_dir.clone(),
                desktop_notifications,
                desktop_settings,
                active_backend_mode,
                active_remote_url.clone(),
                start_minimized_boot,
                boot_phase_rx,
            ));

            register_desktop_notifications_ipc(app.handle());

            // Create system tray icon early so the app is controllable during startup.
            #[cfg(desktop)]
            {
                let remote_host = active_remote_url.as_deref().map(format_remote_host);
                let controller = tray::setup_tray(app.handle(), remote_host)?;
                app.state::<DesktopBackendState>()
                    .set_tray_controller(controller);
            }

            if let Some(error) = lease_error {
                let app_handle = app.handle().clone();
                let failure = BootFailurePayload::new(
                    BootFailureStage::Database,
                    format!("Failed to acquire database lock: {error}"),
                );
                tauri::async_runtime::spawn(async move {
                    show_boot_error_window(&app_handle, failure).await;
                });
                return Ok(());
            }

            // The local backend always runs with authentication: a per-install
            // JWT secret is kept under the app data directory unless `JWT_SECRET`
            // is already set in the environment (a development override).
            let desktop_jwt_secret = if active_backend_mode == BackendMode::Remote
                || std::env::var("JWT_SECRET").is_ok()
            {
                None
            } else {
                match load_or_create_jwt_secret(&data_dir) {
                    Ok(secret) => Some(secret),
                    Err(error) => {
                        let app_handle = app.handle().clone();
                        let failure = BootFailurePayload::new(
                            BootFailureStage::Backend,
                            format!("Failed to initialize desktop JWT secret: {error}"),
                        );
                        boot_phase_tx.send_replace(BootPhase::Settled);
                        tauri::async_runtime::spawn(async move {
                            show_boot_error_window(&app_handle, failure).await;
                        });
                        return Ok(());
                    }
                }
            };

            // Show a lightweight loading window while the backend starts unless
            // launched minimized to the system tray.
            if !start_minimized_boot {
                let splash_url = desktop_page_url("desktop-loading.html");
                let splash_init = {
                    let state = app.state::<DesktopBackendState>();
                    build_init_script(
                        "",
                        None,
                        &state.desktop_notifications(),
                        state.active_backend_mode,
                        state.active_remote_url.as_deref(),
                    )
                };

                let splash_window = tauri::WebviewWindowBuilder::new(app, "splash", splash_url)
                    .title("Rust-Srec")
                    .inner_size(480.0, 360.0)
                    .resizable(false)
                    .decorations(false)
                    .initialization_script(splash_init)
                    .build()?;

                if let Err(e) = splash_window.center() {
                    log::warn!("Failed to center splash window: {}", e);
                }

                // The splash screen listens for `boot-progress` events, but early emits can be lost
                // before the page registers its listener. When the splash signals readiness, replay
                // the latest known progress immediately.
                let listener_handle = app.handle().clone();
                let _ = app
                    .handle()
                    .listen("rust-srec://splash-ready", move |_event| {
                        log::info!("Splash window signaled ready");
                        let state = listener_handle.state::<DesktopBackendState>();
                        let payload = state.current_boot_progress();
                        if let Err(e) = listener_handle.emit("boot-progress", payload) {
                            log::warn!("Failed to replay boot progress: {}", e);
                        }
                    });
            }

            // The main window stays hidden until the frontend emits `rust-srec://frontend-ready`.
            // We register the listener immediately so it works regardless of when the main window is created.
            {
                let listener_handle = app.handle().clone();
                let _ = app
                    .handle()
                    .listen("rust-srec://frontend-ready", move |_event| {
                        log::info!("Desktop frontend signaled ready");
                        let stay_hidden = listener_handle
                            .try_state::<DesktopBackendState>()
                            .is_some_and(|state| {
                                state.frontend_ready_received.store(true, Ordering::SeqCst);
                                state.start_minimized_boot.swap(false, Ordering::SeqCst)
                            });
                        if !stay_hidden {
                            show_main_window(&listener_handle);
                        }
                        if let Some(window) = listener_handle.get_webview_window("splash") {
                            let _ = window.close();
                        }
                    });
            }

            if check_updates_on_startup {
                let update_app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(12)).await;
                    if exit_owned(&update_app) {
                        return;
                    }
                    let Some(settings) = update_app
                        .try_state::<DesktopBackendState>()
                        .map(|state| state.desktop_settings())
                    else {
                        return;
                    };
                    if !settings.check_updates_on_startup {
                        return;
                    }
                    match updates::fetch_update_info().await {
                        Ok(info) => {
                            if info.update_available
                                && settings.skipped_update_version.as_deref()
                                    != Some(info.latest_version.as_str())
                            {
                                if let Err(error) =
                                    update_app.emit(updates::EVENT_DESKTOP_UPDATE_AVAILABLE, &info)
                                {
                                    log::warn!(
                                        "Failed to emit startup update available event: {}",
                                        error
                                    );
                                }
                                let main_visible = update_app
                                    .get_webview_window("main")
                                    .and_then(|window| window.is_visible().ok())
                                    .unwrap_or(false);
                                let notifications_enabled = update_app
                                    .try_state::<DesktopBackendState>()
                                    .is_some_and(|state| state.desktop_notifications().enabled);
                                if !main_visible
                                    && notifications_enabled
                                    && let Err(error) = show_desktop_notification(
                                        &update_app,
                                        "Rust-Srec Update Available".to_string(),
                                        format!(
                                            "Version v{} is available (current: v{}).",
                                            info.latest_version, info.current_version
                                        ),
                                    )
                                    .await
                                {
                                    log::warn!(
                                        "Failed to show startup update notification: {}",
                                        error
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            log::debug!("Background startup update check failed: {}", error);
                        }
                    }
                });
            }

            let app_handle = app.handle().clone();
            let data_dir = data_dir.clone();
            let log_dir_str = log_dir_str.clone();
            tauri::async_runtime::spawn(async move {
                if active_backend_mode == BackendMode::Remote
                    && let Some(remote_url) = active_remote_url
                {
                    run_desktop_remote_init(app_handle, log_dir_str, remote_url).await;
                } else {
                    run_desktop_backend_init(
                        app_handle,
                        data_dir,
                        log_dir_str,
                        desktop_jwt_secret,
                        &boot_phase_tx,
                    )
                    .await;
                }
                // A panic above drops the sender instead, which the exit path
                // also treats as settled.
                boot_phase_tx.send_replace(BootPhase::Settled);
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } = &event
            && label == "main"
        {
            api.prevent_close();
            on_main_window_close_requested(app_handle);
            return;
        }

        // Clicking the Dock icon is the only way back to a window that was
        // hidden to the tray when the menu-bar icon is not at hand.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } = &event
        {
            show_main_window(app_handle);
        }

        #[cfg(desktop)]
        {
            if let tauri::RunEvent::WindowEvent { label, event, .. } = &event
                && label == "main"
            {
                handle_main_window_event(app_handle, event);
            }
        }

        if let tauri::RunEvent::ExitRequested { code, api, .. } = &event {
            if *code == Some(tauri::RESTART_EXIT_CODE) {
                // Tauri ignores `prevent_exit` for restarts. `restart_desktop`
                // drains before requesting one; a restart requested by anything
                // else is drained by the `Exit` arm below.
                return;
            }
            if !begin_exit(app_handle) {
                // Another path owns the exit. Its `finish_exit` passes; any
                // other request (a second tray Quit, the recovery screen's
                // Quit) would abandon the drain in progress, so hold it back.
                let draining = app_handle
                    .try_state::<DesktopBackendState>()
                    .is_some_and(|state| !*state.exit_settled.borrow());
                if draining {
                    api.prevent_exit();
                }
                return;
            }
            api.prevent_exit();

            let active_count = active_recording_count(app_handle);
            notify_desktop_shutting_down(app_handle, active_count);

            let exit_code = code.unwrap_or(0);
            let app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                drain_backend_for_exit(&app_handle).await;
                finish_exit(&app_handle, exit_code);
            });
        }

        if matches!(event, tauri::RunEvent::Exit) {
            // AppKit's `terminate:` (Dock → Quit, logout, a Quit sent by
            // another process) never raises `ExitRequested`: Tao forwards
            // `applicationWillTerminate:` as this final event and the process
            // ends once this callback returns. The drain therefore has to run
            // to completion here, on the main thread; when another path owns
            // the exit, this waits for its drain instead of ending under it.
            if begin_exit(app_handle) {
                tauri::async_runtime::block_on(drain_backend_for_exit(app_handle));
                mark_exit_settled(app_handle);
            } else if let Some(state) = app_handle.try_state::<DesktopBackendState>()
                && !tauri::async_runtime::block_on(wait_for_exit_settled(
                    state.exit_settled.subscribe(),
                    BOOT_SETTLE_CAP + SHUTDOWN_HARD_CAP,
                ))
            {
                log::warn!("Exit owner did not finish draining in time; letting the process end");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn database_url_reaches_a_data_dir_whose_name_needs_escaping() {
        // `?` cannot appear in a Windows file name.
        let dir_name = if cfg!(windows) {
            "acct 100%25 data"
        } else {
            "acct?1 100%25 data"
        };
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let base_dir = std::env::temp_dir().join(format!(
            "rust-srec-desktop-db-url-{}-{unique}",
            std::process::id()
        ));
        let data_dir = base_dir.join(dir_name);
        std::fs::create_dir_all(&data_dir).expect("create data dir");

        let url = desktop_database_url(&data_dir);
        let lease =
            backend::RuntimeLease::acquire_for_database(&url).expect("acquire runtime lease");
        let contention = backend::RuntimeLease::acquire_for_database(&url)
            .err()
            .expect("second runtime lease should contend");
        assert_eq!(
            classify_boot_failure(BootFailureStage::Database, &contention.to_string()),
            BootFailureKind::DatabaseBusy
        );

        let pool = backend::init_migration_pool(&url)
            .await
            .expect("open SQLite pool");
        pool.close().await;
        assert!(data_dir.join("srec.db").exists());
        drop(lease);
        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
