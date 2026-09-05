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
- `src/notification/`: Event-driven system (Discord, Email, Webhooks, Web Push).
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
- **Table rebuilds**: Preserve data, triggers, indexes, and foreign-key integrity, and account for interrupted migration replay. Read the [table rebuild procedure](../CONTRIBUTING.md#sqlite-table-rebuilds) when changing a migration that rebuilds a table; it specifies fixture checks and when a real-data rehearsal is needed.

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
