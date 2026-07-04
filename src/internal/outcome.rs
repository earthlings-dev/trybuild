//! The typed, per-fixture result of a [`try_run`](crate::TestCases::try_run):
//! the [`Report`] handed back to a programmatic caller, plus the per-case
//! [`Outcome`] and its render details.
//!
//! These are the data that the human-facing [`run`](crate::TestCases::run)
//! renders to the terminal; `try_run` returns them untouched so a caller (for
//! example a panic-free test-support wrapper) can map them to its own types.

use std::path::PathBuf;

use crate::TryBuildError;
use crate::internal::model::Expected;

/// The per-fixture result of compiling and checking a registered test suite.
#[derive(Debug)]
pub struct Report {
  /// One entry per registered case, in declaration order.
  pub cases: Vec<CaseReport>,
}

/// The result of a single registered test case.
#[derive(Debug)]
pub struct CaseReport {
  /// The case's source path, relative to the crate root.
  pub path:     PathBuf,
  /// Whether the case was expected to compile or to fail compilation.
  pub expected: Expected,
  /// The case's outcome: `Ok` for a passing, created, or overwritten
  /// snapshot, or the typed failure that occurred.
  pub outcome:  Result<Outcome, TryBuildError>,
}

/// A successful single-case outcome.
#[derive(Debug)]
pub enum Outcome {
  /// The case passed: a `compile_fail` matched its snapshot, or a pass-test
  /// compiled and ran cleanly. Carries the run output a pass-test produced.
  Passed(Box<PassDetail>),
  /// No snapshot existed; a new one was written under `wip` ([`Wip`] mode).
  ///
  /// [`Wip`]: crate::Update::Wip
  CreatedWip(Box<WipDetail>),
  /// The snapshot was written in place ([`Overwrite`] mode).
  ///
  /// [`Overwrite`]: crate::Update::Overwrite
  Overwrote(Box<OverwriteDetail>),
}

/// The captured run output of a passing case (empty for a matched
/// `compile_fail`, which is not executed).
#[derive(Debug)]
pub struct PassDetail {
  /// Cargo's captured stdout from running a pass-test.
  pub stdout:   String,
  /// Cargo's captured stderr from running a pass-test.
  pub stderr:   String,
  /// The preferred rendering of any build warnings.
  pub warnings: String,
}

/// The paths and contents of a newly created `wip` snapshot.
#[derive(Debug)]
pub struct WipDetail {
  /// The `wip/<name>.stderr` path the new snapshot was written to.
  pub wip_path:    PathBuf,
  /// The intended final `.stderr` path the snapshot should be moved to.
  pub stderr_path: PathBuf,
  /// The snapshot contents that were written.
  pub stderr:      String,
}

/// The path and contents of a snapshot overwritten in place.
#[derive(Debug)]
pub struct OverwriteDetail {
  /// The `.stderr` path that was overwritten in place.
  pub stderr_path: PathBuf,
  /// The snapshot contents that were written.
  pub stderr:      String,
}
