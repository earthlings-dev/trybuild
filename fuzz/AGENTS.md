# AGENTS.md

Scope: `fuzz/` — a standalone `cargo-fuzz` package (its own `[workspace]`, not a member of the parent crate's workspace; `name = "fuzz"`, `publish = false`, version tracks the parent trybuild version). Root `AGENTS.md` owns the overall command matrix.

## The one target: `normalize`

`fuzz_targets/normalize.rs` fuzzes the diagnostics normalizer (`normalize::diagnostics`). It does **not** depend on the trybuild crate; instead it white-box reconstructs the needed slice of the library's private `crate::internal` module tree with `#[path]` declarations mirroring `src/internal.rs`, `src/internal/sys.rs`, and `src/internal/diagnostics.rs`, so the engine files' `pub(in crate::internal)` visibilities and `crate::internal::…` paths resolve exactly as under `lib.rs` and `src/` needs no fuzz-only edits. The `fuzz_target!` entry lives inside that reconstructed module for the same visibility reason.

Consequence for library work: the files this target `#[path]`-includes (notably `src/internal/diagnostics/normalize.rs`) must stay free of test wiring — that is why the normalizer snapshot tests are declared from `src/internal/diagnostics.rs` instead (see `../src/AGENTS.md`).

## Commands

Requires nightly plus `cargo-fuzz`:

```sh
cargo fuzz check           # build the target without running
cargo fuzz run normalize   # fuzz
```

## Manifest notes

`fuzz/Cargo.toml` embeds the same strict rustc/rustdoc/clippy lint policy as the parent crate (unsafe forbidden, pedantic/nursery at deny, panic/unwrap/print bans), so fuzz-target code is held to library standards. Dependencies: `libfuzzer-sys`, `automod`, `serde`/`serde_derive`, and the `strict-test-support` git dependency.
