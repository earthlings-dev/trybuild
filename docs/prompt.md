### PROMPT — Expose a panic-free, data-returning core in the `earthlings-dev/trybuild` fork

#### Repository & starting state
- Work in `~/rust-forks/trybuild` (remote `earthlings-dev/trybuild`, branch `master` @ `0ee8f5a`). Tree is clean.
- Source is identical to crates.io `trybuild` 1.0.116 except `src/message.rs`. Zero `unsafe`; pure-Rust.
- Create a branch `earthlings/expose-core` off `master`. All work goes there.

#### Objective
trybuild already separates *computing* a test outcome (internal `Result`-returning `run_all`/`check_*`) from *reporting* it (print to terminal + `panic!` in `Drop`). It just never publishes that seam. **Make the seam public, structured, and silent**, so a downstream crate (`strict-test-support`) can surgically wrap trybuild's core capability — exactly as it wraps `proptest::TestRunner::run` and `snapbox::Assert::try_eq` — turning compile-fail testing into a panic-free `ensure_compile_fail` that returns a typed failure and routes the diagnostic diff through the existing snapshot machinery.

This is an **additive, surgical** change. Preserve trybuild's behavior, its hard-won output normalization, and its existing public API verbatim. The fork must remain rebaseable onto future upstream releases.

#### Hard requirements — the new public API (this is the contract)

Add these to the public surface (re-exported from the crate root in `lib.rs`):

```rust
// error.rs — make public, and enrich failure variants to CARRY their diagnostics
pub enum Error {
    // ... existing variants ...
    Mismatch { expected: String, actual: String },   // was a unit variant
    ShouldNotHaveCompiled { stdout: String },         // carry the build output (recommended)
    // RunFailed / CargoFail etc. may stay as-is or carry context; document whatever you choose
}
pub type Result<T> = std::result::Result<T, Error>;

// run.rs — make public; Report becomes structured per-test data
pub enum Outcome { Passed, CreatedWip }

pub struct TestReport {
    pub path: std::path::PathBuf,
    pub expected: Expected,                  // make `Expected` (lib.rs) public too
    pub outcome: Result<Outcome>,            // Ok(Passed/CreatedWip) or the rich Err
}

pub struct Report { pub tests: Vec<TestReport> }
impl Report {
    pub fn ok(&self) -> bool;                // no failures and no created-wip
    pub fn failures(&self) -> usize;
    pub fn created_wip(&self) -> usize;
}

// lib.rs — TestCases gains a non-panicking, non-printing entry + programmatic blessing
impl TestCases {
    /// Run all registered cases. Never panics, never writes to stdout/stderr.
    /// Returns one `TestReport` per case; I/O/setup failures surface as `Err`.
    pub fn try_run(&self) -> Result<Report>;

    /// Set the blessing mode programmatically (overrides the `TRYBUILD` env var).
    pub fn update(&self, update: Update);    // make `Update` (env.rs) public + re-export
}
```

`Expected` (lib.rs:307) and `Update` (env.rs:5) become `pub` and are re-exported from the crate root.

#### Behavioral invariants (must all hold)
1. **Existing public API and `Drop` semantics are unchanged.** `TestCases::new/pass/compile_fail` and the `cargo test`-driven `Drop` path behave exactly as upstream: same console output, same `panic!`-on-failure, same `TRYBUILD=overwrite`/`wip` workflow, same `.stderr` files. A current trybuild user who never calls `try_run` sees zero behavior change.
2. **`try_run` is silent.** It must not print to stdout/stderr (no `term`/`print!`/`println!`). All diagnostic content reaches the caller as data inside `Report`/`Error` (e.g. `Error::Mismatch { expected, actual }`).
3. **`try_run` never panics for a *test* outcome.** Test failures (mismatch, should-not-compile, build-failed) are `Err` inside the relevant `TestReport.outcome`. Only genuine setup/I/O faults (no manifest, cargo-metadata failure, lock failure) return the outer `Err` from `try_run`.
4. **No double-run.** Add a `Cell<bool>` "ran" guard on `TestCases` (or `Runner`). `try_run` sets it; `Drop` becomes `if !thread::panicking() && !self.ran.get() { ...existing run+panic... }`. Calling `try_run` then dropping the `TestCases` must NOT re-run the suite or panic.
5. **Programmatic `update` takes precedence over the `TRYBUILD` env var**; if `update` is never called, env behavior is unchanged.

#### Recommended implementation (the seam to exploit — file by file)

Keep edits localized to `error.rs`, `run.rs`, `lib.rs`, `env.rs`, `message.rs`. **Do not touch** `normalize.rs`, `cargo.rs`, `dependencies.rs`, `manifest.rs`, `diff.rs`, `expand.rs`, `flock.rs`, `features.rs`, `inherit.rs`, `directory.rs`, `path.rs`, `rustflags.rs`, `term.rs`.

- **error.rs** — flip `pub(crate)` → `pub` on `Error` (line 8) and `Result` (line 29). Convert `Mismatch` to `Mismatch { expected, actual }`; optionally enrich `ShouldNotHaveCompiled`/`RunFailed`. Update the `Display` impl (line 31) and `already_printed` (line 65) accordingly.
- **run.rs** — make `Outcome` (line 365) and a redesigned `Report` (line 44) public; add `TestReport`. **Split `Runner::run` (line 50):** extract a `fn try_run(&mut self) -> Result<Report>` containing the prepare/lock/write + test-execution logic, but (a) propagate setup errors as `Err` instead of `message::prepare_fail(err) + panic!` (lines 60-63), (b) collect each case into a `TestReport` **without** calling `message::*`, (c) return `Ok(report)`. Keep `pub(crate) fn run(&mut self)` as a thin wrapper: call `try_run`, **render** the report via the existing `message::*` functions (so console output is byte-for-byte preserved), then `panic!` on `failures>0` / `created_wip>0` exactly as lines 101-109 do today. In `check_compile_fail` (line 433) construct the rich errors (`Mismatch { expected: expected.clone(), actual: preferred.to_owned() }` at line 489) and move the `message::*` calls out of the pure path into the reporter; keep the `fs::write` blessing side-effects (they produce data, not console output). Source `project.update` from the programmatic override when set, else `Update::env()`.
- **lib.rs** — add the `ran: Cell<bool>` field; add `try_run`/`update`; make `Expected` public; guard `Drop` (line 339); add `pub use` re-exports for `Error, Result, Report, TestReport, Outcome, Expected, Update`. Document every new public item with `///` (and keep the crate-level `//!`).
- **env.rs** — `pub enum Update`.
- **message.rs** — the reporter (the Drop path) drives these; since `Error::Mismatch` now carries `expected`/`actual`, `message::mismatch(&expected, &actual)` is called from the reporter using the data off the error. Adjust signatures only as needed to read from the enriched errors; do not add new printing.

> Note the one acceptable cosmetic delta: the Drop path may now render per-test results from the collected `Report` (batched) rather than streaming each line mid-run. If exact streaming parity is required, thread an optional `&mut dyn FnMut(&TestReport)` observer through the shared core instead — but batched rendering is acceptable and simpler; choose batched unless a test proves parity matters.

#### Non-goals / do not do
- Do **not** change diagnostic normalization, the synthesized-project logic, or any `.stderr` format. Output content must be identical.
- Do **not** add dependencies, remove existing deps, or bump editions.
- Do **not** reformat or "clean up" untouched code (keeps the upstream diff reviewable/rebaseable).
- Do **not** alter the existing 3-function API signatures or the `TRYBUILD` env contract.

#### Cargo.toml / metadata
- Set `repository = "https://github.com/earthlings-dev/trybuild"`; add `publish = false` (it's consumed as a git dependency).
- **Keep `version = "1.0.116"`** — it's pinned downstream via `[patch.crates-io] trybuild = { git = …, branch/rev = … }`, which matches by source, not version, so no bump is needed and `multiple_crate_versions` stays quiet.
- Leave `rust-version = "1.85"` (downstream pins stable ≥ that).

#### Tests to add (both polarities, real fixtures)
Add fixtures under `tests/` and a test that exercises `try_run` directly, asserting structured outcomes — do not rely on the panic path:
- a `pass` fixture that compiles → `Ok(Outcome::Passed)`;
- a `compile_fail` fixture **with** a matching `.stderr` → `Ok(Outcome::Passed)`;
- a `compile_fail` fixture whose `.stderr` does **not** match → `Err(Error::Mismatch { expected, actual })` with both strings populated (assert non-empty and that `actual` contains the expected diagnostic substring);
- a `compile_fail` fixture that actually **compiles** → `Err(Error::ShouldNotHaveCompiled { .. })`;
- a **double-run guard** test: call `try_run`, then drop the `TestCases` in the same test — assert no second run / no panic (e.g. via the report being produced exactly once).

Also confirm the crate's own existing UI suite (`src/tests/*.rs`, the `trybuild-tests` self-harness) still passes unchanged.

#### Verification (run all; all must be green)
```
cargo build
cargo test
cargo clippy --all-targets    # respect trybuild's own lint config
cargo fmt --check
```
Confirm the doctests in `lib.rs` (the `TestCases::new()` examples) still compile.

#### Acceptance criterion (the downstream shape that must work)
This must compile and behave against the fork — panic-free, output-as-data:
```rust
let cases = trybuild::TestCases::new();
cases.compile_fail("tests/ui/*.rs");
let report: trybuild::Report = cases.try_run()?;        // no panic, no stdout/stderr
for t in &report.tests {
    match &t.outcome {
        Ok(trybuild::Outcome::Passed) => { /* ok */ }
        Err(trybuild::Error::Mismatch { expected, actual }) => { /* route to ensure_snapshot */ }
        Err(other) => { /* map to a typed failure */ }
        _ => {}
    }
}
// `cases` dropping here must NOT re-run or panic.
```

#### Commit conventions
Conventional commits, scope required, never `chore`. Imperative subject describing the structural change; body of 1–5 plain-text-header sections, each 3–5 imperative bullets. Example:

```
feat(api): expose a panic-free, data-returning run core

Public surface
- Re-export `Error`, `Result`, `Report`, `TestReport`, `Outcome`, `Expected`, `Update` from the crate root
- Add `TestCases::try_run` returning a structured `Report` without panicking or printing
- Add `TestCases::update` to drive blessing mode programmatically, overriding `TRYBUILD`

Structured outcomes
- Convert `Error::Mismatch` to carry `expected`/`actual`; enrich `ShouldNotHaveCompiled` with build stdout
- Collect per-case results into `TestReport` instead of discarding them at the print boundary

Backward compatibility
- Keep `new`/`pass`/`compile_fail` and the `Drop`-driven `cargo test` path byte-for-byte unchanged
- Guard `Drop` with a `ran` flag so an explicit `try_run` does not trigger a second run
- Re-point `repository` at the fork and mark it `publish = false`
```