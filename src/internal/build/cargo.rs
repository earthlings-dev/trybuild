//! Constructing and invoking `cargo` subprocesses: building or checking the
//! synthesized test binaries, running pass-tests, and reading `cargo metadata`.

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::iter;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;

use serde_derive::Deserialize;
use target_triple::TARGET;

use crate::internal::build::BuildError;
use crate::internal::build::BuildOutput;
use crate::internal::build::MetadataFailure;
use crate::internal::build::Result;
use crate::internal::error;
use crate::internal::model::Name;
use crate::internal::project::KeepGoing;
use crate::internal::project::Project;
use crate::internal::project::rustflags;
use crate::internal::sys::SysError;
use crate::internal::sys::directory::Directory;

/// The subset of `cargo metadata --format-version=1` output trybuild reads.
#[derive(Deserialize)]
pub(in crate::internal) struct Metadata {
  /// The workspace's target directory.
  pub target_directory: Directory,
  /// The directory containing the workspace root manifest.
  pub workspace_root:   Directory,
  /// One entry per workspace member package.
  pub packages:         Vec<PackageMetadata>,
}

/// The subset of a single package's `cargo metadata` entry trybuild reads.
#[derive(Deserialize)]
pub(in crate::internal) struct PackageMetadata {
  /// The package name.
  pub name:          String,
  /// The package's build targets, inspected to detect a library target.
  pub targets:       Vec<BuildTarget>,
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
  reason = "names the CARGO_TARGET_DIR override and documents why it redirects artifacts, keeping cargo_with_rustflags's `.envs(...)` \
            call readable"
)]
fn cargo_target_dir(project: &Project) -> impl Iterator<Item = (&'static str, PathBuf)> {
  iter::once(("CARGO_TARGET_DIR", path!(project.target_dir / "tests" / "trybuild")))
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
  manifest_dir_from(env::var_os("CARGO_MANIFEST_DIR"))
}

/// Locates the crate-under-test manifest directory from an injected
/// `CARGO_MANIFEST_DIR` value, or from the current directory when absent.
#[allow(
  clippy::single_call_fn,
  reason = "the injectable manifest-dir policy is split from the environment-reading shell for focused unit coverage"
)]
fn manifest_dir_from(configured_manifest_dir: Option<OsString>) -> error::Result<Directory> {
  if let Some(manifest_dir) = configured_manifest_dir {
    return Ok(Directory::from(manifest_dir));
  }
  find_manifest_dir(Directory::current().map_err(SysError::Io)?)
}

/// Walks up from `dir` until a directory containing `Cargo.toml` is found.
#[allow(
  clippy::single_call_fn,
  reason = "the manifest walk is a pure filesystem policy seam tested apart from CARGO_MANIFEST_DIR parsing"
)]
fn find_manifest_dir(mut dir: Directory) -> error::Result<Directory> {
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
    .arg(if project.selected.has_pass() { "build" } else { "check" })
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
    .arg(if project.selected.has_pass() { "build" } else { "check" })
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
    .arg(if project.selected.has_pass() { "build" } else { "check" })
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
pub(in crate::internal) fn run_test(project: &Project, name: &Name, executable_path: Option<&Path>) -> Result<Output> {
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
    vec!["--no-default-features".to_owned(), "--features".to_owned(), features.join(",")]
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

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap as Map;
  use std::fs;
  #[cfg(unix)]
  use std::os::unix::fs::PermissionsExt as _;
  use std::path::Path;
  use std::path::PathBuf;
  use std::result::Result as StdResult;

  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;

  use super::*;
  use crate::internal::project::KeepGoing;
  use crate::internal::project::Selected;
  use crate::internal::project::manifest::Bin;
  use crate::internal::project::manifest::Edition;
  use crate::internal::project::manifest::Manifest;
  use crate::internal::project::manifest::Package;
  use crate::internal::sys::env::Update;

  fn project(fixture: &TempDir, project_name: &str, selected: Selected, features: Option<Vec<String>>) -> Project {
    Project {
      dir: Directory::new(fixture.child("project")),
      source_dir: Directory::new(fixture.child("source")),
      target_dir: Directory::new(fixture.child("target")),
      name: project_name.to_owned(),
      update: Update::Verify,
      selected,
      features,
      workspace: Directory::new(fixture.child("workspace")),
      path_dependencies: Vec::new(),
      manifest: Manifest {
        cargo_features: Vec::new(),
        package:        Package {
          name:     project_name.to_owned(),
          version:  "0.0.0".to_owned(),
          edition:  Edition::default(),
          resolver: None,
          publish:  false,
        },
        features:       iter::once(("extra".to_owned(), Vec::new())).collect(),
        dependencies:   Map::new(),
        target:         Map::new(),
        bins:           vec![Bin {
          name: Name(project_name.to_owned()),
          path: PathBuf::from("main.rs"),
        }],
        workspace:      None,
        patch:          Map::new(),
        replace:        Map::new(),
      },
      keep_going: KeepGoing::No,
    }
  }

  fn write_project(project: &Project, main_rs: &str) -> StdResult<(), TestFailure> {
    ensure_ok_source(fs::create_dir_all(project.dir.as_ref()), "project dir can be created")?;
    ensure_ok_source(fs::create_dir_all(project.workspace.as_ref()), "workspace dir can be created")?;
    ensure_ok_source(
      fs::write(
        project.dir.join("Cargo.toml"),
        format!(
          r#"[package]
name = "fixture"
version = "0.0.0"
edition = "2024"

[features]
extra = []

[[bin]]
name = "{}"
path = "main.rs"
"#,
          project.name
        ),
      ),
      "project manifest can be written",
    )?;
    ensure_ok_source(fs::write(project.dir.join("main.rs"), main_rs), "project main can be written")
  }

  #[test]
  fn manifest_dir_from_uses_the_env_value_when_present() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("manifest-env")?;
    let found = ensure_ok_source(
      manifest_dir_from(Some(fixture.path().to_path_buf().into_os_string())),
      "manifest dir resolves from the injected env value",
    )?;

    ensure(
      found.as_ref() == Directory::new(fixture.path()).as_ref(),
      "injected CARGO_MANIFEST_DIR is used directly",
    )
  }

  #[test]
  fn manifest_dir_from_walks_from_current_dir_when_env_is_absent() -> StdResult<(), TestFailure> {
    let found = ensure_ok_source(
      manifest_dir_from(None),
      "manifest dir falls back to walking from the current directory",
    )?;

    ensure(found.join("Cargo.toml").exists(), "the fallback manifest dir contains Cargo.toml")
  }

  #[test]
  fn find_manifest_dir_walks_up_to_the_nearest_manifest() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("manifest-walk")?;
    let nested = fixture.child("nested/deeper");
    ensure_ok_source(fs::create_dir_all(&nested), "nested fixture directory can be created")?;
    ensure_ok_source(
      fs::write(fixture.child("Cargo.toml"), "[package]\nname = \"fixture\"\n"),
      "fixture manifest can be written",
    )?;

    let found = ensure_ok_source(
      find_manifest_dir(Directory::new(&nested)),
      "manifest dir walk finds the nearest Cargo.toml",
    )?;

    ensure(
      found.as_ref() == Directory::new(fixture.path()).as_ref(),
      "manifest dir walk returns the ancestor containing Cargo.toml",
    )
  }

  #[test]
  fn find_manifest_dir_errors_when_no_manifest_exists() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("manifest-missing")?;

    ensure_all(&[
      (
        find_manifest_dir(Directory::new(fixture.path())).is_err(),
        "manifest dir walk reports an error when no Cargo.toml is found",
      ),
      (
        !fixture.child("Cargo.toml").exists(),
        "the missing-manifest fixture stays free of Cargo.toml",
      ),
    ])
  }

  #[test]
  fn build_dependencies_generates_lockfile_and_records_keep_going() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("build-dependencies")?;
    let mut project = project(&fixture, "demo-tests", Selected::CompileFailOnly, Some(vec!["extra".to_owned()]));
    write_project(&project, "fn main() {}\n")?;

    let result = build_dependencies(&mut project);

    ensure_all(&[
      (result.is_ok(), "dependency build succeeds for a tiny generated project"),
      (
        project.dir.join("Cargo.lock").exists(),
        "missing workspace lockfiles are regenerated inside the generated project",
      ),
      (
        project.keep_going == KeepGoing::Yes,
        "modern cargo support for --keep-going is recorded",
      ),
    ])
  }

  #[test]
  fn build_dependencies_reports_dependency_build_errors() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("build-dependencies-error")?;
    let mut project = project(&fixture, "demo-tests", Selected::CompileFailOnly, None);
    write_project(&project, "compile_error!(\"dependency build failed\");\n")?;

    ensure(
      build_dependencies(&mut project).is_err(),
      "dependency build failures are captured as data",
    )
  }

  #[test]
  fn build_dependencies_ignores_non_missing_workspace_lockfile_errors() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("build-dependencies-lock-error")?;
    let mut project = project(&fixture, "demo-tests", Selected::CompileFailOnly, None);
    write_project(&project, "fn main() {}\n")?;
    let workspace_file = fixture.child("workspace-file");
    ensure_ok_source(fs::write(&workspace_file, ""), "workspace-file fixture can be written")?;
    project.workspace = Directory::new(workspace_file);

    ensure(
      build_dependencies(&mut project).is_ok(),
      "non-missing lockfile open errors are ignored in favor of the cargo build result",
    )
  }

  #[test]
  fn run_test_falls_back_to_cargo_run_when_executable_is_absent() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("run-test-cargo")?;
    let project = project(&fixture, "demo-tests", Selected::PassOnly, None);
    write_project(&project, "fn main() { println!(\"fallback\"); }\n")?;

    let output = ensure_ok_source(
      run_test(&project, &Name("demo-tests".to_owned()), None),
      "cargo-run fallback succeeds for a tiny generated project",
    )?;

    ensure_all(&[
      (output.status.success(), "cargo-run fallback exits successfully"),
      (
        String::from_utf8_lossy(&output.stdout).contains("fallback"),
        "cargo-run fallback captures stdout",
      ),
    ])
  }

  #[test]
  fn build_entrypoints_capture_single_and_batched_json_output() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("build-entrypoints")?;
    let compile_only = project(&fixture, "demo-tests", Selected::CompileFailOnly, None);
    write_project(&compile_only, "fn main() {}\n")?;
    let pass_build = project(&fixture, "demo-tests", Selected::PassOnly, None);

    let single = ensure_ok_source(
      build_test(&compile_only, &Name("demo-tests".to_owned())),
      "single-test build entrypoint succeeds",
    )?;
    let batched_check = ensure_ok_source(build_all_tests(&compile_only), "batched compile-fail build entrypoint succeeds")?;
    let batched_build = ensure_ok_source(build_all_tests(&pass_build), "batched pass-test build entrypoint succeeds")?;

    ensure_all(&[
      (single.status.success(), "single-test build exits successfully"),
      (batched_check.status.success(), "batched compile-fail check exits successfully"),
      (batched_build.status.success(), "batched pass-test build exits successfully"),
    ])
  }

  #[cfg(unix)]
  #[test]
  fn run_test_prefers_reported_executable_paths() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("run-test-executable")?;
    let project = project(&fixture, "demo-tests", Selected::PassOnly, None);
    ensure_ok_source(fs::create_dir_all(project.dir.as_ref()), "project dir can be created")?;
    let executable = fixture.child("reported-executable.sh");
    ensure_ok_source(
      fs::write(&executable, "#!/bin/sh\nprintf direct\n"),
      "reported executable can be written",
    )?;
    let mut permissions = ensure_ok_source(fs::metadata(&executable), "reported executable metadata can be read")?.permissions();
    permissions.set_mode(0o755);
    ensure_ok_source(
      fs::set_permissions(&executable, permissions),
      "reported executable can be made executable",
    )?;

    let output = ensure_ok_source(
      run_test(&project, &Name("demo-tests".to_owned()), Some(Path::new(&executable))),
      "reported executable path is runnable",
    )?;

    ensure_all(&[
      (output.status.success(), "reported executable exits successfully"),
      (
        String::from_utf8_lossy(&output.stdout) == "direct",
        "reported executable output is captured directly",
      ),
    ])
  }
}
