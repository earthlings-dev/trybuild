//! Normalization of compiler diagnostics into stable `.stderr` snapshots and
//! comparison against the saved expectations.

pub(in crate::internal) mod normalize;

// The normalizer's snapshot tests live in `src/tests.rs` + `src/tests/*.rs` and are wired in
// here, at the domain parent, rather than inside `normalize.rs` itself: the fuzz target
// white-box-includes `normalize.rs` via `#[path]`, so that file must carry only the unit under
// test, not the library's `#[cfg(test)]` scaffolding of it. The `../` reaches back out of
// `src/internal/` to the crate-root `src/tests.rs`.
#[cfg(test)]
#[path = "../tests.rs"]
mod snapshots;

use std::io;

/// Errors arising while comparing compiler output against a saved snapshot.
#[derive(thiserror::Error, Debug)]
pub enum DiagnosticsError {
    /// The compiler output did not match any saved variation of the snapshot.
    #[error("compiler error does not match expected error")]
    Mismatch,
    /// A `compile_fail` test compiled successfully.
    #[error("expected test case to fail to compile, but it succeeded")]
    ShouldNotHaveCompiled,
    /// Failed to read the expected `.stderr` file.
    #[error("failed to read stderr file: {0}")]
    ReadStderr(#[source] io::Error),
    /// Failed to write the `.stderr` (or `wip`) file.
    #[error("failed to write stderr file: {0}")]
    WriteStderr(#[source] io::Error),
}

impl DiagnosticsError {
    /// Whether this error's diagnostics were already written to the terminal.
    pub(in crate::internal) const fn already_printed(&self) -> bool {
        matches!(self, Self::Mismatch | Self::ShouldNotHaveCompiled)
    }
}
