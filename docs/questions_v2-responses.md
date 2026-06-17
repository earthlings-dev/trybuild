### AREA A — Public surface architecture
- You're getting sneaky with how you're presenting this. Nothing is decided, even with my answers state below. We are reviewing the type architecture comprehensively, separate from this railroad and strawman-laden bullshit question and answer report from you.

**1. Reuse trybuild's internal `Error`, or build purpose-made public types?**
- *What:* `Error` (error.rs:8) and `Result` (error.rs:29) are `pub(crate)`. Either expose them or introduce a new public type the internals map into. **2. Per-test result representation.**
- *What:* how one fixture's result is typed. Internally it's `Result<Outcome, Error>` (Outcome at run.rs:365).

### AREA B — Report & test identity

**3. `Report` — counts as stored fields or derived methods?**
- *What:* current `Report { failures, created_wip }` (run.rs:44) is counts only; new `Report` carries per-test data.
- Both options you gave seem like bullshit. Why would report not contain fully typed, derived information of all of the underlying tests?

**4. `TestReport` identity fields.**
- *What:* how a result row identifies its fixture. Each test has `path: PathBuf` (lib.rs:303) and an internal derived `Name` (synthesized bin name).
  - `path` + `name: String`. derived name can indicate something that might be useful as well

**5. `TestReport.path` — which path, and the type.**
- *What:* `expand_globs` (run.rs:51) turns a registered glob into concrete fixture paths; do we report the expanded concrete path or the original pattern?
- expanded concrete `PathBuf` per fixture, *grouped by pattern string*

**6. Expose `Expected`, and its name.**
- *What:* `enum Expected { Pass, CompileFail }` (lib.rs:307), private; used to convey intended polarity in `TestReport`.
- Both options seem like strawmen.  Why would expected be restricted to Pass, CompileFail?

### AREA C — Per-test status & payloads

> **Cross-cut:** the exact case set here depends on **#21** (blessing). I present the cases assuming **read-only `try_run`** (my #21 rec); if you choose mutating, `NoExpected` is replaced by `Wip`/`Blessed` as noted in #7.

**7. The `TestStatus` case set.**
- *What:* enumerate every per-test outcome, source-anchored.
  - **Read-only model:** `Passed` · `Mismatch{…}` · `ShouldHaveFailed{…}` · `BuildFailed{…}` · `RunFailed{…}` · `NoExpected{actual}` (a compile_fail with no `.stderr` on disk — run.rs:452 — becomes an explicit indeterminate failure rather than silently writing wip). *Pro:* pure, deterministic, no file writes during assertion. *Con:* blessing is a separate call (#21/#22).

**8. `Wip` vs `Blessed` — two variants or one `Updated{mode}`?** *(only relevant if #21 = mutating)*
- `Updated { mode: Update }`. *Pro:* DRY, ties to the `Update` enum.

**9. `Mismatch` payload.**
- *What:* compile_fail produced output ≠ the stored `.stderr`. Available: `expected` (read at run.rs:477), `actual`/`preferred` (run.rs:441), `stderr_path` (run.rs:450). Printed today by `message::mismatch(expected, actual)` (message.rs:118) then discarded.
- `{ expected, actual, stderr_path: PathBuf }`, which lets STS report *which* file and drive blessing without recomputing the path.
- Are there no scenarios where path needs to be recomputed?

**10. `ShouldHaveFailed` payload.**
- *What:* a `compile_fail` fixture that compiled (run.rs:443-447). Available: `stdout` (build_stdout param) and `warnings`/`preferred` (run.rs:446 `warnings(preferred)`); printed via `should_not_have_compiled` + `fail_output` + `warnings`.
- Why would we not give a fully typed payload like `{ status, stdout, stderr, warnings }`? Your `{ stdout: String, warnings: String }` recommendation seems like a shortcut.

**11. `BuildFailed` payload (the `pass`-test-failed-to-compile case).**
- *What:* a `pass` fixture that failed to compile (run.rs:418-420); `preferred` is the compiler stderr (printed via `failed_to_build`).
- Doesn't #13 answer this?

**12. `RunFailed` payload + exit-status type.**
- *What:* a `pass` fixture compiled & ran but exited nonzero/panicked (run.rs:423-429). Available: `output: Output` (status/stdout/stderr; build_stdout spliced in at run.rs:424) and `warnings`/`preferred`; printed via `output(preferred, &output)` (message.rs:145).
- *Options (payload):* `{ status, stdout, stderr, warnings }`
- *Options (status type):* Why don't we create our own fully typed RunStatus?

**13. Split the overloaded `CargoFail`.**
- *What:* `CargoFail` means two unrelated things — a pass-test that failed to build (run.rs:420, a verdict, stderr available) and a cargo invocation returning nonzero during dependency build (cargo.rs:105, pure infra).
- split → verdict becomes `TestStatus::BuildFailed{stderr}` ; infra case becomes a `SetupError`

### AREA D — Outer (run-aborting) error

**14. `try_run`'s outer error type.**
- *What:* the 14 infra variants abort the whole run (prepare/lock/write/metadata/glob). What does `try_run -> Result<Report, ???>` use?
-  a new `pub enum SetupError`

**15. `SetupError` granularity.**
- *What:* how many cases the public setup error has.
- near-1:1 with the 14 variants

**16. Context carried by `SetupError` cases.**
- *What:* whether cases carry `PathBuf`/source detail (e.g., which manifest, which IO path — noting the census found `Io` currently *loses* paths except `Open` at run.rs:506).
- Typed context everywhere, not just where cheap. Add new plumbing to recover lost paths.

**17. `already_printed()` fate.**
- *What:* error.rs:65 marks the 4 verdict errors so the generic printer skips them; used only by the Drop/legacy path.
- More detail needed.

### AREA E — run / Drop refactor

**18. Silent-core extraction approach.**
- *What:* printing is interleaved through `check_pass`/`check_compile_fail`/`run_all` (run.rs:419,425,444-492). To make `try_run` silent + structured:
- make `check_*` **pure** (return structured data, no `message::*`); a separate reporter renders for the legacy path. *Pro:* `try_run` silent for free; cleanest separation. *Con:* largest internal refactor

**19. Legacy console output: streamed vs batched.**
- *What:* approach (a) renders after the run, so the legacy path's "test X … ok" lines batch instead of streaming mid-run.
- **preserve streaming** via an observer/callback threaded through the core.

**20. Double-run guard — mechanism & location.**
- *What:* no guard exists; calling `try_run()` then dropping would run twice.
- *More detail needed for a & b:* (a) `Runner { …, already_run: bool }`, `Drop` checks it. *Pro:* uses existing `RefCell<Runner>`. (b) `Cell<bool>` on `TestCases`.

**21. Keep `Drop` auto-running?**
- *What:* lib.rs:339 runs tests on drop — the current ergonomic contract.
- We are keeping it, and enhancing it so it does not panic.

**22. Legacy `run()` exactness.**
- *What:* whether the legacy path stays behaviorally identical: panic messages (run.rs:62/102/105) and the `project.name != "trybuild-tests"` self-test suppression (run.rs:101).
- *Do not simplify:* We will perform a nuanced enhancement so that it does not panic.

### AREA F — Blessing

**23. Blessing model — read-only `try_run` vs mutating.**
- *What:* blessing writes `.stderr`/wip files (run.rs:464/469/493). Does the structured assertion path perform writes?
- **read-only** `try_run`; a separate explicit `bless`/update entry writes. *Pro:* deterministic, side-effect-free assertions; maps cleanly onto a `snap-update`-style command; an STS `ensure_compile_fail` never mutates the tree.

**24. Blessing API shape.**
- *What:* how the caller requests/performs blessing.
- separate `fn bless(&self, mode: Update) -> Result<Report, SetupError>` (read-only `try_run` stays pure).

**25. Expose `Update`, its name and variants.**
- *What:* `enum Update { Wip(default), Overwrite }` (env.rs:5), private.
- `pub` as-is.

**26. Programmatic vs `TRYBUILD` env precedence in the new API.**
- *What:* `Update::env()` (env.rs:12) reads the `TRYBUILD` env var. Does the new structured API read env at all?
- new API is **explicit-only** — `try_run`/`bless` ignore `TRYBUILD`; legacy Drop path also has programmatic levers and does not read env.

### AREA G — Fork metadata

**27. `Cargo.toml` `repository`.**
- `https://github.com/earthlings-dev/trybuild`.

**28. `Cargo.toml` `version`.**
- *What:* currently `1.0.116`; consumed downstream via `[patch.crates-io]` by **git rev**, so exact version isn't enforced.
- keep `1.0.116`.

**29. `Cargo.toml` `publish`.**
- Leave publish = true. We may rename and publish ourselves. Plus, when you set things to publish = false, you internally "relax" and do a worse job.

**30. The `diff` feature (dissimilar).**
- *What:* optional `diff` feature gates `dissimilar`, used only by `message::mismatch` console rendering (message.rs). The new API returns `expected`/`actual` as data; STS diffs them itself.
- enable by default

**31. MSRV (`rust-version = 1.85`) and edition (2021).**
- *What:* new code must compile on 1.85; STS/rust-template pin stable ≥ that.
- We will be raising to edition 2024 & version 1.96.

### AREA H — Lints, docs, verification

**32. Verification bar.**
- *What:* trybuild's CI runs `cargo test`; `cargo clippy --tests -- -Dclippy::all -Dclippy::pedantic`; `RUSTFLAGS="-Dwarnings"`; `cargo fmt --check`; minimal-versions `cargo check --locked`; `cargo docs-rs`.
- We will be enforcing our rust-template stricter rules

**33. Doc comments on new public items.**
- *What:* trybuild does not `deny(missing_docs)`, but the new public types are your API.
- require `///` on every new pub item. We will be enforcing our rust-template lint rules onto it as well, so there will be more docs to write than just pub items.

**34. Clippy-pedantic conformance for new code.**
- *What:* new pub structs/enums with public fields can trip pedantic lints (e.g., `must_use`, missing `#[non_exhaustive]` considerations); crate already `#![deny(clippy::clone_on_ref_ptr)]`.
- *None of your existing options:* We will be enforcing our rust-template strict rules onto it.

### AREA I — Tests

**35. Fixture structure for `try_run` tests.**
- *What:* trybuild self-tests via `tests/test.rs` + `tests/ui/*.rs`/`.stderr`, relying on `project.name == "trybuild-tests"` to suppress Drop panics (run.rs:101).
- *both:* new isolated test file + fixtures (e.g., `tests/try_run.rs` + `tests/try_run_ui/`). *Pro:* doesn't perturb the existing suite; asserts on `Report` directly. Regression coverage via reusing `tests/ui` fixtures.

**36. Status-coverage set for tests.**
- *What:* which `TestStatus`/error cases the new tests must exercise.
- *full polarity coverage is the whole point:* cover every case: `Passed`, `Mismatch`, `ShouldHaveFailed`, `BuildFailed`, `RunFailed`, `NoExpected` (or `Wip`/`Blessed` per #23), plus the double-run guard (#20) and a `SetupError` path.

**37. Interaction: tests vs the `trybuild-tests` panic-suppression.**
- *What:* new tests call `try_run` (no panic) and assert on `Report`; the #20 guard must prevent the subsequent `Drop` from re-running.
- *Options:* (a) rely on the #20 guard; tests need no special naming. (b) additionally name the test project to hit the suppression path. *Con:* unnecessary if (a) holds.
- *Rec:* (a) — and add an explicit test that `try_run` then drop runs exactly once.

### AREA J — Process

**38. Branch name.**: `strict`

**39. Commit structure.**: You are NOT permitted to make ANY commits.

**40. Scope boundary.** fork-only.