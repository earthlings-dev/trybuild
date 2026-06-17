//! Orchestration: expanding globs, synthesizing the throwaway project, building
//! the test binaries, and checking each one against its `.stderr` snapshot.

mod expand;

use self::expand::{ExpandedTest, expand_globs};
use crate::internal::build::BuildError;
use crate::internal::build::cargo::{self, Metadata, PackageMetadata};
use crate::internal::build::json::{Stderr, parse_cargo_json};
use crate::internal::diagnostics::DiagnosticsError;
use crate::internal::diagnostics::normalize::Variations;
use crate::internal::error;
use crate::internal::model::{Expected, Name, PathDependency, Test};
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
use crate::internal::report::message::{Fail, Messages as _, Warn};
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
use std::path::Path;
use std::result::Result as StdResult;

/// Errors arising while orchestrating or executing the registered test cases.
#[derive(thiserror::Error, Debug)]
pub enum RunnerError {
    /// A pass-test compiled and ran but exited unsuccessfully.
    #[error("execution of the test case was unsuccessful")]
    RunFailed,
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

impl RunnerError {
    /// Whether this error's diagnostics were already written to the terminal.
    pub(in crate::internal) const fn already_printed(&self) -> bool {
        matches!(self, Self::RunFailed)
    }
}

/// Result alias for [`runner`](self) operations.
pub(in crate::internal) type Result<T> = StdResult<T, RunnerError>;

/// Tally of how a run finished, distinguishing outright failures from newly
/// created `wip` snapshots.
struct Report {
    /// Count of cases that failed.
    failures: usize,
    /// Count of cases for which a new `wip` snapshot was written.
    created_wip: usize,
}

/// Entry point: expands globs, synthesizes and builds the project, runs every
/// case, and reports the aggregate outcome.
///
/// Returns `Ok(())` only when every case passed and no new snapshots were
/// created; otherwise a [`RunnerError`] describing the failures or pending `wip`
/// files.
#[allow(
    clippy::single_call_fn,
    reason = "the runner's orchestration entry point invoked from TestCases::run; the engine's top-level driver"
)]
pub(in crate::internal) fn run(registered: &[Test]) -> error::Result<()> {
    let mut tests = expand_globs(registered);
    filter(&mut tests);

    let mut reporter = Reporter::new();

    let prepared = (|| -> error::Result<(Project, Lock)> {
        let mut project = prepare(&mut reporter, &tests)?;
        let lock = Lock::acquire(path!(project.dir / ".lock"))?;
        write(&mut project)?;
        Ok((project, lock))
    })();
    let (project, _lock) = match prepared {
        Ok(pair) => pair,
        Err(err) => {
            reporter.prepare_fail(&err);
            return Err(err);
        }
    };

    reporter.emit(format_args!("\n\n"));

    let len = tests.len();
    let mut report = Report {
        failures: 0,
        created_wip: 0,
    };

    if tests.is_empty() {
        reporter.no_tests_enabled();
    } else if project.keep_going == KeepGoing::Yes && !project.selected.has_pass() {
        report = match run_all(&mut reporter, &project, tests) {
            Ok(failures) => failures,
            Err(err) => {
                reporter.test_fail(&err);
                Report {
                    failures: len,
                    created_wip: 0,
                }
            }
        };
    } else {
        for case in tests {
            match case.run(&mut reporter, &project) {
                Ok(Outcome::Passed) => {}
                Ok(Outcome::CreatedWip) => {
                    report.created_wip = report.created_wip.saturating_add(1);
                }
                Err(err) => {
                    report.failures = report.failures.saturating_add(1);
                    reporter.test_fail(&err);
                }
            }
        }
    }

    reporter.emit(format_args!("\n\n"));

    if report.failures > 0 {
        return Err(RunnerError::Failed {
            failures: report.failures,
            total: len,
        }
        .into());
    }
    if report.created_wip > 0 {
        return Err(RunnerError::Wip {
            count: report.created_wip,
        }
        .into());
    }
    Ok(())
}

/// Synthesizes the throwaway [`Project`] for `tests`: reads cargo metadata and
/// the crate manifest, discovers path dependencies and the active feature set,
/// and builds the generated manifest.
#[allow(
    clippy::single_call_fn,
    reason = "a named orchestration phase — synthesizing the throwaway project — in run's linear pipeline"
)]
fn prepare(reporter: &mut Reporter, tests: &[ExpandedTest]) -> error::Result<Project> {
    let Metadata {
        target_directory: target_dir,
        workspace_root: workspace,
        packages,
    } = cargo::metadata(reporter)?;

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
        update: Update::env()?,
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
    reason = "a named orchestration phase — writing the generated manifest and seeding dependencies — in run's linear pipeline"
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
/// output.
#[allow(
    clippy::single_call_fn,
    reason = "the batched fast-path phase, deliberately parallel to the per-test path that `run` dispatches between"
)]
fn run_all(
    reporter: &mut Reporter,
    project: &Project,
    tests: Vec<ExpandedTest>,
) -> error::Result<Report> {
    let mut report = Report {
        failures: 0,
        created_wip: 0,
    };

    let mut path_map = Map::new();
    for expanded in &tests {
        let src_path = CanonicalPath::new(&project.source_dir.join(&expanded.test.path));
        let _previous = path_map.insert(src_path, (&expanded.name, &expanded.test));
    }

    let output = cargo::build_all_tests(project)?;
    let parsed = parse_cargo_json(project, &output.stdout, &path_map);
    let fallback = Stderr::default();

    for mut expanded in tests {
        let show_expected = false;
        reporter.begin_test(&expanded.test, show_expected);

        if expanded.error.is_none() {
            expanded.error = check_exists(&expanded.test.path).err();
        }

        if expanded.error.is_none() {
            let src_path = CanonicalPath::new(&project.source_dir.join(&expanded.test.path));
            let this_test = parsed.stderrs.get(&src_path).unwrap_or(&fallback);
            match expanded
                .test
                .check(reporter, project, &expanded.name, this_test, "")
            {
                Ok(Outcome::Passed) => {}
                Ok(Outcome::CreatedWip) => {
                    report.created_wip = report.created_wip.saturating_add(1);
                }
                Err(error) => expanded.error = Some(error),
            }
        }

        if let Some(err) = expanded.error {
            report.failures = report.failures.saturating_add(1);
            reporter.test_fail(&err);
        }
    }

    Ok(report)
}

/// The result of checking a single test case.
enum Outcome {
    /// The case passed (or its snapshot was overwritten in place).
    Passed,
    /// No snapshot existed; a new one was written under `wip`.
    CreatedWip,
}

impl Test {
    /// Builds and checks one test case on its own (the per-test path).
    fn run(
        &self,
        reporter: &mut Reporter,
        project: &Project,
        name: &Name,
    ) -> error::Result<Outcome> {
        let show_expected = project.selected.both();
        reporter.begin_test(self, show_expected);
        check_exists(&self.path)?;

        let mut path_map = Map::new();
        let src_path = CanonicalPath::new(&project.source_dir.join(&self.path));
        let _previous = path_map.insert(src_path.clone(), (name, self));

        let output = cargo::build_test(project, name)?;
        let parsed = parse_cargo_json(project, &output.stdout, &path_map);
        let fallback = Stderr::default();
        let this_test = parsed.stderrs.get(&src_path).unwrap_or(&fallback);
        self.check(reporter, project, name, this_test, &parsed.stdout)
    }

    /// Dispatches to [`check_pass`](Self::check_pass) or
    /// [`check_compile_fail`](Self::check_compile_fail) by the case's expectation.
    fn check(
        &self,
        reporter: &mut Reporter,
        project: &Project,
        name: &Name,
        result: &Stderr,
        build_stdout: &str,
    ) -> error::Result<Outcome> {
        let check = match self.expected {
            Expected::Pass => Self::check_pass,
            Expected::CompileFail => Self::check_compile_fail,
        };

        check(
            self,
            reporter,
            project,
            name,
            result.success,
            build_stdout,
            &result.stderr,
        )
    }

    /// Checks a pass-test: it must compile, then its binary must run without
    /// failing.
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
        reporter: &mut Reporter,
        project: &Project,
        name: &Name,
        success: bool,
        build_stdout: &str,
        variations: &Variations,
    ) -> error::Result<Outcome> {
        let preferred = variations.preferred();
        if !success {
            reporter.failed_to_build(preferred);
            return Err(BuildError::CargoFail.into());
        }

        let mut output = cargo::run_test(project, name)?;
        // Prepend the build stdout; `splice` must drop here so the edit applies
        // before `output` is read below.
        drop(output.stdout.splice(..0, build_stdout.bytes()));
        reporter.output(preferred, &output);
        if output.status.success() {
            Ok(Outcome::Passed)
        } else {
            Err(RunnerError::RunFailed.into())
        }
    }

    /// Checks a `compile_fail` test: it must fail to build, and its diagnostics
    /// must match the saved `.stderr` snapshot — creating or overwriting the
    /// snapshot per the update mode when none matches.
    #[allow(
        clippy::single_call_fn,
        reason = "a check strategy selected by function pointer in `check`, paired with check_pass behind the Expected dispatch"
    )]
    fn check_compile_fail(
        &self,
        reporter: &mut Reporter,
        project: &Project,
        _name: &Name,
        success: bool,
        build_stdout: &str,
        variations: &Variations,
    ) -> error::Result<Outcome> {
        let preferred = variations.preferred();

        if success {
            reporter.should_not_have_compiled();
            reporter.fail_output(Fail, build_stdout);
            reporter.warnings(preferred);
            return Err(DiagnosticsError::ShouldNotHaveCompiled.into());
        }

        let stderr_path = self.path.with_extension("stderr");

        if !stderr_path.exists() {
            let outcome = match project.update {
                Update::Wip => {
                    let wip_dir = Path::new("wip");
                    fs::create_dir_all(wip_dir).map_err(SysError::Io)?;
                    let gitignore_path = wip_dir.join(".gitignore");
                    fs::write(gitignore_path, "*\n").map_err(SysError::Io)?;
                    let stderr_name = stderr_path
                        .file_name()
                        .unwrap_or_else(|| OsStr::new("test.stderr"));
                    let wip_path = wip_dir.join(stderr_name);
                    reporter.write_stderr_wip(&wip_path, &stderr_path, preferred);
                    fs::write(wip_path, preferred).map_err(DiagnosticsError::WriteStderr)?;
                    Outcome::CreatedWip
                }
                Update::Overwrite => {
                    reporter.overwrite_stderr(&stderr_path, preferred);
                    fs::write(stderr_path, preferred).map_err(DiagnosticsError::WriteStderr)?;
                    Outcome::Passed
                }
            };
            reporter.fail_output(Warn, build_stdout);
            return Ok(outcome);
        }

        let expected = fs::read_to_string(&stderr_path)
            .map_err(DiagnosticsError::ReadStderr)?
            .replace("\r\n", "\n");

        if variations.any(|stderr| expected == stderr) {
            reporter.ok();
            return Ok(Outcome::Passed);
        }

        match project.update {
            Update::Wip => {
                reporter.mismatch(&expected, preferred);
                Err(DiagnosticsError::Mismatch.into())
            }
            Update::Overwrite => {
                reporter.overwrite_stderr(&stderr_path, preferred);
                fs::write(stderr_path, preferred).map_err(DiagnosticsError::WriteStderr)?;
                Ok(Outcome::Passed)
            }
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

impl ExpandedTest {
    /// Runs an expanded case, short-circuiting to the error captured during
    /// glob expansion if one occurred.
    fn run(self, reporter: &mut Reporter, project: &Project) -> error::Result<Outcome> {
        match self.error {
            None => self.test.run(reporter, project, &self.name),
            Some(error) => {
                let show_expected = false;
                reporter.begin_test(&self.test, show_expected);
                Err(error)
            }
        }
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
    reason = "the command-line filter phase, named distinctly from glob expansion in run's pipeline"
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
