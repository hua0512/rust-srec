# Backend AGENTS.md

These backend-specific rules apply to `src/`, `crates/srec-*`, `migrations/`, `proto/`, and backend build/deployment files in this package. The frontend, docs, and desktop wrapper use their own nested rules plus the repository-wide guidance; backend service conventions do not apply to their unrelated work.

## OVERVIEW
Production-ready recorder backend (REST API + MCP + actor-based recorder control plane + post-processing pipeline + SQLite), split into layered internal workspace crates under `crates/srec-*` and composed by the thin root crate `rust-srec`.

## STRUCTURE
- `crates/srec-core/` (Layer 0): Storage-free domain value objects, shared ports, proxy value types, engine configs, processor contracts, and utilities.
- `crates/srec-sqlite/` (Layer 0): SQLite connection pools, `CommittedWriter`, retry/time helpers, base models/repositories, and schema migration runner (`migrations/` stays at `rust-srec/migrations/`).
- `crates/srec-engines/` (Layer 1): Download engines (`ffmpeg`, `streamlink`, `mesio`).
- `crates/srec-processors/` (Layer 1): Post-processing operators and BaiduPCS-Go runner (no SQLite dependency).
- `crates/srec-notify/` (Layer 1): Event-driven notification channels, Web Push (VAPID), backend `locales/`, and durable outbox drainer.
- `crates/srec-pipeline/` (Layer 2): Post-processing DAG scheduler, job queue, worker pools, throttling, and pipeline repositories.
- `crates/srec-recorder/` (Layer 2): Recording control plane (FSM, per-streamer actors, prober, batch precheck, Bigo notifications), download orchestration, danmu service and runner, credentials, proxies, config service, and streamer state.
- `crates/srec-api/` (Layer 3): Axum REST/WebSocket server, Protobuf generated types (`proto/` stays at `rust-srec/proto/`), OpenAPI/Swagger, MCP server (`mcp/`), auth store/service, CQRS read queries, playback contexts, dynamic logging, and health metrics.
- `src/` (Layer 4, thin root `rust-srec`): `ServiceContainer` wiring, `ConfigBackupPort` export/import, maintenance scheduler, legacy startup upgrades, `StoredLoginAuthenticator`, process supervision (`runtime/`), desktop/server facade (`backend.rs`, `server.rs`), and binaries (`main.rs`, `bin/rust-srec-vapid.rs`).

Each crate depends only on crates in lower layers, except `srec-sqlite`, which depends on `srec-core` in the same layer. `tests/integration/domain_boundaries.rs` enforces the exact allowed dependencies.

## WHERE TO LOOK
- `crates/srec-pipeline/src/manager.rs`: Pipeline execution engine (hotspot).
- `crates/srec-api/src/api/routes/pipeline.rs`: Pipeline control endpoints (hotspot).
- `crates/srec-recorder/src/recorder/`: Streamer FSM, actor runtime, prober, registry, and store.
- `src/services/container.rs`: Central `ServiceContainer` wiring.
- `src/backend.rs`: Public backend integration facade for the server binary and `src-tauri`.
- `src/main.rs`: Entry point and service initialization sequence.
- `migrations/`: SQL schema versions (run automatically on startup).
- `proto/`: Protobuf definitions for logging and progress events (regenerated via `-p srec-api`).
- `docker-compose.yml`: Local containerized environment setup.

## CONVENTIONS
- **State**: Use `Arc<ServiceContainer>` for shared ownership across services.
- **API State**: `AppState` (`crates/srec-api/src/api/server.rs`) is the shared context for Axum routes.
- **Errors**: Each crate's error type holds the failures that originate in that crate, `Core(CoreError)` for the shared kinds (not found, validation, conflict, ...) and one wrapper per lower crate error it propagates, such as `Db(DbError)`. Add a variant to the crate that raises the failure. The `From` impls move a wrapped `CoreError` up to `Core` and a wrapped `DbError` up to `Db`, so match a shared kind as `Error::Core(CoreError::NotFound { .. })` at any layer. Root `rust-srec`'s `crate::error::Error` is `srec_api::ServerError`. Map errors to HTTP statuses and codes in `crates/srec-api/src/api/error.rs`.
- **Migrations**: New schema changes go in new files in `migrations/`; never edit a shipped one — sqlx checksums applied migrations and any change breaks startup on existing installs, so correct it with another new file.
- **Table rebuilds**: Follow the [table rebuild procedure](../CONTRIBUTING.md#sqlite-table-rebuilds) for any migration that rebuilds a table. Rebuilding `streamers`, `template_config`, `platform_config` or `credential_profiles` always needs its `-- no-transaction` / `PRAGMA foreign_keys=OFF` form; inside SQLx's default transaction the rebuild deletes or fails on the account selections that reference those tables.

- **Pipeline step inputs**: the DAG gives each step the merged *outputs* of the steps it `depends_on` (`ready_steps` in `crates/srec-pipeline/src/repositories/dag.rs`). Transform steps output only their new file (remux returns no passthrough), rclone copy/sync pass their inputs through, and rclone move outputs nothing. So a `delete` step after a transform deletes the converted result, not the recording. To delete the source after converting, use the transform's own flag (`remove_input_on_success`, `delete_source_xml_on_success`, `delete_source_videos_on_success`); the workflow editor warns about the other pattern (`components/pipeline/workflows/delete-warning.ts`).

## TESTING
Pin each behavior at the highest layer that can reach it without the public network:
1. **End-to-end, `tests/e2e/`**: the default for anything observable through the REST API or WebSockets: status codes, API error `code`s, JSON the frontend reads, persisted effects read back through the API, and auth/security outcomes. `support::TestApp` starts the real backend in-process (fresh SQLite, `ServiceContainer`, JWT-authenticated server on an ephemeral loopback port); tests talk to it only over HTTP. Write one scenario per test, asserting the precise contract.
2. **Database integration, `tests/integration/`**: repository and migration contracts on real SQLite, one module per suite in a single test binary.
3. **Unit tests beside the module**: only for logic the E2E suite cannot reach offline: engine and extractor behavior, external-tool argument building and output parsing, state machines, time math, concurrency, cancellation and fault injection. Keep one focused test per behavior.

Before adding a test:
- Extend an existing test when it already sets up the scenario: a table row or a few assertions cost milliseconds, while a new E2E test starts its own app (about 0.3 s) and a new unit test adds to its crate's test-build compile time.
- Pin a route's status codes, error `code`s and JSON in E2E, not in a unit test that calls the handler or router in-process. Unit tests stay for what E2E cannot reach: modes and settings `TestApp` does not start (desktop mode, environment parsing), fault injection, races, cancellation and time math.
- Check that the test fails when the behavior it pins is broken: run it against a temporary mutation of the production code, or against the code before the fix.
- Don't wait on real timers set in seconds. Configure the delay in milliseconds, or give the component a test-only setting, so the test finishes in well under a second.
- Reuse the existing fixtures before writing a new one: `TestApp` and its `Seed`s for E2E, and the migration template in `srec-sqlite` (`test_migration_template`) for unit tests that need a migrated database.

Rules:
- Remove a test only when its behavior stays pinned by an E2E test or by a kept unit test, and the covering test asserts the same behavior, not just the same flow or endpoint. Never weaken or delete E2E tests to make a change pass.
- E2E reliability:
  - Never reach the public network: create streamers with `enabled: false`; use `LocalHttpServer` or `ClosedPort` for outbound targets; use IP literals, not DNS names.
  - Monitoring and recording scenarios enable streamers only in an app started with `TestApp::start_with(Seed::live())`, which routes every platform but the offline `support::FakePlatform` to a proxy that refuses everything. They use `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` and shut the app down before the fake platform drops.
  - Never sleep for a fixed time: use `eventually` with `WAIT`.
  - Each test starts its own `TestApp` and ends with `shutdown()`.
  - Don't mutate the process environment. The suite relies on nextest's process-per-test model. A scenario that relies on an environment variable's default adds it to `DEFAULT_ONLY_ENV` in `tests/e2e/support.rs`, so an exported value fails the run with a clear message instead of a confusing assertion.
- A confirmed bug found while writing a test becomes a `known_bug_*` test with `#[ignore = "bug: …"]` that asserts the correct behavior. CI's tool-gated tests exclude `known_bug_` tests. The fix removes the ignore and renames the test.
- Commands: `cargo nextest run --locked -p rust-srec --test e2e`; one area: `-E 'test(/^auth::/)'`.

## ANTI-PATTERNS
- **Isolation**: Wire production services through `ServiceContainer`; focused tests may construct the service under test directly.
- **Blocking**: Avoid long-running synchronous work in async handlers; use `spawn_blocking`.
- **Locks**: Never hold a synchronous mutex guard across `.await`. Keep async-lock critical sections short and await while holding one only when the operation requires that serialization.

## COMMANDS
- **Run (Dev)**: `cargo run --locked -p rust-srec --bin rust-srec` (set `JWT_SECRET`, or `AUTH_DISABLED=true` with a loopback `API_BIND_ADDRESS`)
- **Docker**: `docker compose up -d` (from `rust-srec/` directory)
- **Environment Variables**:
  - `JWT_SECRET`: Required to start the API unless `AUTH_DISABLED=true`.
  - `AUTH_DISABLED`: `true`/`1` runs the API without authentication; allowed only when `API_BIND_ADDRESS` is a loopback IP (local development).
  - `DATABASE_URL`: SQLite connection string (default: `sqlite:srec.db?mode=rwc`).
  - `LOG_DIR`: Directory for log files (default: `logs`).
  - `API_BIND_ADDRESS`: Server host (default: `0.0.0.0`).
  - `API_PORT`: Server port (default: `12555`).
  - `API_CORS_ORIGINS`: Comma-separated exact browser origins (`scheme://host[:port]`) allowed when authentication is disabled (`AUTH_DISABLED=true`). Default: `http://localhost:15275`, `http://127.0.0.1:15275`, `http://[::1]:15275`, `tauri://localhost`, `http://tauri.localhost`. Ignored while `JWT_SECRET` is set (any origin is allowed then).
  - `API_LOGIN_MAX_FAILURES`: Failed logins tolerated per account per window (default: `5`).
  - `API_LOGIN_IP_MAX_FAILURES`: Failed logins tolerated per source address per window (default: `100`). Loose on purpose — behind the bundled frontend or any reverse proxy, every login shares the proxy's address.
  - `API_LOGIN_WINDOW_SECS`: Length of the failed-login window in seconds (default: `900`).
