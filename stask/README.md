<!-- Do not edit; generated file. -->

# stask

This crate is the consumer-compiled repository-specific extension seam. It exposes only the guarded `x` router; every standard repository workflow is provided by the installed `template` binary and reached through the root `justfile`.

Local tests cover composition of this repository’s registry. Reusable runner, command, policy, update, documentation, analysis, and Cargo behavior is tested in the owning `template-rs` crate.

## Entry Point

Invoke repository extensions as `just x <name>`. The `RunnerMode::Stask` guard rejects direct unguarded execution unless `STASK_VIA_JUST=1` or `CI` is present.

Installed commands use the independent `RunnerMode::Installed` identity and may be invoked directly as `template <command>`. Installed color policy uses `TEMPLATE_COLOR`; extension-runner color policy uses `STASK_COLOR`; both may fall back to `CARGO_TERM_COLOR`.

## Generated Markdown

Generated Markdown is not composed in this crate. `template agent generate` materializes the `[guidance.source]` branch, discovers `<guidance-source-repo>/<fragments-root>/**`, overlays optional `<consumer-repo>/docs/fragments/**`, validates and renders the complete plan, writes changed targets atomically, and cleans the temporary checkout.

Edit `<consumer-repo>/docs/fragments/stask/AGENTS.md/*.md` and `<consumer-repo>/docs/fragments/stask/README.md/*.md` for this target; in this checkout those paths are `rust-template/docs/fragments/stask/AGENTS.md/*.md` and `rust-template/docs/fragments/stask/README.md/*.md`. Then run `just gen-md`. Use `just migrate-md <PATH>` to split existing maintained Markdown into target-local fragment inputs.

## Source Map

```text
stask/
|-- Cargo.toml          # template-core + template-stask composition only
|-- README.md           # generated from branch guidance plus local overlays
|-- AGENTS.md           # generated maintainer guidance
`-- src/
    |-- lib.rs          # local registry composition
    |-- main.rs         # thin call to stask::run()
    `-- extensions.rs   # repository-specific just-x registry
```

## Extending Automation

This checkout intentionally starts with `template_stask::empty_registry(...)`, so `just x` remains a real extension surface even when no repository-specific command is registered.

Add a local command with `template_stask::extension_command(...)`, assemble unique nested commands with `template_stask::registry(...)`, and dispatch into a consumer-owned enum and handler. Do not add a standard installed command set or move reusable behavior into this local crate.

Changes needed by more than one repository belong in the capability-owning `template-rs` crate and are exposed through `template-cli` when they are standard commands.

## Workspace Setup

A consumer workspace needs the installed `template` executable for standard workflows and Git dependencies on `template-core` plus `template-stask` only when it retains repository-specific `x` extensions. The root `justfile` dispatches standard recipes to `template` and reserves local compilation for `just x`.

Run `just init` once to install developer tools and the `template precommit` hook launcher.
