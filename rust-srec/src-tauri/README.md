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

## Windows notifications

Before submitting a native notification, the app registers its configured bundle identifier
under `HKCU\Software\Classes\AppUserModelId`, with a display name and an icon stored in
the app data directory. This supplies the notification identity for portable ZIP builds
without requiring an installer or administrator access. Existing activation settings and
Windows notification preferences are preserved. The registration and icon remain after exit.

The desktop notification **Test** button reports registration and submission errors.
Successful submission does not guarantee a banner: Windows notification settings and
Do Not Disturb can suppress it.

To validate on Windows, use a user profile where rust-srec has never been installed:

1. Extract the portable ZIP and launch its executable.
2. Open **Notifications → Desktop Notifications → Test** and verify a native rust-srec popup.
3. Trigger a selected event (for example, a stream going online) and verify its popup.
4. Exit, move the portable folder, relaunch, and repeat the test.
5. Disable rust-srec notifications in Windows settings and verify that testing does not
   re-enable them. Also smoke-test an installed build to check coexistence.
