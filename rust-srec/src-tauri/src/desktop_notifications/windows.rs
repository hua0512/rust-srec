use std::path::{Path, PathBuf};

use anyhow::Context;

const NOTIFICATION_ICON_FILENAME: &str = "notification.png";
const NOTIFICATION_ICON_BYTES: &[u8] = include_bytes!("../../icons/128x128.png");

/// Write the bundled notification icon to `data_dir`, refreshing the file on disk
/// whenever the bundled icon changes across app upgrades.
fn sync_notification_icon(data_dir: &Path, icon_bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let icon_path = data_dir.join(NOTIFICATION_ICON_FILENAME);
    let up_to_date = std::fs::read(&icon_path)
        .map(|existing| existing == icon_bytes)
        .unwrap_or(false);
    if !up_to_date {
        std::fs::write(&icon_path, icon_bytes).with_context(|| {
            format!(
                "Failed to write notification icon at {}",
                icon_path.display()
            )
        })?;
    }
    Ok(icon_path)
}

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
    let icon_path = sync_notification_icon(data_dir, NOTIFICATION_ICON_BYTES)?;
    let registration = windows_registry::CURRENT_USER
        .create(format!(r"Software\Classes\AppUserModelId\{app_id}"))
        .context("Failed to register the Windows notification identity")?;
    registration
        .set_string("DisplayName", display_name)
        .context("Failed to register the Windows notification display name")?;
    registration
        .set_string("IconUri", icon_path.to_string_lossy())
        .context("Failed to register the Windows notification icon")?;
    registration
        .set_string("IconBackgroundColor", "0")
        .context("Failed to register the Windows notification icon background")?;

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
