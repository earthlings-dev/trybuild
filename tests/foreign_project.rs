//! Integration coverage for crates outside trybuild's own workspace.
//!
//! The parent test writes tiny synthetic crates, then re-execs ignored children
//! with `CARGO_MANIFEST_DIR` and cwd pointed at those crates. This covers the
//! generated-project setup paths using real cargo metadata and dependency builds.

#[cfg(test)]
mod tests {
  use std::fs;
  use std::path::Path;

  use strict_standard::EnvironmentChange;
  use strict_standard::ProcessRequest;
  use strict_test_support::Expect;
  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::capture_ignored_test_with;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_expectations;
  use strict_test_support::ensure_ok_source;
  use strict_test_support::ensure_some;

  fn write_foreign_crate(root: &Path, package: &str, lib_rs: &str) -> Result<(), TestFailure> {
    ensure_ok_source(fs::create_dir_all(root.join("src")), "foreign crate src directory can be created")?;
    ensure_ok_source(
      fs::create_dir_all(root.join("tests").join("ui")),
      "foreign crate ui directory can be created",
    )?;
    ensure_ok_source(
      fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname = \"{package}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n"),
      ),
      "foreign crate manifest can be written",
    )?;
    ensure_ok_source(
      fs::write(root.join("src").join("lib.rs"), lib_rs),
      "foreign crate lib can be written",
    )?;
    ensure_ok_source(
      fs::write(root.join("tests").join("ui").join("pass.rs"), "fn main() {}\n"),
      "foreign crate pass fixture can be written",
    )
  }

  /// Writes a minimal library crate used as an external path dependency.
  fn write_path_dependency(root: &Path, package: &str, lib_rs: &str) -> Result<(), TestFailure> {
    ensure_ok_source(fs::create_dir_all(root.join("src")), "path dependency src directory can be created")?;
    ensure_ok_source(
      fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname = \"{package}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n"),
      ),
      "path dependency manifest can be written",
    )?;
    ensure_ok_source(
      fs::write(root.join("src").join("lib.rs"), lib_rs),
      "path dependency library can be written",
    )
  }

  /// Point one ignored child invocation at its synthetic foreign crate and
  /// isolated target directory.
  fn configure_foreign_child(request: &mut ProcessRequest, crate_root: &Path, target_dir: &Path) {
    request.current_dir = Some(crate_root.to_path_buf());
    request.environment.extend([
      EnvironmentChange::Set {
        name:  "CARGO_MANIFEST_DIR".into(),
        value: crate_root.as_os_str().to_os_string(),
      },
      EnvironmentChange::Set {
        name:  "CARGO_TARGET_DIR".into(),
        value: target_dir.as_os_str().to_os_string(),
      },
    ]);
  }

  #[test]
  fn try_run_sets_up_foreign_projects_and_normalizes_nested_path_dependencies() -> Result<(), TestFailure> {
    let lock_crate = TempDir::new("foreign-lock")?;
    write_foreign_crate(lock_crate.path(), "trybuild-foreign-lock", "pub fn value() -> u8 { 1 }\n")?;
    let lock_target = lock_crate.child("target-root");
    let generated_lockfile = lock_target.join("tests/trybuild/trybuild-foreign-lock/Cargo.lock");
    let lockfile_child = capture_ignored_test_with("tests::lockfile_generation_child", |request| {
      configure_foreign_child(request, lock_crate.path(), &lock_target);
    })?;

    let broken_crate = TempDir::new("foreign-broken")?;
    write_foreign_crate(
      broken_crate.path(),
      "trybuild-foreign-broken",
      "pub fn broken() { let _: u8 = \"no\"; }\n",
    )?;
    let broken_target = broken_crate.child("target-root");
    let dependency_child = capture_ignored_test_with("tests::dependency_build_failure_child", |request| {
      configure_foreign_child(request, broken_crate.path(), &broken_target);
    })?;

    let path_fixture = TempDir::new("foreign-nested-path-deps")?;
    let path_crate = path_fixture.child("project");
    write_foreign_crate(&path_crate, "trybuild-foreign-path-deps", "pub fn value() -> u8 { 1 }\n")?;
    let parent_dependency = path_fixture.child("deps").join("parent");
    let nested_dependency = parent_dependency.join("nested");
    write_path_dependency(&parent_dependency, "parent_dep", "pub fn parent() {}\n")?;
    write_path_dependency(&nested_dependency, "nested_dev", "pub fn need_u8(_: u8) {}\n")?;
    ensure_ok_source(
      fs::write(
        path_crate.join("Cargo.toml"),
        concat!(
          "[package]\n",
          "name = \"trybuild-foreign-path-deps\"\n",
          "version = \"0.0.0\"\n",
          "edition = \"2024\"\n\n",
          "[workspace]\n\n",
          "[dev-dependencies]\n",
          "parent_dep = { path = \"../deps/parent\" }\n",
          "nested_dev = { path = \"../deps/parent/nested\" }\n",
        ),
      ),
      "foreign nested-dependency manifest can be written",
    )?;
    ensure_ok_source(
      fs::write(
        path_crate.join("tests").join("ui").join("nested.rs"),
        "fn main() { nested_dev::need_u8(\"wrong\"); }\n",
      ),
      "nested dependency compile-fail fixture can be written",
    )?;
    ensure_ok_source(
      fs::write(path_crate.join("tests").join("ui").join("nested.stderr"), "error: stale snapshot\n"),
      "stale nested dependency snapshot can be written",
    )?;
    let path_target = path_fixture.child("target-root");
    let path_dependency_child = capture_ignored_test_with("tests::nested_dev_path_dependency_child", |request| {
      configure_foreign_child(request, &path_crate, &path_target);
    })?;

    ensure_all(&[
      (
        lockfile_child.status.success(),
        "foreign lockfile-generation child completed its assertions",
      ),
      (
        dependency_child.status.success(),
        "foreign dependency-failure child completed its assertions",
      ),
      (
        path_dependency_child.status.success(),
        "foreign nested-path-dependency child completed its assertions",
      ),
      (
        generated_lockfile.exists(),
        "a foreign crate without Cargo.lock gets a generated-project lockfile",
      ),
      (lockfile_child.stderr.is_empty(), "successful try_run child stays terminal-free"),
      (
        dependency_child.stderr.is_empty(),
        "dependency-failure try_run child stays terminal-free",
      ),
      (
        path_dependency_child.stderr.is_empty(),
        "nested-path-dependency try_run child stays terminal-free",
      ),
    ])
  }

  #[test]
  #[ignore = "driven by try_run_sets_up_foreign_projects_and_normalizes_nested_path_dependencies via capture_ignored_test_with"]
  fn lockfile_generation_child() -> Result<(), TestFailure> {
    let mut cases = trybuild::TestCases::new();
    cases.pass("tests/ui/pass.rs");

    let report = ensure_ok_source(
      cases.try_run(trybuild::Update::Verify),
      "foreign crate try_run completes setup and execution",
    )?;
    let case = ensure_some(report.cases.first(), "foreign pass fixture is reported")?;

    ensure_all(&[
      (report.cases.len() == 1, "foreign try_run reports the one registered fixture"),
      (
        matches!(case.outcome, Ok(trybuild::Outcome::Passed(_))),
        "the foreign pass fixture compiles and runs",
      ),
    ])
  }

  #[test]
  #[ignore = "driven by try_run_sets_up_foreign_projects_and_normalizes_nested_path_dependencies via capture_ignored_test_with"]
  fn dependency_build_failure_child() -> Result<(), TestFailure> {
    let mut cases = trybuild::TestCases::new();
    cases.pass("tests/ui/pass.rs");

    let error = ensure_some(
      cases.try_run(trybuild::Update::Verify).err(),
      "broken foreign dependencies fail during setup",
    )?;
    ensure_expectations(&format!("{error}"), &[Expect::Present(
      "cargo failed to build the generated project's dependencies",
      "dependency build failures carry their setup context",
    )])?;
    ensure_all(&[(
      matches!(error, trybuild::TryBuildError::Build(trybuild::BuildError::DependencyBuild(_))),
      "broken foreign crates return the dependency-build error variant",
    )])
  }

  #[test]
  #[ignore = "driven by try_run_sets_up_foreign_projects_and_normalizes_nested_path_dependencies via capture_ignored_test_with"]
  fn nested_dev_path_dependency_child() -> Result<(), TestFailure> {
    let mut cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/nested.rs");

    let report = ensure_ok_source(
      cases.try_run(trybuild::Update::Verify),
      "nested dev path dependency case completes compilation",
    )?;
    let actual_diagnostic = report
      .cases
      .first()
      .and_then(|case| case.outcome.as_ref().err())
      .and_then(|error| {
        if let trybuild::TryBuildError::Diagnostics(trybuild::DiagnosticsError::Mismatch(ref detail)) = *error {
          Some(detail.actual.as_str())
        } else {
          None
        }
      });
    let actual = ensure_some(actual_diagnostic, "the stale snapshot exposes the normalized actual diagnostic")?;

    ensure_all(&[
      (
        actual.contains("$NESTED_DEV/src/lib.rs"),
        "the nested dev dependency owns its compiler location",
      ),
      (
        !actual.contains("$PARENT_DEP/nested/src/lib.rs"),
        "the shorter parent dependency does not capture the nested location",
      ),
      (
        !actual.contains("/deps/parent/nested"),
        "the absolute nested dependency path does not leak into the typed diagnostic",
      ),
    ])
  }
}
