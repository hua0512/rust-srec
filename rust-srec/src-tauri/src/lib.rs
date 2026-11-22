use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rust_srec::backend;
use tauri::{Emitter, Listener, Manager};

mod boot_error;
mod desktop_notifications;
mod lifecycle;
#[cfg(desktop)]
mod tray;
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
use lifecycle::{
    BOOT_SETTLE_CAP, BootPhase, SHUTDOWN_HARD_CAP, begin_exit, drain_backend_for_exit, exit_owned,
    finish_exit, mark_exit_settled, wait_for_exit_settled,
};
#[cfg(desktop)]
use tray::{APP_MENU_QUIT_ID, app_menu, setup_tray_icon};
#[cfg(desktop)]
use window_state::handle_main_window_event;
use window_state::{center_main_window_once, on_main_window_close_requested, show_main_window};

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
    logging_config.start_retention_cleanup(container.cancellation_token());
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

    if init_cancel.is_cancelled() {
        return;
    }

    let backend_url = format!("http://{}", backend_addr);
    let desktop_notifications = state.desktop_notifications();
    let init_script = build_init_script(&backend_url, None, &desktop_notifications);

    let webview_url = if tauri::is_dev() {
        tauri::WebviewUrl::External(
            tauri::Url::parse("http://127.0.0.1:15275/index.desktop.html")
                .unwrap_or_else(|_| tauri::Url::parse("http://127.0.0.1:15275/").unwrap()),
        )
    } else {
        tauri::WebviewUrl::App("index.desktop.html".into())
    };

    if app_handle.get_webview_window("main").is_none() {
        match tauri::WebviewWindowBuilder::new(&app_handle, "main", webview_url)
            .title("Rust-Srec")
            .inner_size(1280.0, 1024.0)
            .min_inner_size(800.0, 600.0)
            .visible(false)
            .initialization_script(init_script)
            .build()
        {
            Ok(window) => {
                #[cfg(feature = "devtools")]
                {
                    if cfg!(debug_assertions)
                        || std::env::var_os("RUST_SREC_DESKTOP_DEVTOOLS").is_some()
                    {
                        window.open_devtools();
                    }
                }
                let _ = window; // suppress unused warning
            }
            Err(e) => {
                show_boot_error_window(
                    &app_handle,
                    BootFailurePayload::new(
                        BootFailureStage::Window,
                        format!("Failed to create main window: {e}"),
                    ),
                )
                .await;
                return;
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

            if let Some(main) = fallback_handle.get_webview_window("main") {
                let is_visible = main.is_visible().unwrap_or(false);
                if !is_visible {
                    center_main_window_once(&fallback_handle);
                    show_main_window(&fallback_handle);
                }
            }

            if let Some(splash) = fallback_handle.get_webview_window("splash") {
                let _ = splash.close();
            }
        });
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

        // Installed at runtime rather than behind a cfg so every target
        // compiles the menu code. Only macOS gets it: on Windows and Linux
        // `Builder::menu` would put a menu bar on every window.
        if cfg!(target_os = "macos") {
            builder = builder.menu(app_menu).on_menu_event(|app, event| {
                if event.id() == APP_MENU_QUIT_ID {
                    app.exit(0);
                }
            });
        }
    }

    let app = builder
        .invoke_handler(tauri::generate_handler![
            boot_error::open_desktop_recovery_location,
            lifecycle::restart_desktop,
            lifecycle::quit_desktop,
            desktop_notifications::test_desktop_notifications
        ])
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let desktop_notifications =
                load_or_create_desktop_notifications_config(&data_dir).unwrap_or_default();

            let log_dir = app.path().app_log_dir()?;
            std::fs::create_dir_all(&log_dir)?;
            let log_dir_str = log_dir.to_string_lossy().to_string();

            // Defense-in-depth: ensure we never start a 2nd backend instance against the same
            // local SQLite DB, even if the single-instance plugin doesn't exit early on some
            // platforms/configurations.
            //
            // Keyed on the database rather than on this app's data directory,
            // so a standalone `rust-srec` server pointed at the same file is
            // excluded too — it acquires the same lease before it starts.
            let database_url = desktop_database_url(&data_dir);
            let (instance_lock, lease_error) =
                match backend::RuntimeLease::acquire_for_database(&database_url) {
                    Ok(lease) => (Some(lease), None),
                    Err(error) => {
                        log::warn!("Failed to acquire database runtime lease: {error}");
                        (None, Some(error))
                    }
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
                boot_phase_rx,
            ));

            register_desktop_notifications_ipc(app.handle());

            // Create system tray icon early so the app is controllable during startup.
            #[cfg(desktop)]
            setup_tray_icon(app)?;

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

            // Desktop: ensure authentication is consistently enabled by providing a per-install JWT secret.
            // If JWT_SECRET is already set in the process env, we assume the user wants to manage auth
            // externally (dev override). Otherwise we persist a per-install secret under app data.
            let desktop_jwt_secret = if std::env::var("JWT_SECRET").is_ok() {
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

            // Show a lightweight loading window while the backend starts.
            let splash_url = if tauri::is_dev() {
                tauri::WebviewUrl::External(tauri::Url::parse(
                    "http://127.0.0.1:15275/desktop-loading.html",
                )?)
            } else {
                tauri::WebviewUrl::App("desktop-loading.html".into())
            };

            let splash_init = {
                let state = app.state::<DesktopBackendState>();
                build_init_script("", None, &state.desktop_notifications())
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
            {
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
                        center_main_window_once(&listener_handle);
                        show_main_window(&listener_handle);
                        if let Some(window) = listener_handle.get_webview_window("splash") {
                            let _ = window.close();
                        }
                    });
            }

            let app_handle = app.handle().clone();
            let data_dir = data_dir.clone();
            let log_dir_str = log_dir_str.clone();
            tauri::async_runtime::spawn(async move {
                run_desktop_backend_init(
                    app_handle,
                    data_dir,
                    log_dir_str,
                    desktop_jwt_secret,
                    &boot_phase_tx,
                )
                .await;
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
