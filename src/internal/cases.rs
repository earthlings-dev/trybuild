//! The public [`TestCases`] builder: the value users construct, register cases
//! on, and finally [`run`](TestCases::run).

use std::fmt::{self, Debug};
use std::path::{Path, PathBuf};

use crate::internal::model::{Expected, Test};
use crate::internal::runner;

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
        reason = "the public TestCases constructor — one of the crate's three-function API — invoked by users, not just by the local Default impl"
    )]
    pub const fn new() -> Self {
        Self { tests: Vec::new() }
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

    /// Appends one registration to the set.
    fn push(&mut self, path: PathBuf, expected: Expected) {
        self.tests.push(Test { path, expected });
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
