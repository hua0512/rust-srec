# rust-srec-desktop AGENTS.md

Applies to the desktop wrapper. Follow root workflow rules; apply parent backend service conventions only when changing the embedded backend integration.

## OVERVIEW
- Tauri wrapper for `rust-srec`.
- Local mode (default) runs the backend in-process (scheduler + API server), bound to `127.0.0.1:0` (ephemeral port).
- Remote mode points the webview at another rust-srec server and starts no local database, lease or API.
- Enforces single-instance: a second launch focuses the existing window.
- Injects `__RUST_SREC_BACKEND_URL__`, `__RUST_SREC_BOOT_ERROR__`, `__RUST_SREC_DESKTOP_NOTIFICATIONS__`, `__RUST_SREC_BACKEND_MODE__`, and `__RUST_SREC_REMOTE_URL__` into the webview.

## WHERE TO LOOK
- `src/main.rs`: Binary shim calling `rust_srec_desktop_lib::run()`; keep its `windows_subsystem` attribute.
- `src/lib.rs`: App setup, single-instance plugin, database lease, local and remote backend initialization, the runtime watcher (tray status, sleep inhibition), run-loop event dispatch.
- `src/lifecycle.rs`: `DesktopBackendState`, boot phases, quit/restart confirmation, and the exit path that drains the backend before the process ends.
- `src/boot_error.rs`: Startup failure classification, the recovery window, boot progress, and the webview init script.
- `src/tray.rs`: Tray icon, live recording status and menu, and the macOS application menu.
- `src/window_state.rs`: Saved window geometry and its restore; showing, hiding and centring the main window; close and minimize-to-tray handling.
- `src/desktop_settings.rs`: `desktop_settings.json`, the settings commands, remote backend URL checks, and local auto-login sessions.
- `src/desktop_media.rs`: Commands that open folders and recordings with system programs, and the path checks in front of them.
- `src/sleep_inhibitor.rs`, `src/tool_check.rs`, `src/updates.rs`: Sleep inhibition while recording, external tool probing, and the release check.
- `src/json_file.rs`: Atomic writer for the JSON files kept in the app data directory.
- `src/desktop_notifications.rs`: Native OS notification integration.
- `tauri.conf.json`: Tauri configuration (capabilities, build settings, beforeDevCommand).

## CONVENTIONS
- **In-process backend**: Backend tasks run on Tauri's async runtime (`tauri::async_runtime`).
- **Port management**: Always use ephemeral port `0` for binding; read back resolved port for webview injection.
- **Persistence**: 
  - `JWT_SECRET` generated and persisted in app data dir if not in env.
  - A `RuntimeLease` keyed on the SQLite database keeps a second backend (desktop or standalone server) off the same DB.
  - Shell-owned JSON files in the app data dir are written through `json_file::write_json_atomically`, each under its own write lock.
- **Auto-login**: `require_login` defaults to off. While it is off, the backend refuses password logins for accounts that still have `must_change_password` (the seeded default password), set before the API starts and on every `commit_desktop_settings`. `require_login` cannot be stored as on while the auto-login account still has its initial password; `set_desktop_password` replaces it without the old one, once. Sessions issued without a password come only from the `desktop_auto_login` command, are listed in `local_desktop_sessions.json`, and are revoked before `require_login` is stored as on. Commands that act on backend-supplied paths refuse to run in remote mode.
- **First-run rewrite**: Detect docker-default `/app/output` in DB and rewrite to `<app_data_dir>/output`.

## ANTI-PATTERNS
- **Hardcoded ports**: Never hardcode the API port; it must remain ephemeral to avoid conflicts.
- **Static output paths**: Don't assume `/app/output` is valid; check for rewrite on first desktop run.
- **Environment mutation**: Prefer reading `JWT_SECRET` from file in app data dir rather than forcing system env vars.

## COMMANDS

Run Tauri commands from `rust-srec/`, with the frontend and platform prerequisites installed. Node and pnpm versions come from the root `.nvmrc` and frontend `package.json`.

- Dev: `cargo tauri dev`
  - `beforeDevCommand` starts the desktop frontend dev server (`pnpm -C frontend dev:desktop`) at `http://127.0.0.1:15275`.
- Build: `cargo tauri build`
- Check: `cargo check --locked -p rust-srec-desktop`

For a local wrapper change, start with the package check and relevant tests. Build the desktop frontend when its integration changes; use a Tauri build for changes affecting packaging, capabilities, or application startup. Exercise affected startup/persistence behavior on a supported platform. Full desktop checks are not prerequisites for unrelated backend or prose changes.

For core backend or frontend commands, see [root AGENTS.md](../../AGENTS.md) and the [frontend guide](../frontend/AGENTS.md).
