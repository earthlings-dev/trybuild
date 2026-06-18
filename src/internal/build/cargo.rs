//! Constructing and invoking `cargo` subprocesses: building or checking the
//! synthesized test binaries, running pass-tests, and reading `cargo metadata`.

use crate::internal::build::{BuildError, BuildOutput, MetadataFailure, Result};
use crate::internal::error;
use crate::internal::model::Name;
use crate::internal::project::KeepGoing;
use crate::internal::project::Project;
use crate::internal::project::rustflags;
use crate::internal::sys::SysError;
use crate::internal::sys::directory::Directory;
use serde_derive::Deserialize;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::{env, io, iter};
use target_triple::TARGET;

/// The subset of `cargo metadata --format-version=1` output trybuild reads.
#[derive(Deserialize)]
pub(in crate::internal) struct Metadata {
    /// The workspace's target directory.
    pub target_directory: Directory,
    /// The directory containing the workspace root manifest.
    pub workspace_root: Directory,
    /// One entry per workspace member package.
    pub packages: Vec<PackageMetadata>,
}

/// The subset of a single package's `cargo metadata` entry trybuild reads.
#[derive(Deserialize)]
pub(in crate::internal) struct PackageMetadata {
    /// The package name.
    pub name: String,
    /// The package's build targets, inspected to detect a library target.
    pub targets: Vec<BuildTarget>,
    /// Absolute path to the package's `Cargo.toml`.
    pub manifest_path: PathBuf,
}

/// One build target of a package, reduced to the field trybuild inspects.
#[derive(Deserialize)]
pub(in crate::internal) struct BuildTarget {
    /// The target's crate types, e.g. `["lib"]` or `["bin"]`.
    pub crate_types: Vec<String>,
}

/// The `cargo` command to invoke, honoring the `CARGO` environment variable.
fn raw_cargo() -> Command {
    env::var_os("CARGO").map_or_else(|| Command::new("cargo"), Command::new)
}

/// A `cargo` command pre-configured to operate on `project` with trybuild's
/// default rustflags.
fn cargo(project: &Project) -> Command {
    cargo_with_rustflags(project, &[])
}

/// A `cargo` command for `project` whose rustflags are trybuild's defaults plus
/// `extra_rustflags`.
///
/// Rustflags are injected via `--config` (and `RUSTFLAGS` is removed from the
/// environment) so they still reach the test crates when `--target` is passed.
fn cargo_with_rustflags(project: &Project, extra_rustflags: &[&'static str]) -> Command {
    let rustflags = rustflags::toml(extra_rustflags);
    let mut cmd = raw_cargo();
    // The builder methods configure `cmd` in place and return `&mut Command`
    // only to enable chaining; bind the final reborrow so the result is not a
    // discarded expression statement.
    let _: &mut Command = cmd
        .current_dir(&project.dir)
        .envs(cargo_target_dir(project))
        .env_remove("RUSTFLAGS")
        .env("CARGO_INCREMENTAL", "0")
        .arg("--offline")
        .arg(format!("--config=build.rustflags={rustflags}"))
        .arg(format!("--config=target.{TARGET}.rustflags={rustflags}"));
    cmd
}

/// The `CARGO_TARGET_DIR` override that places the generated project's build
/// artifacts under the host project's own `tests/trybuild` directory.
#[allow(
    clippy::single_call_fn,
    reason = "names the CARGO_TARGET_DIR override and documents why it redirects artifacts, keeping cargo_with_rustflags's `.envs(...)` call readable"
)]
fn cargo_target_dir(project: &Project) -> impl Iterator<Item = (&'static str, PathBuf)> {
    iter::once((
        "CARGO_TARGET_DIR",
        path!(project.target_dir / "tests" / "trybuild"),
    ))
}

/// Locates the crate-under-test's manifest directory.
///
/// Uses `CARGO_MANIFEST_DIR` when set (the normal case under `cargo test`),
/// otherwise walks up from the current directory looking for a `Cargo.toml`.
#[allow(
    clippy::single_call_fn,
    reason = "locating the crate-under-test manifest is a distinct domain step the orchestrator calls by name"
)]
pub(in crate::internal) fn manifest_dir() -> error::Result<Directory> {
    if let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR") {
        return Ok(Directory::from(manifest_dir));
    }
    let mut dir = Directory::current().map_err(SysError::Io)?;
    loop {
        if dir.join("Cargo.toml").exists() {
            return Ok(dir);
        }
        dir = dir.parent().ok_or(BuildError::ProjectDir)?;
    }
}

/// Seeds the generated project's lockfile, builds its dependencies once up
/// front, and probes whether this cargo supports `--keep-going`.
///
/// Copies the workspace `Cargo.lock` if present (or has cargo generate one),
/// then builds the placeholder `main.rs` bin so dependency compilation happens
/// before the per-test builds. Records the detected `--keep-going` capability
/// on `project`, enabling the batched build fast path.
#[allow(
    clippy::single_call_fn,
    reason = "one of the named cargo build/check entry points forming this module's surface to the runner"
)]
pub(in crate::internal) fn build_dependencies(project: &mut Project) -> Result<()> {
    // Try copying or generating lockfile.
    match File::open(path!(project.workspace / "Cargo.lock")) {
        Ok(mut workspace_cargo_lock) => {
            if let Ok(mut new_cargo_lock) = File::create(path!(project.dir / "Cargo.lock")) {
                // Not fs::copy in order to avoid producing a read-only destination
                // file if the source file happens to be read-only. Best-effort:
                // if the copy does not complete, cargo regenerates the lockfile.
                let _seeded = io::copy(&mut workspace_cargo_lock, &mut new_cargo_lock);
            }
        }
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound {
                // Best-effort: a failure surfaces from the build invocation below.
                // Captured (not inherited) so `try_run` writes nothing to the terminal.
                let _generated = cargo(project).arg("generate-lockfile").output();
            }
        }
    }

    let mut command = cargo(project);
    let _: &mut Command = command
        .arg(if project.selected.has_pass() {
            "build"
        } else {
            "check"
        })
        .args(target())
        .arg("--bin")
        .arg(&project.name)
        .args(features(project));

    // Captured rather than inherited so the typed core stays terminal-free; a
    // failure carries cargo's output as data instead of leaking it.
    let output = command.output().map_err(BuildError::Cargo)?;
    if !output.status.success() {
        return Err(BuildError::DependencyBuild(Box::new(BuildOutput {
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        })));
    }

    // Check if this Cargo contains https://github.com/rust-lang/cargo/pull/10383
    let supports_keep_going = command
        .arg("--keep-going")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|exit| exit.success());
    project.keep_going = if supports_keep_going {
        KeepGoing::Yes
    } else {
        KeepGoing::No
    };

    // Best-effort suite-level clean: dependency artifacts remain reusable, while
    // stale generated-package diagnostics from a prior run are cleared once.
    let _cleaned = cargo(project)
        .arg("clean")
        .arg("--package")
        .arg(&project.name)
        .arg("--color=never")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    Ok(())
}

/// Builds (or checks) a single named test bin, capturing its JSON diagnostics.
///
/// The suite has already cleaned the generated package once, so rustc emits
/// diagnostics without forcing every fixture to throw away prior bin builds.
#[allow(
    clippy::single_call_fn,
    reason = "a named cargo build entry point on this module's surface to the runner, deliberately parallel to build_all_tests"
)]
pub(in crate::internal) fn build_test(project: &Project, name: &Name) -> Result<Output> {
    cargo_with_rustflags(project, &["--diagnostic-width=140"])
        .arg(if project.selected.has_pass() {
            "build"
        } else {
            "check"
        })
        .args(target())
        .arg("--bin")
        .arg(name)
        .args(features(project))
        .arg("--quiet")
        .arg("--color=never")
        .arg("--message-format=json")
        .output()
        .map_err(BuildError::Cargo)
}

/// Builds all test bins at once with `--keep-going`, capturing the combined
/// JSON diagnostics.
///
/// The batched fast path taken when every case is `compile_fail` and cargo
/// supports `--keep-going`; the suite-level clean has already made diagnostics
/// fresh for this run.
#[allow(
    clippy::single_call_fn,
    reason = "the batched --keep-going build entry point, deliberately parallel to build_test on this module's runner-facing surface"
)]
pub(in crate::internal) fn build_all_tests(project: &Project) -> Result<Output> {
    cargo_with_rustflags(project, &["--diagnostic-width=140"])
        .arg(if project.selected.has_pass() {
            "build"
        } else {
            "check"
        })
        .args(target())
        .arg("--bins")
        .args(features(project))
        .arg("--quiet")
        .arg("--color=never")
        .arg("--message-format=json")
        .arg("--keep-going")
        .output()
        .map_err(BuildError::Cargo)
}

/// Runs a successfully built pass-test's binary and captures its output.
///
/// Prefer the executable path cargo reported in the build JSON, avoiding a
/// second cargo invocation after the diagnostic build has already produced the
/// binary. Fall back to `cargo run` only when cargo omitted that artifact path.
#[allow(
    clippy::single_call_fn,
    reason = "the pass-test execution entry point on this module's cargo surface, kept beside the build entry points it complements"
)]
pub(in crate::internal) fn run_test(
    project: &Project,
    name: &Name,
    executable_path: Option<&Path>,
) -> Result<Output> {
    if let Some(path) = executable_path {
        return run_built_executable(project, path);
    }
    run_via_cargo(project, name)
}

/// Runs the already-built test executable directly.
#[allow(
    clippy::single_call_fn,
    reason = "the normal pass-test runtime path; split from the cargo-run compatibility fallback"
)]
fn run_built_executable(project: &Project, executable: &Path) -> Result<Output> {
    let mut command = Command::new(executable);
    let _: &mut Command = command
        .current_dir(&project.dir)
        .envs(cargo_target_dir(project))
        .env_remove("RUSTFLAGS")
        .env("CARGO_INCREMENTAL", "0");
    command.output().map_err(BuildError::Cargo)
}

/// Compatibility fallback for cargo versions or edge cases that omit an
/// executable path from the build JSON.
#[allow(
    clippy::single_call_fn,
    reason = "compatibility fallback kept separate from the normal direct-executable path"
)]
fn run_via_cargo(project: &Project, name: &Name) -> Result<Output> {
    cargo(project)
        .arg("run")
        .args(target())
        .arg("--bin")
        .arg(name)
        .args(features(project))
        .arg("--quiet")
        .arg("--color=never")
        .output()
        .map_err(BuildError::Cargo)
}

/// Runs `cargo metadata --no-deps` and deserializes the [`Metadata`] trybuild
/// needs; cargo's stderr is captured into the error if deserialization fails.
#[allow(
    clippy::single_call_fn,
    reason = "the `cargo metadata` entry point the orchestrator calls to discover the workspace, a named domain boundary"
)]
pub(in crate::internal) fn metadata() -> Result<Metadata> {
    let output = raw_cargo()
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version=1")
        .output()
        .map_err(BuildError::Cargo)?;

    serde_json::from_slice(&output.stdout).map_err(|source| {
        BuildError::Metadata(Box::new(MetadataFailure {
            source,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }))
    })
}

/// The `--no-default-features` / `--features` arguments selecting the active
/// feature set, or no arguments when no features were detected.
fn features(project: &Project) -> Vec<String> {
    project.features.as_ref().map_or_else(Vec::new, |features| {
        vec![
            "--no-default-features".to_owned(),
            "--features".to_owned(),
            features.join(","),
        ]
    })
}

/// The `--target <host triple>` arguments passed to cargo by default.
///
/// Empty under the `trybuild_no_target` cfg; the inline comment explains why a
/// coverage build needs that escape hatch.
fn target() -> Vec<&'static str> {
    // When --target flag is passed, cargo does not pass RUSTFLAGS to rustc when
    // building proc-macro and build script even if the host and target triples
    // are the same. Therefore, if we always pass --target to cargo, tools such
    // as coverage that require RUSTFLAGS do not work for tests run by trybuild.
    //
    // To avoid that problem, do not pass --target to cargo if we know that it
    // has not been passed.
    //
    // Currently, cargo does not have a way to tell the build script whether
    // --target has been passed or not, and there is no heuristic that can
    // handle this well.
    //
    // Therefore, expose a cfg to always treat the target as host.
    if cfg!(trybuild_no_target) {
        vec![]
    } else {
        vec!["--target", TARGET]
    }
}
