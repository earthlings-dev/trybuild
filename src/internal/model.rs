//! Shared value types threaded across trybuild's domains.
//!
//! These are the plain data the lower domains (project synthesis, diagnostics)
//! and the orchestrator all refer to. Keeping them in one leaf module avoids
//! lower layers reaching up into the orchestrator just for a type.

use crate::internal::sys::directory::Directory;
use serde_derive::Serialize;
use std::ffi::OsStr;
use std::path::PathBuf;

/// A single registered ui test: a source file and its expected outcome.
#[derive(Clone, Debug)]
pub(in crate::internal) struct Test {
    /// Path to the `.rs` test file, relative to the crate root.
    pub(in crate::internal) path: PathBuf,
    /// Whether the file is expected to compile or to fail compilation.
    pub(in crate::internal) expected: Expected,
}

/// The expected compilation outcome of a registered test case.
#[derive(Copy, Clone, Debug)]
pub enum Expected {
    /// The file must compile and its binary must run without panicking.
    Pass,
    /// The file must fail to compile, matching its `.stderr` snapshot.
    CompileFail,
}

/// Generated `[[bin]]` target name for a synthesized test (e.g. `trybuild007`).
#[derive(Serialize, Clone, Debug)]
pub(in crate::internal) struct Name(pub(in crate::internal) String);

impl AsRef<OsStr> for Name {
    fn as_ref(&self) -> &OsStr {
        self.0.as_ref()
    }
}

/// A path dependency of the crate under test, captured so its paths can be
/// normalized out of diagnostics.
#[derive(Debug)]
pub(in crate::internal) struct PathDependency {
    /// The dependency's crate name.
    pub(in crate::internal) name: String,
    /// The canonicalized path to the dependency on disk.
    pub(in crate::internal) normalized_path: Directory,
}
