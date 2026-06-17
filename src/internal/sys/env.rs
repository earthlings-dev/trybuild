//! The `TRYBUILD` environment variable controlling how missing or mismatched
//! snapshots are reconciled.

use crate::internal::sys::{Result, SysError};
use std::env;

/// How the runner reconciles a `compile_fail` test with its `.stderr` snapshot.
///
/// [`run`](crate::TestCases::run) reads this from the `TRYBUILD` environment
/// variable; [`try_run`](crate::TestCases::try_run) takes it as an explicit
/// argument so a programmatic caller controls it without touching the
/// environment.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Update {
    /// Fail when a snapshot is missing or mismatched, writing nothing.
    Verify,
    /// Default: write a new snapshot under `wip` (or report a mismatch) without
    /// touching the saved file.
    #[default]
    Wip,
    /// Write the snapshot in place (`TRYBUILD=overwrite`).
    Overwrite,
}

impl Update {
    /// Reads the update mode from the `TRYBUILD` environment variable,
    /// defaulting to [`Wip`](Self::Wip) when the variable is unset.
    ///
    /// # Errors
    ///
    /// Returns [`SysError::UpdateVar`] if the variable holds an unrecognized value.
    #[allow(
        clippy::single_call_fn,
        reason = "the constructor reading the update mode from the TRYBUILD variable, kept on Update's own type so the env-var contract lives with the enum it produces"
    )]
    pub(in crate::internal) fn env() -> Result<Self> {
        let Some(var) = env::var_os("TRYBUILD") else {
            return Ok(Self::default());
        };

        match var.as_os_str().to_str() {
            Some("verify") => Ok(Self::Verify),
            Some("wip") => Ok(Self::Wip),
            Some("overwrite") => Ok(Self::Overwrite),
            _ => Err(SysError::UpdateVar(var)),
        }
    }
}
