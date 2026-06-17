#### Section 1 — Foundational (these gate everything below)

**D-1 — Public API: expose trybuild's internal `Error`, or a purpose-built public type?**
- (a) Make the existing internal `Error` (error.rs:8) `pub` and enrich it. *Smallest code, but publishes all 18 internal variants — couples STS to trybuild internals and churns your public API on every upstream rebase.*
- (b) **Keep internal `Error` private; add a new, purpose-built public result/error type** that the internal `Error` maps into. *More code + a mapping layer, but a clean, decoupled, strongly-typed public surface that survives upstream rebases.*
- *Rec: (b).* It matches "strict typing / structured results," decouples the wrap from trybuild's guts, and — importantly — it **dissolves most of Section 3** (no need to tighten trybuild's 14 messy infra variants; we just map them).

**D-2 — Per-test result representation.**
- (a) `Result<Outcome, Error>` per test (Outcome for pass-ish, Error for fail).
- (b) **One strongly-typed `enum TestStatus`** covering every per-test outcome (pass / wip / blessed / each failure kind).
- *Rec: (b)* — a single exhaustive typed enum is more structured and lets STS `match` once. Shape proposed in D-4.

**D-3 — `Report` / `TestReport` shape.** Proposed (react to fields/names):
```rust
pub struct Report { pub tests: Vec<TestReport> }
impl Report { pub fn ok(&self)->bool; pub fn failed(&self)->usize; pub fn wip(&self)->usize; }
pub struct TestReport {
    pub path: PathBuf,
    pub expected: Expected,        // make Expected (lib.rs:307) pub
    pub status: TestStatus,
}
```
- *Decision:* confirm fields/names. Open sub-choices: include the test `name: String` (run.rs Name) too? Keep counts as methods (rec) vs stored fields?

#### Section 2 — Per-test failure taxonomy (strongly typed, source-anchored)

**D-4 — The `TestStatus` cases + typed payloads.** Every payload below is data already in scope at the construction site (so it's capturable, not invented):
```rust
pub enum TestStatus {
    Passed,                                              // run.rs:483/427 etc.
    Wip,                                                 // CreatedWip; run.rs:465
    Blessed,                                             // Overwrite wrote .stderr; run.rs:470/494
    Mismatch        { expected: String, actual: String },          // run.rs:477 + :441
    ShouldHaveFailed{ stdout: String, warnings: String },          // run.rs:444-447 (build_stdout + preferred)
    BuildFailed     { stderr: String },                            // run.rs:418-420 (preferred)
    RunFailed       { status: ExitStatus, stdout: String, stderr: String, warnings: String }, // run.rs:423-429
}
```
- *Decision:* confirm the case set, names, and each payload. Sub-choices: `RunFailed.status` as `ExitStatus` (rec, strongly typed) vs `Option<i32>` code; do you want `Mismatch` to also carry `stderr_path: PathBuf` (run.rs:450)?

**D-5 — `CargoFail` is overloaded — split it?** It means two different things: a **pass-test that failed to compile** (run.rs:420, a test verdict, stderr available) vs **a cargo invocation returning nonzero** (cargo.rs:105, pure infrastructure).
- *Rec: split* — verdict case becomes `TestStatus::BuildFailed { stderr }` (D-4); the cargo.rs:105 case stays an infrastructure/setup error (Section 3). Strict typing demands not conflating them.

#### Section 3 — Infrastructure (run-aborting) errors

These are the 14 non-verdict variants that abort the whole run (no manifest, cargo exec failure, TOML parse, IO, glob, etc.).

**D-6 — How to represent setup failures publicly.** *(Largely determined by D-1.)*
- Under **D-1(b)**: map all 14 into a small typed public enum, e.g. `pub enum SetupError { Cargo, Manifest, Metadata, Io, Glob, Toml, Env, … }` (typed, bounded; carries `String`/`PathBuf` context where useful) — and `try_run(&self) -> Result<Report, SetupError>`. *No need to touch trybuild's internal infra variants.*
- Under **D-1(a)**: you'd additionally have to decide per-variant whether to tighten each (capture `Io` paths, replace `GetManifest(Box<Error>)` with typed variants, capture `Metadata` stderr, etc.) — a much larger, churnier diff.
- *Rec:* D-1(b) + a compact typed `SetupError`. *Decision:* the exact `SetupError` cases/granularity (one opaque case vs a handful of typed ones).

**D-7 — Keep `already_printed()` (error.rs:65)?** It's how the Drop path avoids double-printing verdict detail.
- *Rec: keep it* for the internal Drop wrapper (Section 4), unaffected by the public API. *Decision:* confirm.

#### Section 4 — Run/Drop behavior

**D-8 — Silent-core extraction approach.** The census found `message::*` printing is *interleaved* through `check_pass`/`check_compile_fail`/`run_all` (e.g. run.rs:419,425,444-492).
- (a) **Make `check_*` pure** (return structured data, no `message::*`); a separate reporter renders from the returned `Report`. *Cleanest, matches structured-results; `try_run` is silent for free.*
- (b) Add `_silent` twin functions. (c) Thread a `silent: bool` through every message call.
- *Rec: (a).*

**D-9 — Console output ordering in the Drop path: streamed vs batched.** Approach (a) renders *after* the run (batched) rather than streaming each "test X ... ok" mid-run.
- (a) **Batched** rendering from the `Report` — simpler; cosmetic-only change to trybuild's own console output.
- (b) **Preserve exact streaming** — requires threading an observer/callback through the core; more invasive.
- *This is your call:* do you require byte-for-byte parity with upstream's streamed output, or is batched acceptable? *Rec: batched* (STS doesn't consume trybuild's console; the fork's own UI is non-load-bearing).

**D-10 — Double-run guard.** Add `already_run: bool` to `Runner`; `Drop` checks it so an explicit `try_run()` followed by drop doesn't re-run. *Rec: yes (correctness). Decision:* confirm location (`Runner` field).

**D-11 — Keep `Drop` auto-running by default?** Preserves drop-in back-compat for existing trybuild users (lib.rs:339). *Rec: keep. Decision:* confirm (the alternative — making Drop a no-op — breaks upstream compatibility).

#### Section 5 — Blessing (the `.stderr` update path)

**D-12 — Does `try_run` perform blessing, or is it check-only?** Blessing mutates files (wip/overwrite, run.rs:464/469/493). This matters for how STS wires `snap-update`.
- (a) `try_run` honors the update mode and performs blessing writes (consistent with Drop).
- (b) `try_run` is **read-only check**; report what *would* change; a **separate** explicit `bless`/update entry performs writes.
- *This is your call* and shapes the STS integration. *Rec: (b)* — a read-only assertion path + an explicit bless action is the cleaner, more predictable contract for a panic-free test vocabulary, and maps naturally onto a `snap-update`-style command.

**D-13 — Blessing API shape + `Update` exposure.**
- Shape: `TestCases::update(Update)` setter vs `try_run_with(Update)` parameter vs a builder.
- Expose `Update` (env.rs:5) as `pub` as-is (`Wip`/`Overwrite`)? Precedence: programmatic over `TRYBUILD` env (rec).
- *Rec:* depends on D-12; if (b), an explicit method taking `Update`. *Decision:* shape + naming.

#### Section 6 — Metadata, lints, tests, process (quick confirmations)

**D-14 — Cargo.toml:** `repository` → `earthlings-dev/trybuild`; **keep `version = "1.0.116"`** (patched downstream by git rev, so no bump needed); add `publish = false`. *Decision:* confirm each.

**D-15 — Lint/verification bar:** target trybuild's **own** CI gate — `cargo test`, `cargo clippy --tests -- -Dclippy::all -Dclippy::pedantic`, `RUSTFLAGS="-Dwarnings"`, `cargo fmt --check` — **not** STS's stricter policy (the fork is an external patched dep). Add `///` docs to all new public items. *Decision:* confirm.

**D-16 — Tests:** add `try_run` tests asserting on the returned `Report` (no panic), covering each `TestStatus` polarity (pass, mismatch, should-have-failed, build-failed) plus the double-run guard and blessing, mirroring the existing `tests/test.rs` + `tests/ui/*.rs`/`.stderr` fixture pattern. *Decision:* confirm approach.

**D-17 — Scope boundary:** this plan is **fork-only**. The downstream `[patch.crates-io]` rev pin and the STS `ensure_compile_fail` façade are separate later tasks. *Decision:* confirm.

**D-18 — Branch + commits:** branch `earthlings/expose-core`; commits in your conventional format. *Decision:* confirm name.