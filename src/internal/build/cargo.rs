//! Constructing and invoking `cargo` subprocesses: building or checking the
//! synthesized test binaries, running pass-tests, and reading `cargo metadata`.

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::iter;
use std::path::Path;
use std::path::PathBuf;

use serde_derive::Deserialize;
use strict_standard::EnvironmentChange;
use strict_standard::OutputPolicy;
use strict_standard::ProcessExecutor;
use strict_standard::ProcessOutput;
use strict_standard::ProcessRequest;
use strict_standard::SystemEffects;
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

/// Deterministic Cargo request planner and executor.
#[derive(Debug)]
struct CargoContext<E> {
  /// Shared process capability.
  executor:            E,
  /// Cargo program selected by the host or a deterministic test.
  program:             OsString,
  /// Host `RUSTFLAGS` value incorporated into generated `--config` arguments.
  inherited_rustflags: Option<OsString>,
}

impl<E> CargoContext<E>
where
  E: ProcessExecutor,
{
  /// Construct an unconfigured Cargo request.
  fn raw_request<I, S>(&self, arguments: I) -> ProcessRequest
  where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
  {
    let mut request = ProcessRequest::new(self.program.clone(), arguments);
    request.stdout = OutputPolicy::Capture;
    request.stderr = OutputPolicy::Capture;
    request
  }

  /// Construct a project Cargo request with trybuild's deterministic environment
  /// and Rust flags.
  fn project_request<I, S>(&self, project: &Project, extra_rustflags: &[&'static str], arguments: I) -> ProcessRequest
  where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
  {
    let rustflags = rustflags::toml_from(self.inherited_rustflags.clone(), extra_rustflags);
    let mut request = self.raw_request(
      [
        OsString::from("--offline"),
        OsString::from(format!("--config=build.rustflags={rustflags}")),
        OsString::from(format!("--config=target.{TARGET}.rustflags={rustflags}")),
      ]
      .into_iter()
      .chain(arguments.into_iter().map(Into::into)),
    );
    configure_project_process(&mut request, project);
    request
  }

  /// Execute one planned request.
  fn execute(&self, request: &ProcessRequest) -> Result<ProcessOutput> {
    self.executor.execute(request).map_err(BuildError::Cargo)
  }
}

/// Apply the generated project's deterministic working directory and process
/// environment to either a Cargo invocation or a directly executed test bin.
fn configure_project_process(request: &mut ProcessRequest, project: &Project) {
  request.current_dir = Some(project.dir.as_ref().to_path_buf());
  request.environment.extend([
    EnvironmentChange::Set {
      name:  "CARGO_TARGET_DIR".into(),
      value: path!(project.target_dir / "tests" / "trybuild").into_os_string(),
    },
    EnvironmentChange::Remove {
      name: "RUSTFLAGS".into()
    },
    EnvironmentChange::Set {
      name:  "CARGO_INCREMENTAL".into(),
      value: "0".into(),
    },
  ]);
}

/// Build a production Cargo context from current host observations.
fn system_cargo() -> CargoContext<SystemEffects> {
  CargoContext {
    executor:            SystemEffects,
    program:             env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")),
    inherited_rustflags: env::var_os("RUSTFLAGS"),
  }
}

/// Locates the crate-under-test's manifest directory.
///
/// Uses `CARGO_MANIFEST_DIR` when set (the normal case under `cargo test`),
/// otherwise walks up from the current directory looking for a `Cargo.toml`.
#[allow(
  clippy::single_call_fn,
  reason = "manifest discovery is the production boundary that resolves the crate-under-test root from host observations"
)]
pub(in crate::internal) fn manifest_dir() -> error::Result<Directory> {
  manifest_dir_from(env::var_os("CARGO_MANIFEST_DIR"))
}

/// Locates the crate-under-test manifest directory from an injected
/// `CARGO_MANIFEST_DIR` value, or from the current directory when absent.
#[allow(
  clippy::single_call_fn,
  reason = "injected manifest discovery separates explicit Cargo configuration from the upward filesystem search policy"
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
  reason = "the upward manifest walk defines the fallback root-discovery rule independently of environment parsing"
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
  reason = "dependency preparation binds production Cargo execution to lockfile seeding, keep-going detection, and suite cleanup"
)]
pub(in crate::internal) fn build_dependencies(project: &mut Project) -> Result<()> {
  build_dependencies_with(&system_cargo(), project)
}

/// Execute dependency preparation through an injected Cargo context.
#[allow(
  clippy::single_call_fn,
  reason = "injected dependency preparation defines the ordered Cargo request protocol and its terminal failure boundary"
)]
fn build_dependencies_with<E>(cargo: &CargoContext<E>, project: &mut Project) -> Result<()>
where
  E: ProcessExecutor,
{
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
        let request = cargo.project_request(project, &[], [OsString::from("generate-lockfile")]);
        let _generated = cargo.execute(&request);
      }
    }
  }

  let mut arguments = vec![OsString::from(if project.selected.has_pass() { "build" } else { "check" })];
  arguments.extend(target().into_iter().map(OsString::from));
  arguments.extend([OsString::from("--bin"), OsString::from(project.name.as_str())]);
  arguments.extend(features(project).into_iter().map(OsString::from));
  let request = cargo.project_request(project, &[], arguments);

  // Captured rather than inherited so the typed core stays terminal-free; a
  // failure carries cargo's output as data instead of leaking it.
  let output = cargo.execute(&request)?;
  if !output.status.success() {
    return Err(BuildError::DependencyBuild(Box::new(BuildOutput {
      output: String::from_utf8_lossy(&output.stderr).into_owned(),
    })));
  }

  // Check if this Cargo contains https://github.com/rust-lang/cargo/pull/10383
  let mut keep_going_request = request;
  keep_going_request.arguments.push("--keep-going".into());
  keep_going_request.stdout = OutputPolicy::Discard;
  keep_going_request.stderr = OutputPolicy::Discard;
  let supports_keep_going = cargo
    .execute(&keep_going_request)
    .is_ok_and(|observed| observed.status.success());
  project.keep_going = if supports_keep_going {
    KeepGoing::Yes
  } else {
    KeepGoing::No
  };

  // Best-effort suite-level clean: dependency artifacts remain reusable, while
  // stale generated-package diagnostics from a prior run are cleared once.
  let mut clean_request = cargo.project_request(project, &[], [
    OsString::from("clean"),
    OsString::from("--package"),
    OsString::from(project.name.as_str()),
    OsString::from("--color=never"),
  ]);
  clean_request.stdout = OutputPolicy::Discard;
  clean_request.stderr = OutputPolicy::Discard;
  let _cleaned = cargo.execute(&clean_request);

  Ok(())
}

/// Builds (or checks) a single named test bin, capturing its JSON diagnostics.
///
/// The suite has already cleaned the generated package once, so rustc emits
/// diagnostics without forcing every fixture to throw away prior bin builds.
#[allow(
  clippy::single_call_fn,
  reason = "single-case build binds production Cargo execution to the named-bin diagnostic request contract"
)]
pub(in crate::internal) fn build_test(project: &Project, name: &Name) -> Result<ProcessOutput> {
  build_test_with(&system_cargo(), project, name)
}

/// Execute one named test build through an injected Cargo context.
#[allow(
  clippy::single_call_fn,
  reason = "injected single-case build constructs the exact named-bin diagnostic request independently of host execution"
)]
fn build_test_with<E>(cargo: &CargoContext<E>, project: &Project, name: &Name) -> Result<ProcessOutput>
where
  E: ProcessExecutor,
{
  let arguments = build_arguments(project, BuildSelection::Named(name), false);
  let request = cargo.project_request(project, &["--diagnostic-width=140"], arguments);
  cargo.execute(&request)
}

/// Builds all test bins at once with `--keep-going`, capturing the combined
/// JSON diagnostics.
///
/// The batched fast path taken when every case is `compile_fail` and cargo
/// supports `--keep-going`; the suite-level clean has already made diagnostics
/// fresh for this run.
#[allow(
  clippy::single_call_fn,
  reason = "batched build binds production Cargo execution to the all-bins keep-going diagnostic contract"
)]
pub(in crate::internal) fn build_all_tests(project: &Project) -> Result<ProcessOutput> {
  build_all_tests_with(&system_cargo(), project)
}

/// Execute the batched test build through an injected Cargo context.
#[allow(
  clippy::single_call_fn,
  reason = "injected batched build constructs the all-bins keep-going Cargo grammar independently of host execution"
)]
fn build_all_tests_with<E>(cargo: &CargoContext<E>, project: &Project) -> Result<ProcessOutput>
where
  E: ProcessExecutor,
{
  let arguments = build_arguments(project, BuildSelection::All, true);
  let request = cargo.project_request(project, &["--diagnostic-width=140"], arguments);
  cargo.execute(&request)
}

/// Runs a successfully built pass-test's binary and captures its output.
///
/// Prefer the executable path cargo reported in the build JSON, avoiding a
/// second cargo invocation after the diagnostic build has already produced the
/// binary. Fall back to `cargo run` only when cargo omitted that artifact path.
#[allow(
  clippy::single_call_fn,
  reason = "pass-test execution binds production process effects to the direct-binary and Cargo-fallback selection policy"
)]
pub(in crate::internal) fn run_test(project: &Project, name: &Name, executable_path: Option<&Path>) -> Result<ProcessOutput> {
  run_test_with(&system_cargo(), project, name, executable_path)
}

/// Execute one pass test through an injected Cargo context.
#[allow(
  clippy::single_call_fn,
  reason = "injected pass-test execution selects between the direct executable request and Cargo fallback without host coupling"
)]
fn run_test_with<E>(cargo: &CargoContext<E>, project: &Project, name: &Name, executable_path: Option<&Path>) -> Result<ProcessOutput>
where
  E: ProcessExecutor,
{
  if let Some(path) = executable_path {
    let request = built_executable_request(project, path);
    return cargo.execute(&request);
  }
  let request = cargo_run_request(cargo, project, name);
  cargo.execute(&request)
}

/// Construct a direct request for one already-built pass-test executable.
#[allow(
  clippy::single_call_fn,
  reason = "this planner defines the direct pass-test execution contract separately from the Cargo fallback grammar"
)]
fn built_executable_request(project: &Project, executable: &Path) -> ProcessRequest {
  let mut request = ProcessRequest::new(executable, iter::empty::<OsString>());
  configure_project_process(&mut request, project);
  request.stdout = OutputPolicy::Capture;
  request.stderr = OutputPolicy::Capture;
  request
}

/// Compatibility fallback for cargo versions or edge cases that omit an
/// executable path from the build JSON.
#[allow(
  clippy::single_call_fn,
  reason = "this planner preserves the distinct Cargo-run fallback grammar used only when build JSON omits an executable"
)]
fn cargo_run_request<E>(cargo: &CargoContext<E>, project: &Project, name: &Name) -> ProcessRequest
where
  E: ProcessExecutor,
{
  let mut arguments = vec![OsString::from("run")];
  arguments.extend(target().into_iter().map(OsString::from));
  arguments.extend([OsString::from("--bin"), name.as_ref().to_os_string()]);
  arguments.extend(features(project).into_iter().map(OsString::from));
  arguments.extend([OsString::from("--quiet"), OsString::from("--color=never")]);
  cargo.project_request(project, &[], arguments)
}

/// Runs `cargo metadata --no-deps` and deserializes the [`Metadata`] trybuild
/// needs; cargo's stderr is captured into the error if deserialization fails.
#[allow(
  clippy::single_call_fn,
  reason = "metadata discovery binds production Cargo execution to trybuild's workspace-fact decoding boundary"
)]
pub(in crate::internal) fn metadata() -> Result<Metadata> {
  metadata_with(&system_cargo())
}

/// Execute metadata discovery through an injected Cargo context.
#[allow(
  clippy::single_call_fn,
  reason = "this injected metadata boundary keeps raw process bytes separate from trybuild's Cargo metadata interpretation"
)]
fn metadata_with<E>(cargo: &CargoContext<E>) -> Result<Metadata>
where
  E: ProcessExecutor,
{
  let request = cargo.raw_request([
    OsString::from("metadata"),
    OsString::from("--no-deps"),
    OsString::from("--format-version=1"),
  ]);
  let output = cargo.execute(&request)?;

  serde_json::from_slice(&output.stdout).map_err(|source| {
    BuildError::Metadata(Box::new(MetadataFailure {
      source,
      stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }))
  })
}

/// Build-target selection for generated test binaries.
#[derive(Clone, Copy, Debug)]
enum BuildSelection<'name> {
  /// Build one named binary.
  Named(&'name Name),
  /// Build every generated binary.
  All,
}

/// Construct the Cargo arguments shared by single and batched diagnostic builds.
fn build_arguments(project: &Project, selection: BuildSelection<'_>, keep_going: bool) -> Vec<OsString> {
  let mut arguments = vec![OsString::from(if project.selected.has_pass() { "build" } else { "check" })];
  arguments.extend(target().into_iter().map(OsString::from));
  match selection {
    BuildSelection::Named(name) => {
      arguments.extend([OsString::from("--bin"), name.as_ref().to_os_string()]);
    }
    BuildSelection::All => arguments.push("--bins".into()),
  }
  arguments.extend(features(project).into_iter().map(OsString::from));
  arguments.extend([
    OsString::from("--quiet"),
    OsString::from("--color=never"),
    OsString::from("--message-format=json"),
  ]);
  if keep_going {
    arguments.push("--keep-going".into());
  }
  arguments
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
  use std::path::PathBuf;
  use std::result::Result as StdResult;

  use strict_test_support::RecordingEffects;
  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;
  use strict_test_support::ensure_some;
  use strict_test_support::process_output;

  use super::*;
  use crate::internal::build::json::parse_cargo_json;
  use crate::internal::model::Expected;
  use crate::internal::model::Test;
  use crate::internal::path::CanonicalPath;
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

  /// Build the deterministic Cargo capability used by request-planning tests.
  fn recording_cargo(recorder: &RecordingEffects, inherited_rustflags: Option<OsString>) -> CargoContext<RecordingEffects> {
    CargoContext {
      executor: recorder.clone(),
      program: OsString::from("cargo-fixture"),
      inherited_rustflags,
    }
  }

  /// Verify the shared generated-project working directory and deterministic
  /// environment without reusing the production configurator.
  fn ensure_project_process_context(request: &ProcessRequest, project: &Project) -> StdResult<(), TestFailure> {
    let target_dir = ensure_some(
      request.environment.first(),
      "the first environment change must select the target directory",
    )?;
    let removed_rustflags = ensure_some(
      request.environment.get(1),
      "the second environment change must remove inherited Rust flags",
    )?;
    let incremental = ensure_some(
      request.environment.get(2),
      "the third environment change must disable incremental compilation",
    )?;

    ensure_all(&[
      (
        request.current_dir.as_deref() == Some(project.dir.as_ref()),
        "project processes must execute inside the generated project",
      ),
      (
        request.environment.len() == 3,
        "project processes must emit exactly the deterministic project environment",
      ),
      (
        target_dir
          == &EnvironmentChange::Set {
            name:  "CARGO_TARGET_DIR".into(),
            value: path!(project.target_dir / "tests" / "trybuild").into_os_string(),
          },
        "project processes must isolate generated artifacts under trybuild's target directory",
      ),
      (
        removed_rustflags
          == &EnvironmentChange::Remove {
            name: "RUSTFLAGS".into()
          },
        "project processes must remove inherited Rust flags after encoding them in Cargo config",
      ),
      (
        incremental
          == &EnvironmentChange::Set {
            name:  "CARGO_INCREMENTAL".into(),
            value: "0".into(),
          },
        "project processes must disable incremental compilation",
      ),
    ])
  }

  /// Verify that a child-process status was returned as observable trybuild
  /// data instead of being converted into an execution error.
  fn ensure_observed_status(output: &ProcessOutput, expected: i32, context: &'static str) -> StdResult<(), TestFailure> {
    ensure(output.status.code() == Some(expected), context)
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

  /// Create a dependency-preparation project with both directory roots and a
  /// workspace lockfile, leaving each test to specialize the destination state.
  fn dependency_project_with_workspace_lockfile(fixture: &TempDir, contents: &str) -> StdResult<Project, TestFailure> {
    let project = project(fixture, "demo-tests", Selected::CompileFailOnly, None);
    ensure_ok_source(
      fs::create_dir_all(project.workspace.as_ref()),
      "the workspace fixture directory must exist",
    )?;
    ensure_ok_source(
      fs::create_dir_all(project.dir.as_ref()),
      "the generated project fixture directory must exist",
    )?;
    ensure_ok_source(
      fs::write(project.workspace.join("Cargo.lock"), contents),
      "the workspace lockfile fixture must exist",
    )?;
    Ok(project)
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
  fn dependency_preparation_continues_when_the_destination_lockfile_cannot_be_seeded() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("dependency-lock-destination")?;
    let mut project = dependency_project_with_workspace_lockfile(&fixture, "workspace lockfile")?;
    ensure_ok_source(
      fs::create_dir_all(project.dir.join("Cargo.lock")),
      "a directory collision must make the destination lockfile uncreatable",
    )?;
    let recorder = RecordingEffects::default();
    recorder.queue_process_result(Ok(process_output(0, Vec::new(), Vec::new())?));
    recorder.queue_process_result(Ok(process_output(0, Vec::new(), Vec::new())?));
    recorder.queue_process_result(Ok(process_output(0, Vec::new(), Vec::new())?));
    let cargo = recording_cargo(&recorder, None);

    ensure_ok_source(
      build_dependencies_with(&cargo, &mut project),
      "dependency preparation must defer an unseedable destination lockfile to Cargo",
    )?;

    ensure_all(&[
      (
        project.dir.join("Cargo.lock").is_dir(),
        "best-effort lockfile seeding must preserve the colliding directory",
      ),
      (
        recorder.process_requests().len() == 3,
        "an unseedable lockfile must not skip dependency build, keep-going probing, or cleanup",
      ),
    ])
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

  #[test]
  fn run_test_executes_the_artifact_reported_by_a_completed_build() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("run-test-executable")?;
    let project = project(&fixture, "demo-tests", Selected::PassOnly, None);
    write_project(&project, "fn main() { println!(\"direct\"); }\n")?;
    let name = Name("demo-tests".to_owned());
    let source = project.dir.join("main.rs");
    let source_path = CanonicalPath::new(&source);
    let case = Test {
      path:     PathBuf::from("main.rs"),
      expected: Expected::Pass,
    };
    let mut path_map = Map::new();
    let _previous = path_map.insert(source_path.clone(), (&name, &case));

    let build_output = ensure_ok_source(
      build_test(&project, &name),
      "the fixture executable can be produced by a completed Cargo build",
    )?;
    ensure(
      build_output.status.success(),
      "the Cargo build that publishes the executable fixture must succeed",
    )?;
    let parsed = parse_cargo_json(&project, &build_output.stdout, &path_map);
    let executable = ensure_some(
      parsed.executables.get(&source_path),
      "the completed Cargo build must report its published executable path",
    )?;

    let output = ensure_ok_source(
      run_test(&project, &name, Some(executable)),
      "the executable reported after Cargo exits is runnable",
    )?;

    ensure_all(&[
      (output.status.success(), "reported executable exits successfully"),
      (
        String::from_utf8_lossy(&output.stdout) == "direct\n",
        "reported executable output is captured directly",
      ),
    ])
  }

  #[test]
  fn single_build_plans_the_exact_cargo_request() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("single-build-request")?;
    let project = project(&fixture, "demo-tests", Selected::CompileFailOnly, Some(vec!["extra".to_owned()]));
    let recorder = RecordingEffects::default();
    recorder.queue_process_result(Ok(process_output(0, b"json".to_vec(), Vec::new())?));
    let cargo = recording_cargo(&recorder, Some(OsString::from("-C instrument-coverage")));

    let output = ensure_ok_source(
      build_test_with(&cargo, &project, &Name("case-name".to_owned())),
      "the single build must return the scripted process outcome",
    )?;
    let request = ensure_some(
      recorder.process_requests().into_iter().next(),
      "the single build must execute one process request",
    )?;
    let rustflags = rustflags::toml_from(Some(OsString::from("-C instrument-coverage")), &["--diagnostic-width=140"]);
    let mut expected_arguments = vec![
      OsString::from("--offline"),
      OsString::from(format!("--config=build.rustflags={rustflags}")),
      OsString::from(format!("--config=target.{TARGET}.rustflags={rustflags}")),
      OsString::from("check"),
    ];
    expected_arguments.extend(target().into_iter().map(OsString::from));
    expected_arguments.extend([
      OsString::from("--bin"),
      OsString::from("case-name"),
      OsString::from("--no-default-features"),
      OsString::from("--features"),
      OsString::from("extra"),
      OsString::from("--quiet"),
      OsString::from("--color=never"),
      OsString::from("--message-format=json"),
    ]);
    ensure_project_process_context(&request, &project)?;

    ensure_all(&[
      (output.stdout == b"json", "the process outcome bytes must be returned unchanged"),
      (
        recorder.process_requests().len() == 1,
        "a single diagnostic build must execute exactly one request",
      ),
      (
        request.program == "cargo-fixture",
        "single-build planning must select the injected Cargo program",
      ),
      (
        request.arguments == expected_arguments,
        "single-build planning must preserve the complete Cargo argument grammar and order",
      ),
      (
        request.stdout == OutputPolicy::Capture,
        "single-build planning must capture standard output",
      ),
      (
        request.stderr == OutputPolicy::Capture,
        "single-build planning must capture standard error",
      ),
    ])
  }

  #[test]
  fn batched_build_keeps_nonzero_status_as_trybuild_data() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("batched-build-request")?;
    let project = project(&fixture, "demo-tests", Selected::CompileFailOnly, None);
    let recorder = RecordingEffects::default();
    recorder.queue_process_result(Ok(process_output(101, b"compiler-json".to_vec(), b"compiler-stderr".to_vec())?));
    let cargo = recording_cargo(&recorder, None);

    let output = ensure_ok_source(
      build_all_tests_with(&cargo, &project),
      "a compiler failure status must remain an observed process outcome",
    )?;
    let request = ensure_some(
      recorder.process_requests().into_iter().next(),
      "the batched build must execute one process request",
    )?;

    ensure_observed_status(&output, 101, "strict-standard must not interpret Cargo's compiler-failure status")?;
    ensure_all(&[
      (
        request.arguments.iter().any(|argument| argument == "--bins"),
        "the batched grammar must select all bins",
      ),
      (
        request.arguments.iter().any(|argument| argument == "--keep-going"),
        "the batched grammar must retain compilation after an individual bin fails",
      ),
      (
        request.arguments.iter().all(|argument| argument != "--bin"),
        "the batched grammar must not select one named bin",
      ),
    ])
  }

  #[test]
  fn dependency_preparation_stops_after_a_terminal_build_failure() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("dependency-terminal-failure")?;
    let mut project = dependency_project_with_workspace_lockfile(&fixture, "")?;
    let recorder = RecordingEffects::default();
    recorder.queue_process_result(Ok(process_output(101, Vec::new(), b"dependency failed".to_vec())?));
    let cargo = recording_cargo(&recorder, None);

    ensure(
      build_dependencies_with(&cargo, &mut project).is_err(),
      "a failed dependency build must terminate preparation",
    )?;
    ensure(
      recorder.process_requests().len() == 1,
      "keep-going probing and cleanup must not run after terminal dependency failure",
    )
  }

  #[test]
  fn dependency_preparation_interprets_keep_going_in_both_directions() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("dependency-keep-going")?;
    let workspace = fixture.child("workspace");
    let generated = fixture.child("project");
    ensure_ok_source(fs::create_dir_all(&workspace), "the workspace fixture directory must exist")?;
    ensure_ok_source(
      fs::write(workspace.join("Cargo.lock"), ""),
      "the workspace lockfile must suppress lockfile generation",
    )?;
    ensure_ok_source(fs::create_dir_all(&generated), "the generated project directory must exist")?;

    for (probe_code, expected) in [(0, KeepGoing::Yes), (1, KeepGoing::No)] {
      let mut project = project(&fixture, "demo-tests", Selected::CompileFailOnly, None);
      let recorder = RecordingEffects::default();
      recorder.queue_process_result(Ok(process_output(0, Vec::new(), Vec::new())?));
      recorder.queue_process_result(Ok(process_output(probe_code, Vec::new(), Vec::new())?));
      recorder.queue_process_result(Ok(process_output(0, Vec::new(), Vec::new())?));
      let cargo = recording_cargo(&recorder, None);

      ensure_ok_source(
        build_dependencies_with(&cargo, &mut project),
        "scripted dependency preparation must complete",
      )?;
      let requests = recorder.process_requests();
      let probe = ensure_some(requests.get(1), "the second request must be the keep-going probe")?;
      let clean = ensure_some(requests.get(2), "the third request must be the suite-level clean")?;
      ensure(
        project.keep_going == expected,
        "trybuild must interpret the keep-going probe status in both directions",
      )?;
      ensure(
        requests.len() == 3,
        "successful preparation must build dependencies, probe keep-going, and clean",
      )?;
      ensure_all(&[
        (
          probe.stdout == OutputPolicy::Discard,
          "the keep-going probe must discard standard output",
        ),
        (
          probe.stderr == OutputPolicy::Discard,
          "the keep-going probe must discard standard error",
        ),
        (clean.stdout == OutputPolicy::Discard, "suite cleanup must discard standard output"),
        (clean.stderr == OutputPolicy::Discard, "suite cleanup must discard standard error"),
      ])?;
    }
    Ok(())
  }

  #[test]
  fn run_and_metadata_requests_keep_distinct_domain_grammars() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("run-metadata-requests")?;
    let project = project(&fixture, "demo-tests", Selected::PassOnly, None);
    let recorder = RecordingEffects::default();
    recorder.queue_process_result(Ok(process_output(7, b"run".to_vec(), Vec::new())?));
    let metadata_json = format!(
      r#"{{"target_directory":"{}","workspace_root":"{}","packages":[]}}"#,
      fixture.child("target").display(),
      fixture.path().display(),
    );
    recorder.queue_process_result(Ok(process_output(0, metadata_json.into_bytes(), Vec::new())?));
    let cargo = recording_cargo(&recorder, None);

    let run_output = ensure_ok_source(
      run_test_with(&cargo, &project, &Name("demo-tests".to_owned()), None),
      "the Cargo-run fallback must return its nonzero outcome as data",
    )?;
    let metadata = ensure_ok_source(metadata_with(&cargo), "metadata parsing must decode the captured raw bytes")?;
    let requests = recorder.process_requests();
    let run_request = ensure_some(requests.first(), "the first request must be the Cargo-run fallback")?;
    let metadata_request = ensure_some(requests.get(1), "the second request must be Cargo metadata")?;
    let expected_metadata_arguments = ["metadata", "--no-deps", "--format-version=1"]
      .into_iter()
      .map(OsString::from)
      .collect::<Vec<_>>();
    ensure_project_process_context(run_request, &project)?;

    ensure_observed_status(&run_output, 7, "pass-test status interpretation must remain in trybuild's runner")?;
    ensure_all(&[
      (
        run_request.arguments.iter().any(|argument| argument == "run"),
        "the pass-test fallback must retain the Cargo run grammar",
      ),
      (
        metadata_request.arguments == expected_metadata_arguments,
        "metadata discovery must retain its exact Cargo grammar",
      ),
    ])?;
    ensure(
      metadata.packages.is_empty(),
      "metadata parsing must preserve the decoded package collection",
    )
  }
}
