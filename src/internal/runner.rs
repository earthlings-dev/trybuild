//! Orchestration: expanding globs, synthesizing the throwaway project, building
//! the test binaries, and checking each one against its `.stderr` snapshot.
//!
//! The core ([`compute`]) writes nothing to the terminal: it returns a typed
//! per-fixture [`Report`]. The human entry ([`run`]) renders that report through
//! a [`Reporter`] as each case resolves; the programmatic entry ([`try_run`])
//! returns it untouched.

mod expand;

use self::expand::{ExpandedTest, expand_globs};
use crate::internal::build::cargo::{self, Metadata, PackageMetadata};
use crate::internal::build::json::{Stderr, parse_cargo_json};
use crate::internal::build::{BuildError, CompileFailure};
use crate::internal::diagnostics::normalize::{self, Variations};
use crate::internal::diagnostics::{DiagnosticsError, MismatchDetail, UnexpectedSuccess};
use crate::internal::error;
use crate::internal::model::{Expected, Name, PathDependency, Test};
use crate::internal::outcome::{
    CaseReport, Outcome, OverwriteDetail, PassDetail, Report, WipDetail,
};
use crate::internal::path::CanonicalPath;
use crate::internal::project::KeepGoing;
use crate::internal::project::Project;
use crate::internal::project::ProjectError;
use crate::internal::project::Selected;
use crate::internal::project::dependencies::{
    self, Dependency, EditionOrInherit, TargetDependencies,
};
use crate::internal::project::features;
use crate::internal::project::manifest::{Bin, Edition, Manifest, Package, Workspace};
use crate::internal::report::message::{render_case, render_no_tests, render_setup_fail};
use crate::internal::report::reporter::Reporter;
use crate::internal::sys::SysError;
use crate::internal::sys::directory::Directory;
use crate::internal::sys::env::Update;
use crate::internal::sys::flock::Lock;
use std::collections::BTreeMap as Map;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::iter;
use std::mem;
use std::path::{Path, PathBuf};
use std::result::Result as StdResult;

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
        total: usize,
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
    pub stdout: String,
    /// The test binary's captured stderr.
    pub stderr: String,
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
fn compute(
    registered: &[Test],
    update: Update,
    view: &mut dyn FnMut(&CaseReport, bool),
) -> error::Result<Report> {
    let mut tests = expand_globs(registered);
    filter(&mut tests);

    let mut project = prepare(&tests, update)?;
    let _lock = Lock::acquire(path!(project.dir / ".lock"))?;
    write(&mut project)?;

    let show_expected = project.selected.both();
    let mut cases = Vec::new();

    if tests.is_empty() {
        return Ok(Report { cases });
    }

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
            let report = CaseReport {
                path: case.path,
                expected: case.expected,
                outcome,
            };
            view(&report, show_expected);
            cases.push(report);
        }
    }

    Ok(Report { cases })
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
    reason = "the programmatic orchestration entry point invoked from TestCases::try_run, paired with run on this module's surface"
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
    reason = "the human orchestration entry point invoked from TestCases::run, paired with try_run on this module's surface"
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
    reason = "the named aggregate-collapse step of run, kept separate from the streaming render so the count logic reads on its own"
)]
fn aggregate(report: &Report) -> error::Result<()> {
    let total = report.cases.len();
    let failures = report
        .cases
        .iter()
        .filter(|case| case.outcome.is_err())
        .count();
    let created_wip = report
        .cases
        .iter()
        .filter(|case| matches!(case.outcome, Ok(Outcome::CreatedWip(_))))
        .count();

    if failures > 0 {
        return Err(RunnerError::Failed { failures, total }.into());
    }
    if created_wip > 0 {
        return Err(RunnerError::Wip { count: created_wip }.into());
    }
    Ok(())
}

/// Synthesizes the throwaway [`Project`] for `tests`: reads cargo metadata and
/// the crate manifest, discovers path dependencies and the active feature set,
/// and builds the generated manifest under the requested `update` mode.
#[allow(
    clippy::single_call_fn,
    reason = "a named orchestration phase — synthesizing the throwaway project — in compute's linear pipeline"
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

    let path_dependencies = source_manifest
        .dependencies
        .iter()
        .filter_map(|(name, dep)| {
            let path = dep.path.as_ref()?;
            if packages.iter().any(|pkg| &pkg.name == name) {
                // Skip path dependencies coming from the workspace itself
                None
            } else {
                Some(PathDependency {
                    name: name.clone(),
                    normalized_path: path.canonicalize().ok()?,
                })
            }
        })
        .collect();

    let crate_name = &source_manifest.package.name;
    let project_dir = path!(target_dir / "tests" / "trybuild" / crate_name /);
    fs::create_dir_all(&project_dir).map_err(SysError::Io)?;

    let project_name = format!("{crate_name}-tests");
    let manifest = make_manifest(
        &workspace,
        &project_name,
        &source_dir,
        &packages,
        tests,
        source_manifest,
    )?;

    if let Some(enabled_features) = features.as_mut() {
        enabled_features.retain(|feature| manifest.features.contains_key(feature));
    }

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

/// Writes the generated `Cargo.toml` and placeholder `main.rs` to disk and
/// builds the project's dependencies once up front.
#[allow(
    clippy::single_call_fn,
    reason = "a named orchestration phase — writing the generated manifest and seeding dependencies — in compute's linear pipeline"
)]
fn write(project: &mut Project) -> error::Result<()> {
    let manifest_toml = toml::to_string(&project.manifest).map_err(ProjectError::TomlSer)?;
    fs::write(path!(project.dir / "Cargo.toml"), manifest_toml).map_err(SysError::Io)?;

    let main_rs = b"\
        #![allow(unused_crate_dependencies, missing_docs)]\n\
        fn main() {}\n\
    ";
    fs::write(path!(project.dir / "main.rs"), &main_rs[..]).map_err(SysError::Io)?;

    cargo::build_dependencies(project)?;

    Ok(())
}

/// Builds the generated project's [`Manifest`]: merges the crate's deps and
/// dev-deps, adds a path dependency on the crate under test, copies the
/// workspace `[patch]`/`[replace]`, and registers one `[[bin]]` per test file.
#[allow(
    clippy::single_call_fn,
    reason = "a named project-synthesis phase building the generated manifest, lifted out of prepare to keep that phase readable"
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
    let workspace_manifest = dependencies::get_workspace_manifest(workspace);

    let edition = resolve_edition(
        source_manifest.package.edition,
        workspace_manifest.workspace.package.edition,
    )?;

    let cargo_toml_path = source_dir.join("Cargo.toml");
    let mut has_lib_target = true;
    for package_metadata in packages {
        if package_metadata.manifest_path == cargo_toml_path {
            has_lib_target = package_metadata
                .targets
                .iter()
                .any(|target| target.crate_types != ["bin"]);
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

    let features = prune_features(
        source_manifest.features,
        &dependencies,
        &targets,
        has_lib_target,
        &crate_name,
    );

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
    reason = "a named sub-step of `make_manifest`, isolating the edition-inheritance decision and its `NoWorkspaceManifest` error"
)]
fn resolve_edition(
    edition: EditionOrInherit,
    workspace_edition: Option<Edition>,
) -> error::Result<Edition> {
    match edition {
        EditionOrInherit::Edition(specified) => Ok(specified),
        EditionOrInherit::Inherit => {
            workspace_edition.ok_or_else(|| ProjectError::NoWorkspaceManifest.into())
        }
    }
}

/// Merges the crate-under-test's dependencies and dev-dependencies into one map,
/// adding a path dependency on the crate itself when it exposes a library
/// target.
#[allow(
    clippy::single_call_fn,
    reason = "a named sub-step of `make_manifest`, keeping the dependency merge and the self path-dependency insertion in one scope"
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
        let _previous = dependencies.insert(
            crate_name.to_owned(),
            Dependency {
                version: None,
                path: Some(source_dir.clone()),
                optional: false,
                default_features: Some(false),
                features: Vec::new(),
                git: None,
                branch: None,
                tag: None,
                rev: None,
                workspace: false,
                rest: Map::new(),
            },
        );
    }
    dependencies
}

/// Prunes each feature's `dep:<name>` enables to those naming an optional
/// dependency, prefixing the crate's own `<crate>/<feature>` enable when it
/// exposes a library target.
#[allow(
    clippy::single_call_fn,
    reason = "a named sub-step of `make_manifest`, isolating the per-feature enable pruning so its nested retain stays within the nesting budget"
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
    reason = "a named predicate for `prune_features`, flattening the dependencies/targets optional-dep search out of the retain closure"
)]
fn is_optional_dependency(
    dep_name: &str,
    dependencies: &Map<String, Dependency>,
    targets: &Map<String, TargetDependencies>,
) -> bool {
    is_optional(dependencies.get(dep_name))
        || targets
            .values()
            .any(|target| is_optional(target.dependencies.get(dep_name)))
}

/// Whether a looked-up dependency entry is present and marked `optional = true`.
const fn is_optional(dependency: Option<&Dependency>) -> bool {
    matches!(dependency, Some(&Dependency { optional: true, .. }))
}

/// The batched fast path: builds all `compile_fail` bins at once with
/// `--keep-going`, then checks each against its snapshot from the combined
/// output, collecting one [`CaseReport`] per case.
#[allow(
    clippy::single_call_fn,
    reason = "the batched fast-path phase, deliberately parallel to the per-test path that `compute` dispatches between"
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
        let report = CaseReport {
            path: case.path,
            expected: case.expected,
            outcome,
        };
        view(&report, show_expected);
        cases.push(report);
    }

    Ok(())
}

impl Test {
    /// Builds and checks one test case on its own (the per-test path),
    /// producing its [`Outcome`] or the typed failure.
    #[allow(
        clippy::single_call_fn,
        reason = "the per-test build+check step, called once from compute's per-test loop and kept on Test beside its check helpers"
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
        let check = match self.expected {
            Expected::Pass => Self::check_pass,
            Expected::CompileFail => Self::check_compile_fail,
        };

        check(
            self,
            project,
            name,
            result.success,
            build_stdout,
            &result.stderr,
            executable,
        )
    }

    /// Checks a pass-test: it must compile, then its binary must run without
    /// failing — carrying the run output either way.
    #[allow(
        clippy::single_call_fn,
        reason = "a check strategy selected by function pointer in `check`, paired with check_compile_fail behind the Expected dispatch"
    )]
    #[allow(
        clippy::unused_self,
        reason = "the `&self` receiver is unused here but structurally required: `check` dispatches check_pass and check_compile_fail through a single function pointer, so both must share one receiver signature, and check_compile_fail does read `self`"
    )]
    fn check_pass(
        &self,
        project: &Project,
        name: &Name,
        success: bool,
        build_stdout: &str,
        variations: &Variations,
        executable: Option<&Path>,
    ) -> error::Result<Outcome> {
        let preferred = variations.preferred();
        if !success {
            return Err(BuildError::CompileFailed(Box::new(CompileFailure {
                diagnostics: preferred.to_owned(),
            }))
            .into());
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
            Err(RunnerError::RunFailed(Box::new(RunOutput {
                stdout,
                stderr,
                warnings,
            }))
            .into())
        }
    }

    /// Checks a `compile_fail` test: it must fail to build, and its diagnostics
    /// must match the saved `.stderr` snapshot — creating, overwriting, or
    /// reporting a missing/mismatched snapshot per the update mode.
    #[allow(
        clippy::single_call_fn,
        reason = "a check strategy selected by function pointer in `check`, paired with check_pass behind the Expected dispatch"
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
                    stdout: build_stdout.to_owned(),
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
                stdout: String::new(),
                stderr: String::new(),
                warnings: String::new(),
            })));
        }

        match project.update {
            Update::Verify | Update::Wip => {
                Err(DiagnosticsError::Mismatch(Box::new(MismatchDetail {
                    expected,
                    actual: preferred.to_owned(),
                }))
                .into())
            }
            Update::Overwrite => {
                fs::write(&stderr_path, preferred).map_err(DiagnosticsError::WriteStderr)?;
                Ok(Outcome::Overwrote(Box::new(OverwriteDetail {
                    stderr_path,
                    stderr: preferred.to_owned(),
                })))
            }
        }
    }
}

/// Reconciles a missing `.stderr` snapshot per the update mode: failing under
/// [`Verify`](Update::Verify), writing a `wip` copy under [`Wip`](Update::Wip),
/// or creating it in place under [`Overwrite`](Update::Overwrite).
#[allow(
    clippy::single_call_fn,
    reason = "the missing-snapshot reconciliation lifted out of check_compile_fail so that function stays within the cognitive-complexity budget"
)]
fn missing_snapshot(
    update: Update,
    stderr_path: PathBuf,
    preferred: &str,
) -> error::Result<Outcome> {
    match update {
        Update::Verify => Err(DiagnosticsError::SnapshotMissing { path: stderr_path }.into()),
        Update::Wip => {
            let wip_dir = Path::new("wip");
            fs::create_dir_all(wip_dir).map_err(SysError::Io)?;
            let gitignore_path = wip_dir.join(".gitignore");
            fs::write(gitignore_path, "*\n").map_err(SysError::Io)?;
            let stderr_name = stderr_path
                .file_name()
                .unwrap_or_else(|| OsStr::new("test.stderr"));
            let wip_path = wip_dir.join(stderr_name);
            fs::write(&wip_path, preferred).map_err(DiagnosticsError::WriteStderr)?;
            Ok(Outcome::CreatedWip(Box::new(WipDetail {
                wip_path,
                stderr_path,
                stderr: preferred.to_owned(),
            })))
        }
        Update::Overwrite => {
            fs::write(&stderr_path, preferred).map_err(DiagnosticsError::WriteStderr)?;
            Ok(Outcome::Overwrote(Box::new(OverwriteDetail {
                stderr_path,
                stderr: preferred.to_owned(),
            })))
        }
    }
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
    clippy::needless_collect,
    reason = "false positive https://github.com/rust-lang/rust-clippy/issues/5991"
)]
#[allow(
    clippy::single_call_fn,
    reason = "the command-line filter phase, named distinctly from glob expansion in compute's pipeline"
)]
fn filter(tests: &mut Vec<ExpandedTest>) {
    let filters = env::args_os()
        .flat_map(OsString::into_string)
        .filter_map(|arg| {
            const PREFIX: &str = "trybuild=";
            arg.strip_prefix(PREFIX)
                .filter(|rest| !rest.is_empty())
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<String>>();

    if filters.is_empty() {
        return;
    }

    tests.retain(|expanded| {
        filters
            .iter()
            .any(|f| expanded.test.path.to_string_lossy().contains(f))
    });
}
