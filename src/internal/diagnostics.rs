//! Normalization of compiler diagnostics into stable `.stderr` snapshots and
//! comparison against the saved expectations.

pub(in crate::internal) mod normalize;

// The normalizer's snapshot tests live in `src/tests.rs` + `src/tests/*.rs` and are wired in
// here, at the domain parent, rather than inside `normalize.rs` itself: the fuzz target
// white-box-includes `normalize.rs` via `#[path]`, so that file must carry only the unit under
// test, not the library's `#[cfg(test)]` scaffolding of it. The `../` reaches back out of
// `src/internal/` to the crate-root `src/tests.rs`.
#[cfg(test)]
mod normalize_coverage;
#[cfg(test)]
#[path = "../tests.rs"]
mod snapshots;

use std::io;
use std::path::PathBuf;

/// Errors arising while comparing compiler output against a saved snapshot.
#[derive(thiserror::Error, Debug)]
pub enum DiagnosticsError {
  /// The compiler output did not match any saved variation of the snapshot;
  /// carries the expected and actual renderings.
  #[error("compiler error does not match expected error")]
  Mismatch(Box<MismatchDetail>),
  /// A `compile_fail` test compiled successfully; carries the build output.
  #[error("expected test case to fail to compile, but it succeeded")]
  ShouldNotHaveCompiled(Box<UnexpectedSuccess>),
  /// No `.stderr` snapshot exists and the update mode is
  /// [`Verify`](crate::Update::Verify), so none was written.
  #[error("no snapshot exists for {}", .path.display())]
  SnapshotMissing {
    /// The `.stderr` snapshot path that was expected to exist.
    path: PathBuf,
  },
  /// Failed to read the expected `.stderr` file.
  #[error("failed to read stderr file: {0}")]
  ReadStderr(#[source] io::Error),
  /// Failed to write the `.stderr` (or `wip`) file.
  #[error("failed to write stderr file: {0}")]
  WriteStderr(#[source] io::Error),
}

/// The expected and actual diagnostic renderings of a snapshot mismatch.
#[derive(Debug)]
pub struct MismatchDetail {
  /// The committed `.stderr` snapshot contents.
  pub expected: String,
  /// The preferred (most-normalized) rendering of the actual diagnostics.
  pub actual:   String,
}

/// The captured output of a `compile_fail` case that unexpectedly compiled.
#[derive(Debug)]
pub struct UnexpectedSuccess {
  /// Cargo's captured stdout from the unexpectedly-successful build.
  pub stdout:   String,
  /// The preferred rendering of any warnings the build emitted.
  pub warnings: String,
}
