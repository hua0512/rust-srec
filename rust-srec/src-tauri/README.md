# rust-srec Desktop (Tauri)

This folder contains the Tauri desktop wrapper for `rust-srec`.

## What it does

- Starts the `rust-srec` backend in-process (scheduler + API server).
- Binds the API server to `127.0.0.1:0` (ephemeral port).
- Uses a single-instance mechanism so attempting to start the app twice focuses the existing window.
- Acquires a runtime lease keyed on the SQLite database as a safety net, so neither a second
  desktop instance nor a standalone `rust-srec` server can start against the same database.
- Injects the resolved backend base URL into the webview as
  `globalThis.__RUST_SREC_BACKEND_URL__ = "http://127.0.0.1:<port>"`, along with
  `__RUST_SREC_BOOT_ERROR__` and `__RUST_SREC_DESKTOP_NOTIFICATIONS__`.
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

`beforeDevCommand` starts the desktop frontend dev server (`pnpm -C frontend dev:desktop`,
run from `rust-srec/`), and the webview loads it from `http://127.0.0.1:15275`.

## Build

From `rust-srec/src-tauri`:

```bash
cargo tauri build
```

`beforeBuildCommand` builds the desktop frontend (`pnpm -C frontend build:desktop`) into
`rust-srec/frontend/dist-desktop`, which the app embeds.

## Data locations

- SQLite DB is stored in the Tauri app data directory.
- Logs are stored in the Tauri app log directory.
- Recordings default to `<app_data_dir>/output` (only if the DB was still at `/app/output`).

## Windows notifications

Before submitting a native notification, the app registers its configured bundle identifier
under `HKCU\Software\Classes\AppUserModelId`, with the display name **Rust-Srec** and an icon
stored in the app data directory. This supplies the notification identity for portable ZIP builds
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
