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

pub use self::build::BuildError;
pub use self::build::BuildOutput;
pub use self::build::CompileFailure;
pub use self::build::MetadataFailure;
pub use self::cases::TestCases;
pub use self::diagnostics::DiagnosticsError;
pub use self::diagnostics::MismatchDetail;
pub use self::diagnostics::UnexpectedSuccess;
pub use self::error::TryBuildError;
pub use self::model::Expected;
pub use self::outcome::CaseReport;
pub use self::outcome::Outcome;
pub use self::outcome::OverwriteDetail;
pub use self::outcome::PassDetail;
pub use self::outcome::Report;
pub use self::outcome::WipDetail;
pub use self::project::ProjectError;
pub use self::runner::RunOutput;
pub use self::runner::RunnerError;
pub use self::sys::SysError;
pub use self::sys::env::Update;
