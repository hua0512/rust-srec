use std::path::Path;

use anyhow::Context;

/// Register the same identity used to send the toast, including for portable builds
/// that have never run an installer. Only per-user display metadata is written;
/// existing installer activation settings and OS notification preferences are kept.
pub(super) fn show(
    app_id: &str,
    display_name: &str,
    data_dir: &Path,
    title: &str,
    body: &str,
) -> anyhow::Result<()> {
    let icon_path = data_dir.join("notification.ico");
    if !icon_path.exists() {
        std::fs::write(&icon_path, include_bytes!("../../icons/icon.ico")).with_context(|| {
            format!(
                "Failed to write notification icon at {}",
                icon_path.display()
            )
        })?;
    }
    let registration = windows_registry::CURRENT_USER
        .create(format!(r"Software\Classes\AppUserModelId\{app_id}"))
        .context("Failed to register the Windows notification identity")?;
    registration
        .set_string("DisplayName", display_name)
        .context("Failed to register the Windows notification display name")?;
    registration
        .set_string("IconUri", icon_path.to_string_lossy())
        .context("Failed to register the Windows notification icon")?;

    // Tauri's plugin guesses whether the app is installed from the executable's
    // directory and discards delivery errors in a detached task. Explicitly use
    // our registered identity and return WinRT submission errors to the caller.
    notify_rust::Notification::new()
        .app_id(app_id)
        .summary(title)
        .body(body)
        .show()
        .context("Windows could not submit the notification")?;
    Ok(())
}
