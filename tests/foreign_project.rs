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

  #[test]
  fn try_run_sets_up_foreign_projects_and_surfaces_dependency_failures() -> Result<(), TestFailure> {
    let lock_crate = TempDir::new("foreign-lock")?;
    write_foreign_crate(lock_crate.path(), "trybuild-foreign-lock", "pub fn value() -> u8 { 1 }\n")?;
    let lock_target = lock_crate.child("target-root");
    let generated_lockfile = lock_target.join("tests/trybuild/trybuild-foreign-lock/Cargo.lock");
    let lockfile_child = capture_ignored_test_with("tests::lockfile_generation_child", |request| {
      request.current_dir = Some(lock_crate.path().to_path_buf());
      request.environment.extend([
        EnvironmentChange::Set {
          name:  "CARGO_MANIFEST_DIR".into(),
          value: lock_crate.path().as_os_str().to_os_string(),
        },
        EnvironmentChange::Set {
          name:  "CARGO_TARGET_DIR".into(),
          value: lock_target.as_os_str().to_os_string(),
        },
      ]);
    })?;

    let broken_crate = TempDir::new("foreign-broken")?;
    write_foreign_crate(
      broken_crate.path(),
      "trybuild-foreign-broken",
      "pub fn broken() { let _: u8 = \"no\"; }\n",
    )?;
    let broken_target = broken_crate.child("target-root");
    let dependency_child = capture_ignored_test_with("tests::dependency_build_failure_child", |request| {
      request.current_dir = Some(broken_crate.path().to_path_buf());
      request.environment.extend([
        EnvironmentChange::Set {
          name:  "CARGO_MANIFEST_DIR".into(),
          value: broken_crate.path().as_os_str().to_os_string(),
        },
        EnvironmentChange::Set {
          name:  "CARGO_TARGET_DIR".into(),
          value: broken_target.as_os_str().to_os_string(),
        },
      ]);
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
        generated_lockfile.exists(),
        "a foreign crate without Cargo.lock gets a generated-project lockfile",
      ),
      (lockfile_child.stderr.is_empty(), "successful try_run child stays terminal-free"),
      (
        dependency_child.stderr.is_empty(),
        "dependency-failure try_run child stays terminal-free",
      ),
    ])
  }

  #[test]
  #[ignore = "driven by try_run_sets_up_foreign_projects_and_surfaces_dependency_failures via capture_ignored_test_with"]
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
  #[ignore = "driven by try_run_sets_up_foreign_projects_and_surfaces_dependency_failures via capture_ignored_test_with"]
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
}
