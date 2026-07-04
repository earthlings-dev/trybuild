# AGENTS.md

Scope: `src/` — the trybuild library. The repo-root `AGENTS.md` owns the command matrix, the end-to-end architecture walkthrough, and commit conventions; this file maps the source tree and the rules that bite when editing it.

## Layout

`src/lib.rs` is a thin router: rustdoc for the three-function public API, the curated crate-level clippy allow-list, and re-exports from the private `mod internal`. Everything else lives under `src/internal/` (`src/internal.rs` declares the tree), split by domain — each module's `//!` header is its authoritative description:

- `cases.rs` — the public `TestCases` builder users construct, register `pass`/`compile_fail` globs on, and finally `run` (terminal, `TRYBUILD`-env-driven) or `try_run` (typed, terminal-free). Execution is explicit; there is no `Drop`-driven run.
- `runner.rs` + `runner/` — the core `compute` pipeline; `runner/expand.rs` expands globs into uniquely-named `trybuildNNN` bin targets. `filter()` here implements the `trybuild=<file.rs>` argv filter.
- `project.rs` + `project/` — synthesizes the throwaway Cargo project: `dependencies.rs` reads the crate-under-test's manifest, `inherit.rs` resolves workspace inheritance / `[patch]` / `[replace]`, `manifest.rs` generates the project's `Cargo.toml`.
- `build.rs` + `build/` — runs `cargo build`/`check --message-format=json` (`cargo.rs`) and parses the streamed JSON into per-source diagnostics (`json.rs`).
- `diagnostics.rs` + `diagnostics/` — normalization (`normalize.rs`) and comparison against `.stderr` snapshots; also declares the snapshot test modules (see below).
- `outcome.rs` / `report/` — `compute` returns a typed `Report` of `CaseReport`s carrying outcomes and diffs as data; `report/message.rs` (over the `Reporter` in `report/reporter.rs`) is the render view that `run` streams through.
- `error.rs` — the typed `TryBuildError`; per-domain enums (`SysError`/`ProjectError`/`BuildError`/`DiagnosticsError`/`RunnerError`) live in their domain modules.
- `model.rs` — shared value types threaded across the domains (e.g. `PathDependency`).
- `path.rs` — the `path!` macro for assembling `PathBuf`/`Directory` values and the `CanonicalPath` grouping key.
- `sys/` — OS/process seams: `env.rs` parses `TRYBUILD` into the snapshot-reconciliation `Update` mode, `flock.rs` serializes concurrent runs sharing the generated project, `directory.rs` owns the `Directory` newtype.

## The append-only normalization rule (most important)

`internal/diagnostics/normalize.rs` defines an ordered `Normalization` enum whose variants are replayed as successive historical `Variations` of the normalizer's output; a saved `.stderr` passes if it matches any variation, and the last one is what gets written for new snapshots. When adding a normalization step, append the new variant at the marked end of the enum — never insert or reorder, or already-saved snapshots break across every downstream crate.

## Normalizer snapshot cases (`src/tests.rs` + `src/tests/`)

Each `src/tests/<name>.rs` is one normalizer snapshot case, expanded by the name-driven `test_normalize!` macro in `src/tests.rs`. The raw compiler diagnostic input lives in `src/tests/inputs/<name>.stderr`; the expected preferred normalized output lives in `src/tests/snapshots/<name>.snap` and is compared with `strict_test_support::ensure_snapshot`. Case modules should contain only the fixture name plus optional `DIR`/`WORKSPACE`/`INPUT`/`TARGET` context overrides. The cases are mounted via `automod` from `src/internal/diagnostics.rs` — deliberately not from `normalize.rs`, so the fuzz target's `#[path]` include of `normalize.rs` stays free of test wiring (see `fuzz/AGENTS.md`).

- `cargo test --lib` — all snapshot cases, fast (no cargo-inside-cargo).
- `cargo test --lib internal::diagnostics::snapshots::tests::<name>` — a single case.
- `SNAPSHOTS=overwrite cargo test --lib` — refresh committed `src/tests/snapshots/*.snap` files after a deliberate normalizer output change; review the diff afterward. Do not hand-write `.snap` files.

## Lint posture

CI runs `cargo clippy --tests -- -Dclippy::all -Dclippy::pedantic`. Silencing a new pedantic lint usually means extending the curated crate-level allow-list in `src/lib.rs` with a justification, not adding an inline `#[allow]`.
