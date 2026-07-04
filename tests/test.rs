//! Self-hosted integration test.
//!
//! Runs trybuild against its own `tests/ui/*.rs` fixtures. The fixtures
//! deliberately pair files with mismatched expectations (for example a passing
//! file registered as `compile_fail`), so a successful run of this harness is
//! one where [`TestCases::run`](trybuild::TestCases::run) reports an error. The
//! default test re-execs one ignored child through
//! `strict_test_support::capture_ignored_test`, keeping `run`'s terminal report
//! captured instead of leaking into the parent `cargo test` output.

#[cfg(test)]
mod tests {
  use strict_test_support::Expect;
  use strict_test_support::TestFailure;
  use strict_test_support::capture_ignored_test;
  use strict_test_support::ensure;
  use strict_test_support::ensure_expectations;

  /// Re-execs the terminal-rendering self-test and asserts its report is captured.
  #[test]
  fn test() -> Result<(), TestFailure> {
    let captured = capture_ignored_test("tests::self_hosted_run_reports_expected_failures_child")?;
    ensure(
      captured.status.success(),
      "the captured self-hosted run child completed successfully",
    )?;
    ensure_expectations(&captured.stderr, &[
      Expect::Present(
        "tests/ui/print-stdout.rs",
        "the captured run report includes the stdout pass fixture",
      ),
      Expect::Present("STDOUT:", "the captured run report includes a stdout heading"),
      Expect::Present(
        "Chars(['S', 'T', 'D', 'O', 'U', 'T'])",
        "the captured run report includes the stdout payload",
      ),
      Expect::Present(
        "tests/ui/print-stderr.rs",
        "the captured run report includes the stderr pass fixture",
      ),
      Expect::Present("STDERR:", "the captured run report includes a stderr heading"),
      Expect::Present(
        "Chars(['S', 'T', 'D', 'E', 'R', 'R'])",
        "the captured run report includes the stderr payload",
      ),
      Expect::Present(
        "tests/ui/run-pass-3.rs",
        "the captured run report includes the unexpected-success fixture",
      ),
      Expect::Present(
        "Expected test case to fail to compile, but it succeeded.",
        "the captured run report includes the unexpected-success explanation",
      ),
      Expect::Present(
        "tests/ui/run-fail.rs",
        "the captured run report includes the runtime-failure fixture",
      ),
      Expect::Present(
        "Test case failed at runtime.",
        "the captured run report includes the runtime-failure explanation",
      ),
    ])
  }

  /// Runs the full self-hosted suite through `TestCases::run`.
  #[test]
  #[ignore = "driven by tests::test via capture_ignored_test; a direct run intentionally exercises trybuild's terminal renderer in-process"]
  fn self_hosted_run_reports_expected_failures_child() -> Result<(), TestFailure> {
    let mut cases = trybuild::TestCases::new();
    cases.pass("tests/ui/run-pass-0.rs");
    cases.pass("tests/ui/print-stdout.rs");
    cases.pass("tests/ui/run-pass-1.rs");
    cases.pass("tests/ui/print-stderr.rs");
    cases.pass("tests/ui/run-pass-2.rs");
    cases.pass("tests/ui/print-both.rs");
    cases.pass("tests/ui/run-pass-4.rs");
    cases.compile_fail("tests/ui/run-pass-3.rs");
    cases.pass("tests/ui/run-pass-5.rs");
    cases.pass("tests/ui/compile-fail-0.rs");
    cases.pass("tests/ui/run-pass-6.rs");
    cases.pass("tests/ui/run-pass-7.rs");
    cases.pass("tests/ui/run-pass-8.rs");
    cases.compile_fail("tests/ui/compile-fail-1.rs");
    cases.pass("tests/ui/run-fail.rs");
    cases.pass("tests/ui/run-pass-9.rs");
    cases.compile_fail("tests/ui/compile-fail-2.rs");
    cases.compile_fail("tests/ui/compile-fail-3.rs");

    // This self-test deliberately registers files with mismatched expectations
    // (e.g. a passing file as `compile_fail`), so the run is expected to fail.
    ensure(
      cases.run().is_err(),
      "trybuild's self-test fixtures intentionally include failing cases",
    )
  }
}
