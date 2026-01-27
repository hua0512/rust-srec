use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::Instant;

use fs2::FileExt;
use serde::Serialize;
use tauri::Emitter;
use tauri::Listener;
use tauri::Manager;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri_plugin_notification::NotificationExt;

mod desktop_notifications;

use desktop_notifications::{
    DesktopNotificationConfig, load_or_create_desktop_notifications_config,
    register_desktop_notifications_ipc, should_deliver_desktop_notification,
};

#[derive(Clone, Serialize)]
struct BootProgressPayload {
    status: String,
    progress: f32, // 0.0 to 1.0
}

impl Default for BootProgressPayload {
    fn default() -> Self {
        Self {
            status: "Starting...".to_string(),
            progress: 0.0,
        }
    }
}

// Desktop notifications are implemented in `desktop_notifications`.

fn show_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

fn hide_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

fn toggle_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    match window.is_visible() {
        Ok(true) => hide_main_window(app),
        Ok(false) => show_main_window(app),
        Err(_) => show_main_window(app),
    }
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
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
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

struct DesktopBackendState {
    container: std::sync::Mutex<Option<Arc<rust_srec::services::ServiceContainer>>>,
    log_guard: std::sync::Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>,
    _instance_lock: std::fs::File,
    shutdown_started: AtomicBool,

    main_window_centered: AtomicBool,

    boot_progress: std::sync::Mutex<BootProgressPayload>,

    init_cancel: tokio_util::sync::CancellationToken,
    latest_launch: std::sync::Mutex<LaunchArgsPayload>,

    data_dir: PathBuf,
    desktop_notifications: std::sync::Mutex<DesktopNotificationConfig>,
}

#[derive(Clone, Serialize)]
struct LaunchArgsPayload {
    args: Vec<String>,
    cwd: String,
}

impl DesktopBackendState {
    fn new(
        instance_lock: std::fs::File,
        initial_launch: LaunchArgsPayload,
        data_dir: PathBuf,
        desktop_notifications: DesktopNotificationConfig,
    ) -> Self {
        Self {
            container: std::sync::Mutex::new(None),
            log_guard: std::sync::Mutex::new(None),
            _instance_lock: instance_lock,
            shutdown_started: AtomicBool::new(false),
            main_window_centered: AtomicBool::new(false),
            boot_progress: std::sync::Mutex::new(BootProgressPayload::default()),
            init_cancel: tokio_util::sync::CancellationToken::new(),
            latest_launch: std::sync::Mutex::new(initial_launch),

            data_dir,
            desktop_notifications: std::sync::Mutex::new(desktop_notifications),
        }
    }

    fn set_boot_progress(&self, status: &str, progress: f32) -> BootProgressPayload {
        let payload = BootProgressPayload {
            status: status.to_string(),
            progress: progress.clamp(0.0, 1.0),
        };

        if let Ok(mut lock) = self.boot_progress.lock() {
            *lock = payload.clone();
        }

        payload
    }

    fn current_boot_progress(&self) -> BootProgressPayload {
        self.boot_progress
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default()
    }

    fn update_launch(&self, payload: LaunchArgsPayload) {
        if let Ok(mut lock) = self.latest_launch.lock() {
            *lock = payload;
        }
    }

    fn current_launch(&self) -> LaunchArgsPayload {
        self.latest_launch
            .lock()
            .map(|p| p.clone())
            .unwrap_or(LaunchArgsPayload {
                args: Vec::new(),
                cwd: String::new(),
            })
    }

    fn set_container(&self, container: Arc<rust_srec::services::ServiceContainer>) {
        if let Ok(mut lock) = self.container.lock() {
            *lock = Some(container);
        }
    }

    fn set_log_guard(&self, log_guard: tracing_appender::non_blocking::WorkerGuard) {
        if let Ok(mut lock) = self.log_guard.lock() {
            *lock = Some(log_guard);
        }
    }

    fn backend(&self) -> Option<Arc<rust_srec::services::ServiceContainer>> {
        self.container.lock().ok().and_then(|c| c.clone())
    }
}

fn center_main_window_once(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    let should_center = app
        .try_state::<DesktopBackendState>()
        .map(|state| !state.main_window_centered.swap(true, Ordering::SeqCst))
        .unwrap_or(true);

    if !should_center {
        return;
    }

    if let Err(e) = window.center() {
        log::warn!("Failed to center main window: {}", e);
    }
}

fn emit_boot_progress(app: &tauri::AppHandle, status: &str, progress: f32) {
    let payload = app
        .try_state::<DesktopBackendState>()
        .map(|state| state.set_boot_progress(status, progress))
        .unwrap_or(BootProgressPayload {
            status: status.to_string(),
            progress: progress.clamp(0.0, 1.0),
        });

    // Always use global emit for boot progress to ensure the splash window catches it
    // regardless of whether it's listening locally or globally.
    if let Err(e) = app.emit("boot-progress", payload) {
        log::warn!("Failed to emit boot progress: {}", e);
    }
}

fn build_init_script(
    backend_url: &str,
    launch: &LaunchArgsPayload,
    boot_error: Option<&str>,
    desktop_notifications: &DesktopNotificationConfig,
) -> String {
    let backend_url_json =
        serde_json::to_string(backend_url).unwrap_or_else(|_| "\"\"".to_string());
    let launch_args_json = serde_json::to_string(&launch.args).unwrap_or_else(|_| "[]".to_string());
    let launch_cwd_json = serde_json::to_string(&launch.cwd).unwrap_or_else(|_| "\"\"".to_string());
    let boot_error_json = match boot_error {
        Some(msg) => serde_json::to_string(msg).unwrap_or_else(|_| "null".to_string()),
        None => "null".to_string(),
    };

    let desktop_notifications_json =
        serde_json::to_string(desktop_notifications).unwrap_or_else(|_| "null".to_string());

    format!(
        "globalThis.__RUST_SREC_BACKEND_URL__ = {backend_url_json};\
 globalThis.__RUST_SREC_LAUNCH_ARGS__ = {launch_args_json};\
 globalThis.__RUST_SREC_LAUNCH_CWD__ = {launch_cwd_json};\
 globalThis.__RUST_SREC_BOOT_ERROR__ = {boot_error_json};\
 globalThis.__RUST_SREC_DESKTOP_NOTIFICATIONS__ = {desktop_notifications_json};"
    )
}

async fn show_boot_error_window(app_handle: &tauri::AppHandle, message: &str) {
    let state = app_handle.state::<DesktopBackendState>();
    let launch = state.current_launch();
    let desktop_notifications = state.desktop_notifications();

    let webview_url = if tauri::is_dev() {
        tauri::WebviewUrl::External(
            tauri::Url::parse("http://127.0.0.1:15275/index.desktop.html")
                .unwrap_or_else(|_| tauri::Url::parse("http://127.0.0.1:15275/").unwrap()),
        )
    } else {
        tauri::WebviewUrl::App("index.desktop.html".into())
    };

    let init_script = build_init_script("", &launch, Some(message), &desktop_notifications);

    if app_handle.get_webview_window("main").is_none() {
        if let Ok(window) = tauri::WebviewWindowBuilder::new(app_handle, "main", webview_url)
            .title("Rust-Srec")
            .inner_size(1280.0, 1024.0)
            .min_inner_size(800.0, 600.0)
            .initialization_script(init_script)
            .build()
        {
            center_main_window_once(app_handle);
            let _ = window.show();
            let _ = window.set_focus();
        }
    } else {
        show_main_window(app_handle);
    }

    if let Some(splash) = app_handle.get_webview_window("splash") {
        let _ = splash.close();
    }
}

async fn run_desktop_backend_init(
    app_handle: tauri::AppHandle,
    data_dir: PathBuf,
    log_dir_str: String,
    desktop_jwt_secret: Option<String>,
) {
    let overall = Instant::now();
    let state = app_handle.state::<DesktopBackendState>();
    let init_cancel = state.init_cancel.clone();

    if init_cancel.is_cancelled() {
        return;
    }

    emit_boot_progress(&app_handle, "Initializing...", 0.1);

    let db_path = data_dir.join("srec.db");
    let database_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());
    // Parallelize logging init and database pool creation for faster startup.
    let log_and_pool_start = Instant::now();
    let log_dir_str_clone = log_dir_str.clone();
    let logging_future =
        tokio::task::spawn_blocking(move || rust_srec::logging::init_logging(&log_dir_str_clone));

    let pool_future = rust_srec::database::init_pool(&database_url);

    let (logging_result, pool_result) = tokio::join!(logging_future, pool_future);

    // Handle logging result
    let (logging_config, log_guard) = match logging_result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            show_boot_error_window(&app_handle, &format!("Failed to initialize logging: {e}"))
                .await;
            return;
        }
        Err(e) => {
            show_boot_error_window(&app_handle, &format!("Logging init panicked: {e}")).await;
            return;
        }
    };
    rust_srec::panic_hook::install(&log_dir_str);
    state.set_log_guard(log_guard);

    let log_and_pool_ms = log_and_pool_start.elapsed().as_millis();
    log::info!("Desktop init: logging + db pool took {}ms", log_and_pool_ms);

    // Handle database pool result
    let pool = match pool_result {
        Ok(p) => p,
        Err(e) => {
            show_boot_error_window(&app_handle, &format!("Failed to open database: {e}")).await;
            return;
        }
    };

    if init_cancel.is_cancelled() {
        return;
    }

    emit_boot_progress(&app_handle, "Running database migrations...", 0.3);

    let migrations_start = Instant::now();

    if let Err(e) = rust_srec::database::run_migrations(&pool).await {
        show_boot_error_window(&app_handle, &format!("Database migration failed: {e}")).await;
        return;
    }

    let migrations_ms = migrations_start.elapsed().as_millis();
    log::info!("Desktop init: migrations took {}ms", migrations_ms);

    if init_cancel.is_cancelled() {
        return;
    }

    emit_boot_progress(&app_handle, "Creating services...", 0.5);

    let container_start = Instant::now();

    let api_config = rust_srec::api::server::ApiServerConfig {
        bind_address: "127.0.0.1".to_string(),
        port: 0,
        enable_cors: true,
        body_limit: 10 * 1024 * 1024,
    };

    let container = match rust_srec::services::ServiceContainer::with_full_config(
        pool,
        Duration::from_secs(3600),
        256,
        rust_srec::downloader::DownloadManagerConfig::default(),
        rust_srec::pipeline::PipelineManagerConfig::default(),
        rust_srec::danmu::service::DanmuServiceConfig::default(),
        api_config,
    )
    .await
    {
        Ok(c) => Arc::new(c),
        Err(e) => {
            show_boot_error_window(&app_handle, &format!("Backend initialization failed: {e}"))
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
    match container.config_service.get_global_config().await {
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
                    .config_service
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
        .apply_persisted_filter(&container.config_service)
        .await;
    logging_config.start_retention_cleanup(container.cancellation_token());
    container.set_logging_config(logging_config);

    if init_cancel.is_cancelled() {
        return;
    }

    emit_boot_progress(&app_handle, "Starting services...", 0.7);

    let services_start = Instant::now();

    if let Err(e) = container.initialize().await {
        show_boot_error_window(&app_handle, &format!("Service initialization failed: {e}")).await;
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
            show_boot_error_window(&app_handle, &format!("Failed to start API server: {e}")).await;
            return;
        }
    };

    let api_server_ms = api_server_start.elapsed().as_millis();
    log::info!(
        "Desktop init: api server bind/start took {}ms",
        api_server_ms
    );

    state.set_container(container.clone());

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
    let launch = state.current_launch();
    let desktop_notifications = state.desktop_notifications();
    let init_script = build_init_script(&backend_url, &launch, None, &desktop_notifications);

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
                show_boot_error_window(&app_handle, &format!("Failed to create main window: {e}"))
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

            if let Some(main) = fallback_handle.get_webview_window("main") {
                let is_visible = main.is_visible().unwrap_or(false);
                if !is_visible {
                    center_main_window_once(&fallback_handle);
                    let _ = main.show();
                    let _ = main.unminimize();
                    let _ = main.set_focus();
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
        let notification_rx = container.notification_service.subscribe();
        let cancellation_token = container.cancellation_token();
        tauri::async_runtime::spawn(async move {
            run_desktop_notification_listener(app_handle, notification_rx, cancellation_token)
                .await;
        });
    }

    // Spawn minimize-to-tray watcher (hides window when user clicks minimize button).
    #[cfg(desktop)]
    {
        let app_handle = app_handle.clone();
        let cancellation = container.cancellation_token();
        tauri::async_runtime::spawn(async move {
            run_minimize_to_tray_watcher(app_handle, cancellation).await;
        });
    }
}

/// Run the desktop notification listener loop.
/// Receives notification events from the backend and displays OS notifications
/// for whitelisted event types.
async fn run_desktop_notification_listener(
    app_handle: tauri::AppHandle,
    mut rx: tokio::sync::broadcast::Receiver<rust_srec::notification::NotificationEvent>,
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

                        if let Err(e) = app_handle
                            .notification()
                            .builder()
                            .title(&title)
                            .body(&body)
                            .show()
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

/// Watch for minimize events and hide window to tray.
/// Uses smart polling: fast when visible (80ms), slow backoff when hidden (5000ms).
async fn run_minimize_to_tray_watcher(
    app_handle: tauri::AppHandle,
    cancellation: tokio_util::sync::CancellationToken,
) {
    let visible_poll = Duration::from_millis(80);
    let hidden_poll = Duration::from_millis(5000);

    // Edge-trigger so we don't spam hide() if minimized stays true.
    let mut last_seen_minimized = false;

    loop {
        let sleep_for = {
            match app_handle.get_webview_window("main") {
                Some(window) => match window.is_visible() {
                    Ok(true) => visible_poll,
                    _ => hidden_poll,
                },
                None => hidden_poll,
            }
        };

        tokio::select! {
            _ = cancellation.cancelled() => {
                log::debug!("Minimize-to-tray watcher shutting down");
                break;
            }
            _ = tokio::time::sleep(sleep_for) => {}
        }

        let Some(window) = app_handle.get_webview_window("main") else {
            continue;
        };

        let is_visible = window.is_visible().unwrap_or(false);
        if !is_visible {
            last_seen_minimized = false;
            continue;
        }

        let minimized = window.is_minimized().unwrap_or(false);

        // If it just became minimized, hide it to tray.
        if minimized && !last_seen_minimized {
            let _ = window.hide();
            last_seen_minimized = true;
            continue;
        }

        if !minimized {
            last_seen_minimized = false;
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let launch_args: Vec<String> = std::env::args().collect();
    let launch_cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(ToString::to_string))
        .unwrap_or_default();

    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(state) = app.try_state::<DesktopBackendState>() {
                state.update_launch(LaunchArgsPayload {
                    args: _argv.clone(),
                    cwd: _cwd.clone(),
                });
            }

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }

            let _ = app.emit(
                "rust-srec://single-instance",
                LaunchArgsPayload {
                    args: _argv,
                    cwd: _cwd,
                },
            );
        }));
        builder = builder.plugin(tauri_plugin_notification::init());
    }

    let app = builder
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let desktop_notifications =
                load_or_create_desktop_notifications_config(&data_dir).unwrap_or_default();

            let log_dir = app.path().app_log_dir()?;
            std::fs::create_dir_all(&log_dir)?;
            let log_dir_str = log_dir.to_string_lossy().to_string();

            // Desktop: ensure authentication is consistently enabled by providing a per-install JWT secret.
            // If JWT_SECRET is already set in the process env, we assume the user wants to manage auth
            // externally (dev override). Otherwise we persist a per-install secret under app data.
            let desktop_jwt_secret = if std::env::var("JWT_SECRET").is_ok() {
                None
            } else {
                Some(load_or_create_jwt_secret(&data_dir)?)
            };

            // Defense-in-depth: ensure we never start a 2nd backend instance against the same
            // local SQLite DB, even if the single-instance plugin doesn't exit early on some
            // platforms/configurations.
            let lock_path = data_dir.join("app.lock");
            let lock_file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(&lock_path)?;
            if lock_file.try_lock_exclusive().is_err() {
                app.handle().exit(0);
                return Ok(());
            }

            app.manage(DesktopBackendState::new(
                lock_file,
                LaunchArgsPayload {
                    args: launch_args.clone(),
                    cwd: launch_cwd.clone(),
                },
                data_dir.clone(),
                desktop_notifications,
            ));

            register_desktop_notifications_ipc(app.handle());

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
                build_init_script(
                    "",
                    &state.current_launch(),
                    None,
                    &state.desktop_notifications(),
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
                        if let Some(window) = listener_handle.get_webview_window("main") {
                            center_main_window_once(&listener_handle);
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                        if let Some(window) = listener_handle.get_webview_window("splash") {
                            let _ = window.close();
                        }
                    });
            }

            // Desktop notifications IPC bridge lives in `desktop_notifications`.

            // Create system tray icon early so the app is controllable during startup.
            #[cfg(desktop)]
            {
                let handle = app.handle().clone();

                let show_hide = MenuItem::new(&handle, "Show/Hide", true, None::<&str>)?;

                #[cfg(feature = "devtools")]
                let devtools = {
                    let enabled = cfg!(debug_assertions)
                        || std::env::var_os("RUST_SREC_DESKTOP_DEVTOOLS").is_some();
                    enabled
                        .then(|| MenuItem::new(&handle, "DevTools", true, None::<&str>))
                        .transpose()?
                };

                #[cfg(not(feature = "devtools"))]
                let devtools: Option<MenuItem<_>> = None;

                let quit = MenuItem::new(&handle, "Quit", true, None::<&str>)?;
                let menu = if let Some(devtools) = &devtools {
                    Menu::with_items(&handle, &[&show_hide, devtools, &quit])?
                } else {
                    Menu::with_items(&handle, &[&show_hide, &quit])?
                };

                let show_hide_id = show_hide.id().clone();

                #[cfg(feature = "devtools")]
                let devtools_id = devtools.as_ref().map(|item| item.id().clone());

                let quit_id = quit.id().clone();

                let icon = tauri::include_image!("./icons/32x32.png");

                TrayIconBuilder::with_id("rust-srec-tray")
                    .icon(icon)
                    .tooltip("Rust-Srec")
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(move |app: &tauri::AppHandle, event| {
                        if event.id == quit_id {
                            app.exit(0);
                            return;
                        }
                        #[cfg(feature = "devtools")]
                        {
                            if devtools_id.as_ref().is_some_and(|id| event.id == *id) {
                                if let Some(window) = app.get_webview_window("main") {
                                    if window.is_devtools_open() {
                                        window.close_devtools();
                                    } else {
                                        window.open_devtools();
                                    }
                                }
                                return;
                            }
                        }
                        if event.id == show_hide_id {
                            toggle_main_window(app);
                        }
                    })
                    .on_tray_icon_event(|tray: &tauri::tray::TrayIcon, event| match event {
                        TrayIconEvent::Click {
                            button,
                            button_state,
                            ..
                        } => {
                            if button == MouseButton::Left && button_state == MouseButtonState::Up {
                                toggle_main_window(tray.app_handle());
                            }
                        }
                        TrayIconEvent::DoubleClick { button, .. } => {
                            if button == MouseButton::Left {
                                toggle_main_window(tray.app_handle());
                            }
                        }
                        _ => {}
                    })
                    .build(&handle)?;
            }

            let app_handle = app.handle().clone();
            let data_dir = data_dir.clone();
            let log_dir_str = log_dir_str.clone();
            tauri::async_runtime::spawn(async move {
                run_desktop_backend_init(app_handle, data_dir, log_dir_str, desktop_jwt_secret)
                    .await;
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        // Intercept window close button (X) and hide to tray instead of exiting.
        if let tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } = &event
            && label == "main"
        {
            api.prevent_close();
            hide_main_window(app_handle);
            return;
        }

        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            let state = app_handle.state::<DesktopBackendState>();

            // Avoid infinite recursion when we call `app_handle.exit(...)` after shutdown.
            if state.shutdown_started.swap(true, Ordering::SeqCst) {
                return;
            }

            api.prevent_exit();

            if let Some(container) = state.backend() {
                let app_handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = container.shutdown().await {
                        log::error!("Error during shutdown: {}", e);
                    }
                    app_handle.exit(0);
                });
            } else {
                // Backend isn't ready yet; cancel initialization and exit immediately.
                state.init_cancel.cancel();
                app_handle.exit(0);
            }
        }
    });
}
