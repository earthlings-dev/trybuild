//! The private engine behind trybuild's three-function public API.
//!
//! `lib.rs` is a thin router that re-exports [`TestCases`] and [`TryBuildError`] from
//! here; everything else — glob expansion, throwaway-project synthesis, driving
//! cargo, normalizing diagnostics, and reporting — lives under this module and
//! is reachable only from within the crate.

#[macro_use]
mod path;

pub(in crate::internal) mod build;
pub(in crate::internal) mod diagnostics;
pub(in crate::internal) mod error;
pub(in crate::internal) mod model;
pub(in crate::internal) mod outcome;
pub(in crate::internal) mod project;
pub(in crate::internal) mod report;
pub(in crate::internal) mod runner;
pub(in crate::internal) mod sys;

mod cases;

pub use self::build::{BuildError, BuildOutput, CompileFailure, MetadataFailure};
pub use self::cases::TestCases;
pub use self::diagnostics::{DiagnosticsError, MismatchDetail, UnexpectedSuccess};
pub use self::error::TryBuildError;
pub use self::model::Expected;
pub use self::outcome::{CaseReport, Outcome, OverwriteDetail, PassDetail, Report, WipDetail};
pub use self::project::ProjectError;
pub use self::runner::{RunOutput, RunnerError};
pub use self::sys::SysError;
pub use self::sys::env::Update;
