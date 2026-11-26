# Suite pruning

Use this procedure to cut a crate's or package's test suite while keeping its coverage. The goal is faster compile and test runs; test count is only a proxy, so measure time before and after. Deleting tests without checking what still pins their behavior has lost real coverage before: one sampled audit found about a fifth of a pass's deletions had no remaining test. With this procedure, a later pass lost 1 of 45, plus an assertion a merge had dropped, and the audit caught both before the PR.

Work one crate or package per PR.

## 1. Measure

- **Test time**: `cargo nextest run --locked -p <package> -j 4 --status-level pass` prints each test's duration (`-j 4` matches CI's runners). Sum the durations per binary and list the slowest tests. For the backend, include the `rust-srec` E2E binary: it dominates test time, so a change that adds E2E tests can make CI slower while the count drops.
- **Compile time**: change a source file in the lowest crate the package depends on, then run `cargo nextest run --locked -p <package> --no-run --timings`, and read the per-unit durations in the HTML report under `target/cargo-timings/`. Each crate's unit tests build as a separate `lib (test)` unit, so the unit tests' cost shows up there.
- Record the CI test step's duration on the base commit (Linux, macOS and Windows).

## 2. Inventory

Have a fresh read-only agent apply this skill's bar to every test in scope and write an inventory: one row per test with its verdict (DELETE, MERGE, EXTEND, REPLACE, KEEP), the `covered_by` test with its asserting lines or the reason, and the evidence. Split a large crate between auditors by module so each reads its whole area. Give them the list of tests with their durations, and point them at the E2E and integration suites.

The user reviews the inventory before anything is deleted. Close calls (partial coverage, a new E2E harness variant, moving a test to another crate) are theirs.

## 3. Apply

- Re-check each `covered_by` claim before deleting; keep the test when a claim turns out wrong.
- For a MERGE, diff the merged test's assertions against every test it replaces; a dropped assertion is lost coverage.
- Write a deletion log as `file | test | reason | covered_by`, one row per removed or merged test.
- Every new E2E scenario and every assertion added to an existing E2E test must fail against a temporary mutation of the production code that breaks the behavior it pins. Record each mutation and its result, and never commit one.
- Apply the "slow for its signal" fixes in separate commits.
- Remove dead helpers in the same pass, then run the package's formatting, Clippy and tests, and the full E2E binary when its support code changed.

## 4. Audit the deletions

Before the PR, an independent agent (not the implementer) samples at least 20 rows of the deletion log, weighted towards security, concurrency, persistence and recovery. For each it reads the removed test from the base branch, lists every behavior it asserted, and checks the covering test asserts each one, with a mutation where unsure. It also reviews the added test code for the E2E reliability rules in `rust-srec/AGENTS.md`, including the macOS and Windows traps. Restore any lost coverage before opening the PR.

## 5. Report

The PR body gives the test count, the package's test-seconds and unit-test compile time, and the CI test step's duration before and after, plus the deletion log or a link to it.
