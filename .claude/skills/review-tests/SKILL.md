---
name: review-tests
description: Adversarially audit added or existing tests to remove low-signal unit tests before a PR or during suite cleanup. Use when reviewing test diffs or checking whether tests catch realistic bugs.
---

# Review tests

Audit test additions or existing test suites so the repository keeps only tests that catch realistic bugs missed by static checks and higher-layer suites.

## How to run

- **Prefer an independent reviewer**: when you wrote the code and tests in the current session, delegate the audit to a fresh read-only subagent that sees only the diff (or target test files) and this skill. Self-review in the same context carries confirmation bias.
- **Scope**:
  - **Pre-PR review**: inspect only test hunks added or modified relative to the base branch (`git diff <base>...HEAD` or the working-tree diff).
  - **Targeted audit**: inspect the requested packages or directories, checking what `rust-srec/tests/` and `crates/mesio/tests/` already cover before judging unit tests.

## Default-skeptical test bar

Treat every test under review as removable until it answers **both** questions:

1. **What realistic bug would make this test fail?** (Not "a literal changed" or "a mock wasn't called.")
2. **Why wouldn't TypeScript / Rust types, Clippy / oxlint, or an existing E2E / integration test catch it?**

Flag a test for **DELETE** (or **REWRITE** at a higher layer) when it only:
- Restates struct/schema defaults, constructors, enum variants, or derived traits (`Default`, `Debug`, `Display`, `Clone`, `PartialEq`).
- Exercises standard-library, `serde`, Zod, React, or browser API behavior without non-trivial custom logic.
- Asserts static copy, translated strings, empty-state text, CSS classes, badge colors, or icon/label mappings that break on routine UI changes.
- Verifies pure prop/argument forwarding or checks only that a mocked dependency was called with the inputs the test supplied.
- Duplicates a contract already exercised by `rust-srec/tests/e2e/`, `rust-srec/tests/integration/`, or `crates/mesio/tests/`.

Mark a test **KEEP** when it pins:
- Binary/wire protocol parsing edge cases, malformed inputs, or backwards-compatible wire/cookie formats.
- Security, auth, redaction, path containment, or session concurrency invariants.
- Non-trivial state machines, DAG/layout/time math, stream-repair operator invariants, cancellation, backpressure, or fault injection.
- Subtle UI/form bugs that pass types and lint (dirty-state tracking, `null`-reset vs inherit/override merging, IME/keyboard/focus traps, reconnect lifecycles).
- Offline-unreachable backend/engine wiring that E2E cannot exercise (such as external-tool argument building, output parsing, or engine config propagation).

## Output format

Group findings by file with exact test names and line numbers:

- **Delete**: test name, location, and one-line reason (the redundant category or the existing higher-layer test that covers it).
- **Move / Rewrite** (only when the behavior matters but sits at the wrong layer): target layer (`rust-srec/tests/e2e/`, `rust-srec/tests/integration/`, or focused unit behavior).
- **Keep (checked)**: count of retained high-signal tests so the author knows they were reviewed.

When applying deletions, remove dead helpers, fixtures, and unused imports in the same pass, then run the affected package's formatting, lint, typecheck, and test checks.
