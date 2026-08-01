# Lint-allowance audit — `strict` branch

A read-only audit of every inline lint suppression in the repo, judged against **the rule stated in `Cargo.toml`** — not against any narrative doc, and not against a general "is this justifiable?" standard.

## The rule (from your `[workspace.lints.clippy]` meta comment)

> localized `#[allow(clippy::single_call_fn, reason = "...")]` is permitted where a named helper genuinely improves structure. The companion `lint-attrs` stask still rejects every other `#[allow(...)]` / `#[expect(...)]`.

This is a hard binary:

- **Permitted inline:** exactly one form — `#[allow(clippy::single_call_fn, reason = "...")]`, and only where the helper genuinely improves structure.
- **Rejected inline:** every other `#[allow(...)]` / `#[expect(...)]`, no matter how reasonable its `reason` reads.

`allow_attributes_without_reason = "deny"` forces a `reason` on any suppression that exists; it does **not** sanction adding suppressions. The intended hard gate is the `lint-attrs` stask — which is **absent from this repo** (no `stask/` crate; it lives in `strict-test-support-rs`). That absence is why the violations below were able to land: the only thing actually enforcing the rule here is this audit.

Out of scope: **config-level** `allow`/`warn` entries (`restriction`, `implicit_return`, `question_mark_used`, `async_fn_in_trait`, the two `warn`s, …) are you *defining the lint set* — turning off lints that contradict denied lints or don't apply. They are policy, not exceptions. They are summarized at the end but are not "allowances" in the sense your rule governs.

Also out of scope: `docs/clippy-strict-sessions.md` and its "Appendix B" are **not authoritative** and disagree with the rule above; disregard (or delete) them. And generated-code suppressions (`runner.rs:260` emitted `#![allow(unused_crate_dependencies, missing_docs)]`; `rustflags.rs:7` `IGNORED_LINTS`) act on the user's throwaway crate, not on trybuild — not governed by this rule.

## Verdict summary

| Class | Count | Verdict |
|---|---|---|
| `#[allow(clippy::single_call_fn, …)]` | 66 | **Permitted** — the one sanctioned form (each checked for "genuinely improves structure" below). |
| Every other inline `#[allow]` / `#[expect]` | 14 | **VIOLATION** — your policy rejects these outright. |

The 14 violations span 9 distinct lints across 6 files. Each must be **either** refactored until the suppression is unnecessary, **or** — if you decide it's genuinely unavoidable — consciously promoted to a *documented, named exception* in the policy itself (the way `single_call_fn` is). What is not allowed is the current state: an undocumented inline suppression riding on a per-site `reason`.

---

## Violations — inline suppressions your policy does not permit (14)

Verdict for every row: **VIOLATION**. "Why it's here" explains the forcing constraint only so you can judge the fix; it is not a defense. "Removal requires" is the path back to compliance (refactor), with the alternative being an explicit policy amendment.

| # | Location | Lint | Why it's here | Removal requires |
|---|---|---|---|---|
| 1 | `project/dependencies.rs:4` (module `#![allow]`) | `same_name_method` | serde `remote = "Self"` makes the derive emit inherent `serialize`/`deserialize` that the hand-written impls delegate to | Abandon the `remote = "Self"` delegation pattern — hand-write the `Serialize`/`Deserialize` bodies without routing through derive-generated inherent methods, so no name collision exists. Structural, non-trivial. |
| 2 | `project/dependencies.rs:284` | `trivially_copy_pass_by_ref` | serde calls a `skip_serializing_if` predicate as `fn(&T) -> bool`; `&bool` is mandated by that signature | The signature is fixed by serde. Either drop `skip_serializing_if = "is_false"` (serialize the bool unconditionally, or model the field as `Option`/an enum so absence is the skip), or make this a documented policy exception. |
| 3 | `sys/flock.rs:141` | `wildcard_enum_match_arm` | matches `io::ErrorKind`, which is `#[non_exhaustive]` | Replace the `match kind { AlreadyExists => …, _ => … }` with an equality test — `if io_error.kind() == io::ErrorKind::AlreadyExists { … } else { … }` — which needs no wildcard arm and no suppression. |
| 4 | `sys/flock.rs:38` (`#[expect]`) | `dead_code` | `MutexGuard` field held only for its RAII lock effect, never read | The guard must stay owned to hold the lock, but it needn't be a named-read field. Options: bind it in the constructor scope and document, or restructure `Guard` so the held guard isn't a struct field clippy sees as dead. (This is the closest to genuinely-forced of the six dead_code sites.) |
| 5 | `build/json.rs:17` | `dead_code` | `reason: Reason` validates the serde tag but is never read | Read it (e.g. assert/branch on the tag during parse), or drop the field and use `#[serde(tag = …)]`/a custom visitor that consumes the tag without binding a dead field. |
| 6 | `project/inherit.rs:13` | `dead_code` | `workspace: True` exists only to validate the `{ workspace = true }` shape | Deserialize the shape without a dead field — e.g. a unit-validating `Deserialize` impl on the struct, or consume the key in a custom visitor — so nothing is bound-but-unread. |
| 7–9 | `fuzz/.../normalize.rs:27, 34, 42` | `dead_code` (×3; :42 also `single_call_fn`) | the fuzz target white-box-`#[path]`-includes whole modules, so items it doesn't exercise look dead | `#[cfg]`-gate or trim the included surface to what the fuzz target uses, or exercise the unused items. The :42 reason correctly notes `#[expect]` can't be used (state varies fuzz-vs-lib) — but that argues for *not suppressing at all*, i.e. narrowing the include. |
| 10 | `runner.rs:591` | `unused_self` | `check_pass`/`check_compile_fail` are dispatched through one fn-pointer, so both must take `&self`; only the sibling reads it | Drop the fn-pointer dispatch (match on the `Expected` variant and call each as needed), or make `check_pass` an associated `fn` and pass what it needs explicitly, so `&self` isn't a dead receiver. |
| 11 | `runner.rs:734` | `needless_collect` | genuine clippy false positive — the `Vec` is consumed twice (`is_empty()` then `retain`); cited issue rust-clippy#5991 is **closed/fixed** | This is the hard one: it's an upstream FP, so there may be no clean refactor. Confirm against current clippy (it may no longer fire — then just delete the allow). If it still fires, restructure so clippy doesn't see a needless collect (e.g. compute the filter set once via a different shape), or make it a documented exception with a *current* justification, not a dead link. |
| 12 | `lib.rs:233` | `unexpected_cfgs` | the custom `check_cfg` cfg set by `build.rs` is unknown to the compiler's cfg check | Declare the cfg properly instead of suppressing: `[lints.rust.unexpected_cfgs] check-cfg = ['cfg(check_cfg)']` in `Cargo.toml`, or `println!("cargo::rustc-check-cfg=cfg(check_cfg)")` in `build.rs`. Clean structural fix — no suppression needed. |
| 13–14 | `fuzz/.../normalize.rs:3` (crate `#![allow]`) | `unknown_lints`, `mismatched_lifetime_syntaxes` | the `#[path]`-shared `normalize.rs` is compiled under both lib and fuzz lint sets | Resolve the lint in the shared source so neither build needs suppression (fix `mismatched_lifetime_syntaxes` at its site in `normalize.rs`; the `unknown_lints` umbrella then falls away), or gate the shared file's lint expectations by build. |

Notes on the table:
- Several have **clean refactors** that leave the code in the state the lints want — #3 (`==` instead of wildcard `match`), #12 (`check-cfg` declaration). Those should just be fixed.
- A few are **serde- or language-shaped** (#1, #2, #5, #6) — fixable, but the fix changes the (de)serialization design; if you want to keep the current serde shape you must add an explicit documented exception rather than leave an inline suppression.
- #11 (`needless_collect`) is the only one with a plausible "no clean fix exists" outcome, and even there the cited justification is stale (closed issue) and the necessity is unverified against current clippy.

---

## Permitted — `#[allow(clippy::single_call_fn, …)]` (66)

This is the one sanctioned form. The rule's bar is "a named helper **genuinely improves structure**" (vs. the disqualifier it names: "one-off wrappers that only shuffle code around"). I checked each of the 66 against that bar. All 66 clear it — every one names a real role (an API/entry boundary, a serde target, a named predicate, or a decomposed pipeline phase), which is what the rule blesses. None is a content-free wrapper. They are listed by file so you can spot-check; verdict for each is **Permitted**.

**`diagnostics/normalize.rs` (21)** — :107 `diagnostics` (entry point) · :184 `apply` (stage driver) · :268 `normalize_location` (phase) · :323/:364/:386/:589/:636 named branches of `normalize_location` · :422 `WorkspaceLines` lookahead · :441 non-location half of `apply` · :531 verbose-list-counter branch · :622/:663/:723 named predicates (OUT_DIR / rustc-lib / type-name-note shapes) · :681 cargo-registry boundary sub-step · :698 `and N others` span step · :743/:759 number-blanking/stripping mutators · :846 final pass · :898/:932 `unindent` measure/emit phases.

**`runner.rs` (12)** — :92 `run` (engine entry) · :173 `prepare` · :251 `write` · :273 `make_manifest` · :367 `resolve_edition` · :386 `merge_dependencies` · :424 `prune_features` · :449 `is_optional_dependency` (predicate) · :472 `run_all` · :587 `check_pass` · :625 `check_compile_fail` · :738 `filter`.

**`build/cargo.rs` (7)** — :84 `cargo_target_dir` · :99 `manifest_dir` · :123 `build_dependencies` · :183 `build_test` · :221 `build_all_tests` · :254 `run_test` · :273 `metadata` (the module's named cargo surface to the runner).

**`sys/flock.rs` (5)** — :62 `Lock::acquire` · :76 `Guard::acquire` · :88 `FileLock::acquire` · :137 `create` · :192 `poll` (named layers of the documented two-layer lock).

**`project/dependencies.rs` (5)** — :29 `get_manifest` · :54 `get_workspace_manifest` · :64 `try_get_workspace_manifest` · :94 `fix_patches` · :109 `fix_replacements`.

**`runner/expand.rs` (3)** — :25 `expand_globs` · :60 `ExpandedTestSet::new` · :95 `glob`. **`project/features.rs` (3)** — :18 `find` · :51 `try_find` · :122 `is_lower_hex_digit`. **`report/diff.rs` (2)** — :39/:109 `compute` (real + feature-off stub, mirrored).

**Singletons** — `sys/env.rs:25` `env` · `sys/directory.rs:32` `current` · `report/reporter.rs:20` `new` · `project/rustflags.rs:14` `toml` · `project/manifest.rs:100` `serialize_patch` (serde target) · `project.rs:85` `from_flags` · `cases.rs:23` `TestCases::new` (public API) · `fuzz/.../normalize.rs:42` `mod normalize`.

The one thing worth your attention at the class level: `single_call_fn` is your sole inline escape hatch, and it's used 66×, heavily because your *own* `excessive_nesting` / `too_many_lines` denials force extraction — which then trips `single_call_fn`, which you then allow. The policy is relieving its own pressure. That's not a violation (it's the sanctioned form, used as intended), but if you ever want to cut the ritual, the lever is to demote `single_call_fn` to a config-level `allow` with one paragraph of rationale — at the cost of the per-site role notes.

---

## Config-level lint settings (definitions, not exceptions)

These are you choosing the lint set; included only so the picture is complete. They are not governed by the "one inline form" rule. Two have comment-quality issues worth a quick fix:

- `allow_attributes` (`Cargo.toml:470`, mirrored in `fuzz/Cargo.toml:376`) — its comment asserts the `lint-attrs` stask "rejects every other `#[allow]`." **That stask is not in this repo.** The comment describes enforcement that doesn't exist — which is the root cause of the 14 violations above. Either port the stask (so the rule is actually enforced) or rewrite the comment to say enforcement is currently manual/by-review.
- `async_fn_in_trait` (`Cargo.toml:177`) — 7 lines of inherited template prose about "service/port/repository/adapter traits" that this synchronous crate does not contain. Inert, but the justification doesn't describe trybuild. Trim or drop.
- The remaining config `allow`s (`restriction`, `implicit_return`, `semicolon_inside_block`, `separated_literal_suffix`, `self_named_module_files`, `question_mark_used`, `pub_use`, `ref_binding_to_reference`, `blanket_clippy_restriction_lints`) and the `warn`s (`else_if_without_else`, `single_call_fn`, `missing_copy_implementations`) are legitimate definitions; a few of the terse `# contradicts X` one-liners could state which side wins, but none is a rule violation.

Note: the entire policy (all of the above) is **duplicated verbatim** in `fuzz/Cargo.toml` (a detached `[workspace]`), so any comment fix must be applied in both files, and the two can silently drift.

---

## Bottom line

Per your rule as written, the repo currently carries **14 inline policy violations** plus a config comment (`allow_attributes`) that advertises an enforcement gate that isn't installed. The 66 `single_call_fn` allows are compliant (the one permitted form). The fix for the 14 is to refactor each until no suppression is needed — most have a clean path (notably #3 `==`-instead-of-wildcard and #12 `check-cfg`) — or, for any you judge genuinely unavoidable, to amend the policy to name that exception explicitly instead of leaving an undocumented inline `#[allow]`.
