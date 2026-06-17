### AREA A — Public surface architecture

**1. Reuse trybuild's internal `Error`, or build purpose-made public types?**
- *What:* `Error` (error.rs:8) and `Result` (error.rs:29) are `pub(crate)`. Either expose them or introduce a new public type the internals map into.
- *Options:*
  - (a) Make internal `Error` `pub` and enrich it. *Pro:* least code, one type. *Con:* publishes all 18 internal variants — including infra plumbing — as your public contract; every upstream rebase can churn the public API; couples STS to trybuild internals.
  - (b) Keep internal `Error` private; add new public types (`Report`/`TestStatus`/`SetupError`) that internals map into. *Pro:* clean, decoupled, strongly-typed surface; survives rebases; lets you model exactly the structured results STS needs. *Con:* a mapping layer + more code.
- *Rec:* (b). It's the strict-typing/structured-results choice and it decouples the wrap from trybuild's guts.
- *Interacts:* governs #12–#14 (outer error) and #6–#11 (status). If you pick (a), those become "enrich each internal variant" instead of "design a public type."

**2. Per-test result representation.**
- *What:* how one fixture's result is typed. Internally it's `Result<Outcome, Error>` (Outcome at run.rs:365).
- *Options:*
  - (a) Expose `Result<Outcome, Error>` per test. *Pro:* mirrors internals. *Con:* splits a single conceptual "status" across `Ok`/`Err`; forces STS to handle two types; `Outcome`'s `CreatedWip` is a success-shaped value that isn't really "pass."
  - (b) One exhaustive `enum TestStatus` covering every outcome (pass/update/each failure). *Pro:* one `match`, no Ok/Err split, models reality precisely. *Con:* a new enum to maintain.
- *Rec:* (b).
- *Interacts:* its case set is #6/#7.

### AREA B — Report & test identity

**3. `Report` — counts as stored fields or derived methods?**
- *What:* current `Report { failures, created_wip }` (run.rs:44) is counts only; new `Report` carries per-test data.
- *Options:*
  - (a) `Report { pub tests: Vec<TestReport> }`, counts via methods (`ok()`, `failed()`, `wip()`). *Pro:* single source of truth, counts can't desync from `tests`. *Con:* O(n) to count (trivial here).
  - (b) Store `tests` **and** `failures`/`wip` as pub fields. *Pro:* O(1) reads. *Con:* duplicated state that can drift; weaker invariant.
- *Rec:* (a).

**4. `TestReport` identity fields.**
- *What:* how a result row identifies its fixture. Each test has `path: PathBuf` (lib.rs:303) and an internal derived `Name` (synthesized bin name).
- *Options:*
  - (a) `path` only. *Pro:* matches how the caller registered it (`compile_fail("tests/ui/x.rs")`); the path is the user-facing identity. *Con:* no access to the internal bin name.
  - (b) `path` + `name: String`. *Pro:* exposes the derived name. *Con:* leaks an internal artifact STS won't key on; more surface.
- *Rec:* (a).

**5. `TestReport.path` — which path, and the type.**
- *What:* `expand_globs` (run.rs:51) turns a registered glob into concrete fixture paths; do we report the expanded concrete path or the original pattern?
- *Options:* (a) expanded concrete `PathBuf` per fixture — *Pro:* one row per real file, unambiguous. (b) the original registered pattern string — *Con:* ambiguous when a glob matched many files.
- *Rec:* (a) expanded concrete `PathBuf`.

**6. Expose `Expected`, and its name.**
- *What:* `enum Expected { Pass, CompileFail }` (lib.rs:307), private; used to convey intended polarity in `TestReport`.
- *Options:* (a) make `pub` as-is. (b) rename (`Polarity`/`Mode`). (c) don't expose — let the consumer infer from status.
- *Rec:* (a) — already clean and correctly named; STS wants the declared polarity explicitly, not inferred.

### AREA C — Per-test status & payloads

> **Cross-cut:** the exact case set here depends on **#21** (blessing). I present the cases assuming **read-only `try_run`** (my #21 rec); if you choose mutating, `NoExpected` is replaced by `Wip`/`Blessed` as noted in #7.

**7. The `TestStatus` case set.**
- *What:* enumerate every per-test outcome, source-anchored.
- *Options:*
  - (a) **Read-only model:** `Passed` · `Mismatch{…}` · `ShouldHaveFailed{…}` · `BuildFailed{…}` · `RunFailed{…}` · `NoExpected{actual}` (a compile_fail with no `.stderr` on disk — run.rs:452 — becomes an explicit indeterminate failure rather than silently writing wip). *Pro:* pure, deterministic, no file writes during assertion. *Con:* blessing is a separate call (#21/#22).
  - (b) **Mutating model:** replace `NoExpected` with `Wip` and `Blessed` (run.rs:465/470/494), matching today's behavior. *Pro:* one call does check+update. *Con:* assertion path mutates the tree.
- *Rec:* (a).
- *Interacts:* #21 decides (a) vs (b).

**8. `Wip` vs `Blessed` — two variants or one `Updated{mode}`?** *(only relevant if #21 = mutating)*
- *Options:* (a) distinct `Wip` / `Blessed`. *Pro:* they mean different things downstream (wip = human must move the file; blessed = done). (b) `Updated { mode: Update }`. *Pro:* DRY, ties to the `Update` enum.
- *Rec:* (a) if mutating; moot if read-only.

**9. `Mismatch` payload.**
- *What:* compile_fail produced output ≠ the stored `.stderr`. Available: `expected` (read at run.rs:477), `actual`/`preferred` (run.rs:441), `stderr_path` (run.rs:450). Printed today by `message::mismatch(expected, actual)` (message.rs:118) then discarded.
- *Options:* (a) `{ expected: String, actual: String }`. (b) `{ expected, actual, stderr_path: PathBuf }`. (c) add a precomputed diff. *Pro/con:* (a) minimal; (b) lets STS report *which* file and drive blessing without recomputing the path; (c) duplicates STS's own diffing (snapbox already diffs) — avoid.
- *Rec:* (b) — expected + actual + `stderr_path`; STS does its own diff.

**10. `ShouldHaveFailed` payload.**
- *What:* a `compile_fail` fixture that compiled (run.rs:443-447). Available: `stdout` (build_stdout param) and `warnings`/`preferred` (run.rs:446 `warnings(preferred)`); printed via `should_not_have_compiled` + `fail_output` + `warnings`.
- *Options:* (a) unit `ShouldHaveFailed`. *Con:* discards why/what compiled. (b) `{ stdout: String }`. (c) `{ stdout: String, warnings: String }`. *Pro:* both available and currently shown.
- *Rec:* (c).

**11. `BuildFailed` payload (the `pass`-test-failed-to-compile case).**
- *What:* a `pass` fixture that failed to compile (run.rs:418-420); `preferred` is the compiler stderr (printed via `failed_to_build`).
- *Options:* (a) unit. *Con:* discards the compiler error. (b) `{ stderr: String }`.
- *Rec:* (b).
- *Interacts:* depends on #13 (splitting `CargoFail`).

**12. `RunFailed` payload + exit-status type.**
- *What:* a `pass` fixture compiled & ran but exited nonzero/panicked (run.rs:423-429). Available: `output: Output` (status/stdout/stderr; build_stdout spliced in at run.rs:424) and `warnings`/`preferred`; printed via `output(preferred, &output)` (message.rs:145).
- *Options (payload):* (a) `{ status, stdout, stderr, warnings }`. (b) subset (e.g., status + stderr only).
- *Options (status type):* (i) `std::process::ExitStatus` — *Pro:* fully typed, carries Unix signal info. *Con:* not easily constructed/compared/serialized; platform-nuanced. (ii) `Option<i32>` exit code — *Pro:* portable, comparable, serializable. *Con:* loses signal-vs-code distinction. (iii) `i32`. *Con:* can't represent "killed by signal."
- *Rec:* payload (a); status type **(ii) `Option<i32>`** — STS will want to compare/serialize, and the signal nuance isn't load-bearing for compile-fail-centric use. (Flip to ExitStatus if you want maximal fidelity.)

**13. Split the overloaded `CargoFail`.**
- *What:* `CargoFail` means two unrelated things — a pass-test that failed to build (run.rs:420, a verdict, stderr available) and a cargo invocation returning nonzero during dependency build (cargo.rs:105, pure infra).
- *Options:* (a) split → verdict becomes `TestStatus::BuildFailed{stderr}` (#11); infra case becomes a `SetupError` (#13/Area D). (b) leave conflated. *Con:* a single type meaning both a test verdict and an infra failure violates strict typing and forces STS to disambiguate.
- *Rec:* (a) split.

### AREA D — Outer (run-aborting) error

**14. `try_run`'s outer error type.**
- *What:* the 14 infra variants abort the whole run (prepare/lock/write/metadata/glob). What does `try_run -> Result<Report, ???>` use?
- *Options:* (a) reuse internal `Error` (only if #1=a). (b) a new `pub enum SetupError`. (c) opaque `pub struct SetupError(String)`.
- *Rec:* (b), consistent with #1=(b).

**15. `SetupError` granularity.**
- *What:* how many cases the public setup error has.
- *Options:* (a) one opaque `{ message: String }`. *Pro:* zero coupling, smallest surface. *Con:* STS can't distinguish causes. (b) coarse typed: e.g. `Cargo`, `Manifest`, `Metadata`, `Io`, `Toml`, `Glob`, `Env`. *Pro:* typed without 1:1 internal coupling. (c) near-1:1 with the 14 variants. *Con:* maximal coupling/churn.
- *Rec:* (b) coarse typed — these abort the run and STS likely treats them as one "infrastructure failure" tier, but coarse typing keeps it inspectable without binding to internals.

**16. Context carried by `SetupError` cases.**
- *What:* whether cases carry `PathBuf`/source detail (e.g., which manifest, which IO path — noting the census found `Io` currently *loses* paths except `Open` at run.rs:506).
- *Options:* (a) message string only. (b) typed context where cheap (path on IO/manifest, the offending `TRYBUILD` value on env, etc.).
- *Rec:* (b) where the data is already in scope; don't add new plumbing to recover lost paths (that's #1=a territory).

**17. `already_printed()` fate.**
- *What:* error.rs:65 marks the 4 verdict errors so the generic printer skips them; used only by the Drop/legacy path.
- *Options:* (a) keep it for the legacy Drop renderer. (b) remove it and re-derive printing from the new structured statuses.
- *Rec:* (a) keep — it's internal to the legacy path and preserves its output; removing it is churn for no public benefit.

### AREA E — run / Drop refactor

**18. Silent-core extraction approach.**
- *What:* printing is interleaved through `check_pass`/`check_compile_fail`/`run_all` (run.rs:419,425,444-492). To make `try_run` silent + structured:
- *Options:* (a) make `check_*` **pure** (return structured data, no `message::*`); a separate reporter renders for the legacy path. *Pro:* `try_run` silent for free; cleanest separation. *Con:* largest internal refactor. (b) `_silent` twin functions. *Con:* duplicate logic, drift risk. (c) thread a `silent: bool` through every message call. *Con:* couples logic to UI, pervasive.
- *Rec:* (a).

**19. Legacy console output: streamed vs batched.**
- *What:* approach (a) renders after the run, so the legacy path's "test X … ok" lines batch instead of streaming mid-run.
- *Options:* (a) **batched** rendering from `Report`. *Pro:* simple. *Con:* cosmetic change to trybuild's own console ordering. (b) **preserve streaming** via an observer/callback threaded through the core. *Pro:* byte-for-byte parity. *Con:* more invasive plumbing.
- *Rec:* (a) batched — STS never consumes trybuild's console; the fork's own UI is non-load-bearing. (Choose (b) if upstream-parity of console output matters to you.)

**20. Double-run guard — mechanism & location.**
- *What:* no guard exists; calling `try_run()` then dropping would run twice.
- *Options:* (a) `Runner { …, already_run: bool }`, `Drop` checks it. *Pro:* uses existing `RefCell<Runner>`. (b) `Cell<bool>` on `TestCases`. (c) make explicit run **consume** `self` (`fn try_run(self)`). *Con:* (c) breaks the borrow-based `&self` API and can't coexist with the auto-Drop model.
- *Rec:* (a).

**21. Keep `Drop` auto-running?**
- *What:* lib.rs:339 runs tests on drop — the current ergonomic contract.
- *Options:* (a) keep (back-compat for existing trybuild users). (b) make `Drop` a no-op and require explicit run. *Con:* breaks every existing trybuild test. (c) deprecate-warn.
- *Rec:* (a) keep, guarded by #20.

**22. Legacy `run()` exactness.**
- *What:* whether the legacy path stays behaviorally identical: panic messages (run.rs:62/102/105) and the `project.name != "trybuild-tests"` self-test suppression (run.rs:101).
- *Options:* (a) preserve exactly. (b) simplify/normalize messages. *Con:* (b) changes observable behavior for existing users and the crate's own suite.
- *Rec:* (a) preserve exactly.

### AREA F — Blessing

**23. Blessing model — read-only `try_run` vs mutating.**
- *What:* blessing writes `.stderr`/wip files (run.rs:464/469/493). Does the structured assertion path perform writes?
- *Options:* (a) **read-only** `try_run`; a separate explicit `bless`/update entry writes. *Pro:* deterministic, side-effect-free assertions; maps cleanly onto a `snap-update`-style command; an STS `ensure_compile_fail` never mutates the tree. *Con:* two entry points. (b) **mutating** `try_run` honoring `Update`. *Pro:* one call. *Con:* assertions mutate files; surprising in a test predicate.
- *Rec:* (a).
- *Interacts:* sets #7 (status set) and #24 (API).

**24. Blessing API shape.**
- *What:* how the caller requests/performs blessing.
- *Options:* (a) separate `fn bless(&self, mode: Update) -> Result<Report, SetupError>` (read-only `try_run` stays pure). (b) `fn try_run_with(&self, update: Update)` parameter. (c) `fn update(&self, mode: Update)` setter consumed by `try_run`. (d) builder.
- *Rec:* (a) if #23=read-only — an explicit `bless` reads cleanest and keeps `try_run` a pure predicate. If #23=mutating, then (b).

**25. Expose `Update`, its name and variants.**
- *What:* `enum Update { Wip(default), Overwrite }` (env.rs:5), private.
- *Options:* (a) `pub` as-is. (b) rename (`Bless`/`UpdateMode`) and/or rename variants. (c) model differently (e.g., `enum Bless { InPlace, Wip }`).
- *Rec:* (a) expose as-is — already a clean two-state enum; renaming is churn.

**26. Programmatic vs `TRYBUILD` env precedence in the new API.**
- *What:* `Update::env()` (env.rs:12) reads the `TRYBUILD` env var. Does the new structured API read env at all?
- *Options:* (a) new API is **explicit-only** — `try_run`/`bless` ignore `TRYBUILD`; env still drives the legacy Drop path only. *Pro:* no hidden global state in the structured path; deterministic; aligns with STS's no-magic-env ethos. (b) programmatic overrides env, else env. (c) env overrides programmatic.
- *Rec:* (a) explicit-only for the new API.

### AREA G — Fork metadata

**27. `Cargo.toml` `repository`.**
- *Options:* (a) `https://github.com/earthlings-dev/trybuild`. (b) leave dtolnay. *Con:* misattributes the fork. (c) remove.
- *Rec:* (a).

**28. `Cargo.toml` `version`.**
- *What:* currently `1.0.116`; consumed downstream via `[patch.crates-io]` by **git rev**, so exact version isn't enforced.
- *Options:* (a) keep `1.0.116`. *Pro:* simplest; patch matches `trybuild = "1"`. (b) bump to `1.0.117`. *Pro:* signals divergence. *Con:* cosmetic only under rev-pinned patch. (c) build-metadata suffix (e.g., `1.0.116+earthlings`). *Con:* unusual under `[patch]`, no benefit.
- *Rec:* (a) keep `1.0.116`.

**29. `Cargo.toml` `publish`.**
- *Options:* (a) add `publish = false`. *Pro:* prevents accidental `cargo publish` of a fork. (b) leave default (true). *Con:* a stray publish could clobber. *Note:* no effect on git-dep consumption either way.
- *Rec:* (a).

**30. The `diff` feature (dissimilar).**
- *What:* optional `diff` feature gates `dissimilar`, used only by `message::mismatch` console rendering (message.rs). The new API returns `expected`/`actual` as data; STS diffs them itself.
- *Options:* (a) leave as-is (off by default; only the legacy console uses it). (b) enable by default. *Con:* pulls `dissimilar` always for no new-API benefit. (c) remove. *Con:* changes legacy console output.
- *Rec:* (a) leave as-is — the new API has zero dependency on it.

**31. MSRV (`rust-version = 1.85`) and edition (2021).**
- *What:* new code must compile on 1.85; STS/rust-template pin stable ≥ that.
- *Options:* (a) leave both; write new code to 1.85 (avoid newer std APIs). (b) raise MSRV. *Con:* unnecessary; narrows compatibility.
- *Rec:* (a).

### AREA H — Lints, docs, verification

**32. Verification bar.**
- *What:* trybuild's CI runs `cargo test`; `cargo clippy --tests -- -Dclippy::all -Dclippy::pedantic`; `RUSTFLAGS="-Dwarnings"`; `cargo fmt --check`; minimal-versions `cargo check --locked`; `cargo docs-rs`.
- *Options:* (a) match trybuild's CI exactly. (b) reduced subset. *Con:* fork could regress its own gates. (c) also impose STS's stricter policy. *Con:* the fork is an external patched dep; STS lints don't apply to it and pedantic + `-Dwarnings` is already strict.
- *Rec:* (a).

**33. Doc comments on new public items.**
- *What:* trybuild does not `deny(missing_docs)`, but the new public types are your API.
- *Options:* (a) require `///` on every new pub item. *Pro:* clean public surface, good rustdoc. (b) skip docs.
- *Rec:* (a).

**34. Clippy-pedantic conformance for new code.**
- *What:* new pub structs/enums with public fields can trip pedantic lints (e.g., `must_use`, missing `#[non_exhaustive]` considerations); crate already `#![deny(clippy::clone_on_ref_ptr)]`.
- *Options:* (a) write new code to pass pedantic with **no** new `#[allow]`s. (b) add scoped `#[allow(...)]` in the fork where pedantic fights the design. (c) add to the crate-level `#![allow]` list (lib.rs:232).
- *Rec:* (a); fall back to narrowly-scoped (b) only with a stated reason if pedantic and the API genuinely conflict.
- *Sub-question for you:* mark the new public enums `#[non_exhaustive]`? *Pro:* lets you add status/error cases later without a breaking change. *Con:* forces downstream (your own STS) to add a wildcard arm. *Rec:* yes for `SetupError` (likely to grow), no for `TestStatus` (you want exhaustive matching in STS).

### AREA I — Tests

**35. Fixture structure for `try_run` tests.**
- *What:* trybuild self-tests via `tests/test.rs` + `tests/ui/*.rs`/`.stderr`, relying on `project.name == "trybuild-tests"` to suppress Drop panics (run.rs:101).
- *Options:* (a) new isolated test file + fixtures (e.g., `tests/try_run.rs` + `tests/try_run_ui/`). *Pro:* doesn't perturb the existing suite; asserts on `Report` directly. (b) reuse `tests/ui` fixtures. *Con:* couples new assertions to the existing suite's expected outputs.
- *Rec:* (a).

**36. Status-coverage set for tests.**
- *What:* which `TestStatus`/error cases the new tests must exercise.
- *Options:* (a) cover every case: `Passed`, `Mismatch`, `ShouldHaveFailed`, `BuildFailed`, `RunFailed`, `NoExpected` (or `Wip`/`Blessed` per #23), plus the double-run guard (#20) and a `SetupError` path. (b) a representative subset.
- *Rec:* (a) — full polarity coverage is the whole point.

**37. Interaction: tests vs the `trybuild-tests` panic-suppression.**
- *What:* new tests call `try_run` (no panic) and assert on `Report`; the #20 guard must prevent the subsequent `Drop` from re-running.
- *Options:* (a) rely on the #20 guard; tests need no special naming. (b) additionally name the test project to hit the suppression path. *Con:* unnecessary if (a) holds.
- *Rec:* (a) — and add an explicit test that `try_run` then drop runs exactly once.

### AREA J — Process

**38. Branch name.**
- *Options:* (a) `earthlings/expose-core`. (b) other.
- *Rec:* (a).

**39. Commit structure.**
- *Options:* (a) a logical series (e.g., 1: make core types pub; 2: introduce structured `TestStatus`/`Report`; 3: split `CargoFail` + `SetupError`; 4: `try_run` + guard + silent core; 5: blessing API; 6: tests; 7: metadata). *Pro:* reviewable, rebaseable onto upstream. (b) one squashed commit. *Con:* harder to review/rebase.
- *Rec:* (a), your conventional-commit format on each.

**40. Scope boundary.**
- *What:* confirm this plan is **fork-only**; the downstream `[patch.crates-io]` rev pin and the STS `ensure_compile_fail` façade are separate later tasks.
- *Rec:* confirm fork-only.

Rule on these by number (ranges fine, e.g. "1:b, 2:b, 9:b, 12: payload a / status ExitStatus, 23:a…"), and flag anything I've still under-treated or missed. When you've decided, I'll write the decision-complete plan to the plan file — and not before.