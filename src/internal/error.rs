//! The crate's public [`TryBuildError`], composed transparently from the
//! per-domain error enums.

use std::result::Result as StdResult;

use crate::internal::build::BuildError;
use crate::internal::diagnostics::DiagnosticsError;
use crate::internal::project::ProjectError;
use crate::internal::runner::RunnerError;
use crate::internal::sys::SysError;

/// The error type returned by [`TestCases::run`](crate::TestCases::run).
///
/// Each variant wraps a domain-specific error; the [`Display`](std::fmt::Display)
/// output is delegated transparently to the wrapped error.
#[non_exhaustive]
#[derive(thiserror::Error, Debug)]
pub enum TryBuildError {
  /// A host-system error (filesystem, environment, locking).
  #[error(transparent)]
  Sys(#[from] SysError),
  /// An error reading or synthesizing the generated project manifest.
  #[error(transparent)]
  Project(#[from] ProjectError),
  /// An error invoking cargo or parsing its output.
  #[error(transparent)]
  Build(#[from] BuildError),
  /// An error comparing compiler output against a saved snapshot.
  #[error(transparent)]
  Diagnostics(#[from] DiagnosticsError),
  /// An error orchestrating or executing the test cases.
  #[error(transparent)]
  Runner(#[from] RunnerError),
}

/// Result alias for the crate's composed [`TryBuildError`].
pub(in crate::internal) type Result<T> = StdResult<T, TryBuildError>;
