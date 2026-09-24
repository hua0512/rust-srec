# rust-srec Desktop (Tauri)

This folder contains the Tauri desktop wrapper for `rust-srec`.

## What it does

- Starts the `rust-srec` backend in-process (scheduler + API server).
- Binds the API server to `127.0.0.1:0` (ephemeral port).
- Uses a single-instance mechanism so attempting to start the app twice focuses the existing window.
- Acquires a runtime lease keyed on the SQLite database as a safety net, so neither a second
  desktop instance nor a standalone `rust-srec` server can start against the same database.
- Injects the resolved backend base URL into the webview as:
  - `globalThis.__RUST_SREC_BACKEND_URL__ = "http://127.0.0.1:<port>"`
- On first run, if the database output folder is still the docker default (`/app/output`), it is
  updated to a desktop-safe directory under the app data directory.
- Ensures auth is consistently enabled by generating (and persisting) a per-install `JWT_SECRET`
  under the app data directory when not already set in the process environment.

Dev override:
- If `JWT_SECRET` is already set in the process environment, the desktop wrapper will use it and
  will not generate/read the persisted secret.

## Dev

From `rust-srec/src-tauri`:

```bash
cargo tauri dev
```

This uses the frontend dev server at `http://localhost:15275`.

Notes:
- `cargo tauri dev` runs the frontend dev server via `beforeDevCommand`.
- The command locates the frontend directory relative to the repo root (supports both
  `rust-srec/frontend` and `frontend` layouts) using `rust-srec/scripts/run-frontend.cjs`.

## Build

From `rust-srec/src-tauri`:

```bash
cargo tauri build
```

## Data locations

- SQLite DB is stored in the Tauri app data directory.
- Logs are stored in the Tauri app log directory.
- Recordings default to `<app_data_dir>/output` (only if the DB was still at `/app/output`).
