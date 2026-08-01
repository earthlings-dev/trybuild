# Type Architecture — trybuild `strict` fork (v3)

> Status: **review draft.** A holistic design of the public type graph for the panic-free, structured, data-returning core, plus the entry points and the strict-policy constraints it must satisfy. Source references are against the `strict` branch.

---

## 0. Constraints the design must satisfy

From the workspace `Cargo.toml` (`[workspace.lints]`): edition **2024**, rust-version **1.96**, `unsafe_code = "forbid"`, and `deny` on `panic`, `panic_in_result_fn`, `unwrap_used`, `expect_used`, `indexing_slicing`, `print_stdout`, `print_stderr`, `missing_docs`, `missing_docs_in_private_items`, `missing_debug_implementations`, plus the full rust-template clippy set.

Therefore every type here: derives `Debug`; documents every item (public *and* private); has no panicking path; prefers owned, typed fields over stringly data; is `#[non_exhaustive]` where it is expected to grow, and exhaustive where consumers must match every case.

---

## 0. The four layers (holistic view)

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

## 1. The testing model this serves

Consumers test through `strict-test-support`: a test is `#[test] fn ui() -> Result<(), TestFailure>` and **reports failure by returning `Err`**, using the `ensure*` vocabulary (no panics, no assertion macros). Every capability is a function returning `Result<(), TestFailure>` — e.g. `ensure_property(...)` (proptest) and `ensure_snapshot(actual, path, context)` (snapbox), the latter blessing via `SNAPSHOTS=overwrite`.

This fork supplies the **data-returning core** that an STS façade wraps:

- trybuild exposes `TestCases::try_run() -> Result<Report, SetupError>` — runs all fixtures, returns a fully structured `Report`, never panics, never prints.
- STS adds a feature-gated `ensure_compile_fail(cases, context) -> Result<(), TestFailure>` that calls `try_run`, maps the `Report` to `Ok(())` / `Err(TestFailure)`, exactly as `ensure_snapshot` wraps snapbox. Failure flows through the returned value.
- trybuild's `Drop` is panic-free (§5.4); it is a convenience for interactive use, never a correctness channel.

The public surface decomposes into four layers: **Expectation** (§2), **Result** (§3), **SetupError** (§4), **Entry points** (§5).

---

## 2. Layer A — Expectation

Authoring is by verb: the `Expectation` type never appears on the input side; it is reified only on the result (`TestReport.expected`, §3.1), where the field name `expected` reads as "what was expected." Each mode has a registration verb on `TestCases`:

| Verb | Expectation |
|---|---|
| `pass(path)` | `Pass` |
| `compile_fail(path)` | `CompileFail` |
| `pass_with_warnings(path)` | `PassWithWarnings` |
| `run_matches(path, RunExpectation)` | `RunMatches(RunExpectation)` |
| `expand(path)` | `Expand` |

The engine verifies the first two today (dispatch at run.rs:394): `Pass` must compile then run without failure (run.rs:409); `CompileFail` must fail to compile with diagnostics matching its `.stderr` (run.rs:433). The other three are **in scope for this fork**; each adds engine behavior (§2.1).

```rust
/// What a fixture is expected to do. `#[non_exhaustive]` to allow future modes without breakage.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum Expectation {
    /// Compile, then run the binary; it must not fail.
    Pass,
    /// Fail to compile; diagnostics must match the `.stderr` snapshot.
    CompileFail,
    /// Compile with warnings matching the `.warnings` snapshot, then run; it must not fail.
    PassWithWarnings,
    /// Compile and run; the run must match the committed runtime snapshots and `exit`.
    RunMatches(RunExpectation),
    /// Expand macros; the expansion must match the `.rs.expanded` snapshot.
    Expand,
}

/// What `RunMatches` asserts about the executed binary.
#[derive(Clone, Debug)]
pub struct RunExpectation {
    /// Explicit, typed set of runtime streams this fixture asserts (`RunStream::ALL` for all).
    /// Each selected stream is compared against its `.run.<stream>` snapshot; a stream that
    /// produced no output compares against an empty snapshot.
    pub streams: &'static [RunStream],
    /// Required exit, mirroring `RunStatus` (§3.3).
    pub exit: ExpectedExit,
}

/// A runtime output stream a `RunMatches` fixture can assert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStream {
    /// Runtime stdout (`.run.stdout`).
    Stdout,
    /// Runtime stderr (`.run.stderr`).
    Stderr,
}

impl RunStream {
    /// Both runtime streams.
    pub const ALL: &'static [RunStream] = &[RunStream::Stdout, RunStream::Stderr];
}

/// The exit a `RunMatches` binary must produce. Mirrors `RunStatus` (§3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpectedExit {
    /// Exited successfully (0).
    Success,
    /// Exited with this exact code.
    Code(i32),
    /// Any non-success exit (non-zero or signal).
    Failure,
    /// Exit not asserted.
    Any,
}
```

### 2.1 Engine work each new mode adds (in scope)

| Mode | Snapshot file | Captured artifact (source) | New engine behavior |
|---|---|---|---|
| `PassWithWarnings` | `<fixture>.warnings` | build warnings already captured (run.rs:446) | assert build warnings against the snapshot on a successful build |
| `RunMatches` | `<fixture>.run.stdout`, `<fixture>.run.stderr` | the run `Output` (run.rs:423) | compare runtime stdout/stderr + exit instead of only "ran without failure" (run.rs:423-429) |
| `Expand` | `<fixture>.rs.expanded` | none today | shell out to the installed `cargo expand`; needs a nightly toolchain at run time |

All snapshot comparisons reuse the existing normalization + variation machinery (normalize.rs) and the same blessing path as `.stderr` (§5, §6).

`Expand` is **not** feature-gated — `expand()` / `Expectation::Expand` are always present. `cargo expand` is provisioned as an installed tool in the `cargo stask init` bootstrap (not a linked dependency), and its nightly requirement is pay-per-use: incurred only when an `Expand` fixture actually runs. A missing toolchain or tool surfaces as `TestStatus::ExpandFailed`.

---

## 3. Layer B — Result

### 3.1 Shape

`Report` owns the complete per-test data (the source of truth) and derives all summaries. Fixtures are grouped by the pattern that registered them; each row is a concrete expanded fixture.

```rust
/// The outcome of a whole run. Owns per-test data; summaries are derived.
#[derive(Clone, Debug)]
pub struct Report {
    /// One entry per `pass`/`compile_fail` registration, in registration order.
    pub groups: Vec<PatternGroup>,
}

/// All fixtures expanded from a single registered pattern.
#[derive(Clone, Debug)]
pub struct PatternGroup {
    /// The path/glob exactly as registered (`compile_fail("tests/ui/*.rs")`).
    pub pattern: PathBuf,
    /// Expanded concrete fixtures, sorted (glob results are sorted, expand.rs:74).
    pub tests: Vec<TestReport>,
}

/// One fixture's result.
#[derive(Clone, Debug)]
pub struct TestReport {
    /// Expanded concrete fixture path.
    pub path: PathBuf,
    /// Synthesized `[[bin]]` name (`trybuildNNN`, expand.rs:59) — positional, correlates to
    /// cargo's per-bin JSON; not semantic.
    pub name: TestName,
    /// The declared expectation.
    pub expected: Expectation,
    /// What actually happened.
    pub status: TestStatus,
}

/// The synthesized bin name trybuild assigns each fixture.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TestName(pub String);
```

Grouping requires retaining the originating registered pattern through expansion: `expand_globs` (expand.rs:15) currently keeps only `is_from_glob: bool` (expand.rs:12), so the pattern is threaded into `ExpandedTest` as an additive internal change.

### 3.2 Derived summary

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
    pub fn summary(&self) -> Summary;                       // fold over groups
    pub fn is_ok(&self) -> bool;                            // failed == 0
    pub fn tests(&self) -> impl Iterator<Item = &TestReport>;   // flatten groups
    pub fn failures(&self) -> impl Iterator<Item = &TestReport>;
}
```

### 3.3 Per-test status

`try_run` is read-only, so statuses never include snapshot writes; a fixture whose required snapshot is absent on disk is an explicit `NoSnapshot`, not a silent wip write. Payloads carry exactly what the engine produces on each path (anchored below). The enum is flat and exhaustive; `TestReport.expected` supplies the mode, so a status need not re-encode it.

```rust
/// What happened to one fixture. Flat and exhaustive (not `#[non_exhaustive]`).
#[derive(Clone, Debug)]
pub enum TestStatus {
    /// Compiled and ran successfully (`Pass` / `PassWithWarnings` / `RunMatches`).
    PassedRun(RunOutcome),
    /// Met a snapshot-only expectation (`CompileFail` diagnostics / `Expand` expansion).
    PassedMatch { kind: SnapshotKind, matched: String },
    /// A committed snapshot did not match the captured artifact.
    Mismatch { kind: SnapshotKind, expected: String, actual: String, snapshot_path: PathBuf },
    /// A required snapshot is absent on disk (read-only `try_run`).
    NoSnapshot { kind: SnapshotKind, actual: String, snapshot_path: PathBuf },
    /// `CompileFail` that compiled (run.rs:443-447): no execution; carries build stdout + diagnostics.
    ShouldHaveFailed { stdout: String, diagnostics: String },
    /// A build-expecting mode that failed to compile (run.rs:418-420).
    BuildFailed { diagnostics: String },
    /// A run-expecting mode whose binary failed or missed its required exit (run.rs:423-429).
    RunFailed(RunOutcome),
    /// An `Expand` fixture that could not be expanded (cargo-expand / compiler error).
    ExpandFailed { diagnostics: String },
    /// Produced only by `bless` (never by `try_run`): a snapshot was created or overwritten.
    Updated { mode: Update },
}

/// Which committed snapshot a `Mismatch` / `NoSnapshot` / `PassedMatch` refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotKind {
    /// Compile diagnostics (`.stderr`).
    Diagnostics,
    /// Build-time compiler warnings (`.warnings`).
    Warnings,
    /// Build stdout (`.stdout`).
    Stdout,
    /// Runtime stdout (`.run.stdout`).
    RunStdout,
    /// Runtime stderr (`.run.stderr`).
    RunStderr,
    /// Macro expansion (`.rs.expanded`).
    Expanded,
}
```

The split `PassedRun` / `PassedMatch` avoids an always-`None` field, and `SnapshotKind` keeps mismatches to a single variant rather than one per artifact.

Where the payload types are:

```rust
/// The result of executing a compiled binary.
#[derive(Clone, Debug)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub stdout: String,
    pub stderr: String,
    pub warnings: String,
}

/// A portable, typed, serializable view of a process exit (vs the opaque,
/// non-constructible `std::process::ExitStatus`).
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

Anchors and derivations:

- **`RunStatus`** is derived from `std::process::ExitStatus` (today only `.success()` is read — cargo.rs:104/114, message.rs:146, run.rs:426): `success() → Success`; else `code() → Exited(c)` (always `Some` on Windows); else, under `#[cfg(unix)]`, `ExitStatusExt::signal() → Signaled(sig)`; else `Unknown`. Unlike `ExitStatus` it is `Copy`, comparable, and serializable.
- **`snapshot_path`** for `Diagnostics` is `path.with_extension("stderr")` (run.rs:450) — derivable from `path`, carried so a consumer can locate/bless without re-deriving; the other `SnapshotKind`s follow the same adjacent-file convention (§2.1). The engine-owned wip path (`wip/<file>`, run.rs:455-462) is relevant only to `bless`.
- **`actual`** is the preferred normalization variation (`Variations::preferred`, normalize.rs:98); a match is sought across *all* variations (run.rs:481), so `Mismatch`/`NoSnapshot` means none matched.

---

## 4. Layer C — SetupError

`try_run` / `bless` return `Result<Report, SetupError>`. Setup failures abort the whole run before per-test results exist. `SetupError` is a new public enum, near-1:1 with the engine's 14 internal infrastructure failure points, carrying typed context everywhere — including paths recovered at the call sites that currently drop them.

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

Context-recovery sites (where the engine currently loses a path and the typed variant threads it):
- bare `fs::` `?` → path-less today: run.rs:152 (`project_dir`), :186 (`<dir>/Cargo.toml`), :192 (`<dir>/main.rs`), :456 (`wip_dir`), :458 (`gitignore_path`).
- typed but path-less today: run.rs:464/469/493 (`WriteSnapshot`), :477 (`ReadSnapshot`).
- already complete: run.rs:504/506 (`Open` carries its path).
- `Metadata` captures `output.stderr` (cargo.rs:187); `Manifest` replaces the engine's `Box`-of-error with the typed `ManifestError` (dependencies.rs:23).

The four engine verdicts (`compile_fail`/build/run outcomes) are **not** errors here — they are `TestStatus` (§3.3). Consequently the engine's `already_printed` discriminator (which today marks those four) has no role and is removed.

The blessing mode (`Update`) is the only thing the engine reads from the environment today (`Update::env()` at run.rs:173). The new API does not read it; see §6.

---

## 5. Layer D — Entry points

```rust
impl TestCases {
    /// Run all registered fixtures and return structured results. Never panics, never writes snapshots, never prints.
    /// `Err` only for run-aborting setup failures.
    pub fn try_run(&self) -> Result<Report, SetupError>;

    /// Run, and additionally creating/overwriting snapshots per `mode`. Same return type.
    pub fn bless(&self, mode: Update) -> Result<Report, SetupError>;
}
```

### 5.1 Pure core, rendered separately

The check logic (`check_pass`/`check_compile_fail`/`run_all`) becomes pure: it returns structured data and performs no `message::*` output. `try_run`/`bless` build the `Report` from it silently. The legacy `Drop` path (§5.4) renders that data through the existing reporter.

### 5.2 Streamed legacy rendering

The legacy reporter's streamed "test X … ok" output is preserved by threading an observer callback through the pure core: `try_run`/`bless` pass a no-op observer; the legacy `Drop` path passes a printing one.

### 5.3 Console output and the strict lints

The reporter writes through crate-local `print!`/`println!` macros (term.rs:30-43) that expand to `std::write!` on a `termcolor` stream (stderr) — not the std `print!`/`eprintln!` macros, so `clippy::print_stdout`/`print_stderr` do not apply (there are no std prints in the crate). The macro bodies' `let _ = write!(…)` discards are reworked to satisfy `let_underscore_must_use` / `let_underscore_untyped` (part of §7).

### 5.4 `Drop`

`Drop` is panic-free. A `has_run` guard prevents a second run when the suite was already executed explicitly:

```rust
pub struct TestCases { /* … */ has_run: Cell<bool> }   // Cell: Drop reads it with no RefCell borrow
```

`try_run`/`bless` set `has_run`; `Drop` runs only if it is unset. Because every consumer drives the suite through `try_run` / `ensure_compile_fail`, `Drop` is, in practice, a no-op. When the suite was *not* run explicitly, `Drop` runs once and renders the report but never fails (render-only). trybuild already runs-and-reports without panicking for its own fixtures (the `"{crate}-tests"` path, run.rs:101/154); the panic-free `Drop` generalizes that.

---

## 6. STS integration (the consuming half)

In `strict-test-support`, behind a `trybuild` feature, mirroring `ensure_snapshot`:

```rust
pub fn ensure_compile_fail(cases: &trybuild::TestCases, context: &'static str)
    -> Result<(), TestFailure>;
```

It calls `cases.try_run()`, returns `Ok(())` when `report.is_ok()`, otherwise an `Err(TestFailure)` naming the failing fixtures and their statuses; `SetupError` maps into `TestFailure`'s source chain. Blessing parallels `ensure_snapshot`'s `SNAPSHOTS=overwrite`: the façade calls `cases.bless(mode)`. This mapping lives in `strict-test-support` (separate repo); this fork provides only `try_run`/`bless`/`Report`/`TestStatus`/`SetupError`.

---

## 7. Strict-conformance workstream

The workspace lints are in force but the crate body is not yet conformed. This is a parallel effort to the type graph: remove `unwrap`/`expect`/`indexing` (e.g. normalize.rs, rustflags.rs, `self.vec[i]` at expand.rs:51), eliminate the `Drop` panics (subsumed by §5.4), rework the `let _ =` discards (term.rs, cargo.rs:90), and document every public *and* private item. The print lints are not a factor (§5.3). This conformance is part of the same effort as the type changes.
