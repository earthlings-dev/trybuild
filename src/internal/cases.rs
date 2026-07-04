//! The public [`TestCases`] builder: the value users construct, register cases
//! on, and finally [`run`](TestCases::run).

use std::fmt;
use std::fmt::Debug;
use std::path::Path;
use std::path::PathBuf;

use crate::internal::model::Expected;
use crate::internal::model::Test;
use crate::internal::outcome::Report;
use crate::internal::runner;
use crate::internal::sys::env::Update;

/// A collection of ui test cases to compile and check.
///
/// Register files with [`pass`](Self::pass) and
/// [`compile_fail`](Self::compile_fail), then compile and check them all with
/// [`run`](Self::run).
#[must_use = "tests are not run until you call run()"]
pub struct TestCases {
  /// The registered test cases, in declaration order.
  tests: Vec<Test>,
}

impl TestCases {
  /// Creates an empty set of test cases.
  #[allow(
    clippy::single_call_fn,
    reason = "the public TestCases constructor — one of the crate's three-function API — invoked by users, not just by the local Default \
              impl"
  )]
  pub const fn new() -> Self {
    Self {
      tests: Vec::new()
    }
  }

  /// Registers a test file that is expected to compile successfully.
  ///
  /// The resulting binary is also run, and must exit without panicking.
  pub fn pass(&mut self, path: impl AsRef<Path>) {
    self.push(path.as_ref().to_owned(), Expected::Pass);
  }

  /// Registers a test file that is expected to fail compilation.
  ///
  /// Its diagnostics must match the adjacent `.stderr` snapshot.
  pub fn compile_fail(&mut self, path: impl AsRef<Path>) {
    self.push(path.as_ref().to_owned(), Expected::CompileFail);
  }

  /// Compiles and checks every registered case.
  ///
  /// # Errors
  ///
  /// Returns [`TryBuildError`](crate::TryBuildError) if any registered case fails, or if the
  /// harness cannot set up the throwaway project used to build the cases.
  pub fn run(&self) -> Result<(), crate::TryBuildError> {
    runner::run(&self.tests)
  }

  /// Compiles and checks every registered case without writing to the
  /// terminal, returning a per-fixture [`Report`].
  ///
  /// Unlike [`run`](Self::run), the snapshot reconciliation mode is given
  /// explicitly via `update` instead of read from the `TRYBUILD` environment
  /// variable, and nothing is printed — every outcome is returned as data.
  ///
  /// # Errors
  ///
  /// Returns [`TryBuildError`](crate::TryBuildError) if the throwaway project
  /// used to build the cases cannot be set up. Per-case failures are reported
  /// through each [`CaseReport`](crate::CaseReport)'s outcome, not this `Err`.
  pub fn try_run(&self, update: Update) -> Result<Report, crate::TryBuildError> {
    runner::try_run(&self.tests, update)
  }

  /// Appends one registration to the set.
  fn push(&mut self, path: PathBuf, expected: Expected) {
    self.tests.push(Test {
      path,
      expected,
    });
  }
}

impl Default for TestCases {
  fn default() -> Self {
    Self::new()
  }
}

impl Debug for TestCases {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.debug_struct("TestCases").finish_non_exhaustive()
  }
}

#[cfg(test)]
mod tests {
  use std::path::Path;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_some;

  use super::*;

  #[test]
  fn default_matches_new_and_registrations_preserve_order() -> Result<(), TestFailure> {
    let empty = TestCases::default();
    let mut cases = TestCases::new();

    cases.pass("tests/ui/pass.rs");
    cases.compile_fail("tests/ui/fail.rs");
    let pass = ensure_some(cases.tests.first(), "pass registration is present")?;
    let compile_fail = ensure_some(cases.tests.get(1), "compile-fail registration is present")?;

    ensure_all(&[
      (empty.tests.is_empty(), "default creates an empty test collection"),
      (cases.tests.len() == 2, "registrations append test cases"),
      (pass.path == Path::new("tests/ui/pass.rs"), "pass registrations preserve their path"),
      (
        compile_fail.path == Path::new("tests/ui/fail.rs"),
        "compile-fail registrations preserve their path",
      ),
      (
        format!("{:?}", pass.expected) == "Pass",
        "pass registrations preserve their expected outcome",
      ),
      (
        format!("{:?}", compile_fail.expected) == "CompileFail",
        "compile-fail registrations preserve their expected outcome",
      ),
    ])
  }

  #[test]
  fn debug_output_stays_non_exhaustive() -> Result<(), TestFailure> {
    let debug = format!("{:?}", TestCases::new());
    strict_test_support::ensure(
      debug == "TestCases { .. }",
      "TestCases debug output hides private registration details",
    )
  }
}
