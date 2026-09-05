# Repository guidance

## Scope and reading

This file and the `AGENTS.md` files governing the area being changed define project conventions, subject to higher-priority instructions and the user's task scope. Read the applicable rules when entering an area; reuse context already read unless it has changed. Tool-specific entrypoints should refer here for shared rules.

Use the map below to locate the task. Read supporting docs, examples, and skills only when they answer a concrete question or supply a procedure needed for the work. A link is a lookup aid, not a requirement to read every referenced file before each edit.

| Area | Location / task-specific guidance |
| --- | --- |
| Backend: REST API, scheduler, pipeline, SQLite | `rust-srec/`, package `rust-srec`; [backend rules](rust-srec/AGENTS.md) |
| CLIs | `strev-cli/` (package `strev`), `mesio-cli/` (package `mesio`) |
| Protocols, containers, platform extractors, download engine | `crates/*`; use the affected crate's `Cargo.toml` for its package name |
| Web UI and desktop frontend | `rust-srec/frontend/`; [frontend rules](rust-srec/frontend/AGENTS.md) |
| Desktop wrapper | `rust-srec/src-tauri/`; [desktop rules](rust-srec/src-tauri/AGENTS.md) |
| Documentation site | `rust-srec/docs/`; [docs rules](rust-srec/docs/AGENTS.md) |
| Application release planning, preparation, or publishing | Read the [release skill](.claude/skills/release-rust-srec/SKILL.md) only for a `rust-srec` release/version task; it is the canonical procedure even if the tool does not discover `.claude/skills` automatically |

## Toolchain and command reference

- Rust edition and minimum supported version: [Cargo.toml](Cargo.toml). Development toolchain and components: [rust-toolchain.toml](rust-toolchain.toml).
- Node: [.nvmrc](.nvmrc). pnpm and scripts: [frontend package.json](rust-srec/frontend/package.json) and [docs package.json](rust-srec/docs/package.json). Install dependencies when needed with `pnpm install --frozen-lockfile` in the relevant directory.
- Rust builds involving protobuf generation need `protoc` on PATH; the CI version is in [.github/actions/setup-protoc/action.yml](.github/actions/setup-protoc/action.yml).
- Exact CI targets, platform prerequisites, and feature combinations live in [.github/workflows/pr.yml](.github/workflows/pr.yml). Release build combinations live in the corresponding `.github/workflows/release-*.yml` files. Consult these when reproducing CI or changing build/release behavior.

Run Rust commands from the repository root. Replace `<package>` and `<test_name>` with the affected package and test filter:

```sh
cargo build --locked                         # default applications, excludes desktop
cargo build --locked -p <package>
cargo fmt --all -- --check
cargo clippy --locked -p <package> --all-targets -- -D warnings
cargo nextest run --locked -p <package>
cargo nextest run --locked -p <package> -E 'test(<test_name>)'
cargo test --locked -p <package> <test_name>  # fallback if nextest is unavailable
cargo test --locked -p <package> --doc       # when doctests are affected
```

Use `cargo fmt --all` to apply formatting, and review the diff for unrelated changes. Add `--release` when release-mode behavior or a release artifact is relevant. Use default features on Windows; do not use `--all-features` there with the current OpenSSL prerequisites. On Linux, reproduce the specific CI feature selection when it is relevant rather than assuming `--all-features` is equivalent.

For broader integration checks, use `--workspace --exclude rust-srec-desktop` in place of `-p <package>`. Desktop validation is separate because it needs frontend and platform dependencies. Full workspace builds including desktop use `cargo build --locked --workspace` when those prerequisites and that scope apply.

Run/debug examples:

```sh
cargo run --locked -p rust-srec --bin rust-srec
cargo run --locked -p strev -- --help
cargo run --locked -p mesio -- --help
```

Frontend and docs commands are in their respective `AGENTS.md` files. [CONTRIBUTING.md](CONTRIBUTING.md) contains contributor setup and broader local command examples.

## Validation by task

Choose checks from the changed behavior and its consumers. These are local validation requirements, not a requirement to reproduce every CI job for every edit.

| Change | Local validation |
| --- | --- |
| Agent rules, prose, spelling | Review the diff; verify affected paths, commands, links, and factual claims. No Rust/frontend build or new tests solely for prose changes. |
| Docs navigation, configuration, or complex Markdown | Build the docs site and check affected links; preview pages when layout or rendering needs inspection. |
| Local Rust behavior | Formatting, affected-package Clippy, and focused tests covering the behavior. Add or adjust regression tests where they protect an observable contract. |
| Frontend behavior | Formatting check, lint, and relevant tests. Add type checking for type/interface changes, web build for routing/bundling/SSR changes, and desktop build for affected CSR integration. |
| Shared interfaces, dependencies, features, cross-component changes | Expand checks to affected consumers and build/platform combinations; use the wider workspace suite when the impact crosses package boundaries. |
| SQLite migrations | Migration upgrade/integrity checks; for table rebuilds use the conditional procedure in [CONTRIBUTING.md](CONTRIBUTING.md#sqlite-table-rebuilds). |
| Release preparation | Version/lockfile consistency, release-note provenance and locale parity, links, and docs build as described in the release skill. |

Prefer deterministic tests without real network calls unless testing an integration explicitly. Put unit tests beside the module and integration tests under the crate's `tests/`. Use Tokio's test harness for async tests; bound waits that could otherwise hang. Nextest does not run doctests, so run Cargo doctests when documentation examples or the APIs they exercise change.

After relevant checks pass, repeat or broaden them only for new changes, failures, or unresolved risks. If a tool or platform is unavailable, complete independent work and applicable alternative checks, then report the exact unverified portion and its impact. Do not describe blocked validation as passed or a release as ready when required evidence is missing. CI remains the integration gate; see its workflow for the full matrix.

## Rust engineering conventions

- Use rustfmt defaults and idiomatic names. Group explicit imports as `std`, external crates, then `crate`/`super`.
- Prefer extending an existing module unless the change introduces a separate logical component. Use `src/foo.rs` / `src/foo/`; do not introduce `mod.rs` files.
- Comments explain invariants, caller contracts, ordering constraints, and non-obvious reasons. Avoid restating code or narrating a chat/PR history. A relevant upstream issue may support a workaround, but explain the constraint locally so the comment stands on its own.
- Collapse nested `if` statements with let chains when equivalent and clearer, following Clippy. Keep stylistic cleanup within the task's changed area.
- Prefer explicit types over string-based APIs; validate strings at boundaries. Prefer `Arc<T>` for shared ownership and release locks before `.await` where possible; never hold a synchronous mutex guard across an await.
- Do not introduce `unwrap()` / `expect()` in production paths. Propagate errors with context and use targeted `thiserror` variants before flattening errors to strings. `anyhow::Result<()>` is acceptable at binary entrypoints.
- Do not silently discard an unhandled `Result`. Use `?` for propagation or an explicit recovery path with appropriate logging/metrics. `let _ = call()?;` propagates errors and discards the success value; use `call()?;` when no binding is needed. Propagation alone does not require a duplicate log.
- Backend errors use `rust-srec/src/error.rs`; path-related IO should use `Error::io_path(op, path, source)`. Library errors belong to the library's own error type.
- Use structured `tracing` fields. Add `#[instrument]` where context is useful, with `skip(...)` for secrets or unsuitable fields. Log-and-continue only when proceeding is safe; redact tokens, cookies, stream keys, and private data.
- Preserve runtime and architecture boundaries: backend orchestration in `rust-srec/src/main.rs`, services in `rust-srec/src/services/`, Tokio tasks, and the existing allocator setup. Avoid blocking async handlers and unnecessary allocations in hot paths; prefer borrowing or `Bytes` for byte-oriented data.

## Completion, commits, and cleanup

A task is complete when the requested scope is implemented, necessary related docs/generated outputs are updated, relevant validation is accounted for, and the final diff has been reviewed. Report what changed, the checks and their results, and any remaining limitation. A review-only task ends with findings and recommendations, following any requested approval boundary.

Continue authorized work through implementation and validation. Existing user instructions or an agreed issue establish scope; do not reopen that decision for routine implementation choices. If essential information is missing, ask once with the concrete dependency and continue work that does not depend on the answer. Explain an actual blocker rather than stopping at a plan or treating optional checks as mandatory approval gates.

Commit, open a PR, push, or tag according to the task's existing authorization; a request for a PR includes the needed task branch, commit, and branch push. Release tags/publication follow the release skill's separate scope. Review the staged diff and include only the task's files/hunks. An unrequested commit or a completely clean worktree is not a completion requirement.

Remove only clearly disposable temporary artifacts created by this task. Preserve pre-existing changes, untracked work, local data, and useful caches. Avoid repository-wide formatting, unrelated fixes, destructive cleanup, or deleting files merely to make `git status` clean.
