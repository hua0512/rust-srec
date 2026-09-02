# Backend AGENTS.md

## OVERVIEW
Production-ready recorder backend (REST API + scheduler + pipeline + SQLite).

## STRUCTURE
- `api/`: Axum server, JWT auth, and route handlers.
- `scheduler/`: Streamer monitoring and job management.
- `pipeline/`: Post-processing DAG logic (segment, session completion).
- `downloader/`: Manager for `ffmpeg`, `streamlink`, and `mesio` engines.
- `database/`: SQLx repositories, models, and SQLite migrations.
- `notification/`: Event-driven system (Discord, Email, Webhooks, Web Push).
- `credentials/`: Platform-specific credential/cookie management.

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
- **Logging**: Use structured `tracing` macros; avoid `println!`.
- **Errors**: Propagate to `crate::error::Error`; handle API-specific errors in `src/api/error.rs`.
- **Migrations**: New schema changes must be added as new files in `migrations/`.
- **Migration rules**: Never edit a shipped file in `migrations/` — sqlx checksums applied migrations and any change breaks startup on existing installs; correct it with a new file.
- **Table rebuilds**: sqlx runs each migration inside a transaction, where `PRAGMA foreign_keys=OFF` is ignored, so a create-copy-drop-rename on a table with FK children cascades those children away and drops the table's triggers. Such a file must start with `-- no-transaction`, turn the pragma off, wrap its work in its own `BEGIN`…`COMMIT` with a `PRAGMA foreign_key_check` before the commit, re-enable the pragma, and recreate every trigger the dropped table carried.

## ANTI-PATTERNS
- **Panics**: No `unwrap()`/`expect()` in services; return `Result`.
- **Isolation**: Do not instantiate services manually outside of `ServiceContainer`.
- **Blocking**: Avoid long-running synchronous work in async handlers; use `spawn_blocking`.
- **Locks**: Never hold `MutexGuard` across an `.await` point.

## COMMANDS
- **Run (Dev)**: `cargo run -p rust-srec --bin rust-srec`
- **Docker**: `docker compose up -d` (from `rust-srec/` directory)
- **Environment Variables**:
  - `DATABASE_URL`: SQLite connection string (default: `sqlite:srec.db?mode=rwc`).
  - `LOG_DIR`: Directory for log files (default: `logs`).
  - `API_BIND_ADDRESS`: Server host (default: `0.0.0.0`).
  - `API_PORT`: Server port (default: `12555`).
