//! Integration coverage for terminal-rendered setup and no-test reports.
//!
//! Each case re-execs an ignored child so [`TestCases::run`](trybuild::TestCases::run)
//! can render to stderr while the parent asserts the captured text.

#[cfg(test)]
mod tests {
  use std::process::Command;

  use strict_test_support::Expect;
  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::capture_ignored_test_with;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_expectations;

  #[test]
  fn run_reports_env_empty_and_metadata_setup_failures() -> Result<(), TestFailure> {
    let invalid = capture_ignored_test_with("tests::invalid_trybuild_value_child", |command| {
      let _: &mut Command = command.env("TRYBUILD", "later");
    })?;
    let no_cases = capture_ignored_test_with("tests::no_cases_child", |command| {
      let _: &mut Command = command.env_remove("TRYBUILD");
    })?;
    let fixture = TempDir::new("run-report-metadata")?;
    let metadata = capture_ignored_test_with("tests::metadata_failure_child", |command| {
      let _: &mut Command = command.current_dir(fixture.path()).env_remove("CARGO_MANIFEST_DIR");
    })?;

    ensure_all(&[
      (invalid.status.success(), "invalid TRYBUILD child completed its assertion"),
      (no_cases.status.success(), "no-cases child completed its assertion"),
      (metadata.status.success(), "metadata-failure child completed its assertion"),
    ])?;
    ensure_expectations(&invalid.stderr, &[
      Expect::Present(
        "unrecognized value of TRYBUILD",
        "invalid TRYBUILD values are rendered as setup failures",
      ),
      Expect::Present(
        "\"verify\", \"wip\", \"overwrite\"",
        "the TRYBUILD setup failure lists every accepted mode",
      ),
    ])?;
    ensure_expectations(&no_cases.stderr, &[Expect::Present(
      "There are no trybuild tests enabled yet.",
      "an empty run renders the no-tests guidance",
    )])?;
    ensure_expectations(&metadata.stderr, &[
      Expect::Present("ERROR", "metadata failures are rendered as setup failures"),
      Expect::Present(
        "failed to read cargo metadata",
        "metadata failures carry the cargo metadata context",
      ),
    ])
  }

  #[test]
  #[ignore = "driven by run_reports_env_empty_and_metadata_setup_failures via capture_ignored_test_with"]
  fn invalid_trybuild_value_child() -> Result<(), TestFailure> {
    let cases = trybuild::TestCases::new();
    ensure(cases.run().is_err(), "invalid TRYBUILD values fail before running cases")
  }

  #[test]
  #[ignore = "driven by run_reports_env_empty_and_metadata_setup_failures via capture_ignored_test_with"]
  fn no_cases_child() -> Result<(), TestFailure> {
    let cases = trybuild::TestCases::new();
    ensure(cases.run().is_ok(), "empty runs render no-tests guidance without failing")
  }

  #[test]
  #[ignore = "driven by run_reports_env_empty_and_metadata_setup_failures via capture_ignored_test_with"]
  fn metadata_failure_child() -> Result<(), TestFailure> {
    let mut cases = trybuild::TestCases::new();
    cases.pass("tests/ui/metadata-placeholder.rs");
    ensure(cases.run().is_err(), "metadata failures abort setup before case evaluation")
  }
}
