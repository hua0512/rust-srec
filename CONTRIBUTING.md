# Contributing

Thanks for taking the time to contribute!

This repository is a Rust workspace containing the recorder backend, two CLIs, and multiple library crates.
The web UI and desktop wrapper live under `rust-srec/frontend` and `rust-srec/src-tauri`.

## Ways to contribute

- Report bugs and regressions (include repro steps and logs)
- Propose features / UX improvements
- Improve docs (typos, missing setup steps, clarifications)
- Submit pull requests (bug fixes, improvements, new features)

## Before you start

Please avoid sharing secrets. Redact tokens, cookies, stream keys, private URLs, and any personal data from logs/config.

If you are planning a larger change (new platform support, pipeline changes, DB migrations), open an issue first so we can align on scope.

## Development setup

Prereqs:

- Rust stable toolchain
- `protoc` (Protocol Buffers compiler)
- Git

Optional (only if working on the web UI / desktop wrapper):

- Node.js (CI uses Node 22)
- pnpm (CI uses pnpm 10)

## Common commands (repo root)

Format:

```bash
cargo fmt --all
```

Build:

```bash
cargo build
```

Lint (CI-style):

```bash
cargo clippy --locked --workspace --exclude rust-srec-desktop --all-targets -- -D warnings
```

Tests:

```bash
cargo test --locked --workspace --exclude rust-srec-desktop
```

If you have `cargo-nextest` installed, you can run the faster suite used in CI:

```bash
cargo nextest run --locked --workspace --exclude rust-srec-desktop
```

## Frontend (optional)

The web UI is in `rust-srec/frontend`.

```bash
pnpm -C rust-srec/frontend install --frozen-lockfile
pnpm -C rust-srec/frontend lint
pnpm -C rust-srec/frontend build
```

## Desktop wrapper (optional)

The Tauri project is in `rust-srec/src-tauri`.

CI builds without bundling:

```bash
# Requires frontend deps installed (see Frontend section).
cargo build -p rust-srec-desktop
```

## Reporting bugs

When filing an issue, please include:

- What you expected to happen vs what happened
- Minimal reproduction steps (ideally with a public test URL)
- Version information (git SHA or release version)
- Your OS, CPU architecture, and Rust version (`rustc -V`)
- Relevant logs (redacted)

If this involves the web UI or desktop app, also include:

- Node/pnpm versions
- Browser version (web) or desktop OS details (Tauri)

## Pull requests

Keep PRs focused and easy to review.

- One logical change per PR when possible
- Include tests for behavior changes where practical
- Update docs when changing flags/config/API behavior
- Run format + clippy + tests locally if you can

If a change affects multiple surfaces (backend + frontend + desktop), call that out clearly in the PR description.

## Code style / conventions

- Prefer error propagation with context over `unwrap()` / `expect()` in production paths
- Use structured logging (`tracing`) and avoid logging secrets
- Follow existing module/layout conventions in the area you touch
