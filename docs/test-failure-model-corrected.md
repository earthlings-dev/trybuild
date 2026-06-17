# The test-failure model (corrected) — Drop was never a blocker

> Status: **review draft.** Supersedes Part 1 (the (i)/(ii) "Drop dilemma") of `drop-semantics-and-resolved-verifications.md`. That doc's Part 2 (resolved verifications) remains valid. Grounded in `strict-test-support` source at `~/earthlings/strict-test-support-rs`.

## The invalid premise

Earlier `Drop` options (i)/(ii) assumed **"a suite fails CI by panicking, and `Drop` is the failure channel."** That is standard-trybuild's model, not this ecosystem's. It is false here.

## How failure actually flows (source-grounded)

`strict-test-support/src/lib.rs:1-7`: *"Tests report failure by returning `Result<(), TestFailure>` and using the `ensure*` helpers below instead of the standard assertion macros."*

- **Plain `#[test] -> Result<(), TestFailure>`.** No custom harness, no registration: the workspace has **no** `libtest`/`trial`/`datatest`/`inventory`/`linkme`/`ctor` dependency (grep, both `Cargo.toml`s). A test fails by **returning `Err(TestFailure)`** — the value is the failure signal. No panic; `Drop` is not involved.
- **Uniform façade shape:** every capability is a function returning `Result<(), TestFailure>`:
  - `ensure_property<S, F>(strategy, context, property) -> Result<(), TestFailure>` (property.rs:150) — wraps proptest's Result core.
  - `ensure_snapshot(actual, snapshot_path, context) -> Result<(), TestFailure>` (snapshot.rs:136) — wraps snapbox's Result core; blessing via the `SNAPSHOTS=overwrite` env var (lib.rs:50-52).
  - core `ensure` / `ensure_eq` / `ensure_contains` / `ensure_ok` / … (ensure.rs), `TestFailure` (error.rs:22) with `From<io::Error>` so `?` keeps a source chain.
- Default build pulls **no deps**; engines are feature-gated (`proptest`, `snapbox`, `thiserror`, `full`).

## Consequence for the trybuild fork

The `Drop` "dilemma" dissolves:

1. **`try_run() -> Result<Report, SetupError>`** is the panic-free, structured core (unchanged from `type-architecture-v1.md`).
2. **STS adds `ensure_compile_fail` (new, feature-gated — e.g. feature `trybuild`)**, mirroring `ensure_snapshot`:
   ```rust
   // in strict-test-support, behind feature "trybuild":
   pub fn ensure_compile_fail(cases: &trybuild::TestCases, context: &'static str)
       -> Result<(), TestFailure> {
       let report = cases.try_run().map_err(/* SetupError -> TestFailure::Caused */)?;
       // map the structured Report into a TestFailure listing the failing fixtures,
       // or Ok(()) when report.is_ok().
   }
   ```
   The test body returns this `Result`; failure is the returned `Err`. **No panic. `Drop` uninvolved.**
3. **trybuild's `Drop` is simply made panic-free** (guarded no-op via the `Cell<bool>` guard, §2.4 of the prior doc). It is *never* a correctness channel: in façade usage `try_run` has already run inside `ensure_compile_fail`, so `Drop` no-ops. There is no bare-`Drop` reliance to "break" — so (i)'s supposed breaking consequence was fictional, and (ii) (retain a `panic!` against `clippy::panic = "deny"`) solved a non-problem.

Blessing parallels `ensure_snapshot`: trybuild exposes `bless(mode)` (read-only `try_run` stays pure, per #23/#24); STS drives it the same way `ensure_snapshot` honors `SNAPSHOTS=overwrite`.

## What actually remains (not a blocker)

Only a trivial UX preference for the **non-façade / interactive** path — when someone constructs `TestCases` and never calls `try_run`/`ensure_compile_fail`:

- **(a) render-only `Drop`:** runs once and prints the report (never fails). Preserves "I see output from `cargo test`" and avoids a silent no-test.
- **(b) pure no-op `Drop`:** does nothing.

Either is panic-free and policy-clean. Since every consumer here goes through `ensure_compile_fail`, this affects only ad-hoc/interactive use. Lean: **(a)** for the no-silent-footgun property. This is a preference, not a fork that gates anything.

## Scope notes

- The type architecture (`Report` / `TestStatus` / `RunStatus` / `SetupError`) is **unchanged** by this correction — only the `Drop`/failure framing changes.
- `ensure_compile_fail` lives in `strict-test-support` (the library half), per the established split; trybuild only provides `try_run`/`bless`/`Report`. Its exact `Report -> TestFailure` mapping is an STS-side design (separate repo) and not part of the trybuild fork plan.
