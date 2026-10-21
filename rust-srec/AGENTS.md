# Backend AGENTS.md

These backend-specific rules apply to `src/`, `migrations/`, `proto/`, and backend build/deployment files in this package. The frontend, docs, and desktop wrapper use their own nested rules plus the repository-wide guidance; backend service conventions do not apply to their unrelated work.

## OVERVIEW
Production-ready recorder backend (REST API + scheduler + pipeline + SQLite).

## STRUCTURE
- `src/api/`: Axum server, JWT auth, and route handlers.
- `src/scheduler/`: Streamer monitoring and job management.
- `src/pipeline/`: Post-processing DAG logic (segment, session completion).
- `src/downloader/`: Manager for `ffmpeg`, `streamlink`, and `mesio` engines.
- `src/database/`: SQLx repositories and models; migrations are in `migrations/`.
- `src/notification/`: Event-driven notifications; channel implementations live in `channels/`, browser push in `web_push/`.
- `src/credentials/`: Platform-specific credential/cookie management.

## WHERE TO LOOK
- `src/pipeline/manager.rs`: Pipeline execution engine (hotspot).
- `src/api/routes/pipeline.rs`: Pipeline control endpoints (hotspot).
- `src/services/container.rs`: Central `ServiceContainer` state.
- `src/main.rs`: Entry point and service initialization sequence.
- `migrations/`: SQL schema versions (run automatically on startup).
- `proto/`: Protobuf definitions for logging and progress events.
- `docker-compose.yml`: Local containerized environment setup.

## CONVENTIONS
- **State**: Use `Arc<ServiceContainer>` for shared ownership across services.
- **API State**: `AppState` (`src/api/server.rs`) is the shared context for Axum routes.
- **Errors**: Propagate to `crate::error::Error`; handle API-specific errors in `src/api/error.rs`.
- **Migrations**: New schema changes go in new files in `migrations/`; never edit a shipped one — sqlx checksums applied migrations and any change breaks startup on existing installs, so correct it with another new file.
- **Table rebuilds**: Preserve data, triggers, indexes, and foreign-key integrity, and account for interrupted migration replay. Read the [table rebuild procedure](../CONTRIBUTING.md#sqlite-table-rebuilds) when changing a migration that rebuilds a table; it specifies fixture checks and when a real-data rehearsal is needed. Rebuilding `streamers`, `template_config`, `platform_config` or `credential_profiles` inside SQLx's default migration transaction, where foreign keys stay on, deletes account selections (`credential_selections`, `credential_selection_members`) or fails on them; use that procedure's `-- no-transaction` / `PRAGMA foreign_keys=OFF` form.

## TESTING
Pin each behavior at the highest layer that can reach it without the public network:
1. **End-to-end, `tests/e2e/`**: the default for anything observable through the REST API or WebSockets: status codes, API error `code`s, JSON the frontend reads, persisted effects read back through the API, and auth/security outcomes. `support::TestApp` starts the real backend in-process (fresh SQLite, `ServiceContainer`, JWT-authenticated server on an ephemeral loopback port); tests talk to it only over HTTP. Write one scenario per test, asserting the precise contract.
2. **Database integration, other `tests/*.rs`**: repository and migration contracts on real SQLite.
3. **Unit tests beside the module**: only for logic the E2E suite cannot reach offline: engine and extractor behavior, external-tool argument building and output parsing, state machines, time math, concurrency, cancellation and fault injection. Keep one focused test per behavior.

Rules:
- Remove a test only when its behavior stays pinned by an E2E test or by a kept unit test. Never weaken or delete E2E tests to make a change pass.
- E2E reliability:
  - Never reach the public network: create streamers with `enabled: false`; use `LocalHttpServer` or `ClosedPort` for outbound targets; use IP literals, not DNS names.
  - Never sleep for a fixed time: use `eventually` with `WAIT`.
  - Each test starts its own `TestApp` and ends with `shutdown()`.
  - Don't mutate the process environment. The suite relies on nextest's process-per-test model.
- A confirmed bug found while writing a test becomes a `known_bug_*` test with `#[ignore = "bug: …"]` that asserts the correct behavior. CI's tool-gated job excludes `known_bug_` tests. The fix removes the ignore and renames the test.
- Commands: `cargo nextest run --locked -p rust-srec --test e2e`; one area: `-E 'test(/^auth::/)'`.

## ANTI-PATTERNS
- **Isolation**: Wire production services through `ServiceContainer`; focused tests may construct the service under test directly.
- **Blocking**: Avoid long-running synchronous work in async handlers; use `spawn_blocking`.
- **Locks**: Never hold a synchronous mutex guard across `.await`. Keep async-lock critical sections short and await while holding one only when the operation requires that serialization.

## COMMANDS
- **Run (Dev)**: `cargo run -p rust-srec --bin rust-srec`
- **Docker**: `docker compose up -d` (from `rust-srec/` directory)
- **Environment Variables**:
  - `DATABASE_URL`: SQLite connection string (default: `sqlite:srec.db?mode=rwc`).
  - `LOG_DIR`: Directory for log files (default: `logs`).
  - `API_BIND_ADDRESS`: Server host (default: `0.0.0.0`).
  - `API_PORT`: Server port (default: `12555`).
  - `API_CORS_ORIGINS`: Comma-separated exact browser origins (`scheme://host[:port]`) allowed when authentication is disabled. Default: `http://localhost:15275`, `http://127.0.0.1:15275`, `http://[::1]:15275`, `tauri://localhost`, `http://tauri.localhost`. Ignored while `JWT_SECRET` is set (any origin is allowed then).
  - `API_LOGIN_MAX_FAILURES`: Failed logins tolerated per account per window (default: `5`).
  - `API_LOGIN_IP_MAX_FAILURES`: Failed logins tolerated per source address per window (default: `100`). Loose on purpose — behind the bundled frontend or any reverse proxy, every login shares the proxy's address.
  - `API_LOGIN_WINDOW_SECS`: Length of the failed-login window in seconds (default: `900`).
