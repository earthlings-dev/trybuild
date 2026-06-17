# AGENTS.md

This file provides guidance to Agents when working with code in this repository.

## What this is

`trybuild` is a test harness that compiles a set of `.rs` files with `rustc` and asserts the resulting compiler diagnostics match saved `*.stderr` snapshots ("ui tests"). The public API is three functions; see `README.md` and the `src/lib.rs` module docs for user-facing usage.

## Commands

Build / check:
- `cargo build` / `cargo check`

Test:
- `cargo test` — everything (normalizer unit tests + self-hosted integration tests).
- `cargo test --lib` — just the normalizer unit tests (fast; avoids spawning cargo-inside-cargo).
- `cargo test --lib internal::diagnostics::snapshots::tests::<name>` — one normalizer snapshot case, e.g. `internal::diagnostics::snapshots::tests::basic` or `…::consteval`. Cases live in `src/tests/<name>.rs`. (Declared from `src/internal/diagnostics.rs`, not `normalize.rs`, so the fuzz target's `#[path]` include of `normalize.rs` stays free of test wiring.)
- `cargo test --test test` — the self-hosted integration suite (`tests/test.rs` runs trybuild on `tests/ui/*.rs`).
- `cargo test -- test trybuild=<file.rs>` — run a single ui case by filename substring. The bare `test` is cargo's filter selecting the integration `#[test] fn test`; trybuild itself reads the `trybuild=` argument (see `filter()` in `src/internal/runner.rs`). Both tokens are required — `trybuild=…` alone matches no test name and runs nothing.

Update snapshots (do not hand-write `.stderr` files):
- A default run writes any missing snapshot into a `wip/` directory and fails the run, telling you to move it into place.
- `TRYBUILD=overwrite cargo test` — write/overwrite `.stderr` files in place; review with `git diff` afterward. (`TRYBUILD=wip` forces the default wip behavior, `TRYBUILD=verify` fails on a missing or mismatched snapshot without writing anything; parsing is in `src/internal/sys/env.rs`.)

Lint / format / docs (matching CI in `.github/workflows/ci.yml`):
- `cargo clippy --tests -- -Dclippy::all -Dclippy::pedantic`
- `cargo fmt`
- `cargo doc` (CI runs `cargo docs-rs` on nightly with `RUSTDOCFLAGS=-Dwarnings`)

Fuzz (nightly + `cargo-fuzz`): `cargo fuzz check`, `cargo fuzz run normalize` — fuzzes `normalize::diagnostics` (`fuzz/fuzz_targets/normalize.rs`).

MSRV / edition: Rust 1.96 (`rust-version` in `Cargo.toml`), edition 2024. The CI matrix spans nightly/beta/stable and the pinned MSRV. Optional `diff` feature (unix-only) highlights expected-vs-actual output via `dissimilar`.

## Architecture

The crate root (`src/lib.rs`) is a thin router that re-exports the public surface from a private `mod internal`; everything else lives under `src/internal/`. Execution is **explicit** (there is no `Drop`-driven run): `TestCases::{pass,compile_fail}` (`src/internal/cases.rs`) register path globs, and the run happens only when you call `TestCases::run` (terminal, `TRYBUILD`-env-driven) or `TestCases::try_run` (typed, terminal-free). A whole suite is still usually declared in one `#[test] fn` because the cases share one synthesized project.

The runner (`src/internal/runner.rs`) is the core. Its reporter-free `compute` pipeline:
1. Expands globs into uniquely-named bin targets `trybuildNNN` (`src/internal/runner/expand.rs`).
2. Synthesizes a throwaway Cargo project under `<target>/tests/trybuild/<crate>/`. It reads the crate-under-test's manifest (`src/internal/project/dependencies.rs`, with workspace inheritance / `[patch]` / `[replace]` resolution in `src/internal/project/inherit.rs`), then generates a fresh `Cargo.toml` (`src/internal/project/manifest.rs`) that path-depends on the crate-under-test, re-exports its deps + dev-deps, and registers each test file as a `[[bin]]`. A `.lock` file (`src/internal/sys/flock.rs`) serializes concurrent runs that share this generated project.
3. Builds the bins with `cargo build`/`check --message-format=json --target <host>` (`src/internal/build/cargo.rs`, all invocations captured), then parses the streamed JSON (`parse_cargo_json`, `src/internal/build/json.rs`) to extract each rustc diagnostic, keyed by source path.
4. Normalizes each diagnostic (`src/internal/diagnostics/normalize.rs`) and compares against the adjacent `<test>.stderr`. A `compile_fail` test passes iff the build failed **and** the output matches; a `pass` test must compile, after which the binary is executed and must not panic (`run_test`).

### Computed outcomes vs rendered output

`compute` writes nothing to the terminal: it returns a typed `Report` (`src/internal/outcome.rs`) of one `CaseReport` per fixture, each carrying an `Outcome` or a typed `TryBuildError` (`src/internal/error.rs` plus the per-domain `SysError`/`ProjectError`/`BuildError`/`DiagnosticsError`/`RunnerError` enums in their domain modules). Every diagnostic — the expected/actual diff, the build output, the run output — is carried as data in those typed payloads, not printed. `TestCases::run` streams that report to the terminal through the render *view* (`src/internal/report/message.rs`, over the owned `Reporter` in `src/internal/report/reporter.rs`) as each case resolves, then collapses it to the aggregate error; `TestCases::try_run` returns the `Report` untouched. The snapshot-reconciliation mode (`Update`, `src/internal/sys/env.rs`) is read from `TRYBUILD` by `run` and passed explicitly to `try_run`.

### Normalization is append-only (the most important rule)

`src/internal/diagnostics/normalize.rs` defines an ordered `Normalization` enum. `diagnostics()` returns a *set* of `Variations`: each variation is the output as the normalizer would have rendered it at a successive point in its history. A test passes if the saved `.stderr` matches **any** variation; the **last** ("preferred") variation is what gets written when creating or overwriting a snapshot.

Consequence: when adding a normalization step, **append the new enum variant at the marked end of the list** — never insert or reorder. Reordering changes the historical variations and breaks already-saved snapshots across every downstream crate. There is an explicit comment in the enum marking the insertion point. Snapshot cases for each step live in `src/tests/*.rs`, wired up by the `test_normalize!` macro (`src/tests.rs`) and `automod` from `src/internal/diagnostics.rs`.

### Self-hosted integration tests

`tests/test.rs` runs trybuild against `tests/ui/*.rs` via `TestCases::run`. Because `run` returns a `Result` (it never panics), the self-test deliberately pairs files with mismatched expectations and asserts `run().is_err()` rather than relying on a non-panic guard. `tests/try_run.rs` exercises `TestCases::try_run`: it asserts the typed per-fixture outcomes (both polarities of each) and that the run is terminal-free, the latter driven through `strict_test_support::capture_ignored_test`.

### Notable details

- `--keep-going` fast path: if the installed cargo supports `--keep-going` (probed in `build_dependencies`, `src/internal/build/cargo.rs`) and there are no pass-tests, `run_all` builds all bins at once and parses the combined JSON instead of building per-test.
- `--target <host triple>` is passed by default so `RUSTFLAGS` reach the test crates; the `trybuild_no_target` cfg disables this for coverage tooling. `--diagnostic-width=140` is forced for stable line wrapping.
- The crate root (`src/lib.rs`) carries a curated clippy allow-list. Because CI runs `-Dclippy::pedantic`, silencing a new pedantic lint usually means adding to that crate-level list rather than an inline `#[allow]`.

## Commit messages

Use Conventional-Commit style with a **required scope**:

- Subject: `type(scope): imperative description of the structural change`. Append `!` for breaking changes (e.g. `feat(normalize)!: …`).
- Never use `chore`. Even for renames, dependency bumps, or janitorial work, pick a descriptive type: `feat`, `fix`, `refactor`, `perf`, `build`, `ci`, `docs`, `test`, `style`, `revert`, etc.
- Body: 1–5 sections sized to the change. Each section starts with a plain-text header line (no `#`, no bold), followed by 3–5 imperative bullets describing structural changes (what was introduced, replaced, removed, renamed, or rewired). Separate sections with exactly one blank line; don't pad small commits with empty sections.
- Pass the message via a HEREDOC to `git commit -m` so blank lines and bullet spacing survive shell quoting.
