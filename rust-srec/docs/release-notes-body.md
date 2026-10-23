## rust-srec v0.8.0

This release updates Windows desktop notifications for portable builds and makes notification test failures visible.

### Highlights
- **Windows portable notifications** — register the notification name and icon for the current user before sending, without an installer or administrator access, addressing missing popups caused by an unregistered application identity.
- **Clear application name** — Windows notifications show **Rust-Srec**, while the internal application identifier and package name stay unchanged.
- **Visible test failures** — the desktop notification **Test** button now reports registration and submission errors, using the same delivery path as regular event notifications.

### Upgrade notes
No configuration or database migration is required from v0.7.0. Windows notification settings and Do Not Disturb still apply; use **Notifications → Desktop Notifications → Test** after updating. Notification registration and the icon remain in the current user's profile after exit.

Full release notes: https://docs.srec.rs/en/release-notes/v0.8.0 · 中文版：https://docs.srec.rs/zh/release-notes/v0.8.0
