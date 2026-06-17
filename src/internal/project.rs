//! Synthesis of the throwaway Cargo project that hosts the test binaries:
//! manifest parsing, dependency rewriting, workspace inheritance, feature and
//! rustflag discovery.

pub(in crate::internal) mod dependencies;
pub(in crate::internal) mod features;
mod inherit;
pub(in crate::internal) mod manifest;
pub(in crate::internal) mod rustflags;

use self::manifest::Manifest;
use crate::internal::model::PathDependency;
use crate::internal::sys::directory::Directory;
use crate::internal::sys::env::Update;
use std::path::PathBuf;
use std::result::Result as StdResult;
use toml::de::Error as TomlDeError;
use toml::ser::Error as TomlSerError;

/// Errors arising while reading or synthesizing the generated project manifest.
#[derive(thiserror::Error, Debug)]
pub enum ProjectError {
    /// Failed to read the crate-under-test's manifest; carries its path.
    #[error("failed to read manifest {}: {}", .0.display(), .1)]
    GetManifest(PathBuf, #[source] Box<crate::TryBuildError>),
    /// `edition.workspace = true` was used but the workspace defines no edition.
    #[error("Cargo.toml uses edition.workspace=true, but no edition found in workspace's manifest")]
    NoWorkspaceManifest,
    /// A manifest failed to deserialize from TOML.
    #[error(transparent)]
    TomlDe(#[from] TomlDeError),
    /// The generated manifest failed to serialize to TOML.
    #[error(transparent)]
    TomlSer(#[from] TomlSerError),
}

/// Result alias for [`project`](self) operations.
pub(in crate::internal) type Result<T> = StdResult<T, ProjectError>;

/// The synthesized throwaway Cargo project that hosts the generated test
/// binaries, together with the state threaded through building and checking
/// them.
#[derive(Debug)]
pub(in crate::internal) struct Project {
    /// The generated project's directory, under `<target>/tests/trybuild/<crate>/`.
    pub dir: Directory,
    /// The crate-under-test's manifest directory.
    pub source_dir: Directory,
    /// The workspace's target directory.
    pub target_dir: Directory,
    /// The generated package's name, `<crate>-tests`.
    pub name: String,
    /// Snapshot update mode selected by the `TRYBUILD` environment variable.
    pub update: Update,
    /// Which kinds of case — pass and/or `compile_fail` — were registered.
    pub selected: Selected,
    /// The feature set to build the cases with, if detected.
    pub features: Option<Vec<String>>,
    /// The workspace root directory.
    pub workspace: Directory,
    /// Path dependencies of the crate under test, kept for diagnostic normalization.
    pub path_dependencies: Vec<PathDependency>,
    /// The synthesized `Cargo.toml` for the generated project.
    pub manifest: Manifest,
    /// Whether the installed cargo supports `--keep-going` (enables batched builds).
    pub keep_going: KeepGoing,
}

/// Which kinds of test case were registered, replacing the former pair of
/// `has_pass` / `has_compile_fail` booleans with one state.
#[derive(Clone, Copy, Debug)]
pub(in crate::internal) enum Selected {
    /// No cases were registered.
    Neither,
    /// Only pass-tests were registered.
    PassOnly,
    /// Only `compile_fail` tests were registered.
    CompileFailOnly,
    /// Both pass-tests and `compile_fail` tests were registered.
    Both,
}

impl Selected {
    /// Builds the selection from whether each kind of case was seen.
    #[allow(
        clippy::single_call_fn,
        reason = "the Selected constructor mapping the (has_pass, has_compile_fail) pair onto the enum, kept on the type beside has_pass/both rather than inlined at its lone call site in prepare"
    )]
    pub(in crate::internal) const fn from_flags(has_pass: bool, has_compile_fail: bool) -> Self {
        match (has_pass, has_compile_fail) {
            (false, false) => Self::Neither,
            (true, false) => Self::PassOnly,
            (false, true) => Self::CompileFailOnly,
            (true, true) => Self::Both,
        }
    }

    /// Whether any registered case is a pass-test.
    pub(in crate::internal) const fn has_pass(self) -> bool {
        matches!(self, Self::PassOnly | Self::Both)
    }

    /// Whether both kinds of case were registered, so each test line should
    /// label its expected outcome.
    pub(in crate::internal) const fn both(self) -> bool {
        matches!(self, Self::Both)
    }
}

/// Whether the installed cargo supports `--keep-going`, gating the batched build
/// fast path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::internal) enum KeepGoing {
    /// `--keep-going` is supported; all bins can build in one batched invocation.
    Yes,
    /// `--keep-going` is unsupported; build the bins one at a time.
    No,
}
