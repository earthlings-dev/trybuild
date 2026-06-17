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
    /// Cargo exited unsuccessfully; its diagnostics were already rendered.
    #[error("cargo reported an error")]
    CargoFail,
    /// Failed to deserialize `cargo metadata` output.
    #[error("failed to read cargo metadata: {0}")]
    Metadata(#[from] serde_json::Error),
    /// Could not determine the name of the project directory.
    #[error("failed to determine name of project dir")]
    ProjectDir,
}

impl BuildError {
    /// Whether this error's diagnostics were already written to the terminal.
    pub(in crate::internal) const fn already_printed(&self) -> bool {
        matches!(self, Self::CargoFail)
    }
}

/// Result alias for [`build`](self) operations.
pub(in crate::internal) type Result<T> = StdResult<T, BuildError>;
