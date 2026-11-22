#[cfg(feature = "devtools")]
use tauri::Manager;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::window_state::{show_main_window, toggle_main_window};

/// Identifier of the application menu's Quit item.
pub(crate) const APP_MENU_QUIT_ID: &str = "app-menu-quit";

/// Tauri's default macOS menu with Quit routed through this app.
///
/// The predefined Quit item sends AppKit `terminate:`, which never raises
/// `ExitRequested`, so Cmd-Q and the application menu would skip the backend
/// drain that the tray's Quit performs. A regular item with the same key
/// equivalent goes through `AppHandle::exit` instead. Everything else mirrors
/// `Menu::default` so the Edit key equivalents keep reaching the webview and
/// the Window and Help submenus keep their AppKit roles.
#[cfg(desktop)]
pub(crate) fn app_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    use tauri::menu::{
        AboutMetadata, HELP_SUBMENU_ID, PredefinedMenuItem, Submenu, WINDOW_SUBMENU_ID,
    };

    let package = app.package_info();
    let bundle = &app.config().bundle;
    let about = AboutMetadata {
        name: Some(package.name.clone()),
        version: Some(package.version.to_string()),
        copyright: bundle.copyright.clone(),
        authors: bundle.publisher.clone().map(|publisher| vec![publisher]),
        ..Default::default()
    };
    let quit = MenuItem::with_id(
        app,
        APP_MENU_QUIT_ID,
        format!("Quit {}", package.name),
        true,
        Some("CmdOrCtrl+Q"),
    )?;

    let application = Submenu::with_items(
        app,
        package.name.clone(),
        true,
        &[
            &PredefinedMenuItem::about(app, None, Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[&PredefinedMenuItem::close_window(app, None)?],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[&PredefinedMenuItem::fullscreen(app, None)?],
    )?;
    let window = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    let help = Submenu::with_id_and_items(app, HELP_SUBMENU_ID, "Help", true, &[])?;

    Menu::with_items(app, &[&application, &file, &edit, &view, &window, &help])
}

#[cfg(desktop)]
pub(crate) fn setup_tray_icon(app: &tauri::App) -> tauri::Result<()> {
    let handle = app.handle().clone();

    let show_hide = MenuItem::new(&handle, "Show/Hide", true, None::<&str>)?;

    #[cfg(feature = "devtools")]
    let devtools = {
        let enabled =
            cfg!(debug_assertions) || std::env::var_os("RUST_SREC_DESKTOP_DEVTOOLS").is_some();
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
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => {
                toggle_main_window(tray.app_handle());
            }
            // Platforms emit Click(Up) before DoubleClick; calling show_main_window
            // rather than toggling a second time avoids showing then immediately
            // hiding the window on a double-click.
            TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => {
                show_main_window(tray.app_handle());
            }
            _ => {}
        })
        .build(&handle)?;

    Ok(())
}
