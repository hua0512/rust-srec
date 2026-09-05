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

For a larger external contribution (new platform support, pipeline changes, DB migrations) whose scope has not been discussed, propose an issue to align with maintainers. An existing issue, task description, or explicit maintainer request can already establish that scope; no additional issue or confirmation is needed. Agents should not post an issue on the contributor's behalf without authorization.

## Development setup

Prereqs:

- Rust toolchain pinned in [rust-toolchain.toml](rust-toolchain.toml); the minimum supported version is recorded separately in [Cargo.toml](Cargo.toml)
- `protoc` for Rust builds involving protobuf generation (CI version: [.github/actions/setup-protoc/action.yml](.github/actions/setup-protoc/action.yml))
- Git

For the web UI, desktop wrapper, or docs site:

- Node.js from [.nvmrc](.nvmrc)
- pnpm from the relevant `package.json` `packageManager` field

## Common commands (repo root)

Choose the scope using [Validation by task](AGENTS.md#validation-by-task). The workspace examples below cover broader integration work; replace `--workspace --exclude rust-srec-desktop` with `-p <package>` for a local package change. Prose-only changes do not require Rust builds or tests.

Format:

```bash
cargo fmt --all
```

Build:

```bash
cargo build --locked
```

Lint with default features:

```bash
cargo clippy --locked --workspace --exclude rust-srec-desktop --all-targets -- -D warnings
```

Tests:

```bash
cargo test --locked --workspace --exclude rust-srec-desktop
```

If you have `cargo-nextest` installed, use it for the test suite:

```bash
cargo nextest run --locked --workspace --exclude rust-srec-desktop
```

Nextest does not run doctests. When examples or the APIs they exercise change, run `cargo test --locked -p <package> --doc`. Exact CI features and platform prerequisites are maintained in [.github/workflows/pr.yml](.github/workflows/pr.yml); Linux CI enables selected TLS fallback features. Follow the Windows feature restriction in [AGENTS.md](AGENTS.md#toolchain-and-command-reference).

## Frontend (optional)

The web UI is in `rust-srec/frontend`. These are setup and command examples; see its [agent guide](rust-srec/frontend/AGENTS.md#validation) for when each check applies.

```bash
pnpm -C rust-srec/frontend install --frozen-lockfile
pnpm -C rust-srec/frontend fmt:check
pnpm -C rust-srec/frontend lint
pnpm -C rust-srec/frontend test
pnpm -C rust-srec/frontend build
```

## Desktop wrapper (optional)

The Tauri project is in `rust-srec/src-tauri`.

CI builds without bundling:

```bash
# Requires frontend deps installed (see Frontend section).
cargo build --locked -p rust-srec-desktop
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
- Run the checks appropriate to the changed behavior and consumers; report failures or unavailable checks with their impact

If a change affects multiple surfaces (backend + frontend + desktop), call that out clearly in the PR description.

Use the [completion, commit, and cleanup rules](AGENTS.md#completion-commits-and-cleanup) for agent-assisted work. Review the final diff and keep unrelated work out of the commit; existing local changes do not need to be removed to finish a task.

## SQLite table rebuilds

Read this procedure when a migration rebuilds an existing SQLite table. Ordinary schema changes still go in new migration files; never edit a shipped migration because SQLx verifies its checksum on existing installations.

- Preserve the old table's data, indexes, and triggers. `DROP TABLE` removes its triggers, so a create-copy-drop-rename must recreate them as well as the indexes.
- When a rebuild needs foreign keys disabled to avoid cascading into child tables, set `PRAGMA foreign_keys=OFF` outside a transaction. Use SQLx's `-- no-transaction` marker at the start of the file, then manage the rebuild in its own `BEGIN` / `COMMIT` and restore foreign-key enforcement afterward.
- Account for replay: SQLx records the migration version only after a `-- no-transaction` script returns. Make the migration safe to rerun after interruption or completion, including temporary tables and indexes; `DROP TABLE IF EXISTS <t>_new` and `CREATE INDEX IF NOT EXISTS` can help, but do not alone prove replay safety.
- Exercise the upgrade on representative fixtures, including related rows and triggers, and check retained data and behavior. Query `PRAGMA foreign_key_check` and assert that it returns no violations. A pragma inside the migration is insufficient because SQLx discards its result set.
- When correctness depends on historical data distributions or upgrade states not represented by fixtures, rehearse on a suitably redacted real database copy before shipping. If that evidence is unavailable, finish independent implementation and fixture checks, document the missing release validation, and do not claim the migration is ready to ship. Never use the live database for this rehearsal.

## Code style / conventions

- Prefer error propagation with context over `unwrap()` / `expect()` in production paths
- Use structured logging (`tracing`) and avoid logging secrets
- Follow existing module/layout conventions in the area you touch
