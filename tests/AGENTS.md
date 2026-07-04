# AGENTS.md

Scope: `tests/` — trybuild's self-hosted integration suites and their `tests/ui/` fixtures. These spawn cargo-inside-cargo, so they are the slow part of `cargo test`; the fast normalizer-only loop is `cargo test --lib` (see `../src/AGENTS.md`). Root `AGENTS.md` owns the full command matrix and the snapshot-update workflow.

## The two suites

- `test.rs` (`cargo test --test test`) — runs trybuild against `tests/ui/*.rs` via `TestCases::run`. The fixtures deliberately pair files with mismatched expectations (e.g. a passing file registered as `compile_fail`), so a successful run of this harness is one where `run` reports an error — the test asserts `is_err()`, not a panic guard. The default test re-execs one ignored child through `strict_test_support::capture_ignored_test` so `run`'s terminal report stays captured instead of leaking into the parent `cargo test` output.
- `try_run.rs` (`cargo test --test try_run`) — exercises the typed, terminal-free `TestCases::try_run` core: it asserts each fixture's outcome as data (both polarities of pass and compile-fail) and proves terminal silence, again via `capture_ignored_test`, using the `strict_test_support::ensure*` helpers.

## Fixtures (`tests/ui/`)

Fixture names encode their role: `compile-fail-*.rs`, `run-pass-*.rs`, `run-fail.rs`, `print-stdout.rs`/`print-stderr.rs`/`print-both.rs`, and `try-run-mismatch.rs`. Expected diagnostics live adjacent as `<name>.stderr` snapshots — but not every fixture carries one, because the registration list in `test.rs` is deliberately adversarial, covering one failure mode each: `run-pass-3.rs` (compiles fine) is registered as `compile_fail` (unexpected success), `compile-fail-0.rs` (a `compile_error!`) is registered as `pass` (unexpected compile failure), `compile-fail-1.rs` is registered as `compile_fail` with no committed snapshot (the missing-snapshot path), and `run-fail.rs` is registered as `pass` but panics at runtime (the runtime-failure path).

## Running and updating

- One ui case: `cargo test -- test trybuild=<file.rs>`. Both tokens are required — the bare `test` is cargo's substring filter selecting the integration `#[test] fn test`, and trybuild itself reads the `trybuild=` argument (`filter()` in `src/internal/runner.rs`). `trybuild=…` alone matches no test name and runs nothing.
- Never hand-write `.stderr` files. A default run writes missing snapshots into a scratch directory (named in the failure output) and fails, telling you to move them into place; `TRYBUILD=overwrite cargo test` writes them in place (review with `git diff`); `TRYBUILD=verify` fails on missing/mismatched snapshots without writing. Parsing lives in `src/internal/sys/env.rs`.
