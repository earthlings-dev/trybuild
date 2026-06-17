//! Driving `cargo` to compile the test binaries and parsing the streamed JSON
//! diagnostics it emits.
//!
//! Note: this is the `crate::internal::build` module (`src/build.rs`), distinct from the
//! repository-root `build.rs` Cargo build script.

pub(in crate::internal) mod cargo;
pub(in crate::internal) mod json;

use std::io;
use std::result::Result as StdResult;

/// Errors arising while invoking cargo or reading its output.
#[derive(thiserror::Error, Debug)]
pub enum BuildError {
    /// Failed to spawn or execute the `cargo` process.
    #[error("failed to execute cargo: {0}")]
    Cargo(#[source] io::Error),
    /// The one-time dependency build of the generated project failed; carries
    /// the build's captured output.
    #[error("cargo failed to build the generated project's dependencies")]
    DependencyBuild(Box<BuildOutput>),
    /// A registered pass-test failed to compile; carries its normalized
    /// compiler diagnostics.
    #[error("expected the test case to compile, but it failed to build")]
    CompileFailed(Box<CompileFailure>),
    /// Failed to deserialize `cargo metadata` output; carries cargo's stderr.
    #[error("failed to read cargo metadata: {}", .0.source)]
    Metadata(Box<MetadataFailure>),
    /// Could not determine the name of the project directory.
    #[error("failed to determine name of project dir")]
    ProjectDir,
}

/// The captured output of a failed dependency build of the generated project.
#[derive(Debug)]
pub struct BuildOutput {
    /// Cargo's captured output from the failed dependency build.
    pub output: String,
}

/// The normalized compiler diagnostics from a pass-test that failed to compile.
#[derive(Debug)]
pub struct CompileFailure {
    /// The preferred (most-normalized) rendering of the compiler diagnostics.
    pub diagnostics: String,
}

/// The cause and captured cargo stderr of a `cargo metadata` parse failure.
#[derive(Debug)]
pub struct MetadataFailure {
    /// The deserialization error reported by `serde_json`.
    pub source: serde_json::Error,
    /// Cargo's captured stderr, retained so the parse failure is diagnosable.
    pub stderr: String,
}

/// Result alias for [`build`](self) operations.
pub(in crate::internal) type Result<T> = StdResult<T, BuildError>;
