//! Orchestration: expanding globs, synthesizing the throwaway project, building
//! the test binaries, and checking each one against its `.stderr` snapshot.
//!
//! The core ([`compute`]) writes nothing to the terminal: it returns a typed
//! per-fixture [`Report`]. The human entry ([`run`]) renders that report through
//! a [`Reporter`] as each case resolves; the programmatic entry ([`try_run`])
//! returns it untouched.

mod expand;

use std::collections::BTreeMap as Map;
use std::env;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::fs::File;
use std::iter;
use std::mem;
use std::path::Path;
use std::path::PathBuf;
use std::result::Result as StdResult;

use self::expand::ExpandedTest;
use self::expand::expand_globs;
use crate::internal::build::BuildError;
use crate::internal::build::CompileFailure;
use crate::internal::build::cargo;
use crate::internal::build::cargo::Metadata;
use crate::internal::build::cargo::PackageMetadata;
use crate::internal::build::json::Stderr;
use crate::internal::build::json::parse_cargo_json;
use crate::internal::diagnostics::DiagnosticsError;
use crate::internal::diagnostics::MismatchDetail;
use crate::internal::diagnostics::UnexpectedSuccess;
use crate::internal::diagnostics::normalize;
use crate::internal::diagnostics::normalize::Variations;
use crate::internal::error;
use crate::internal::model::Expected;
use crate::internal::model::Name;
use crate::internal::model::PathDependency;
use crate::internal::model::Test;
use crate::internal::outcome::CaseReport;
use crate::internal::outcome::Outcome;
use crate::internal::outcome::OverwriteDetail;
use crate::internal::outcome::PassDetail;
use crate::internal::outcome::Report;
use crate::internal::outcome::WipDetail;
use crate::internal::path::CanonicalPath;
use crate::internal::project::KeepGoing;
use crate::internal::project::Project;
use crate::internal::project::ProjectError;
use crate::internal::project::Selected;
use crate::internal::project::dependencies;
use crate::internal::project::dependencies::Dependency;
use crate::internal::project::dependencies::EditionOrInherit;
use crate::internal::project::dependencies::GitSource;
use crate::internal::project::dependencies::TargetDependencies;
use crate::internal::project::features;
use crate::internal::project::manifest::Bin;
use crate::internal::project::manifest::Edition;
use crate::internal::project::manifest::Manifest;
use crate::internal::project::manifest::Package;
use crate::internal::project::manifest::Workspace;
use crate::internal::report::message::render_case;
use crate::internal::report::message::render_no_tests;
use crate::internal::report::message::render_setup_fail;
use crate::internal::report::reporter::Reporter;
use crate::internal::sys::SysError;
use crate::internal::sys::directory::Directory;
use crate::internal::sys::env::Update;
use crate::internal::sys::flock::Lock;

/// Errors arising while orchestrating or executing the registered test cases.
#[derive(thiserror::Error, Debug)]
pub enum RunnerError {
  /// A pass-test compiled and ran but exited unsuccessfully; carries its output.
  #[error("execution of the test case was unsuccessful")]
  RunFailed(Box<RunOutput>),
  /// A glob entry could not be read.
  #[error(transparent)]
  Glob(#[from] glob::GlobError),
  /// A test-path glob pattern was invalid.
  #[error(transparent)]
  Pattern(#[from] glob::PatternError),
  /// One or more registered test cases failed.
  #[error("{failures} of {total} tests failed")]
  Failed {
    /// How many registered cases failed.
    failures: usize,
    /// How many cases ran in total.
    total:    usize,
  },
  /// New `.stderr` snapshots were created and must be moved into place.
  #[error("successfully created new stderr files for {count} test cases")]
  Wip {
    /// How many `wip` snapshots were created.
    count: usize,
  },
}

/// The captured output of a pass-test that ran but exited unsuccessfully.
#[derive(Debug)]
pub struct RunOutput {
  /// The test binary's captured stdout, with the build stdout prepended.
  pub stdout:   String,
  /// The test binary's captured stderr.
  pub stderr:   String,
  /// The preferred rendering of any build warnings.
  pub warnings: String,
}

/// Result alias for [`runner`](self) operations.
pub(in crate::internal) type Result<T> = StdResult<T, RunnerError>;

/// The reporter-free core: expands globs, synthesizes and builds the project,
/// and checks every case — invoking `view` as each case resolves and collecting
/// every case into a [`Report`].
///
/// Writes nothing to the terminal. The outer `Err` is a setup failure (no cases
/// ran); per-case failures live in each [`CaseReport::outcome`].
fn compute(registered: &[Test], update: Update, view: &mut dyn FnMut(&CaseReport, bool)) -> error::Result<Report> {
  let mut tests = expand_globs(registered);
  filter(&mut tests);

  let mut cases = Vec::new();
  if tests.is_empty() {
    return Ok(Report {
      cases,
    });
  }

  let mut project = prepare(&tests, update)?;
  let _lock = Lock::acquire(path!(project.dir / ".lock"))?;
  write(&mut project)?;

  let show_expected = project.selected.both();

  if project.keep_going == KeepGoing::Yes && !project.selected.has_pass() {
    run_all(&project, tests, view, show_expected, &mut cases)?;
  } else {
    for expanded in tests {
      let ExpandedTest {
        name,
        test: case,
        error: maybe_error,
        is_from_glob: _,
      } = expanded;
      let outcome = maybe_error.map_or_else(|| case.evaluate(&project, &name), Err);
      record_case(case, outcome, view, show_expected, &mut cases);
    }
  }

  Ok(Report {
    cases,
  })
}

/// Wraps one resolved case in a [`CaseReport`], streams it through the run's
/// `view` callback, and collects it for the aggregate [`Report`].
fn record_case(
  case: Test,
  outcome: error::Result<Outcome>,
  view: &mut dyn FnMut(&CaseReport, bool),
  show_expected: bool,
  cases: &mut Vec<CaseReport>,
) {
  let report = CaseReport {
    path: case.path,
    expected: case.expected,
    outcome,
  };
  view(&report, show_expected);
  cases.push(report);
}

/// Programmatic entry: runs every registered case under the given `update` mode
/// without writing to the terminal, returning the per-fixture [`Report`].
///
/// # Errors
///
/// Returns a [`TryBuildError`](crate::TryBuildError) if the throwaway project
/// cannot be set up. Per-case failures are reported through each
/// [`CaseReport::outcome`], not this outer `Err`.
#[allow(
  clippy::single_call_fn,
  reason = "terminal-free orchestration returns the complete per-case outcome model without invoking the human rendering layer"
)]
pub(in crate::internal) fn try_run(registered: &[Test], update: Update) -> error::Result<Report> {
  compute(registered, update, &mut |_case, _show_expected| {})
}

/// Human entry: runs every registered case, streaming per-case progress to the
/// terminal and returning the aggregate outcome.
///
/// # Errors
///
/// Returns a [`TryBuildError`](crate::TryBuildError) when any case fails, when a
/// new `wip` snapshot was created, or when the throwaway project cannot be set
/// up.
#[allow(
  clippy::single_call_fn,
  reason = "human orchestration owns environment-selected reconciliation, streaming terminal rendering, and aggregate failure semantics"
)]
pub(in crate::internal) fn run(registered: &[Test]) -> error::Result<()> {
  let mut reporter = Reporter::new();

  let update = match Update::env() {
    Ok(update) => update,
    Err(sys_error) => {
      let error = sys_error.into();
      render_setup_fail(&mut reporter, &error);
      return Err(error);
    }
  };

  reporter.emit(format_args!("\n\n"));
  // Bind before matching so the borrowing render closure is dropped here,
  // freeing `reporter` for the `Err` arm and the trailing output below.
  let computed = compute(registered, update, &mut |case, show_expected| {
    render_case(&mut reporter, case, show_expected);
  });
  let report = match computed {
    Ok(report) => report,
    Err(err) => {
      render_setup_fail(&mut reporter, &err);
      return Err(err);
    }
  };
  if report.cases.is_empty() {
    render_no_tests(&mut reporter);
  }
  reporter.emit(format_args!("\n\n"));

  aggregate(&report)
}

/// Collapses a per-fixture [`Report`] into the aggregate `run()` result.
#[allow(
  clippy::single_call_fn,
  reason = "aggregate collapse defines failure-count precedence over newly created snapshot outcomes for the human run contract"
)]
fn aggregate(report: &Report) -> error::Result<()> {
  let total = report.cases.len();
  let failures = report.cases.iter().filter(|case| case.outcome.is_err()).count();
  let created_wip = report
    .cases
    .iter()
    .filter(|case| matches!(case.outcome, Ok(Outcome::CreatedWip(_))))
    .count();

  if failures > 0 {
    return Err(
      RunnerError::Failed {
        failures,
        total,
      }
      .into(),
    );
  }
  if created_wip > 0 {
    return Err(
      RunnerError::Wip {
        count: created_wip
      }
      .into(),
    );
  }
  Ok(())
}

/// Synthesizes the throwaway [`Project`] for `tests`: reads cargo metadata and
/// the crate manifest, discovers path dependencies and the active feature set,
/// and builds the generated manifest under the requested `update` mode.
#[allow(
  clippy::single_call_fn,
  reason = "project preparation is the orchestration phase that converts registered cases and Cargo metadata into one throwaway build \
            model"
)]
fn prepare(tests: &[ExpandedTest], update: Update) -> error::Result<Project> {
  let Metadata {
    target_directory: target_dir,
    workspace_root: workspace,
    packages,
  } = cargo::metadata()?;

  let mut has_pass = false;
  let mut has_compile_fail = false;
  for expanded in tests {
    match expanded.test.expected {
      Expected::Pass => has_pass = true,
      Expected::CompileFail => has_compile_fail = true,
    }
  }
  let selected = Selected::from_flags(has_pass, has_compile_fail);

  let source_dir = cargo::manifest_dir()?;
  let source_manifest = dependencies::get_manifest(&source_dir)?;

  let mut features = features::find();

  let path_dependencies = path_dependencies_of(&source_manifest, &packages);

  let crate_name = &source_manifest.package.name;
  let project_dir = path!(target_dir / "tests" / "trybuild" / crate_name /);
  fs::create_dir_all(&project_dir).map_err(SysError::Io)?;

  let project_name = format!("{crate_name}-tests");
  let manifest = make_manifest(&workspace, &project_name, &source_dir, &packages, tests, source_manifest)?;

  retain_known_features(&mut features, &manifest);

  Ok(Project {
    dir: project_dir,
    source_dir,
    target_dir,
    name: project_name,
    update,
    selected,
    features,
    workspace,
    path_dependencies,
    manifest,
    keep_going: KeepGoing::No,
  })
}

/// Discovers non-workspace path dependencies from the crate-under-test manifest.
#[allow(
  clippy::single_call_fn,
  reason = "path-dependency selection excludes workspace members and canonicalizes only external dependencies for diagnostic normalization"
)]
fn path_dependencies_of(source_manifest: &dependencies::Manifest, packages: &[PackageMetadata]) -> Vec<PathDependency> {
  source_manifest
    .dependencies
    .iter()
    .filter_map(|(name, dep)| {
      let path = dep.path.as_ref()?;
      if packages.iter().any(|pkg| &pkg.name == name) {
        // Skip path dependencies coming from the workspace itself.
        None
      } else {
        Some(PathDependency {
          name:            name.clone(),
          normalized_path: path.canonicalize().ok()?,
        })
      }
    })
    .collect()
}

/// Drops active feature names that the generated manifest does not define.
#[allow(
  clippy::single_call_fn,
  reason = "active feature retention intersects observed test-binary features with the generated manifest's declared feature vocabulary"
)]
fn retain_known_features(features: &mut Option<Vec<String>>, manifest: &Manifest) {
  if let Some(enabled_features) = features.as_mut() {
    enabled_features.retain(|feature| manifest.features.contains_key(feature));
  }
}

/// Writes the generated `Cargo.toml` and placeholder `main.rs` to disk and
/// builds the project's dependencies once up front.
#[allow(
  clippy::single_call_fn,
  reason = "project materialization writes the generated crate and completes dependency preparation before case compilation begins"
)]
fn write(project: &mut Project) -> error::Result<()> {
  let manifest_toml = toml::to_string(&project.manifest).map_err(ProjectError::TomlSer)?;
  fs::write(path!(project.dir / "Cargo.toml"), manifest_toml).map_err(SysError::Io)?;

  let main_rs = b"fn main() {}\n";
  fs::write(path!(project.dir / "main.rs"), &main_rs[..]).map_err(SysError::Io)?;

  cargo::build_dependencies(project)?;

  Ok(())
}

/// Builds the generated project's [`Manifest`]: merges the crate's deps and
/// dev-deps, adds a path dependency on the crate under test, copies the
/// workspace `[patch]`/`[replace]`, and registers one `[[bin]]` per test file.
#[allow(
  clippy::single_call_fn,
  reason = "manifest synthesis owns dependency merging, target registration, feature pruning, and workspace inheritance as one \
            convergence phase"
)]
fn make_manifest(
  workspace: &Directory,
  project_name: &str,
  source_dir: &Directory,
  packages: &[PackageMetadata],
  tests: &[ExpandedTest],
  source_manifest: dependencies::Manifest,
) -> error::Result<Manifest> {
  let crate_name = source_manifest.package.name;
  let workspace_manifest = dependencies::try_get_workspace_manifest(workspace).unwrap_or_default();

  let edition = resolve_edition(source_manifest.package.edition, workspace_manifest.workspace.package.edition)?;

  let cargo_toml_path = source_dir.join("Cargo.toml");
  let mut has_lib_target = true;
  for package_metadata in packages {
    if package_metadata.manifest_path == cargo_toml_path {
      has_lib_target = package_metadata.targets.iter().any(|target| target.crate_types != ["bin"]);
    }
  }

  let dependencies = merge_dependencies(
    &crate_name,
    source_dir,
    source_manifest.dependencies,
    source_manifest.dev_dependencies,
    has_lib_target,
  );

  let mut targets = source_manifest.target;
  for target in targets.values_mut() {
    let dev_dependencies = mem::take(&mut target.dev_dependencies);
    target.dependencies.extend(dev_dependencies);
  }

  let features = prune_features(source_manifest.features, &dependencies, &targets, has_lib_target, &crate_name);

  let mut manifest = Manifest {
    cargo_features: source_manifest.cargo_features,
    package: Package {
      name: project_name.to_owned(),
      version: "0.0.0".to_owned(),
      edition,
      resolver: source_manifest.package.resolver,
      publish: false,
    },
    features,
    dependencies,
    target: targets,
    bins: Vec::new(),
    workspace: Some(Workspace {
      dependencies: workspace_manifest.workspace.dependencies,
    }),
    // Within a workspace, only the [patch] and [replace] sections in
    // the workspace root's Cargo.toml are applied by Cargo.
    patch: workspace_manifest.patch,
    replace: workspace_manifest.replace,
  };

  manifest.bins.push(Bin {
    name: Name(project_name.to_owned()),
    path: Path::new("main.rs").to_owned(),
  });

  for expanded in tests {
    if expanded.error.is_none() {
      manifest.bins.push(Bin {
        name: expanded.name.clone(),
        path: source_dir.join(&expanded.test.path),
      });
    }
  }

  Ok(manifest)
}

/// Resolves the generated package's edition, inheriting the workspace default
/// when the crate under test declares `edition.workspace = true`.
#[allow(
  clippy::single_call_fn,
  reason = "edition resolution defines the direct-versus-workspace inheritance state transition and its missing-workspace failure"
)]
fn resolve_edition(edition: EditionOrInherit, workspace_edition: Option<Edition>) -> error::Result<Edition> {
  match edition {
    EditionOrInherit::Edition(specified) => Ok(specified),
    EditionOrInherit::Inherit => workspace_edition.ok_or_else(|| ProjectError::NoWorkspaceManifest.into()),
  }
}

/// Merges the crate-under-test's dependencies and dev-dependencies into one map,
/// adding a path dependency on the crate itself when it exposes a library
/// target.
#[allow(
  clippy::single_call_fn,
  reason = "dependency convergence merges normal and development dependencies and conditionally inserts the crate-under-test library edge"
)]
fn merge_dependencies(
  crate_name: &str,
  source_dir: &Directory,
  source_dependencies: Map<String, Dependency>,
  source_dev_dependencies: Map<String, Dependency>,
  has_lib_target: bool,
) -> Map<String, Dependency> {
  let mut dependencies = Map::new();
  dependencies.extend(source_dependencies);
  dependencies.extend(source_dev_dependencies);
  if has_lib_target {
    let _previous = dependencies.insert(crate_name.to_owned(), Dependency {
      version:          None,
      path:             Some(source_dir.clone()),
      optional:         false,
      default_features: Some(false),
      features:         Vec::new(),
      git:              GitSource::default(),
      workspace:        false,
      rest:             Map::new(),
    });
  }
  dependencies
}

/// Prunes each feature's `dep:<name>` enables to those naming an optional
/// dependency, prefixing the crate's own `<crate>/<feature>` enable when it
/// exposes a library target.
#[allow(
  clippy::single_call_fn,
  reason = "feature pruning retains only valid optional dependency enables and prefixes the crate-under-test feature edge when a library \
            exists"
)]
fn prune_features(
  mut features: Map<String, Vec<String>>,
  dependencies: &Map<String, Dependency>,
  targets: &Map<String, TargetDependencies>,
  has_lib_target: bool,
  crate_name: &str,
) -> Map<String, Vec<String>> {
  for (feature, enables) in &mut features {
    enables.retain(|en| {
      en.strip_prefix("dep:")
        .is_some_and(|dep_name| is_optional_dependency(dep_name, dependencies, targets))
    });
    if has_lib_target {
      drop(enables.splice(0..0, iter::once(format!("{crate_name}/{feature}"))));
    }
  }
  features
}

/// Whether `dep_name` names an optional dependency, in either the top-level
/// dependencies or any `[target.*]` table.
#[allow(
  clippy::single_call_fn,
  reason = "optional-dependency classification searches both top-level and target-specific Cargo dependency domains"
)]
fn is_optional_dependency(dep_name: &str, dependencies: &Map<String, Dependency>, targets: &Map<String, TargetDependencies>) -> bool {
  is_optional(dependencies.get(dep_name)) || targets.values().any(|target| is_optional(target.dependencies.get(dep_name)))
}

/// Whether a looked-up dependency entry is present and marked `optional = true`.
const fn is_optional(dependency: Option<&Dependency>) -> bool {
  matches!(
    dependency,
    Some(&Dependency {
      optional: true,
      ..
    })
  )
}

/// The batched fast path: builds all `compile_fail` bins at once with
/// `--keep-going`, then checks each against its snapshot from the combined
/// output, collecting one [`CaseReport`] per case.
#[allow(
  clippy::single_call_fn,
  reason = "batched evaluation attributes one keep-going Cargo result across every compile-fail case before applying per-case snapshot \
            policy"
)]
fn run_all(
  project: &Project,
  tests: Vec<ExpandedTest>,
  view: &mut dyn FnMut(&CaseReport, bool),
  show_expected: bool,
  cases: &mut Vec<CaseReport>,
) -> error::Result<()> {
  let mut path_map = Map::new();
  for expanded in &tests {
    let src_path = CanonicalPath::new(&project.source_dir.join(&expanded.test.path));
    let _previous = path_map.insert(src_path, (&expanded.name, &expanded.test));
  }

  let output = cargo::build_all_tests(project)?;
  let parsed = parse_cargo_json(project, &output.stdout, &path_map);
  let fallback = Stderr::default();

  for expanded in tests {
    let ExpandedTest {
      name,
      test: case,
      error: maybe_error,
      is_from_glob: _,
    } = expanded;
    let outcome = if let Some(error) = maybe_error {
      Err(error)
    } else if let Err(error) = check_exists(&case.path) {
      Err(error)
    } else {
      let src_path = CanonicalPath::new(&project.source_dir.join(&case.path));
      let this_test = parsed.stderrs.get(&src_path).unwrap_or(&fallback);
      case.check(project, &name, this_test, "", None)
    };
    record_case(case, outcome, view, show_expected, cases);
  }

  Ok(())
}

impl Test {
  /// Builds and checks one test case on its own (the per-test path),
  /// producing its [`Outcome`] or the typed failure.
  #[allow(
    clippy::single_call_fn,
    reason = "single-case evaluation is the state transition from source registration through Cargo output attribution to typed outcome \
              policy"
  )]
  fn evaluate(&self, project: &Project, name: &Name) -> error::Result<Outcome> {
    check_exists(&self.path)?;

    let mut path_map = Map::new();
    let src_path = CanonicalPath::new(&project.source_dir.join(&self.path));
    let _previous = path_map.insert(src_path.clone(), (name, self));

    let output = cargo::build_test(project, name)?;
    let parsed = parse_cargo_json(project, &output.stdout, &path_map);
    let fallback = Stderr::default();
    let this_test = parsed.stderrs.get(&src_path).unwrap_or(&fallback);
    let executable = parsed.executables.get(&src_path).map(PathBuf::as_path);
    self.check(project, name, this_test, &parsed.stdout, executable)
  }

  /// Dispatches to [`check_pass`](Self::check_pass) or
  /// [`check_compile_fail`](Self::check_compile_fail) by the case's expectation.
  fn check(
    &self,
    project: &Project,
    name: &Name,
    result: &Stderr,
    build_stdout: &str,
    executable: Option<&Path>,
  ) -> error::Result<Outcome> {
    match self.expected {
      Expected::Pass => Self::check_pass(project, name, result.success, build_stdout, &result.stderr, executable),
      Expected::CompileFail => self.check_compile_fail(project, name, result.success, build_stdout, &result.stderr, executable),
    }
  }

  /// Checks a pass-test: it must compile, then its binary must run without
  /// failing — carrying the run output either way.
  #[allow(
    clippy::single_call_fn,
    reason = "pass-case interpretation requires successful compilation followed by successful executable completion with captured \
              diagnostics"
  )]
  fn check_pass(
    project: &Project,
    name: &Name,
    success: bool,
    build_stdout: &str,
    variations: &Variations,
    executable: Option<&Path>,
  ) -> error::Result<Outcome> {
    let preferred = variations.preferred();
    if !success {
      return Err(
        BuildError::CompileFailed(Box::new(CompileFailure {
          diagnostics: preferred.to_owned(),
        }))
        .into(),
      );
    }

    let mut output = cargo::run_test(project, name, executable)?;
    // Prepend the build stdout; `splice` must drop here so the edit applies
    // before `output` is read below.
    drop(output.stdout.splice(..0, build_stdout.bytes()));
    let stdout = normalize::trim(&output.stdout);
    let stderr = normalize::trim(&output.stderr);
    let warnings = preferred.to_owned();
    if output.status.success() {
      Ok(Outcome::Passed(Box::new(PassDetail {
        stdout,
        stderr,
        warnings,
      })))
    } else {
      Err(
        RunnerError::RunFailed(Box::new(RunOutput {
          stdout,
          stderr,
          warnings,
        }))
        .into(),
      )
    }
  }

  /// Checks a `compile_fail` test: it must fail to build, and its diagnostics
  /// must match the saved `.stderr` snapshot — creating, overwriting, or
  /// reporting a missing/mismatched snapshot per the update mode.
  #[allow(
    clippy::single_call_fn,
    reason = "compile-fail interpretation requires failed compilation followed by snapshot reconciliation under the selected update policy"
  )]
  fn check_compile_fail(
    &self,
    project: &Project,
    _name: &Name,
    success: bool,
    build_stdout: &str,
    variations: &Variations,
    _executable: Option<&Path>,
  ) -> error::Result<Outcome> {
    let preferred = variations.preferred();

    if success {
      return Err(
        DiagnosticsError::ShouldNotHaveCompiled(Box::new(UnexpectedSuccess {
          stdout:   build_stdout.to_owned(),
          warnings: preferred.to_owned(),
        }))
        .into(),
      );
    }

    let stderr_path = self.path.with_extension("stderr");

    if !stderr_path.exists() {
      return missing_snapshot(project.update, stderr_path, preferred);
    }

    let expected = fs::read_to_string(&stderr_path)
      .map_err(DiagnosticsError::ReadStderr)?
      .replace("\r\n", "\n");

    if variations.any(|stderr| expected == stderr) {
      return Ok(Outcome::Passed(Box::new(PassDetail {
        stdout:   String::new(),
        stderr:   String::new(),
        warnings: String::new(),
      })));
    }

    match project.update {
      Update::Verify | Update::Wip => Err(
        DiagnosticsError::Mismatch(Box::new(MismatchDetail {
          expected,
          actual: preferred.to_owned(),
        }))
        .into(),
      ),
      Update::Overwrite => overwrite_snapshot(stderr_path, preferred),
    }
  }
}

/// Reconciles a missing `.stderr` snapshot per the update mode: failing under
/// [`Verify`](Update::Verify), writing a `wip` copy under [`Wip`](Update::Wip),
/// or creating it in place under [`Overwrite`](Update::Overwrite).
#[allow(
  clippy::single_call_fn,
  reason = "missing-snapshot reconciliation exhaustively maps Verify, Wip, and Overwrite modes onto typed failures or filesystem \
            transitions"
)]
fn missing_snapshot(update: Update, stderr_path: PathBuf, preferred: &str) -> error::Result<Outcome> {
  match update {
    Update::Verify => Err(
      DiagnosticsError::SnapshotMissing {
        path: stderr_path
      }
      .into(),
    ),
    Update::Wip => {
      let wip_dir = Path::new("wip");
      fs::create_dir_all(wip_dir).map_err(SysError::Io)?;
      let gitignore_path = wip_dir.join(".gitignore");
      fs::write(gitignore_path, "*\n").map_err(SysError::Io)?;
      let stderr_name = stderr_path.file_name().unwrap_or_else(|| OsStr::new("test.stderr"));
      let wip_path = wip_dir.join(stderr_name);
      fs::write(&wip_path, preferred).map_err(DiagnosticsError::WriteStderr)?;
      Ok(Outcome::CreatedWip(Box::new(WipDetail {
        wip_path,
        stderr_path,
        stderr: preferred.to_owned(),
      })))
    }
    Update::Overwrite => overwrite_snapshot(stderr_path, preferred),
  }
}

/// Writes the preferred rendering over the `.stderr` snapshot in place, per
/// [`Overwrite`](Update::Overwrite), and reports the overwrite as its outcome.
fn overwrite_snapshot(stderr_path: PathBuf, preferred: &str) -> error::Result<Outcome> {
  fs::write(&stderr_path, preferred).map_err(DiagnosticsError::WriteStderr)?;
  Ok(Outcome::Overwrote(Box::new(OverwriteDetail {
    stderr_path,
    stderr: preferred.to_owned(),
  })))
}

/// Confirms the test source file exists, returning a descriptive error if not.
fn check_exists(path: &Path) -> error::Result<()> {
  if path.exists() {
    return Ok(());
  }
  match File::open(path) {
    Ok(_) => Ok(()),
    Err(err) => Err(SysError::Open(path.to_owned(), err).into()),
  }
}

/// Restricts `tests` to those selected by a `trybuild=<filter>` command-line
/// argument; with no such argument every case is kept.
///
/// ```text
/// $ cargo test -- ui trybuild=tuple_structs.rs
/// ```
///
/// The first argument after `--` must be the trybuild test name (the `#[test]`
/// function that calls trybuild) so Cargo runs the test at all. The next
/// argument starting with `trybuild=` provides a filename filter: only cases
/// whose filename contains the filter string are run.
#[allow(
  clippy::single_call_fn,
  reason = "the command-line filter boundary isolates host argument observation from deterministic case-selection policy"
)]
fn filter(tests: &mut Vec<ExpandedTest>) {
  filter_with(tests, env::args_os());
}

/// Restricts `tests` to those selected by `trybuild=<filter>` arguments.
#[allow(
  clippy::single_call_fn,
  reason = "iterator-parameterized filtering defines deterministic trybuild argument selection independently of process arguments"
)]
fn filter_with(tests: &mut Vec<ExpandedTest>, args: impl Iterator<Item = OsString>) {
  let filters = TrybuildFilters::from_args(args);

  if filters.fragments.is_empty() {
    return;
  }

  tests.retain(|expanded| filters.matches_path(&expanded.test.path));
}

/// The `trybuild=<fragment>` command-line filters selected for this run.
struct TrybuildFilters {
  /// Non-empty path fragments matched against registered test paths.
  fragments: Vec<String>,
}

impl TrybuildFilters {
  /// Extracts non-empty `trybuild=<fragment>` filters from process arguments.
  #[allow(
    clippy::single_call_fn,
    reason = "filter parsing owns the nonempty `trybuild=` argument grammar before path selection is applied"
  )]
  fn from_args(args: impl Iterator<Item = OsString>) -> Self {
    let fragments = args
      .flat_map(OsString::into_string)
      .filter_map(|arg| {
        const PREFIX: &str = "trybuild=";
        arg.strip_prefix(PREFIX).filter(|rest| !rest.is_empty()).map(ToOwned::to_owned)
      })
      .collect();
    Self {
      fragments,
    }
  }

  /// Whether `path` matches at least one selected filter fragment.
  #[allow(
    clippy::single_call_fn,
    reason = "path matching is the filter value object's core query and names the OR semantics across fragments"
  )]
  fn matches_path(&self, path: &Path) -> bool {
    let rendered = path.to_string_lossy();
    self.fragments.iter().any(|fragment| rendered.contains(fragment))
  }
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;
  use std::result::Result as StdResult;

  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;
  use strict_test_support::ensure_some;

  use super::*;
  use crate::internal::build::cargo::BuildTarget;

  fn expanded(path: impl Into<PathBuf>, expected: Expected) -> ExpandedTest {
    ExpandedTest {
      name:         Name("trybuild000".to_owned()),
      test:         Test {
        path: path.into(),
        expected,
      },
      error:        None,
      is_from_glob: false,
    }
  }

  fn dependency_path(path: impl Into<PathBuf>) -> Dependency {
    Dependency {
      version:          None,
      path:             Some(Directory::new(path.into())),
      optional:         false,
      default_features: None,
      features:         Vec::new(),
      git:              GitSource::default(),
      workspace:        false,
      rest:             Map::new(),
    }
  }

  fn dependency(optional: bool) -> Dependency {
    Dependency {
      version: None,
      path: None,
      optional,
      default_features: None,
      features: Vec::new(),
      git: GitSource::default(),
      workspace: false,
      rest: Map::new(),
    }
  }

  fn case_report(path: &str, outcome: error::Result<Outcome>) -> CaseReport {
    CaseReport {
      path: PathBuf::from(path),
      expected: Expected::CompileFail,
      outcome,
    }
  }

  fn has_case(tests: &[ExpandedTest], file: &str) -> bool {
    tests.iter().any(|case| case.test.path.ends_with(Path::new(file)))
  }

  #[test]
  fn filter_with_keeps_only_matching_trybuild_arguments() -> StdResult<(), TestFailure> {
    let mut tests = vec![
      expanded("tests/ui/alpha.rs", Expected::Pass),
      expanded("tests/ui/beta.rs", Expected::CompileFail),
    ];

    filter_with(&mut tests, [OsString::from("test"), OsString::from("trybuild=beta.rs")].into_iter());
    let only = ensure_some(tests.first(), "one filtered test remains")?;

    ensure_all(&[
      (tests.len() == 1, "trybuild filters remove non-matching cases"),
      (
        only.test.path == Path::new("tests/ui/beta.rs"),
        "trybuild filters keep the matching case",
      ),
    ])
  }

  #[test]
  fn filter_with_leaves_tests_when_no_filter_is_present() -> StdResult<(), TestFailure> {
    let mut tests = vec![
      expanded("tests/ui/alpha.rs", Expected::Pass),
      expanded("tests/ui/beta.rs", Expected::CompileFail),
    ];

    filter_with(&mut tests, [OsString::from("test")].into_iter());

    ensure_all(&[
      (tests.len() == 2, "no trybuild filter keeps all cases"),
      (has_case(&tests, "alpha.rs"), "the first case remains without a filter"),
      (has_case(&tests, "beta.rs"), "the second case remains without a filter"),
    ])
  }

  #[test]
  fn filter_with_matches_any_non_empty_trybuild_argument() -> StdResult<(), TestFailure> {
    let mut tests = vec![
      expanded("tests/ui/alpha.rs", Expected::Pass),
      expanded("tests/ui/beta.rs", Expected::CompileFail),
      expanded("tests/ui/gamma.rs", Expected::CompileFail),
    ];

    filter_with(
      &mut tests,
      [
        OsString::from("test"),
        OsString::from("trybuild=alpha"),
        OsString::from("trybuild="),
        OsString::from("trybuild=gamma"),
      ]
      .into_iter(),
    );

    ensure_all(&[
      (tests.len() == 2, "multiple trybuild filters keep every matching case"),
      (has_case(&tests, "alpha.rs"), "the first non-empty filter is applied"),
      (has_case(&tests, "gamma.rs"), "the second non-empty filter is applied"),
      (!has_case(&tests, "beta.rs"), "non-matching cases are removed"),
    ])
  }

  #[test]
  fn filter_with_can_select_no_cases() -> StdResult<(), TestFailure> {
    let mut tests = vec![
      expanded("tests/ui/alpha.rs", Expected::Pass),
      expanded("tests/ui/beta.rs", Expected::CompileFail),
    ];

    filter_with(&mut tests, [OsString::from("test"), OsString::from("trybuild=missing")].into_iter());

    ensure(tests.is_empty(), "a filter with no matching path removes every case")
  }

  #[test]
  fn retain_known_features_drops_unknown_active_features() -> StdResult<(), TestFailure> {
    let manifest = Manifest {
      cargo_features: Vec::new(),
      package:        Package {
        name:     "trybuild-tests".to_owned(),
        version:  "0.0.0".to_owned(),
        edition:  Edition::default(),
        resolver: None,
        publish:  false,
      },
      features:       [("diff".to_owned(), Vec::new()), ("serde".to_owned(), Vec::new())]
        .into_iter()
        .collect(),
      dependencies:   Map::new(),
      target:         Map::new(),
      bins:           Vec::new(),
      workspace:      None,
      patch:          Map::new(),
      replace:        Map::new(),
    };
    let mut features = Some(vec!["diff".to_owned(), "unknown".to_owned(), "serde".to_owned()]);

    retain_known_features(&mut features, &manifest);
    let retained = ensure_some(features.as_ref(), "features remain present after retention")?;
    let expected = vec!["diff".to_owned(), "serde".to_owned()];
    let mut no_features = None;
    retain_known_features(&mut no_features, &manifest);

    ensure_all(&[
      (retained == &expected, "only manifest-defined features remain"),
      (no_features.is_none(), "missing active feature detection remains missing"),
    ])
  }

  #[test]
  fn path_dependencies_of_keeps_external_canonical_paths() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("path-deps")?;
    let external = fixture.child("external");
    let workspace = fixture.child("workspace");
    fs::create_dir_all(&external).map_err(|error| TestFailure::Caused {
      context: "create external dependency directory",
      source:  Box::new(error),
    })?;
    fs::create_dir_all(&workspace).map_err(|error| TestFailure::Caused {
      context: "create workspace dependency directory",
      source:  Box::new(error),
    })?;

    let mut manifest = dependencies::Manifest::default();
    let _external = manifest.dependencies.insert("external".to_owned(), dependency_path(&external));
    let _workspace = manifest
      .dependencies
      .insert("workspace".to_owned(), dependency_path(&workspace));
    let _missing = manifest
      .dependencies
      .insert("missing".to_owned(), dependency_path(fixture.child("missing")));
    let packages = [PackageMetadata {
      name:          "workspace".to_owned(),
      targets:       Vec::<BuildTarget>::new(),
      manifest_path: PathBuf::new(),
    }];

    let paths = path_dependencies_of(&manifest, &packages);
    let only = ensure_some(paths.first(), "one external path dependency remains")?;

    ensure_all(&[
      (paths.len() == 1, "workspace and non-canonicalizable path dependencies are skipped"),
      (only.name == "external", "external path dependencies are retained by name"),
      (
        only.normalized_path.as_ref()
          == Directory::new(external.canonicalize().map_err(|error| TestFailure::Caused {
            context: "canonicalize external dependency",
            source:  Box::new(error),
          })?)
          .as_ref(),
        "external path dependencies are canonicalized",
      ),
    ])
  }

  #[test]
  fn aggregate_reports_failures_wip_and_clean_suites() -> StdResult<(), TestFailure> {
    let clean = Report {
      cases: vec![case_report(
        "tests/ui/pass.rs",
        Ok(Outcome::Passed(Box::new(PassDetail {
          stdout:   String::new(),
          stderr:   String::new(),
          warnings: String::new(),
        }))),
      )],
    };
    let failed = Report {
      cases: vec![case_report(
        "tests/ui/fail.rs",
        Err(
          RunnerError::Failed {
            failures: 1, total: 1
          }
          .into(),
        ),
      )],
    };
    let wip = Report {
      cases: vec![case_report(
        "tests/ui/wip.rs",
        Ok(Outcome::CreatedWip(Box::new(WipDetail {
          wip_path:    PathBuf::from("wip/wip.stderr"),
          stderr_path: PathBuf::from("tests/ui/wip.stderr"),
          stderr:      "error: new\n".to_owned(),
        }))),
      )],
    };

    ensure_all(&[
      (aggregate(&clean).is_ok(), "clean reports aggregate successfully"),
      (aggregate(&failed).is_err(), "case failures make the aggregate fail"),
      (aggregate(&wip).is_err(), "created wip snapshots make the aggregate fail"),
    ])
  }

  #[test]
  fn snapshot_helpers_verify_missing_and_overwrite_in_place() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("snapshot-overwrite")?;
    let missing = fixture.child("missing.stderr");
    let overwrite = fixture.child("overwrite.stderr");

    let missing_result = missing_snapshot(Update::Verify, missing, "error: missing\n");
    let overwrite_result = missing_snapshot(Update::Overwrite, overwrite.clone(), "error: created\n");
    let _replaced = overwrite_snapshot(overwrite.clone(), "error: replaced\n").map_err(|source| TestFailure::Caused {
      context: "overwrite existing snapshot",
      source:  Box::new(source),
    })?;

    ensure_all(&[
      (missing_result.is_err(), "verify mode reports a missing snapshot"),
      (overwrite_result.is_ok(), "overwrite mode creates a missing snapshot in place"),
      (
        ensure_ok_source(fs::read_to_string(&overwrite), "overwritten snapshot can be read")? == "error: replaced\n",
        "overwrite_snapshot replaces the snapshot bytes",
      ),
    ])
  }

  #[test]
  fn check_exists_accepts_files_and_reports_missing_paths() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("check-exists")?;
    let present = fixture.child("present.rs");
    ensure_ok_source(fs::write(&present, "fn main() {}\n"), "present fixture can be written")?;
    let missing = fixture.child("missing.rs");

    ensure_all(&[
      (check_exists(&present).is_ok(), "existing test sources pass the existence check"),
      (check_exists(&missing).is_err(), "missing test sources report an open error"),
    ])
  }

  #[test]
  fn merge_dependencies_adds_self_only_for_library_targets() -> StdResult<(), TestFailure> {
    let source_dir = Directory::new("/crate");
    let mut source_dependencies = Map::new();
    let _normal = source_dependencies.insert("normal".to_owned(), dependency(false));
    let mut source_dev_dependencies = Map::new();
    let _dev = source_dev_dependencies.insert("dev".to_owned(), dependency(false));

    let with_lib = merge_dependencies(
      "demo",
      &source_dir,
      source_dependencies.clone(),
      source_dev_dependencies.clone(),
      true,
    );
    let without_lib = merge_dependencies("demo", &source_dir, source_dependencies, source_dev_dependencies, false);
    let self_dep = ensure_some(with_lib.get("demo"), "library targets add a self dependency")?;

    ensure_all(&[
      (with_lib.contains_key("normal"), "source dependencies are retained"),
      (with_lib.contains_key("dev"), "dev dependencies are merged"),
      (
        self_dep.path.as_ref().map(Directory::as_ref) == Some(source_dir.as_ref()),
        "self dependencies point at the source dir",
      ),
      (!without_lib.contains_key("demo"), "bin-only crates do not add a self dependency"),
    ])
  }

  #[test]
  fn prune_features_keeps_only_optional_dependency_enables() -> StdResult<(), TestFailure> {
    let mut dependencies = Map::new();
    let _top = dependencies.insert("top".to_owned(), dependency(true));
    let _plain = dependencies.insert("plain".to_owned(), dependency(false));
    let mut target_dependencies = Map::new();
    let _target_dep = target_dependencies.insert("targeted".to_owned(), dependency(true));
    let mut targets = Map::new();
    let _target = targets.insert("cfg(unix)".to_owned(), TargetDependencies {
      dependencies:     target_dependencies,
      dev_dependencies: Map::new(),
    });
    let mut features = Map::new();
    let _feature = features.insert("feat".to_owned(), vec![
      "dep:top".to_owned(),
      "dep:targeted".to_owned(),
      "dep:plain".to_owned(),
      "plain".to_owned(),
    ]);

    let with_lib = prune_features(features.clone(), &dependencies, &targets, true, "demo");
    let without_lib = prune_features(features, &dependencies, &targets, false, "demo");
    let with_lib_enables = ensure_some(with_lib.get("feat"), "feature remains with a library target")?;
    let without_lib_enables = ensure_some(without_lib.get("feat"), "feature remains without a library target")?;

    ensure_all(&[
      (
        with_lib_enables == &["demo/feat", "dep:top", "dep:targeted"],
        "library targets prefix self feature enables and keep optional dependencies",
      ),
      (
        without_lib_enables == &["dep:top", "dep:targeted"],
        "bin-only crates keep optional dependency enables without a self prefix",
      ),
    ])
  }

  #[test]
  fn make_manifest_skips_bins_for_expansion_errors() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("make-manifest")?;
    let workspace = Directory::new(fixture.child("workspace"));
    let source_dir = Directory::new(fixture.child("source"));
    ensure_ok_source(fs::create_dir_all(source_dir.as_ref()), "source dir can be created")?;
    let mut source_manifest = dependencies::Manifest::default();
    source_manifest.package.name = "demo".to_owned();
    let tests = vec![expanded("tests/ui/ok.rs", Expected::CompileFail), ExpandedTest {
      name:         Name("trybuild001".to_owned()),
      test:         Test {
        path:     PathBuf::from("tests/ui/bad.rs"),
        expected: Expected::CompileFail,
      },
      error:        Some(
        RunnerError::Failed {
          failures: 1, total: 1
        }
        .into(),
      ),
      is_from_glob: false,
    }];
    let packages = [PackageMetadata {
      name:          "demo".to_owned(),
      targets:       vec![BuildTarget {
        crate_types: vec!["lib".to_owned()],
      }],
      manifest_path: source_dir.join("Cargo.toml"),
    }];

    let manifest =
      make_manifest(&workspace, "demo-tests", &source_dir, &packages, &tests, source_manifest).map_err(|source| TestFailure::Caused {
        context: "generated manifest can be built",
        source:  Box::new(source),
      })?;

    ensure_all(&[
      (manifest.bins.len() == 2, "the placeholder and successful test bins are registered"),
      (
        manifest.bins.iter().any(|bin| bin.path.ends_with("tests/ui/ok.rs")),
        "successful expansions become bins",
      ),
      (
        !manifest.bins.iter().any(|bin| bin.path.ends_with("tests/ui/bad.rs")),
        "expansion errors do not become bins",
      ),
    ])
  }
}
