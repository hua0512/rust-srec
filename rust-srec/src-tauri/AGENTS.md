# rust-srec-desktop AGENTS.md

## OVERVIEW
- Tauri wrapper for `rust-srec`.
- Runs backend in-process (scheduler + API server).
- Binds to `127.0.0.1:0` (ephemeral port).
- Enforces single-instance with window focusing and `rust-srec://single-instance` events.
- Injects `__RUST_SREC_BACKEND_URL__`, `__RUST_SREC_LAUNCH_ARGS__`, and `__RUST_SREC_LAUNCH_CWD__` into webview.

## WHERE TO LOOK
- `src/main.rs`: Entry point, single-instance setup, lock file management.
- `src/lib.rs`: App setup, backend initialization, state management, window events.
- `src/desktop_notifications.rs`: Native OS notification integration.
- `tauri.conf.json`: Tauri configuration (capabilities, build settings, beforeDevCommand).

## CONVENTIONS
- **In-process backend**: Backend runs in a dedicated thread managed by Tauri.
- **Port management**: Always use ephemeral port `0` for binding; read back resolved port for webview injection.
- **Persistence**: 
  - `JWT_SECRET` generated and persisted in app data dir if not in env.
  - Exclusive lock file prevents concurrent access to same SQLite DB.
- **First-run rewrite**: Detect docker-default `/app/output` in DB and rewrite to `<app_data_dir>/output`.

## ANTI-PATTERNS
- **Hardcoded ports**: Never hardcode the API port; it must remain ephemeral to avoid conflicts.
- **Static output paths**: Don't assume `/app/output` is valid; check for rewrite on first desktop run.
- **Environment mutation**: Prefer reading `JWT_SECRET` from file in app data dir rather than forcing system env vars.

## COMMANDS
- Dev: `cargo tauri dev`
  - Uses frontend dev server at `http://localhost:15275`.
- Build: `cargo tauri build`
- Check: `cargo check -p rust-srec-desktop`

*Note: For core backend or frontend commands, see root `AGENTS.md` and `rust-srec/frontend/README.md` respectively.*
