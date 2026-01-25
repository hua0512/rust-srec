This file is for agentic coding tools operating in this repository.

Repo overview

- Rust workspace (edition 2024) with multiple crates and binaries.
- Backend app: `rust-srec/` (package `rust-srec`) - REST API + scheduler + pipeline + SQLite.
- CLIs: `strev-cli/` (package `strev`), `mesio-cli/` (package `mesio`).
- Libraries: `crates/*` (protocol/container + platform extractors + download engine).
- Frontend UI: `rust-srec/frontend/` (Vite/TanStack + Tailwind, pnpm).
- Docs site: `rust-srec/docs/` (VitePress, npm).

Build / lint / test

Rust (run from repo root; match CI by adding `--locked`)

- Build (debug): `cargo build`
- Build (release): `cargo build --release`
- Build specific package: `cargo build -p rust-srec` (or `-p strev`, `-p mesio`)
- Format: `cargo fmt --all`
- Lint (CI): `cargo clippy --locked --all-targets --all-features -- -D warnings`
  **Windows note**: do not use `--all-features` on Windows; OpenSSL is not available. Use default features instead.
- Test (Cargo): `cargo test --locked --workspace`
- Doctests only (CI runs on ubuntu): `cargo test --locked --workspace --doc`

Run / debug (Rust)

- Run backend (dev): `cargo run -p rust-srec --bin rust-srec`
- Run backend (release): `cargo run -p rust-srec --release --bin rust-srec`
- Run CLI (strev): `cargo run -p strev -- --help`
- Run CLI (mesio): `cargo run -p mesio -- --help`

Common env vars (backend)

- `DATABASE_URL` default: `sqlite:srec.db?mode=rwc` (see `rust-srec/src/main.rs`)
- `LOG_DIR` default: `logs` (see `rust-srec/src/main.rs`)
- `API_BIND_ADDRESS`, `API_PORT` (see `rust-srec/src/api/server.rs`)

Release build notes (CI parity)

- CI builds cross-target releases with `--locked` and uses `static-ssl` for musl builds.
  Example (static Linux): `cargo build -p rust-srec --locked --release --target x86_64-unknown-linux-musl --features static-ssl --bins`
  Example (static Linux strev): `cargo build -p strev --locked --release --target x86_64-unknown-linux-musl --features static-ssl`

Fast test runner (recommended; used in CI)

- Install: `cargo install cargo-nextest --locked`
- Run full suite: `cargo nextest run --locked --workspace`
- Run a single package: `cargo nextest run --locked -p rust-srec`
- Run a single test binary: `cargo nextest run --locked -p rust-srec --test <test_target_name>`
- Filter by test name substring (nextest expression):
  `cargo nextest run --locked -p rust-srec -E 'test(name ~ "some_substring")'`

Run a single test (Cargo)

- Unit test by substring: `cargo test -p rust-srec -- <substring>`
- Integration test target: `cargo test -p rust-srec --test <test_target_name>`
- Specific test in a module: `cargo test -p rust-srec some::module::tests::test_name`

Protobuf toolchain

- CI installs `protoc` (see `.github/actions/setup-protoc/action.yml`, version default 33.2).
- If you touch `prost-build` outputs / proto generation, ensure `protoc` is on PATH locally.

Frontend (run from `rust-srec/frontend/`)

- Install: `pnpm install` (CI uses `pnpm install --frozen-lockfile`)
- Dev server: `pnpm dev` (uses port 15275)
- Build: `pnpm build`
- Lint: `pnpm run lint` (oxlint)
- Format: `pnpm run format` (oxfmt)
- Test: `pnpm test` (vitest)

Frontend toolchain

- pnpm is pinned via `packageManager` in `rust-srec/frontend/package.json` (`pnpm@10.28.1`).
- CI uses Node 22 for frontend jobs (see `.github/workflows/pr.yml`).

Docs site (run from `rust-srec/docs/`)

- Install: `npm ci`
- Dev: `npm run docs:dev`
- Build: `npm run docs:build`
- Preview: `npm run docs:preview`

Docs toolchain

- CI uses Node 20 for docs deploy (see `.github/workflows/deploy-docs.yml`).

Code style / engineering conventions

Authoritative local rules

- Cursor rules: `.cursorrules` points to `.rules` (treat `.rules` as the enforced policy).
- Copilot instructions: none found (`.github/copilot-instructions.md` is absent).

Rust style

- Formatting: use rustfmt defaults (`cargo fmt --all`); no `rustfmt.toml` present.
- Imports: group as `std` -> external crates -> `crate`/`super` modules; keep imports explicit.
  Example: `rust-srec/src/credentials/service.rs`.
- Modules/files: do not introduce `mod.rs` modules; prefer `src/foo.rs` / `src/foo/` layout.
  Policy is in `.rules`.
- Comments/doc: avoid “organizational” comments; only write comments explaining non-obvious "why".
  Policy is in `.rules`.
- Collapsible ifs: always collapse nested `if` statements using `if let ... && condition` syntax.
  Example: prefer `if let Some(x) = opt && !x.is_empty() { ... }` over nested `if let` + `if`.

Naming / API shape

- Use idiomatic Rust naming: `PascalCase` for types/traits, `snake_case` for fns/modules/vars.
- Prefer small, explicit types over `Stringly-typed` APIs; when you must use strings, validate early.
- Prefer `Arc<T>` for shared state across async tasks; avoid holding locks across `.await`.

Error handling

- Avoid panics: do not add `unwrap()` / `expect()` in production code.
  Policy is in `.rules` (and the repo currently appears to avoid them).
- Never silently discard errors:
  avoid `let _ = fallible_call()?;` or `let _ = fallible_call();`.
  If ignoring errors is required, do it explicitly and with visibility (log/metrics) per `.rules`.
- App-wide errors (`rust-srec`): prefer `rust-srec/src/error.rs` (thiserror enum + `Result<T>` alias).
- Prefer adding context instead of flattening errors into `String` too early; use targeted variants.
- For path-related IO, prefer `Error::io_path(op, path, source)` (see `rust-srec/src/error.rs`).
- Library crate errors: prefer `thiserror` enums in `crates/*/src/error.rs` (e.g. `crates/tars-codec/src/error.rs`).
- Binary entrypoints: `anyhow::Result<()>` is acceptable at the outermost boundary for user-facing errors.
  Example: `rust-srec/src/main.rs`, `rust-srec/src/bin/rust-srec-vapid.rs`.

Logging / observability

- Use `tracing` for logging; prefer structured fields (e.g. `warn!(error = %e, ...)`).
  Examples: `rust-srec/src/notification/web_push.rs`, `rust-srec/src/database/maintenance.rs`.
- For async functions with important context, use `#[instrument]` and `skip(...)` to avoid logging secrets.
  Examples: `rust-srec/src/credentials/service.rs`, `crates/mesio/src/hls/fetcher.rs`.
- Keep a clear fatal vs non-fatal boundary: log-and-continue only when the system can safely proceed.
  Example: startup/shutdown notifications in `rust-srec/src/main.rs`.

Testing conventions

- Prefer deterministic tests; avoid real network calls unless explicitly testing integrations.
- Put unit tests next to the module; integration tests live under each crate's `tests/` (Cargo standard).
- When adding async tests, use Tokio's test harness (`#[tokio::test]`) and keep timeouts explicit.

Performance / runtime conventions

- Backend uses Tokio (`#[tokio::main]`) and a global mimalloc allocator.
  Example: `rust-srec/src/main.rs`.
- Avoid unnecessary allocations in hot paths; prefer borrowing / `Bytes` for byte-oriented code.
  See zero-copy patterns in `crates/tars-codec/`.

When adding new code

- Prefer extending existing modules over creating lots of tiny files (.rules).
- Follow existing abstraction boundaries: app orchestration in `rust-srec/src/main.rs` and services in `rust-srec/src/services/`.
- Add/adjust tests close to the crate you changed; use nextest to verify quickly.

CI parity checklist (before PR)

- `cargo fmt --all`
- `cargo clippy --locked --all-targets --all-features -- -D warnings`
- `cargo nextest run --locked --workspace` (or `cargo test --locked --workspace` if nextest not available)
- If frontend changed: from `rust-srec/frontend/` run `pnpm run lint` and `pnpm run build`
