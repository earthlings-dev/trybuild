# Drop semantics (D1) in full, and every deferred verification resolved

> **SUPERSEDED (Part 1):** the (i)/(ii) "Drop dilemma" below rests on an invalid premise (that a suite fails CI via `Drop`-panic). In this ecosystem tests are `#[test] -> Result<(), TestFailure>` and fail by *returning* `Err`; `Drop` is not a failure channel. See `test-failure-model-corrected.md`. **Part 2 (resolved verifications) remains valid and current.**
>
> Status: **review draft.** Companion to `type-architecture-v1.md`. This doc (a) explains the two `Drop` options in full, grounded in how a Rust test actually fails, and (b) closes every "to-verify" / deferred item from v1 with source facts, so there is nothing left to discover at implementation time. Nothing here is decided.

---

## Part 1 — The `Drop` decision (D1), in detail

### 1.0 The forcing constraint (why this is even a dilemma)

A standard `#[test]` reports failure in exactly two ways:

1. The test function (or anything it calls) **panics** — libtest catches the unwind and marks the test failed.
2. The test function is `#[test] fn … -> Result<_, E>` and **returns `Err`**.

`Drop::drop(&mut self)` returns `()` and runs *after* the test function's body has already produced its value. So **`Drop` cannot return `Err`, and cannot influence the function's return value. The only failure signal available from inside `Drop` is a panic.**

trybuild's current contract relies on exactly that:

```rust
#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs"); // only REGISTERS
    // `t` drops here → Drop → Runner::run → panic!(...) on failure (run.rs:62/102/105)
}                                     // libtest catches the panic → test fails
```

- `impl Drop for TestCases` (lib.rs:339) calls `runner.run()` **only if** `!thread::panicking()` (lib.rs:341) — so it won't double-panic if the body already panicked.
- `Runner::run` (run.rs:50) panics on any failure: run.rs:62 (`tests failed`), run.rs:102 (`{n} of {m} tests failed`), run.rs:105 (`created … stderr files`).

So: **today, the bare pattern fails CI purely because `Drop` panics.** Remove the panic and the bare pattern can no longer fail CI. That is the crux of (i) vs (ii).

### 1.1 The existing precedent: trybuild's own non-panicking mode

trybuild *already* runs-and-reports without panicking — for its own test suite:

- `crate_name = &source_manifest.package.name` (run.rs:150); `project_name = "{crate_name}-tests"` (run.rs:154); stored as `Project.name` (run.rs:172).
- Guard: `if report.failures > 0 && project.name != "trybuild-tests" { panic!(…) }` (run.rs:101); same for created-wip (run.rs:104). When trybuild compiles its *own* `tests/ui` fixtures the project is `trybuild-tests`, so **failures and wip-creation print but do not panic** — letting the self-suite exercise mismatch/wip paths without failing CI (AGENTS.md:52).

**Consequence for the design:** a "render but don't panic" run path is not exotic — it already exists, gated on a name check. Option (i) generalizes it to all callers; option (ii) keeps the panic for everyone except that self-test carve-out.

### 1.2 The newly-decisive fact: `clippy::panic = "deny"` is in force

The fork's `Cargo.toml` sets `panic = "deny"` and `panic_in_result_fn = "deny"` (`[workspace.lints.clippy]`). `clippy::panic` flags the `panic!` macro. The three panics in `Runner::run` (run.rs:62/102/105) therefore **violate the strict policy**. So:

- **Option (i) removes the panics** → no policy exception needed.
- **Option (ii) keeps them** → requires a sanctioned `#[allow(clippy::panic, reason = "…")]` on the Drop path (or `expect`-style justification), i.e. a deliberate, documented exception to your own panic-free policy.

This is a concrete, source-grounded differentiator that did not surface in v1.

### 1.3 Option (i) — `Drop` becomes render-only / no-op (guarded)

**What changes:** `Drop` never panics. Behavior:

- If `try_run`/`bless` was already called (double-run guard, §2.4) → `Drop` is a **no-op**.
- If not → `Drop` runs the suite once and **renders the report to the terminal** (the existing streamed reporter, #19) but **does not panic and does not fail the test**.

**How you assert failures:** you must consume the result.

```rust
#[test]
fn ui() -> Result<(), trybuild::SetupError> {
    let report = trybuild::TestCases::new().compile_fail_("tests/ui/*.rs").try_run()?;
    // assert on `report` — e.g. STS: ensure(report.is_ok(), …)
    Ok(())
}
```

In the STS world this is the *only* path that matters: `ensure_compile_fail` calls `try_run` and returns a `TestFailure`; nobody relies on `Drop`.

**Pros**
- Fully consistent with the strict policy — **no `panic!`, no `#[allow]`** (see §1.2).
- Matches the STS Result-based model exactly (failures flow through `Result`, not unwinding).
- One run path, panic-free, reused by both the explicit API and the (now render-only) `Drop`.

**Cons / consequences to ratify**
- **Breaking semantic change vs upstream trybuild:** a suite written the bare way (`t.compile_fail(…)` and rely on `Drop`) **silently stops failing CI** on a mismatch. It will still *print* the failure (render-only), but green CI. Anyone porting upstream-style suites must migrate to `try_run()?`.
- The fork's own `tests/test.rs` (bare pattern) would need to migrate to `try_run` to *assert* (today it doesn't truly assert via Drop anyway — the `trybuild-tests` guard already suppresses its panics, §1.1).
- A "render-only that never fails" `Drop` is a mild footgun for anyone who forgets to consume the report. Mitigation options (all non-panicking): a prominent rendered warning ("results not asserted; call `try_run()?`"), or making `Drop` a pure no-op (no render) so the absence of output is itself a signal. (Sub-choice within (i); see Open Question D1a.)

### 1.4 Option (ii) — keep `Drop` panicking for the bare pattern

**What changes:** `Drop` keeps today's behavior (run + panic on failure), so bare suites fail CI exactly as upstream. The **new** `try_run`/`bless` methods are panic-free and structured; when you call them, the double-run guard makes `Drop` a no-op (no double run, no panic).

```rust
// legacy/bare — unchanged, still fails CI via Drop-panic:
#[test] fn ui() { let t = TestCases::new(); t.compile_fail("tests/ui/*.rs"); }

// new — panic-free, structured:
#[test] fn ui() -> Result<(), SetupError> { …TestCases::new()…try_run()?; Ok(()) }
```

**Pros**
- **Smallest change; fully back-compatible** with upstream trybuild semantics (and with the fork's current `tests/test.rs`).
- Easiest to keep rebasing onto upstream trybuild (Drop semantics unchanged).

**Cons / consequences to ratify**
- **Collides with `clippy::panic = "deny"` (§1.2):** the retained `panic!`s need a sanctioned `#[allow(clippy::panic, reason = "Drop is the only failure channel for the legacy bare pattern")]`. That is a deliberate, documented hole in the panic-free policy you just adopted — for a fork whose whole purpose is to be panic-safe. Worth weighing.
- Two run-report behaviors coexist (panicking Drop vs silent `try_run`), a little more surface area.
- Does not advance the crate toward "panic-free" — it preserves the panic you forked to remove.

### 1.5 Side-by-side

| | (i) render-only/no-op Drop | (ii) keep Drop panic |
|---|---|---|
| Bare pattern fails CI on mismatch | **No** (prints only) | **Yes** (unchanged) |
| `panic!` remains in crate | No | Yes → needs `#[allow(clippy::panic)]` |
| Fits `panic = "deny"` policy cleanly | **Yes** | No (documented exception) |
| Back-compat with upstream/bare suites | Breaking | **Full** |
| Fits STS Result-world (`ensure_compile_fail`) | **Native** | Works (via `try_run`) |
| Migration cost for existing call sites | Must adopt `try_run()?` | None |
| Upstream rebase friction | Higher (Drop diverges) | Lower |

**Framing, not a decision:** (i) is the panic-free end-state that matches both the strict policy and the STS model, at the cost of a breaking change to the bare contract. (ii) is the minimal, back-compatible change, at the cost of carrying a sanctioned `panic!` exception in a panic-free fork. Because *every* consumer of this fork is your own repos going through STS, the back-compat value of (ii) is mostly about the fork's own self-tests and upstream-rebase ease — not external users.

**Open Question D1a (only if (i)):** render-only `Drop` (prints the report, never fails) vs pure no-op `Drop` (silent). Render-only preserves "I see output from `cargo test`"; pure no-op makes a forgotten `try_run` produce nothing (loud by absence).

---

## Part 2 — Deferred verifications, now resolved (no implementation-time discovery left)

### 2.1 `print_stdout` / `print_stderr` — NON-ISSUE (verified)

- `term.rs:29-43` defines crate-local `macro_rules! print` / `println` expanding to `std::write!($crate::term::lock(), …)` — i.e. `write!`/`writeln!` to `Term` (a `termcolor::StandardStream` on **stderr**, term.rs:55), brought in crate-wide via `#[macro_use] mod term` (lib.rs:264).
- `clippy::print_stdout`/`print_stderr` fire on the **std** `print!`/`println!`/`eprint!`/`eprintln!` macros (matched by macro identity), not on a same-named crate-local macro that expands to `write!`.
- Grep for genuine std prints (`eprintln!|eprint!|std::print|std::eprint|io::stdout|io::stderr`): **none.** The lone non-definition `print!` (cargo.rs:187) is the shadowed term macro.
- **Conclusion:** the strict print lints do **not** fire on trybuild's reporter. The console layer survives the strict policy untouched by these two lints.

### 2.2 …but the macro bodies trip `let_underscore_*` (new finding, conformance)

`term.rs:33/41` use `let _ = std::write!(…)`; other `let _ = …` discards exist (term.rs:69/84, cargo.rs:90). The strict policy denies `let_underscore_untyped` and `let_underscore_must_use`. These are **conformance items** (part of F1), independent of the type architecture; noting them so they aren't a surprise. (Fix shape: handle/observe the `io::Result` rather than `let _ =`.)

### 2.3 The `trybuild-tests` non-panic guard — mechanism pinned

`crate_name` = the crate-under-test's package name (run.rs:150) → `project_name = "{crate}-tests"` (run.rs:154) → `Project.name` (run.rs:172). Guard at run.rs:101/104 suppresses panic/wip-panic when `name == "trybuild-tests"`. This is the existing render-without-panic path that (i) generalizes.

### 2.4 Double-run guard — final mechanism

Use **`TestCases { …, has_run: Cell<bool> }`** (not a flag inside `RefCell<Runner>`):

- `try_run`/`bless` set `has_run.set(true)`.
- `Drop` checks `!self.has_run.get()` before running — a `Cell<bool>` read is `Copy`/infallible, so `Drop` never takes a `RefCell` borrow (which could conflict/panic). This also composes with the existing `impl RefUnwindSafe for TestCases` (lib.rs:336). Decisive over the `Runner.has_run` variant purely on the no-borrow-in-`Drop` property.

### 2.5 `RunStatus` — exact, portable derivation

trybuild only ever reads `ExitStatus::success()` today (cargo.rs:104/114, message.rs:146, run.rs:426). The typed replacement, fully specified:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus { Success, Exited(i32), Signaled(i32), Unknown }

// derivation (no deferral):
//   if status.success()            => Success
//   else if let Some(c)=status.code() => Exited(c)            // Windows: always Some
//   else (Unix, killed by signal):  Signaled(sig)            // via std::os::unix::process::ExitStatusExt::signal()
//                                                             //   behind #[cfg(unix)]
//   else                           => Unknown
```

`Signaled` is populated only under `#[cfg(unix)]` (needs `use std::os::unix::process::ExitStatusExt`); on non-Unix the arm is unreachable and folds to `Exited`/`Unknown`. Unlike `std::process::ExitStatus`, `RunStatus` is `Copy`, comparable, and trivially serializable.

### 2.6 `SetupError` context recovery (#16) — exact sites enumerated

Bare `?` `fs::` sites that currently collapse to a path-less `Error::Io` (must add the path): run.rs:152 (`project_dir`), run.rs:186 (`<dir>/Cargo.toml`), run.rs:192 (`<dir>/main.rs`), run.rs:456 (`wip_dir`), run.rs:458 (`gitignore_path`).

Typed-but-path-less sites (carry an `io::Error` but not the path; add the path): run.rs:464/469/493 (`Error::WriteStderr` → the wip/stderr path), run.rs:477 (`Error::ReadStderr` → `stderr_path`).

Already complete: run.rs:504/506 (`Error::Open(path, …)` carries the path).

Other infra context: `Metadata` can capture `output.stderr` (cargo.rs:187, currently printed then dropped); `GetManifest(PathBuf, Box<Error>)` becomes `Manifest { path, source: ManifestError(io|toml) }` (dependencies.rs:23). This is the full plumbing list for "typed context everywhere."

### 2.7 The env read site (#26)

`Project.update = Update::env()?` at **run.rs:173** is the single place `TRYBUILD` is read. The new API (`try_run` read-only, `bless(mode)` explicit) never calls `Update::env`. Whether the legacy `Drop` path also stops reading it (E1) is your call; the change is localized to run.rs:173.

---

## Part 3 — Open questions still needing your ruling (now fully researched)

- **D1**: (i) render-only/no-op Drop vs (ii) keep Drop panic — with the `panic = "deny"` tension (§1.2) and the back-compat tradeoff (§1.5) now on the table. **D1a** (only under (i)): render-only vs pure no-op.
- Everything in `type-architecture-v1.md` §8 (A1, B1–B4, C1, D2, E1, F1) remains for your ruling; none now depend on unresolved facts.
