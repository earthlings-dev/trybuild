# Clippy strict-branch resolution — session plan

Resolving the remaining `clippy.toml` + `[workspace.lints.clippy]` restriction-lint errors on the `strict` branch. As of the latest run: **171 errors + 3 warnings**, down from 236.

Policy (decided): **fix everything — no new `#[allow]`** except the externally-forced ones that already landed (see Appendix B). Line numbers below are from the 171-error run and **will drift as edits land — match by construct, not by number**, and re-run clippy to get the current list.

---

## 0. Shared convention (read first — applies to every session)

These three lints dominate the remainder. The idiom below is taken verbatim from `rust-template` / `strict-test-support-rs` (the source of this config). There is **no shared helper crate** — `strict_test_support` exports only test assertions, so this is a plain inline idiom, repeated at each site.

### Arithmetic (`arithmetic_side_effects`)
Use **`saturating_*`**, choosing the op that matches the domain. In this codebase every flagged `+`/`-` is an index/offset/counter where the value is already proven in-range by a preceding `find()`/`len()`/guard, so saturating is **behavior-preserving** (the saturated branch is unreachable).
```rust
indent + 4            ->  indent.saturating_add(4)
component.len() - 17  ->  component.len().saturating_sub(17)
count += 1            ->  count = count.saturating_add(1)
5 * common_len        ->  common_len.saturating_mul(5)
```
`saturating_*` dominates the reference repos (~40 uses vs ~2 `checked_*`). Reach for `checked_* + ?` only where `None`/overflow is a real, reachable case.

### Slicing & indexing (`string_slice`, `indexing_slicing`)
Never `x[i]` / `s[a..b]`. Use **`.get(..)` returning `Option`** with a graceful tail.
```rust
s[a..b]                ->  s.get(a..b).unwrap_or("")        // str
s[a..]                 ->  s.get(a..).unwrap_or("")
slice[i]               ->  slice.get(i)…  (+ ? / .copied().unwrap_or_default())
matches[0]             ->  matches.first().ok_or(Err)?
arr.last().unwrap()    ->  arr.last().expect("…non-empty")  // expect_used is NOT enabled
```
Put saturating arithmetic **inside** the `.get()` range — the canonical combined form:
```rust
// strict-test-support-rs/xtask/src/lint_attrs.rs:93
haystack.get(i..i.saturating_add(needle.len())) == Some(needle)
// gen_config_template.rs:131 / badges.rs:262
haystack.get(start..end).unwrap_or("")
digits.get(..split).unwrap_or_default()
```

### Banned panicking methods (`disallowed_methods`)
`clippy.toml` bans these and names the replacement:
```rust
str::split_at(n)            ->  split_at_checked(n)            (Option)
String::truncate(len)       ->  s.replace_range(len.., "")     (replace_range is allowed)
String::split_off(n)        ->  strip_prefix(..) / get(n..).map(str::to_owned)
Vec::insert(0, x)           ->  v.splice(0..0, iter::once(x))  (splice is allowed)
```

### `pattern_type_mismatch`
Stop relying on match-ergonomics; put the reference in the pattern, or `as_ref`/`as_mut` the scrutinee:
```rust
let Some((a, b)) = map.get(k)              ->  let Some(&(a, b)) = map.get(k)        // tuple is Copy
if let Some(x) = &mut opt                  ->  if let Some(x) = opt.as_mut()
if let Some(Dependency{optional:true,..})  ->  if let Some(&Dependency{optional:true,..})  // only Copy fields bound
match chunk { Chunk::Equal(c) => … }       ->  match *chunk { … }                    // Chunk is Copy
```

### Structure (`too_many_lines`, `excessive_nesting`)
Extract helpers and flatten. Do **structure first, then safety** within a function: the small scopes are where `?`, `.get()`, and saturating math read cleanly, and extraction dissolves most `shadow_*`.

### Global rules
- **No `#[allow]`** beyond Appendix B.
- `normalize.rs` is **append-only**: never insert/reorder `Normalization` variants.
- `normalize.rs` is `#[path]`-included by the fuzz target, and `tests.rs` by `diagnostics.rs` — keep both compiling.
- Commit per AGENTS.md (`type(scope): …`, required scope, no `chore`). Suggested scopes below.

---

## Session 1 — `normalize.rs` (the bulk: ~130 errors + 3 warnings)

Independent of all other sessions. **Phase A (structure) → Phase B (safety).** Scope: `refactor(normalize)` / `fix(normalize)`.

### Phase A — structure
- `too_many_lines` (218, `Filter::apply`) + `excessive_nesting` ×17 (264, 284, 301, 315, 321, 336, 341, 351, 370, 398, 471, 496, 509, 666, 671, 685, 689) + `else_if_without_else` ×3 warnings (346, 495, 506).
- Decompose `Filter::apply`: lift the `--> `/`:::` path-rewriting ladder (target-dir/`$OUT_DIR`, source-dir/`$DIR`, `$WORKSPACE`, path-deps, `$RUST`, `$CARGO`/ `$VERSION`) into named helper methods on `Filter`, each returning `Option<bool>`/an enum so the `else if` chain becomes early returns (clears `else_if_without_else`). Do the same for the `AndOthersVerbose` block and the `unindent` inner loop.
- Extraction also resolves most `shadow_unrelated`/`shadow_reuse` (111, 144, 267, 462, 464, 479, 490, 579, 588, 663, 666, 682) by giving each helper its own scope; rename any that remain.

### Phase B — safety (apply §0 idiom)
- `arithmetic_side_effects` (~60 sites) → `saturating_*`.
- `string_slice` (~26) + `indexing_slicing` (115, 219) → `.get(..)`.
  - 115 `result.variations[i] = …` → iterate: `for (slot, n) in result.variations.iter_mut().zip(Normalization::ALL) { *slot = apply(&output, *n, context); }` (drops `i`).
  - 219 `self.all_lines[index]` → `self.all_lines.get(index).copied()?` (`apply` returns `Option`).
- `unwrap_used` (132, 364, 384, 682, 686) + `unwrap_in_result` (364, 384):
  - 132 `variations.last().unwrap()` → `.expect("Normalization::ALL is non-empty")`.
  - 364/384 (`find('-').unwrap()`, `end_of_version.unwrap()`) → `?` / restructure the `and_then` chain so the index is computed once and reused.
  - 682/686 (in `unindent`: `lines.next().unwrap()`, `line.find(' ').unwrap()`) → fold the `for _ in 0..lines_in_block { lines.next().unwrap() }` into the iterator; `find(' ')` → `?`/`unwrap_or`.
- `disallowed_methods` truncate (160, 450, 562) → `replace_range(len.., "")`:
  - 160 `trim()`: `let len = normalized.trim_end().len(); normalized.replace_range(len.., "");`
  - 450: `let end = line.trim_end().len(); line.replace_range(end.., "");`
  - 562 `hide_trailing_numbers`: `let cut = line.len().saturating_sub(digits).saturating_sub(1); line.replace_range(cut.., "");`
- `range_plus_one` (241, 546) → `..=` (e.g. `cut_start..=cut_end`, `i..=i`).
- `pattern_type_mismatch` (495) → `self.other_types.as_mut()`.

**Verify:** snapshots must stay byte-identical. Run (authorize first): `cargo test --lib diagnostics::snapshots::tests` and `cargo fuzz check`. This is the one session with real off-by-one risk; do not skip the snapshot run.

---

## Session 2 — Project model + `runner.rs` + `cargo.rs`

Finishes the architecture (the last `struct_excessive_bools`) and the orchestrator's safety/structure. Touches only these three files. Scope: `refactor(runner)` / `refactor(project)`.

### Step 1 — Project model (`project.rs`, `struct_excessive_bools` @44)
Replace the 3 bools (`has_pass`, `has_compile_fail`, `keep_going`) with two enums:
```rust
#[derive(Clone, Copy, Debug)]
pub(in crate::internal) enum Selected { Neither, PassOnly, CompileFailOnly, Both }
impl Selected {
    pub(in crate::internal) const fn has_pass(self) -> bool { matches!(self, Self::PassOnly | Self::Both) }
    pub(in crate::internal) const fn has_compile_fail(self) -> bool { matches!(self, Self::CompileFailOnly | Self::Both) }
    pub(in crate::internal) const fn both(self) -> bool { matches!(self, Self::Both) }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::internal) enum KeepGoing { Yes, No }
```
`Neither` covers the empty suite. `Project` keeps `selected: Selected`, `keep_going: KeepGoing`.

### Step 2 — rewire read/write sites
- `runner::prepare`: build `selected` from the loop (`Selected::from_flags`-style); init `keep_going: KeepGoing::No`.
- `cargo::build_dependencies`: `project.keep_going = if … { KeepGoing::Yes } else { KeepGoing::No };`
- Reads: `project.has_pass` (×3 in `cargo.rs` build-vs-check) → `project.selected.has_pass()`; `run()` dispatch `project.keep_going && !project.has_pass` → `project.keep_going == KeepGoing::Yes && !project.selected.has_pass()`; `show_expected` `has_pass && has_compile_fail` → `project.selected.both()`.

### Step 3 — `cargo.rs` straggler
- `shadow_unrelated` (164): rename the closure binding `|status| status.success()` → `|exit| exit.success()` (outer `let status` stays).

### Step 4 — `runner.rs` structure then safety
- `too_many_lines` (270, `make_manifest`): extract `resolve_edition`, `merge_dependencies`, `prune_features` helpers.
- `excessive_nesting` (339) + `pattern_type_mismatch` (335, 339): rewrite the optional-dep check as `targets.values().any(|t| matches!(t.dependencies.get(dep_name), Some(&Dependency { optional: true, .. })))`; line 335 → `Some(&Dependency { optional: true, .. })`.
- `pattern_type_mismatch` (222): `features.as_mut()`.
- `disallowed_methods` (346 `Vec::insert`): `enables.splice(0..0, core::iter::once(format!("{crate_name}/{feature}")));`
- `disallowed_methods` (665 `String::split_off`): replace the whole `.then(|| arg.split_off(..))` with `arg.strip_prefix(PREFIX).filter(|rest| !rest.is_empty()).map(ToOwned::to_owned)` (drops the `mut` on `arg`).
- `arithmetic_side_effects` (136, 138, 432, 438): `report.created_wip = report.created_wip.saturating_add(1);` / `report.failures = report.failures.saturating_add(1);`

**Verify:** `cargo build`; full `cargo clippy --tests -- -Dclippy::all -Dclippy::pedantic` then grep the persisted output for `project.rs`/`runner.rs`/`cargo.rs` — none should remain.

---

## Session 3 — report + leaf safety sweep (~27 errors, fan-out by file)

All independent — split across agents per file. Scope: `fix(report)` / `fix(<module>)`.

- **`report/diff.rs`** — `arithmetic_side_effects` (44, 56, 61×2), `excessive_nesting` (55), `pattern_type_mismatch` (55, 82). Collapse the `common_len` loop to `chunks.iter().filter_map(|c| match *c { Chunk::Equal(s) => Some(s.len()), Chunk::Delete(_) | Chunk::Insert(_) => None }).sum()` (clears nesting + pattern + the `+=`); 44 → `saturating_add`; 61 → `saturating_mul`; 82 → `match *chunk`.
- **`report/reporter.rs`** — `indexing_slicing` (88, 90), `range_plus_one` (88), `arithmetic_side_effects` (88, 90): `buf.get(..=line_len)` for the head; `buf.get(line_len.saturating_add(1)..).unwrap_or(&[])` for the tail.
- **`report/message.rs`** — `pattern_type_mismatch` (224): drop the `&` — `for (name, content) in [("STDOUT", &stdout), ("STDERR", &stderr)] { … }`.
- **`sys/flock.rs`** — `pattern_type_mismatch` (117, 130): restructure the `Drop`s, don't match `&mut self`. `Lock::drop` → direct assignment (`self.lockfile = FileLock::NotLocked; self.intraprocess_guard = Guard::NotLocked;`). `FileLock::drop` → `if let Self::Locked { path, done } = &*self { … }` with explicit `&`+`ref` if the lint persists.
- **`build/json.rs`** — `disallowed_methods` split_at (100, 103) → `split_at_checked` with `let Some((a, b)) = …split_at_checked(n) else { break };`; `arithmetic` (102) → `end.saturating_add(1)`; `pattern_type_mismatch` (116) → `let Some(&(name, case)) = path_map.get(&src_path)`.
- **`project/features.rs`** — `arithmetic` (65×2, 67) → `len().saturating_sub(..)` inside the existing `hash_range`; `string_slice` (70) → `hash.get(1..).unwrap_or("")`; `indexing_slicing` (103, 116) → `hash_matches.first().ok_or(Ignored)?` / `json_matches.first().ok_or(Ignored)?`.
- **`project/rustflags.rs`** — `unwrap_used` (36): `.expect("static flag list is valid TOML")`.
- **`runner/expand.rs`** — `indexing_slicing` (75): `self.vec.get_mut(i)` (+ handle `None`).
- **`path.rs`** — `tests_outside_test_module` (68): wrap `test_path_macro` in `#[cfg(test)] mod tests { use super::*; … }`.

**Verify:** full `cargo clippy --tests …`, grep the persisted output for each filename.

---

## Verification & definition-of-done

Clippy is whole-crate, so it won't go green until all three sessions land. Per session: run the gate **unfiltered**, let it persist, then `Grep`/`Read` the saved file for your filenames (per the global "never filter at the pipe" rule).

Final gate (all green, zero warnings):
```
cargo clippy --tests -- -Dclippy::all -Dclippy::pedantic
cargo test            # normalizer snapshots + self-hosted integration
cargo fuzz check
cargo fmt --check
```

Parallelism: Sessions 1, 2, 3 share no files — run concurrently. Session 1 dominates and carries the only correctness risk (snapshots).

---

## Appendix A — already complete (do NOT redo)

- **Error architecture**: public `TryBuildError`; domain enums `BuildError` / `DiagnosticsError` / `ProjectError` / `RunnerError` / `SysError`; `already_printed` is `const`; `Context` → `&Context` (incl. json caller, `test_normalize!` macro, fuzz); `TestCases::run(&self)` + `new()` const.
- **All idiom/rename lints**: `disallowed_names`, `unused_trait_names`, `enum_glob_use`, `manual_range_contains`, `non_ascii_literal`, `module_inception` (→ `mod snapshots`), `option_if_let_else`, `single_match_else`, `if_then_some_else_none`, `while_let_on_iterator`, `struct_field_names`, `partial_pub_fields`, `allow_attributes_without_reason`, leaf `missing_const_for_fn`. `flock::poll` was fixed (takes refs), not allowed.

## Appendix B — the only sanctioned `#[allow]`s (already in place)

Externally forced; do not remove, do not add others:
- `same_name_method` — `dependencies.rs` module-level (serde `remote = "Self"`).
- `trivially_copy_pass_by_ref` — `is_false` (serde `skip_serializing_if` signature).
- `wildcard_enum_match_arm` — `flock::create` (`io::ErrorKind` is `#[non_exhaustive]`).
- `unused_self` — `Test::check_pass` (shared fn-pointer signature with `check_compile_fail`).
