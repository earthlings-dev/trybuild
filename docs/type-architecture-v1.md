# Type Architecture Review — trybuild `strict` fork (v1)

> Status: **review draft. Nothing here is decided.** This is a holistic design of the public type graph for the panic-free, structured, data-returning core, synthesized from `docs/prompt.md` and `docs/questions_v2-responses.md`. Every type is a proposal to be reviewed, revised, or rejected. Source references are against the current `strict` branch.
>
> Companion follow-ups will be new docs (one per significant exchange), e.g. `type-architecture-v2.md`.

---

## 0. Constraints in force (the type design must satisfy these itself)

From the fork's `Cargo.toml` (`[workspace.lints]`, now strict):

- edition **2024**, rust-version **1.96**, `unsafe_code = "forbid"`.
- `deny`: `panic`, `panic_in_result_fn`, `unwrap_used`, `expect_used`, `indexing_slicing`, `print_stdout`, `print_stderr`, `missing_docs`, `missing_docs_in_private_items`, `missing_debug_implementations`, plus the full rust-template clippy set.

Implications for *these* types: every type (and field, and private item) carries a doc comment; every type derives `Debug`; no panicking constructors; prefer owned, typed fields over stringly data; enums that we expect to grow are `#[non_exhaustive]`, enums STS must match exhaustively are **not**.

**Scope flag (not buried):** the crate's *existing* code is not yet conformed to this policy (e.g. `message.rs` has 54 print-sites, `normalize.rs` has unwraps), so the crate currently fails its own deny-level lints. Conforming the whole crate (docs on every private item, removing `unwrap`/`expect`/`indexing`, panic-free) is a large workstream **largely orthogonal to** — and compounding — the type architecture. See §7.

---

## 1. The four layers (holistic view)

The public surface decomposes into four layers. Designing them together (rather than as isolated questions) is the point of this doc.

```
(A) Expectation   — what you ASK trybuild to verify        (§2)
(B) Result        — what trybuild REPORTS back             (§3)
        Report → PatternGroup → TestReport → TestStatus
(C) SetupError    — how the run aborts BEFORE results      (§4)
(D) Entry points  — try_run / bless / Drop                 (§5)
```

The current engine (source-anchored) only distinguishes two expectations and reports counts; this design widens (B) into a fully-typed graph and (C) into a typed error set, and confronts the behavioral constraints (D) that bind them.

---

## 2. Layer A — the Expectation model

**Today (source):** `enum Expected { Pass, CompileFail }` (lib.rs:307). Dispatch at run.rs:394 (`check_pass` vs `check_compile_fail`). Semantics:

- `Pass` (run.rs:409): must compile; then the binary is **executed** and must not fail.
- `CompileFail` (run.rs:433): must **fail** to compile; normalized diagnostics must match the adjacent `.stderr`.

**Your pushback (#6):** why restrict to two? Honest answer: the *engine* only implements these two. Anything richer is not "expose the core" — it's new engine behavior. So the real decision is **how far to widen the expectation model, and to pay the per-case engine cost**. The faithful options, each with its cost:

| Expectation case | Meaning | Engine work required |
|---|---|---|
| `Pass` | compiles + runs OK | none (exists) |
| `CompileFail` | fails to compile, diagnostics match | none (exists) |
| `PassWithWarnings { warnings: Snapshot }` | compiles, **and** the warning diagnostics match a snapshot | new: today warnings are captured (`message::warnings`, run.rs:446) but **not asserted** in Pass mode |
| `RunMatches { stdout?/stderr?/code? }` | compiles, runs, and runtime output/exit matches | new: today a Pass test only checks "ran without failure" (run.rs:423-429); asserting runtime output is new |
| `Expand { expanded: Snapshot }` | macro-expansion matches (à la `macrotest`) | **not present at all**; trybuild has no expansion pass (`expand.rs` is *glob* expansion, not macro expansion) — large new feature |

**Proposal to review (provisional names):**

```rust
/// What a fixture is expected to do. `#[non_exhaustive]` so future modes
/// (warnings-as-snapshot, runtime-output-as-snapshot) can be added without breakage.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum Expectation {
    /// Must compile, then the built binary must run without failure.
    Pass,
    /// Must fail to compile; normalized diagnostics must match the `.stderr` snapshot.
    CompileFail,
}
```

**Open question A1:** ship the faithful 2-case `Expectation` now (renamed from `Expected`, `#[non_exhaustive]` to leave room), or commit to one or more of the richer cases above and their engine work? My read of "comprehensive" is: model the *type* as `#[non_exhaustive]` now, and decide the richer cases as explicit, separately-scoped features — but this is yours to set, and I will not assume it.

**Open question A2:** name — `Expectation` vs keeping `Expected`. (`Expected` reads oddly as a public noun; `Expectation` is clearer. Your call.)

---

## 3. Layer B — the Result model

### 3.1 The shape (grouped by pattern, full per-test data)

Your answers: Report must contain **fully-typed derived info of all tests** (#3); `TestReport` carries `path` + `name` (#4); fixtures **grouped by pattern string** (#5).

```rust
/// The outcome of a whole run. Owns the complete per-test data (source of truth)
/// and derives all summaries from it.
#[derive(Clone, Debug)]
pub struct Report {
    /// One entry per registered `pass`/`compile_fail` call, in registration order.
    pub groups: Vec<PatternGroup>,
}

/// All fixtures that came from a single registered pattern (`compile_fail("ui/*.rs")`).
#[derive(Clone, Debug)]
pub struct PatternGroup {
    /// The path/glob exactly as registered by the caller.
    pub pattern: PathBuf,
    /// Expanded concrete fixtures, sorted (expand.rs sorts glob results).
    pub tests: Vec<TestReport>,
}

/// One fixture's result.
#[derive(Clone, Debug)]
pub struct TestReport {
    /// Expanded concrete fixture path (one row per real file).
    pub path: PathBuf,
    /// Synthesized bin name, e.g. `trybuild037` (see note below).
    pub name: TestName,
    /// The declared expectation for this fixture.
    pub expected: Expectation,
    /// What actually happened.
    pub status: TestStatus,
}

/// The synthesized `[[bin]]` name trybuild assigns each fixture (`trybuildNNN`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TestName(pub String);
```

**Grounding / honesty notes:**

- **Grouping needs new plumbing.** `expand_globs` (expand.rs:15) currently *discards* the originating pattern; each `ExpandedTest` keeps only `is_from_glob: bool` (expand.rs:12). To group by pattern we must retain the originating registered `Test.path` (the pattern) through expansion. This is a real, additive internal change — flagged, not hidden.
- **`name` is positional, not semantic (#4).** It's `Name(format!("trybuild{:03}", index))` (expand.rs:59) — e.g. `trybuild037`. Its only utility: correlating a `TestReport` back to the synthesized `[[bin]]` and the cargo JSON keyed by it. It tells you *which slot*, not *what*. Cheap to include; just know that's what it is. **Open question B1:** include it (my lean: yes, as `TestName`) or omit as noise?

### 3.2 Derived summaries (#3 — "fully typed derived information")

Report owns the per-test data; aggregates are derived. The question is whether the aggregate is a **stored typed value** or **computed on demand**:

```rust
/// Typed roll-up of a Report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub updated: usize,      // wip/blessed (see §3.3)
}

impl Report {
    pub fn summary(&self) -> Summary { /* fold over groups */ }
    pub fn is_ok(&self) -> bool { self.summary().failed == 0 }
    pub fn tests(&self) -> impl Iterator<Item = &TestReport> { /* flatten groups */ }
    pub fn failures(&self) -> impl Iterator<Item = &TestReport> { /* filter */ }
}
```

**Open question B2:** `Summary` as a method (above — single source of truth, no drift) vs a precomputed `pub summary: Summary` field on `Report` (O(1) reads, but duplicated state to keep consistent). I lean **method** (derivation can't desync); you flagged both prior framings as too thin, so this is the honest minimal-but-real tradeoff that remains.

### 3.3 `TestStatus` — the per-test outcome (unified, #2/#7)

Under read-only `try_run` (your #23) and full typed payloads (#10/#12), and noting that the available data differs by code path (source-anchored below):

```rust
/// What happened to one fixture. STS matches this exhaustively, so it is NOT
/// `#[non_exhaustive]`.
#[derive(Clone, Debug)]
pub enum TestStatus {
    /// The fixture met its expectation.
    Passed(Passed),
    /// `compile_fail` that compiled (run.rs:443-447).
    ShouldHaveFailed { stdout: String, diagnostics: String },
    /// `pass` that failed to compile (run.rs:418-420).
    BuildFailed { diagnostics: String },
    /// `pass` that compiled but the binary failed at runtime (run.rs:423-429).
    RunFailed(RunOutcome),
    /// `compile_fail` that failed correctly but the diagnostics differ from the snapshot
    /// (run.rs:486-489).
    Mismatch { expected: String, actual: String, stderr_path: PathBuf },
    /// `compile_fail` that failed correctly but there is **no** `.stderr` on disk
    /// (run.rs:452). Under read-only `try_run` this is an explicit indeterminate result,
    /// not a silent wip write.
    NoSnapshot { actual: String, stderr_path: PathBuf },
}
```

Where the payload types are:

```rust
/// Detail for a fixture that passed (carries run output for `pass` fixtures).
#[derive(Clone, Debug)]
pub struct Passed {
    /// Present for `Pass` fixtures (the binary ran); `None` for matched `CompileFail`.
    pub run: Option<RunOutcome>,
}

/// The result of executing a compiled `pass` binary.
#[derive(Clone, Debug)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub stdout: String,
    pub stderr: String,
    pub warnings: String,
}

/// A portable, typed, serializable view of a process exit (vs the opaque,
/// non-constructible `std::process::ExitStatus`). Answers #12.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    /// Exited 0.
    Success,
    /// Exited with a non-zero code.
    Exited(i32),
    /// Terminated by a signal (Unix).
    Signaled(i32),
    /// Neither a code nor a signal was available.
    Unknown,
}
```

**Source-anchored answers to your specific questions:**

- **#10 `ShouldHaveFailed` payload.** You asked why not `{ status, stdout, stderr, warnings }`. Source truth: a `compile_fail` that *compiles* is **never executed** (run.rs:443-447 reports and returns; there is no `run_test`). So there is no runtime `status`/`stderr` — they don't exist on this path. What *does* exist is the build's `stdout` (`build_stdout` param) and the normalized diagnostics (`preferred`, run.rs:446). So the faithful "full" payload is `{ stdout, diagnostics }` — not a shortcut, but the complete set the engine produces here. (If you want a runtime status, that requires *running* a should-have-failed binary — new behavior, and semantically odd.)
- **#12 `RunFailed` payload + custom `RunStatus`.** Yes — designed above. `RunOutcome` carries the full `{ status, stdout, stderr, warnings }`, and `RunStatus` is our own typed enum instead of `std::process::ExitStatus` (which isn't constructible/serializable and hides code-vs-signal). Source: `output: Output` at run.rs:423 (build_stdout spliced in at :424), rendered by `message::output` (message.rs:145).
- **#9 `Mismatch.stderr_path` — recomputation.** `stderr_path = self.path.with_extension("stderr")` (run.rs:450). So for the **expected** file it's trivially recomputable from `TestReport.path` — carrying it is convenience, not necessity. **But** the *wip* path is `wip/<filename>` (run.rs:455-462) — a relocation **not** derivable from `path` alone. So: store `stderr_path` on `Mismatch`/`NoSnapshot` (the expected-file path; convenient + lets STS bless without re-deriving), and treat any wip path as engine-owned (only relevant to `bless`, §5). That's the precise answer to "are there scenarios where path needs recomputation?" — expected: no; wip: yes, and it's not on `path`.
- **#11 `BuildFailed` vs #13 split.** Yes, #13 answers #11: the verdict half of the overloaded `CargoFail` (run.rs:420, a `pass` that didn't compile) becomes `TestStatus::BuildFailed`; the infrastructure half (cargo.rs:105, a cargo invocation returning non-zero) becomes a `SetupError` (§4). `diagnostics` here = `preferred` (run.rs:419 `failed_to_build`).

**Open question B3 (the real one):** does `Passed` carry payload, and should payload differ by expectation? A `Pass` success has a `RunOutcome`; a `CompileFail` success has matched diagnostics (== `expected`). Options: (i) `Passed { run: Option<RunOutcome> }` as above (flat, `None` for compile_fail); (ii) two variants `PassedRun(RunOutcome)` / `PassedMatch { matched: String }`; (iii) `Passed` is a unit (drop success detail entirely). Strict-typing leans (ii) (no `Option` that's "always None for one polarity"); ergonomics lean (i); minimalism leans (iii). Your call.

**Open question B4:** flat `TestStatus` (all polarities' outcomes in one enum, with `expected` on `TestReport` telling you the polarity) vs a nested `enum TestStatus { Passed, Failed(Failure) }` vs expectation-parameterized status. Flat is simplest to match; nested separates pass/fail cleanly.

---

## 4. Layer C — the SetupError model (#14/#15/#16/#17)

`try_run` / `bless` return `Result<Report, SetupError>`. Per your answers: a new public enum (#14), **near-1:1** with the 14 infra variants (#15), **typed context everywhere incl. recovered paths** (#16).

Mapping the 14 current infra variants (error.rs:8, classification from the census) → `SetupError`, with the typed payload and whether **new plumbing** is needed to recover context that is currently lost:

| `Error` variant (today) | → `SetupError` variant | Typed payload | New plumbing? |
|---|---|---|---|
| `Cargo(io::Error)` | `CargoSpawn` | `{ source: io::Error }` (+ which cmd) | carry the command (cargo.rs:103…) |
| `CargoFail` (infra half, cargo.rs:105) | `DependencyBuild` | `{ stderr: String }` | capture child stderr |
| `Metadata(serde_json::Error)` | `Metadata` | `{ source, stderr: String }` | capture `output.stderr` (cargo.rs:187, currently printed then lost) |
| `GetManifest(PathBuf, Box<Error>)` | `Manifest` | `{ path, source: ManifestError }` | replace `Box<Error>` with a **typed** inner enum (io vs toml) |
| `NoWorkspaceManifest` | `WorkspaceEdition` | `{ manifest: PathBuf }` | thread the manifest path (run.rs:217) |
| `Io(io::Error)` | `Io` | `{ path: PathBuf, source }` | **recover lost paths** — bare `?`/`fs::*` sites (run.rs:152,186,…) must `.map_err` with the path |
| `Open(PathBuf, io::Error)` | `Open` | `{ path, source }` | none (already carries path, run.rs:506) |
| `ReadStderr(io::Error)` | `ReadSnapshot` | `{ path: PathBuf, source }` | thread `stderr_path` (run.rs:478) |
| `WriteStderr(io::Error)` | `WriteSnapshot` | `{ path: PathBuf, source }` | thread the wip/stderr path (run.rs:464/469/493) |
| `Glob(GlobError)` | `Glob` | `{ pattern: String, source }` | thread the pattern (expand.rs:72) |
| `Pattern(PatternError)` | `Pattern` | `{ pattern: String, source }` | thread the pattern |
| `TomlDe(toml::de::Error)` | `TomlParse` | `{ path: Option<PathBuf>, source }` | thread the manifest path where known |
| `TomlSer(toml::ser::Error)` | `TomlEmit` | `{ source }` | none |
| `ProjectDir` | `ProjectDir` | `{}` (unit) | none |
| `UpdateVar(OsString)` | — | (drops out: env parsing only matters to the legacy path; see #26) | n/a |

```rust
/// A failure that aborts the whole run before per-test results exist.
/// `#[non_exhaustive]` — likely to grow, and STS treats it as one "infrastructure" tier.
#[non_exhaustive]
#[derive(Debug)]
pub enum SetupError {
    CargoSpawn { source: std::io::Error },
    DependencyBuild { stderr: String },
    Metadata { source: serde_json::Error, stderr: String },
    Manifest { path: PathBuf, source: ManifestError },
    WorkspaceEdition { manifest: PathBuf },
    Io { path: PathBuf, source: std::io::Error },
    Open { path: PathBuf, source: std::io::Error },
    ReadSnapshot { path: PathBuf, source: std::io::Error },
    WriteSnapshot { path: PathBuf, source: std::io::Error },
    Glob { pattern: String, source: glob::GlobError },
    Pattern { pattern: String, source: glob::PatternError },
    TomlParse { path: Option<PathBuf>, source: toml::de::Error },
    TomlEmit { source: toml::ser::Error },
    ProjectDir,
}

/// Typed replacement for the current `GetManifest(PathBuf, Box<Error>)`.
#[derive(Debug)]
pub enum ManifestError { Read(std::io::Error), Parse(toml::de::Error) }
```

**Cost note (not hidden):** "typed context everywhere + recover lost paths" (#16) is the biggest non-type chunk of work here — the lost-path recovery means editing the bare `?`/`fs::` call sites (run.rs:152/186/192/456/458 …) to attach paths. It's mechanical but spreads across `run.rs`, `cargo.rs`, `dependencies.rs`. Worth it for fidelity; just sizing it honestly.

**#17 — `already_printed()` fate (the detail you asked for).** Today (error.rs:65) it returns `true` for exactly `CargoFail | Mismatch | RunFailed | ShouldNotHaveCompiled`. Its sole job: the legacy reporter (`message::test_fail`/`prepare_fail`, message.rs:17/29) **skips** the generic `ERROR: {e}` line for those four, because their rich detail was already printed at the failure site. **Under this architecture those four become `TestStatus`, not `Error`** — they leave the error type entirely. So none of the remaining `SetupError` variants are "already printed" (they're all generic-printed by the legacy path). Therefore `already_printed` becomes **vestigial and should be removed**, with the legacy reporter (§5) printing all `SetupError`s uniformly. **Open question C1:** confirm removal (my lean) vs keep as a no-op.

---

## 5. Layer D — entry points & the non-panic constraint (#18–#22)

This is behavioral but it *binds the types*, so it's in scope for the review.

```rust
impl TestCases {
    /// Run all registered fixtures, returning structured results. Never panics; never
    /// writes snapshots; never prints. `Err` only for run-aborting setup failures.
    pub fn try_run(&self) -> Result<Report, SetupError>;

    /// Run, and additionally create/overwrite snapshots per `mode`. Same return type.
    pub fn bless(&self, mode: Update) -> Result<Report, SetupError>;
}
```

- **#18 extraction:** make `check_*` **pure** (return structured data; no `message::*`); a separate reporter renders for the legacy `Drop` path. `try_run` becomes silent for free.
- **#19 streaming:** preserve the legacy path's streamed "test X … ok" output via an observer/callback threaded through the pure core (so `try_run` passes a no-op observer and the legacy path passes a printing one).
- **#20 double-run guard (detail you asked for).** `TestCases` is `{ runner: RefCell<Runner> }`
  + `impl RefUnwindSafe` (lib.rs:291/336); `Drop` calls `runner.borrow_mut().run()` (lib.rs:342). Two mechanisms:
  - **(a) `Runner { …, has_run: bool }`** — set inside the run core; `Drop` does `if !self.runner.borrow().has_run { … }`. *Cohesive (state lives with the runner)*, but `Drop` takes a `RefCell` borrow.
  - **(b) `TestCases { …, has_run: Cell<bool> }`** — `Drop` does `if !self.has_run.get() { … }`. *`Cell` is `Copy`/infallible, so no borrow in `Drop`* — marginally safer under the panic-free policy (a stray conflicting borrow in `Drop` can't arise). Lean: **(b)** for the Drop-safety property; either is fine. Confirm.
- **#21/#22 — "keep Drop, make it not panic" (the hard one).** Surfacing the std constraint plainly: **a stock `#[test]` can only fail by panicking or by returning `Err` — and `Drop` can do neither.** So "Drop must not panic" cannot mean "Drop still fails the test by some other in-`Drop` mechanism" (the only alternatives are `process::exit`, which is nuclear and `deny`-listed, or nothing). The only coherent readings:
  - **(i) Drop becomes render-only / no-op** (guarded by #20). Failures surface **only** through `try_run()?` / the STS `ensure_compile_fail` façade — which is exactly the intended consumer. Consequence to ratify: a suite written the *old* bare way (`t.compile_fail(…)`; rely on `Drop`) **no longer fails CI** on a mismatch — by design, because verdicts now flow through `Result`. This is a semantic change to the legacy contract.
  - **(ii) Keep `Drop` panicking for the legacy bare pattern**, and "not panic" applies only to the new `try_run`/`bless` path (which already never panics). I.e. you keep both: bare usage still panics on failure (back-compat), explicit usage is panic-free. I will **not** assume which you mean. My read of your STS direction is **(i)** (Result-based world, façade owns failure), but (ii) is the smaller, fully back-compatible change. This is **Open question D1** and it gates the whole entry-point design.

**Open question D2:** does `bless` return the same `Report` (with `Updated`/`Wip` statuses) — and if so, `TestStatus` needs `Updated { mode: Update }` (your #8) and a `Wip`-vs-in-place distinction that `try_run` (read-only) never produces. I.e., the **status set is conditional on the entry point**: `try_run` yields the §3.3 set; `bless` additionally yields `Updated`. Confirm whether `bless` reports via the same `TestStatus` (then it gains `Updated { mode }`) or its own type.

---

## 6. `Update` and env (#25/#26)

```rust
/// How `bless` writes snapshots. Exposed as-is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Update { #[default] Wip, Overwrite }
```

Per #26, the new API is **explicit-only**: neither `try_run` nor `bless` reads `TRYBUILD` (env.rs:12). The legacy `Drop` path also gets a programmatic lever rather than reading env — **Open question E1:** confirm the legacy path drops `TRYBUILD` entirely (a behavior change for existing users who rely on `TRYBUILD=overwrite cargo test`), or retains env reading *only* for the legacy path. (Your #26 says "does not read env" — I read that as dropping it everywhere; confirming because it changes a documented workflow in `AGENTS.md:23`.)

---

## 7. The strict-conformance workstream (sized, not buried)

Independent of the type graph, the fork's `[workspace.lints]` are already strict but the code isn't conformed. Confronting #32–34 honestly:

- **`missing_docs` + `missing_docs_in_private_items`**: every item, public and private, needs a doc comment. This is the largest mechanical cost (hundreds of private items across `run.rs`, `normalize.rs`, `cargo.rs`, `dependencies.rs`, …).
- **`unwrap_used` / `expect_used` / `indexing_slicing` / `panic`**: real removals in `normalize.rs` (5), `rustflags.rs`, `expand.rs` (`self.vec[i]`, expand.rs:51), the `Drop` panics (handled by #21), etc.
- **`print_stdout` / `print_stderr`**: likely a **non-issue** — trybuild's console layer uses *custom* `term::print!`/`println!` macros (term.rs) that expand to `write!(termcolor_stream)`, **not** `std::print!`/`eprintln!`, so the clippy lints (which target the std macros) should not fire. **To verify** before relying on it; if a few genuine `std` prints exist (lib.rs counted 2), they're isolated. So the feared "strict policy kills the reporter" conflict is probably small — but I'm flagging it as *to-verify*, not asserting it.

**Open question F1:** is crate-wide strict conformance part of *this* effort (same branch, same review cycle) or a separate pass that lands first/after? It's a big enough chunk that sequencing matters.

---

## 8. Consolidated open questions (for your review — each is a real fork, not a binary strawman)

- **A1** how far to widen `Expectation` (and pay each case's engine cost); **A2** name.
- **B1** include positional `TestName` (`trybuildNNN`) or omit; **B2** `Summary` method vs stored field; **B3** does `Passed` carry payload, and does it differ by polarity (`Option<RunOutcome>` vs split variants vs unit); **B4** flat vs nested `TestStatus`.
- **C1** remove vestigial `already_printed`.
- **D1** the non-panic `Drop` semantics — (i) render-only/no-op (failures only via `Result`) vs (ii) keep legacy panic, panic-free only on the new path. **Gates the entry-point design.** **D2** does `bless` report via the same `TestStatus` (adding `Updated { mode }`)?
- **E1** drop `TRYBUILD` env everywhere vs retain for legacy only.
- **F1** is crate-wide strict conformance in-scope here or a separate sequenced pass?

Plus: `RunStatus` shape (signal handling / serializability), and whether `SetupError`'s lost-path recovery (#16) is worth its spread-out cost — both detailed in §3.3/§4.
