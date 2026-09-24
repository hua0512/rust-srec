# rust-srec-desktop AGENTS.md

Applies to the desktop wrapper. Follow root workflow rules; apply parent backend service conventions only when changing the embedded backend integration.

## OVERVIEW
- Tauri wrapper for `rust-srec`.
- Runs backend in-process (scheduler + API server).
- Binds to `127.0.0.1:0` (ephemeral port).
- Enforces single-instance: a second launch focuses the existing window.
- Injects `__RUST_SREC_BACKEND_URL__`, `__RUST_SREC_BOOT_ERROR__`, and `__RUST_SREC_DESKTOP_NOTIFICATIONS__` into the webview.

## WHERE TO LOOK
- `src/main.rs`: Binary shim calling `rust_srec_desktop_lib::run()`; keep its `windows_subsystem` attribute.
- `src/lib.rs`: App setup, single-instance plugin, database lease, backend initialization, state management, window events.
- `src/desktop_notifications.rs`: Native OS notification integration.
- `tauri.conf.json`: Tauri configuration (capabilities, build settings, beforeDevCommand).

## CONVENTIONS
- **In-process backend**: Backend tasks run on Tauri's async runtime (`tauri::async_runtime`).
- **Port management**: Always use ephemeral port `0` for binding; read back resolved port for webview injection.
- **Persistence**: 
  - `JWT_SECRET` generated and persisted in app data dir if not in env.
  - A `RuntimeLease` keyed on the SQLite database keeps a second backend (desktop or standalone server) off the same DB.
- **First-run rewrite**: Detect docker-default `/app/output` in DB and rewrite to `<app_data_dir>/output`.

## ANTI-PATTERNS
- **Hardcoded ports**: Never hardcode the API port; it must remain ephemeral to avoid conflicts.
- **Static output paths**: Don't assume `/app/output` is valid; check for rewrite on first desktop run.
- **Environment mutation**: Prefer reading `JWT_SECRET` from file in app data dir rather than forcing system env vars.

## COMMANDS

Run Tauri commands from `rust-srec/`, with the frontend and platform prerequisites installed. Node and pnpm versions come from the root `.nvmrc` and frontend `package.json`.

- Dev: `cargo tauri dev`
  - Uses frontend dev server at `http://localhost:15275`.
- Build: `cargo tauri build`
- Check: `cargo check --locked -p rust-srec-desktop`

For a local wrapper change, start with the package check and relevant tests. Build the desktop frontend when its integration changes; use a Tauri build for changes affecting packaging, capabilities, or application startup. Exercise affected startup/persistence behavior on a supported platform. Full desktop checks are not prerequisites for unrelated backend or prose changes.

For core backend or frontend commands, see [root AGENTS.md](../../AGENTS.md) and the [frontend guide](../frontend/AGENTS.md).
