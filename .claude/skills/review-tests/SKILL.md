---
name: review-tests
description: Adversarially audit added or existing tests to remove low-signal unit tests before a PR or during suite cleanup. Use when reviewing test diffs, checking whether tests catch realistic bugs, or pruning a crate's test suite.
---

# Review tests

Audit test additions or existing test suites so the repository keeps only tests that catch realistic bugs missed by static checks and higher-layer suites.

## How to run

- **Prefer an independent reviewer**: when you wrote the code and tests in the current session, delegate the audit to a fresh read-only subagent that sees only the diff (or target test files) and this skill. Self-review in the same context carries confirmation bias. An implementer that adds tests does not count as the reviewer of its own diff.
- **Scope**:
  - **Pre-PR review**: inspect only test hunks added or modified relative to the base branch (`git diff <base>...HEAD` or the working-tree diff).
  - **Targeted audit**: inspect the requested packages or directories, checking what `rust-srec/tests/` and `crates/mesio/tests/` already cover before judging unit tests.
  - **Suite pruning** (cutting a crate's test count while keeping coverage): follow [the pruning procedure](references/suite-pruning.md), which wraps the audit below in measurement, user review and a sampled check of the deletions.

## Default-skeptical test bar

Treat every test under review as removable until it answers **both** questions:

1. **What realistic bug would make this test fail?** (Not "a literal changed" or "a mock wasn't called.")
2. **Why wouldn't TypeScript / Rust types, Clippy / oxlint, or an existing E2E / integration test catch it?**

Flag a test for **DELETE** (or a move to a higher layer) when it only:
- Restates struct/schema defaults, constructors, enum variants, or derived traits (`Default`, `Debug`, `Display`, `Clone`, `PartialEq`).
- Exercises standard-library, `serde`, Zod, React, axum/tower, or browser API behavior without non-trivial custom logic.
- Asserts static copy, translated strings, empty-state text, CSS classes, badge colors, or icon/label mappings that break on routine UI changes.
- Verifies pure prop/argument forwarding or checks only that a mocked dependency was called with the inputs the test supplied.
- Restates a validation or mapping table row for row (one row per entry of a macro or `match`), when one representative row already pins the mechanism.
- Duplicates a contract already exercised by `rust-srec/tests/e2e/`, `rust-srec/tests/integration/`, or `crates/mesio/tests/`.

**Duplicate means the same assertion, not the same flow.** A `covered_by` claim must cite the lines in the covering test that check the same behavior, and that test must fail if the behavior broke (prove it with a temporary mutation when unsure). When the covering test checks only part of what the unit test asserts, name the part that would be lost and choose a move or KEEP instead of DELETE. Watch for fixtures that only ever use the current defaults: an E2E login with a hash made by today's parameters does not pin that hashes with other parameters (older installs, a library upgrade) still verify, and the same goes for old stored formats and schema versions.

Mark a test **KEEP** when it pins:
- Binary/wire protocol parsing edge cases, malformed inputs, or backwards-compatible wire/cookie formats.
- Security, auth, redaction, path containment, or session concurrency invariants.
- Non-trivial state machines, DAG/layout/time math, stream-repair operator invariants, cancellation, backpressure, or fault injection.
- Subtle UI/form bugs that pass types and lint (dirty-state tracking, `null`-reset vs inherit/override merging, IME/keyboard/focus traps, reconnect lifecycles).
- Offline-unreachable backend/engine wiring that E2E cannot exercise (such as external-tool argument building, output parsing, or engine config propagation), and modes or settings the E2E harness does not start.

Also flag a test that is **slow for its signal**: a fixed sleep, a wait on a real timer set in seconds, thousands of iterations guarding a property the code cannot violate, or per-test setup (such as running every migration) that a shared fixture could do once. Propose the fix (a millisecond setting, polling, a smaller input, a shared fixture) alongside the verdict.

## Verdicts

- **DELETE**: covered elsewhere (cite the asserting lines) or low-signal (name the category, and the realistic bug it would miss, which should be none).
- **MERGE**: several unit tests check one behavior with different inputs; fold them into one table test that keeps every assertion of the tests it replaces (diff their asserts against the base branch).
- **EXTEND**: the behavior is observable over HTTP or WebSockets but not pinned there; add a row or a few assertions to an existing E2E test that already sets up the scenario, then delete the unit test. Prefer this to a new test.
- **REPLACE**: no existing E2E test fits; propose one new E2E scenario and list every unit test it replaces. A one-to-one replacement adds runtime (each E2E test starts its own app), so say whether it is still worth it.
- **KEEP**: give the reason from the list above.

## Output format

Group findings by file with exact test names and line numbers:

- **Delete / Merge**: test name, location, and one-line reason (the redundant category or the existing higher-layer test and its asserting lines).
- **Extend / Replace**: the target test or new scenario and what it must assert.
- **Slow for its signal**: test, duration if known, and the proposed fix.
- **Keep (checked)**: count of retained high-signal tests so the author knows they were reviewed.

When applying deletions, remove dead helpers, fixtures, mocks, and unused imports in the same pass. A crate with a lib target does not warn on unused `pub` items, so grep for test-only helpers whose last caller went. Then run the affected package's formatting, lint, typecheck, and test checks.
